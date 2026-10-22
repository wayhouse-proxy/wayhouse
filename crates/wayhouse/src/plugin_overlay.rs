//! Routes that controller plugins declare (`docs/plugins.md` "Routes", #236), merged into
//! the operator's config for listeners that opted in with `plugin_routes:`.
//!
//! Only meaningful with `--controller`: the controller publishes the overlay on
//! `GET /plugin-routes/subscribe` (a full document on connect and on every change). The
//! last overlay received is kept here; every place that turns controller config text into a
//! `Config` runs it through [`merge`], and a change asks for a re-fetch with
//! `RuntimeHandle::request_reload` (which `controller_client::watch_admin_reloads` serves).
//! The overlay is process-wide on purpose: there is one config source per process, and
//! threading it through the three apply sites would add a parameter to each for no gain.
//!
//! A controller without plugins answers 501 (or 404 on an older build): the overlay is
//! cleared (a restart without `--plugins` withdraws the routes at once) and the client
//! retries slowly and quietly. An unreachable controller keeps the last overlay.
//!
//! The rebuild itself is retried: if the re-fetch of the controller's config fails after a
//! change (`controller_client::watch_admin_reloads`), it is repeated with a growing delay
//! until it succeeds, so a withdrawn route does not linger until the next unrelated change.

use std::sync::Mutex;
use std::time::Duration;

use anyhow::Context;
use serde::Deserialize;
use wayhouse_config::plugin_routes::{merge_text, PluginRoute, PortProto};
use wayhouse_core::RuntimeHandle;

const RECONNECT_MIN: Duration = Duration::from_secs(1);
const RECONNECT_MAX: Duration = Duration::from_secs(60);

static OVERLAY: Mutex<Vec<PluginRoute>> = Mutex::new(Vec::new());

#[derive(Deserialize)]
struct Doc {
    routes: Vec<Entry>,
}

#[derive(Deserialize)]
struct Entry {
    /// A hostname route (`sni`/`sniffer` listeners).
    #[serde(default)]
    host: Option<String>,
    /// A port route (`type: port` listeners), with `proto` `udp`, `tcp` or `both`.
    #[serde(default)]
    port: Option<u16>,
    #[serde(default)]
    proto: Option<String>,
    backend: String,
}

impl Entry {
    /// An entry this build does not understand is left out, never fatal: a newer
    /// controller may add kinds.
    fn route(self) -> Option<PluginRoute> {
        match (self.host, self.port) {
            (Some(host), None) => Some(PluginRoute::host(host, self.backend)),
            (None, Some(port)) => {
                let proto = PortProto::parse(self.proto.as_deref()?)?;
                Some(PluginRoute::port(port, proto, self.backend))
            }
            _ => None,
        }
    }
}

fn lock() -> std::sync::MutexGuard<'static, Vec<PluginRoute>> {
    OVERLAY
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// `text` with the current overlay applied. Falls back to `text` itself when the merge
/// fails, so a plugin can never take a valid operator config down.
pub fn merge(text: &str) -> String {
    let routes = lock().clone();
    match merge_text(text, &routes) {
        Ok(m) => {
            for s in &m.skipped {
                tracing::warn!("plugin route skipped: {s}");
            }
            m.text
        }
        Err(e) => {
            tracing::warn!(error = %e, "could not merge plugin routes; using the config as submitted");
            text.to_string()
        }
    }
}

/// Parses one SSE data block into routes (`None` for keep-alives and junk).
fn parse_event(event: &str) -> Option<Vec<PluginRoute>> {
    let data = event
        .split('\n')
        .find_map(|l| l.strip_prefix("data:"))?
        .trim_start();
    let doc: Doc = serde_json::from_str(data).ok()?;
    Some(doc.routes.into_iter().filter_map(Entry::route).collect())
}

/// Replaces the overlay; returns whether it changed.
fn set(routes: Vec<PluginRoute>) -> bool {
    let mut cur = lock();
    if *cur == routes {
        return false;
    }
    *cur = routes;
    true
}

/// Whether this answer means the controller has no plugin routes at all (plugins off, or
/// an older build), as opposed to a transient failure that keeps the last overlay.
fn withdraws_routes(status: reqwest::StatusCode) -> bool {
    matches!(status.as_u16(), 501 | 404)
}

