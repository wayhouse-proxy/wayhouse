//! Plugin routes, materialised (`docs/plugins.md` "Routes"; design #236).
//!
//! Plugins declare `{host, backend}` entries; the controller does not rewrite the operator's
//! config. It publishes the entries of every enabled install as an *overlay* and each proxy
//! appends them to the listeners that opted in (`plugin_routes:` in the config), after the
//! operator's own routes, so operator routes always win. Two installs claiming the same
//! hostname: the earlier install wins and the later claim is reported, not applied.

use std::collections::BTreeMap;
use std::convert::Infallible;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;

use super::api::PluginsState;
use super::ports::PortAssignment;
use super::{InstallRecord, PluginStore, PluginStoreError};
use wayhouse_plugin_host::routes::RouteKind;
use wayhouse_plugin_host::{PortProtoSel, RouteEntry};

/// One materialised route.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OverlayRoute {
    pub host: String,
    pub backend: String,
    /// `plugin:<install id>`.
    pub owner: String,
    /// The plugin's install name, for display.
    pub plugin: String,
}

/// A claim that lost to an earlier install.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conflict {
    pub host: String,
    pub backend: String,
    pub owner: String,
    pub plugin: String,
    /// The owner that holds the hostname.
    pub held_by: String,
}

/// One materialised port route: the entry port the host assigned a plugin's key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OverlayPortRoute {
    pub key: String,
    pub port: u16,
    pub proto: PortProtoSel,
    pub backend: String,
    /// `plugin:<install id>`.
    pub owner: String,
    pub plugin: String,
}

/// A port route that got no entry port because the approved ports were all taken.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExhaustedPort {
    pub key: String,
    pub backend: String,
    pub owner: String,
    pub plugin: String,
}

/// What the controller publishes to proxies and shows operators.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Overlay {
    /// In match order: exact names before suffix patterns, longer suffixes first.
    pub routes: Vec<OverlayRoute>,
    pub conflicts: Vec<Conflict>,
    /// Entry-port routes (`type: port` listeners), by port. A separate list so a proxy that
    /// predates port routes keeps reading `routes` unchanged.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub port_routes: Vec<OverlayPortRoute>,
    /// Routes of kind `web` (ABI 0.3): sites of the `web:` front door. A separate list for
    /// the same reason as `port_routes`: a proxy that predates the kind would otherwise
    /// append them to a TCP listener.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub web_routes: Vec<OverlayRoute>,
    /// Port routes waiting for a free entry port.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exhausted: Vec<ExhaustedPort>,
}

/// More specific first: exact names, then `*.suffix` by descending suffix length.
fn specificity(host: &str) -> (u8, std::cmp::Reverse<usize>) {
    if host.starts_with("*.") {
        (1, std::cmp::Reverse(host.len()))
    } else {
        (0, std::cmp::Reverse(0))
    }
}

/// The overlay for `installs` (each with the route set it last committed). Disabled
/// installs publish nothing. The earlier install (by creation time, then id) wins a
/// hostname.
pub fn materialise(mut installs: Vec<(InstallRecord, Vec<RouteEntry>)>) -> Overlay {
    installs.sort_by(|a, b| (a.0.created_at, &a.0.id).cmp(&(b.0.created_at, &b.0.id)));
    let mut held: BTreeMap<String, String> = BTreeMap::new();
    let mut out = Overlay::default();
    for (rec, entries) in installs {
        if !rec.enabled {
            continue;
        }
        let owner = format!("plugin:{}", rec.id);
        for e in entries {
            match held.get(&e.host) {
                Some(winner) if *winner != owner => out.conflicts.push(Conflict {
                    host: e.host,
                    backend: e.backend,
                    owner: owner.clone(),
                    plugin: rec.name.clone(),
                    held_by: winner.clone(),
                }),
                _ => {
                    held.insert(e.host.clone(), owner.clone());
                    let route = OverlayRoute {
                        host: e.host,
                        backend: e.backend,
                        owner: owner.clone(),
                        plugin: rec.name.clone(),
                    };
                    match e.kind {
                        RouteKind::Tcp => out.routes.push(route),
                        RouteKind::Web => out.web_routes.push(route),
                    }
                }
            }
        }
    }
    // Stable: ties keep install order, which keeps the output deterministic.
    out.routes.sort_by_key(|r| specificity(&r.host));
    out.web_routes.sort_by_key(|r| specificity(&r.host));
    out
}

/// Adds the entry-port routes of every enabled install to `out`.
fn materialise_ports(installs: &[(InstallRecord, Vec<PortAssignment>)], out: &mut Overlay) {
    for (rec, table) in installs {
        if !rec.enabled {
            continue;
        }
        let owner = format!("plugin:{}", rec.id);
        for a in table {
            match a.port {
                Some(port) => out.port_routes.push(OverlayPortRoute {
                    key: a.key.clone(),
                    port,
                    proto: a.proto,
                    backend: a.backend.clone(),
                    owner: owner.clone(),
                    plugin: rec.name.clone(),
                }),
                None => out.exhausted.push(ExhaustedPort {
                    key: a.key.clone(),
                    backend: a.backend.clone(),
                    owner: owner.clone(),
                    plugin: rec.name.clone(),
                }),
            }
        }
    }
    out.port_routes.sort_by_key(|p| p.port);
}

impl PluginStore {
    /// The overlay as this replica sees it now.
    pub fn overlay(&self) -> Result<Overlay, PluginStoreError> {
        let mut installs = Vec::new();
        let mut ports = Vec::new();
        for rec in self.list()? {
            // A suspended capability withdraws what the plugin already committed; the stored
            // table stays, so resuming brings the same routes and ports back.
            let routes = if rec.enabled && !rec.suspended.contains("routes") {
                self.routes(&rec.id)?
            } else {
                Vec::new()
            };
            let table = if rec.suspended.contains("port_routes") {
                Vec::new()
            } else {
                self.port_routes(&rec.id)?
            };
            ports.push((rec.clone(), table));
            installs.push((rec, routes));
        }
        let mut overlay = materialise(installs);
        materialise_ports(&ports, &mut overlay);
        Ok(overlay)
    }
}

/// `GET /plugin-routes`: the current overlay.
pub async fn show(State(st): State<PluginsState>) -> Response {
    match st.store.overlay() {
        Ok(o) => Json(o).into_response(),
        Err(e) => super::api::internal(e).into_response(),
    }
}

/// `GET /plugin-routes/subscribe`: the overlay now, then again whenever it changes.
pub async fn subscribe(State(st): State<PluginsState>) -> Response {
    let Ok(permit) = SUBSCRIBERS.try_acquire() else {
        tracing::warn!(
            max = MAX_SUBSCRIBERS,
            "plugin route subscriber cap reached; refusing a stream"
        );
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            [(axum::http::header::RETRY_AFTER, "5")],
            Json(serde_json::json!({"error": "too many subscribers; retry shortly"})),
        )
            .into_response();
    };
    // The slot is held by the stream's task and given back when it ends, even by a panic.
    permit.forget();
    let slot = Slot;
    let (tx, rx) = tokio::sync::mpsc::channel(4);
    let store = st.store.clone();
    tokio::spawn(async move {
        let _slot = slot;
        let mut changes = store.watch();
        let mut last: Option<Overlay> = None;
        loop {
            match store.overlay() {
                Ok(now) if last.as_ref() != Some(&now) => {
                    let Ok(doc) = serde_json::to_string(&now) else {
                        return;
                    };
                    if tx.send(doc).await.is_err() {
                        return;
                    }
                    last = Some(now);
                }
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "could not read the plugin route overlay"),
            }
            // A client that went away frees its slot at once, not at the next change.
            tokio::select! {
                () = tx.closed() => return,
                changed = changes.changed() => {
                    if changed.is_err() {
                        return;
                    }
                }
            }
        }
    });
    Sse::new(ReceiverStream::new(rx).map(|doc| Ok::<_, Infallible>(Event::default().data(doc))))
        .keep_alive(KeepAlive::default())
        .into_response()
}