/// Runs forever: subscribe, keep the overlay, ask for a rebuild on every change.
pub async fn run(base_url: String, token: Option<String>, handle: RuntimeHandle) {
    let mut backoff = RECONNECT_MIN;
    loop {
        match subscribe_once(&base_url, token.as_deref(), &handle).await {
            Ok(()) => backoff = RECONNECT_MIN,
            Err(e) => tracing::debug!(error = %format_args!("{e:#}"), "plugin route stream ended"),
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(RECONNECT_MAX);
    }
}

async fn subscribe_once(
    base_url: &str,
    token: Option<&str>,
    handle: &RuntimeHandle,
) -> anyhow::Result<()> {
    let url = format!("{base_url}/plugin-routes/subscribe");
    let mut req = wayhouse_http::client().get(&url);
    if let Some(token) = token {
        req = req.bearer_auth(token);
    }
    let mut resp = req
        .send()
        .await
        .with_context(|| format!("connecting to {url}"))?;
    if !resp.status().is_success() {
        // 501: plugins are off on this controller. Not an error worth shouting about.
        if withdraws_routes(resp.status()) && set(Vec::new()) {
            tracing::info!("controller serves no plugin routes; withdrawing them");
            handle.request_reload();
        }
        anyhow::bail!("controller answered {} for {url}", resp.status());
    }
    let mut buf = wayhouse_http::sse::EventBuffer::new();
    loop {
        let Some(chunk) = resp
            .chunk()
            .await
            .map_err(|e| anyhow::anyhow!("reading {url}: {}", wayhouse_http::error_chain(&e)))?
        else {
            return Ok(());
        };
        buf.push(&chunk)
            .map_err(|e| anyhow::anyhow!("stream from {url}: {e}"))?;
        while let Some(event) = buf.next_event() {
            if let Some(routes) = parse_event(&event) {
                if set(routes) {
                    tracing::info!("plugin routes changed; rebuilding the configuration");
                    handle.request_reload();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_data_event_yields_routes_and_a_keep_alive_does_not() {
        let r = parse_event(
            r#"data: {"routes":[{"host":"a.example.com","backend":"10.0.0.1:1","owner":"plugin:1","plugin":"p"}],"conflicts":[]}"#,
        )
        .unwrap();
        assert_eq!(r[0], PluginRoute::host("a.example.com", "10.0.0.1:1"));
        assert!(parse_event(": keep-alive").is_none());
        assert!(parse_event("data: nope").is_none());
    }

    #[test]
    fn a_port_entry_yields_a_port_route_and_an_unknown_kind_is_left_out() {
        let r = parse_event(
            r#"data: {"routes":[{"port":30003,"proto":"udp","backend":"10.0.0.1:7777","key":"k"},{"port":30004,"proto":"both","backend":"10.0.0.2:1"},{"port":30005,"proto":"sctp","backend":"10.0.0.3:1"},{"backend":"10.0.0.4:1"}]}"#,
        )
        .unwrap();
        assert_eq!(
            r,
            vec![
                PluginRoute::port(30003, PortProto::Udp, "10.0.0.1:7777"),
                PluginRoute::port(30004, PortProto::Both, "10.0.0.2:1"),
            ]
        );
    }

    #[test]
    fn only_501_and_404_withdraw_the_overlay() {
        use reqwest::StatusCode as S;
        assert!(withdraws_routes(S::NOT_IMPLEMENTED));
        assert!(withdraws_routes(S::NOT_FOUND));
        assert!(!withdraws_routes(S::SERVICE_UNAVAILABLE));
        assert!(!withdraws_routes(S::UNAUTHORIZED));
    }

    #[test]
    fn the_merge_applies_the_overlay_and_a_change_is_noticed() {
        let text = "schema_version: 2\npools:\n  - name: p\n    targets: [\"10.9.0.1:1\"]\nlisteners:\n  - name: l\n    bind: \"0.0.0.0:1\"\n    protocol: tcp\n    pool: p\n    plugin_routes: { type: sni }\n";
        assert!(set(vec![PluginRoute::host("a.example.com", "10.0.1.1:1")]));
        let merged = merge(text);
        assert!(merged.contains("a.example.com"));
        assert!(!set(vec![PluginRoute::host("a.example.com", "10.0.1.1:1")]));
        assert!(set(vec![]));
        assert_eq!(merge(text), text);
    }
}