/// Most live `GET /plugin-routes/subscribe` streams at once, like the config and registry streams.
pub(super) const MAX_SUBSCRIBERS: usize = 1024;
pub(super) static SUBSCRIBERS: tokio::sync::Semaphore =
    tokio::sync::Semaphore::const_new(MAX_SUBSCRIBERS);

/// A taken subscriber slot; dropping it gives the slot back.
struct Slot;

impl Drop for Slot {
    fn drop(&mut self) {
        SUBSCRIBERS.add_permits(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wayhouse_plugin_host::Capabilities;

    fn rec(id: &str, name: &str, at: u64, enabled: bool) -> InstallRecord {
        InstallRecord {
            id: id.into(),
            name: name.into(),
            sha256: "0".repeat(64),
            size: 3,
            approved: Capabilities::parse(br#"{"log":true}"#).unwrap(),
            config: serde_json::json!({}),
            enabled,
            created_at: at,
            created_by: None,
            webhook: None,
            suspended: std::collections::BTreeSet::default(),
        }
    }

    fn e(host: &str, backend: &str) -> RouteEntry {
        RouteEntry {
            host: host.into(),
            backend: backend.into(),
            kind: wayhouse_plugin_host::routes::RouteKind::default(),
        }
    }

    #[test]
    fn a_web_route_goes_to_its_own_list_and_still_competes_for_the_hostname() {
        let web = |host: &str, backend: &str| RouteEntry {
            kind: RouteKind::Web,
            ..e(host, backend)
        };
        let o = materialise(vec![
            (
                rec("0000000000000001", "first", 10, true),
                vec![web("site.example.com", "10.0.0.1:80")],
            ),
            (
                rec("0000000000000002", "second", 20, true),
                vec![
                    e("site.example.com", "10.0.0.2:1"),
                    e("tcp.example.com", "10.0.0.2:1"),
                ],
            ),
        ]);
        assert_eq!(o.web_routes.len(), 1);
        assert_eq!(o.web_routes[0].plugin, "first");
        assert_eq!(
            o.routes.len(),
            1,
            "a proxy that predates web routes sees only tcp"
        );
        assert_eq!(o.routes[0].host, "tcp.example.com");
        assert_eq!(o.conflicts.len(), 1);
        // And an overlay without web routes serialises exactly as before.
        let plain = serde_json::to_value(materialise(vec![])).unwrap();
        assert!(plain.get("web_routes").is_none());
    }

    #[test]
    fn the_earlier_install_wins_a_hostname_and_the_conflict_is_reported() {
        let o = materialise(vec![
            (
                rec("0000000000000002", "later", 20, true),
                vec![
                    e("a.example.com", "10.0.0.2:1"),
                    e("b.example.com", "10.0.0.2:1"),
                ],
            ),
            (
                rec("0000000000000001", "earlier", 10, true),
                vec![e("a.example.com", "10.0.0.1:1")],
            ),
        ]);
        let hosts: Vec<_> = o
            .routes
            .iter()
            .map(|r| (r.host.as_str(), r.plugin.as_str()))
            .collect();
        assert_eq!(
            hosts,
            [("a.example.com", "earlier"), ("b.example.com", "later")]
        );
        assert_eq!(o.conflicts.len(), 1);
        assert_eq!(o.conflicts[0].plugin, "later");
        assert_eq!(o.conflicts[0].held_by, "plugin:0000000000000001");
    }

    #[test]
    fn disabled_installs_publish_nothing_and_exact_names_come_before_suffixes() {
        let o = materialise(vec![
            (
                rec("0000000000000001", "p", 1, true),
                vec![
                    e("*.example.com", "10.0.0.1:1"),
                    e("*.eu.example.com", "10.0.0.1:2"),
                    e("play.example.com", "10.0.0.1:3"),
                ],
            ),
            (
                rec("0000000000000002", "off", 2, false),
                vec![e("x.example.com", "10.0.0.9:1")],
            ),
        ]);
        let hosts: Vec<_> = o.routes.iter().map(|r| r.host.as_str()).collect();
        assert_eq!(
            hosts,
            ["play.example.com", "*.eu.example.com", "*.example.com"]
        );
    }
}
