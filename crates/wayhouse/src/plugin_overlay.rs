//! Routes that controller plugins declare (`docs/plugins.md` "Routes", #236), merged into
//! the operator's config for listeners that opted in with `plugin_routes:`.
//!
//! The controller publishes the overlay on `GET /plugin-routes/subscribe` (a full document on
//! connect and on every change). A proxy takes it with `--controller`, or, running from a
//! config file, with `--plugin-overlay-url` ([`load`] is the file-mode read). The
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

use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::Context;
use serde::Deserialize;
use wayhouse_config::plugin_routes::{
    merge_text, merge_web_text, PluginRoute, PortProto, RouteKey,
};
use wayhouse_core::RuntimeHandle;

const RECONNECT_MIN: Duration = Duration::from_secs(1);
const RECONNECT_MAX: Duration = Duration::from_secs(60);
const FILE_POLL: Duration = Duration::from_secs(2);

static OVERLAY: Mutex<Vec<PluginRoute>> = Mutex::new(Vec::new());
/// `(host, backend)` of the overlay's `web` routes (ABI 0.3), for the `web:` front door.
static WEB_OVERLAY: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

#[derive(Deserialize)]
struct Doc {
    routes: Vec<Entry>,
    /// Entry-port routes; absent from a controller that predates them.
    #[serde(default)]
    port_routes: Vec<PortEntry>,
    /// Routes of kind `web`; absent from a controller that predates them.
    #[serde(default)]
    web_routes: Vec<WebEntry>,
}

#[derive(Deserialize)]
struct WebEntry {
    host: String,
    backend: String,
}

#[derive(Deserialize)]
struct PortEntry {
    port: u16,
    proto: String,
    backend: String,
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

/// The overlay's routes minus the entry-port ones while the `port_routes` flag is off.
fn in_force(routes: &[PluginRoute], ports: bool) -> Vec<PluginRoute> {
    routes
        .iter()
        .filter(|r| ports || !matches!(r.key, RouteKey::Port { .. }))
        .cloned()
        .collect()
}

/// `text` with the current overlay applied. Falls back to `text` itself when the merge
/// fails, so a plugin can never take a valid operator config down.
pub fn merge(text: &str) -> String {
    let routes = in_force(
        &lock(),
        wayhouse_core::flags::on(wayhouse_config::features::PORT_ROUTES),
    );
    merge_routes(text, &routes)
}

/// A config file read like `wayhouse_config::load`, with the current overlay applied (file
/// mode with `--plugin-overlay-url`; with no overlay it is the file as written).
pub fn load(path: &Path) -> Result<wayhouse_config::Config, wayhouse_config::ConfigError> {
    let routes = in_force(
        &lock(),
        wayhouse_core::flags::on(wayhouse_config::features::PORT_ROUTES),
    );
    load_with(path, &routes)
}

/// `load` with the routes given, already filtered by `in_force`.
fn load_with(
    path: &Path,
    routes: &[PluginRoute],
) -> Result<wayhouse_config::Config, wayhouse_config::ConfigError> {
    let text =
        std::fs::read_to_string(path).map_err(|source| wayhouse_config::ConfigError::Read {
            path: path.display().to_string(),
            source,
        })?;
    wayhouse_config::parse_str(&merge_routes(&text, routes))
}

fn merge_routes(text: &str, routes: &[PluginRoute]) -> String {
    let web = WEB_OVERLAY
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    merge_web(&merge_plain(text, routes), &web)
}

fn merge_web(text: &str, web: &[(String, String)]) -> String {
    match merge_web_text(text, web) {
        Ok(m) => {
            for s in &m.skipped {
                tracing::warn!("plugin route skipped: {s}");
            }
            m.text
        }
        Err(e) => {
            tracing::warn!(error = %e, "could not merge plugin web routes; using the config as submitted");
            text.to_string()
        }
    }
}

fn merge_plain(text: &str, routes: &[PluginRoute]) -> String {
    match merge_text(text, routes) {
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
    Some(doc_routes(serde_json::from_str(data).ok()?))
}

/// The tcp, hostname and entry-port routes of a document (the `web` ones are [`doc_web`]).
fn doc_routes(doc: Doc) -> Vec<PluginRoute> {
    let ports = doc.port_routes.into_iter().filter_map(|p| {
        Some(PluginRoute::port(
            p.port,
            PortProto::parse(&p.proto)?,
            p.backend,
        ))
    });
    doc.routes
        .into_iter()
        .filter_map(Entry::route)
        .chain(ports)
        .collect()
}

/// The `web` routes of one SSE data block (empty for junk, or a controller without them).
fn parse_web(event: &str) -> Vec<(String, String)> {
    let Some(data) = event
        .split('\n')
        .find_map(|l| l.strip_prefix("data:"))
        .map(str::trim_start)
    else {
        return Vec::new();
    };
    serde_json::from_str::<Doc>(data)
        .map(|d| {
            d.web_routes
                .into_iter()
                .map(|w| (w.host, w.backend))
                .collect()
        })
        .unwrap_or_default()
}

/// Replaces the web overlay; returns whether it changed.
fn set_web(web: Vec<(String, String)>) -> bool {
    let mut cur = WEB_OVERLAY
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if *cur == web {
        return false;
    }
    *cur = web;
    true
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

/// What `--plugin-overlay-file` holds: the routes and the `web` routes.
type FileOverlay = (Vec<PluginRoute>, Vec<(String, String)>);

/// The overlay file's text as routes. A missing file is an empty overlay; text that is not the
/// overlay document is an error, so a typo never silently withdraws every route.
fn parse_file(text: Option<&str>) -> Result<FileOverlay, serde_json::Error> {
    let Some(text) = text else {
        return Ok((Vec::new(), Vec::new()));
    };
    let doc: Doc = serde_json::from_str(text)?;
    let web = doc
        .web_routes
        .iter()
        .map(|w| (w.host.clone(), w.backend.clone()))
        .collect();
    Ok((doc_routes(doc), web))
}

fn read_file_text(path: &Path) -> anyhow::Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// Reads the overlay file into the process-wide overlay; returns whether it changed. Used at
/// start, where an unreadable or malformed file refuses the start like a bad config does.
pub fn load_file(path: &Path) -> anyhow::Result<bool> {
    let text = read_file_text(path)?;
    let (routes, web) =
        parse_file(text.as_deref()).with_context(|| format!("parsing {}", path.display()))?;
    Ok(set(routes) | set_web(web))
}

/// Runs forever: re-reads the overlay file and asks for a rebuild when its routes change. A bad
/// update keeps the last good overlay and is logged once per distinct message.
/// shortcut: polls every 2 s (the file is tiny and an atomic rename defeats some file watches),
/// upgrade to the `notify` watch of `reload.rs` if the delay ever matters.
pub async fn run_file(path: std::path::PathBuf, handle: RuntimeHandle) {
    let mut last_error = String::new();
    loop {
        tokio::time::sleep(FILE_POLL).await;
        match load_file(&path) {
            Ok(changed) => {
                last_error.clear();
                if changed {
                    tracing::info!("plugin overlay file changed; rebuilding the configuration");
                    handle.request_reload();
                }
            }
            Err(e) => {
                let msg = format!("{e:#}");
                if msg != last_error {
                    tracing::warn!(error = %msg, "plugin overlay file not applied; keeping the last good one");
                }
                last_error = msg;
            }
        }
    }
}

/// Whether this answer means the controller has no plugin routes at all (plugins off, or
/// an older build), as opposed to a transient failure that keeps the last overlay.
fn withdraws_routes(status: reqwest::StatusCode) -> bool {
    matches!(status.as_u16(), 501 | 404)
}

/// Runs forever: subscribe, keep the overlay, ask for a rebuild on every change.
pub async fn run(base_url: String, token: Option<String>, handle: RuntimeHandle) {
    let mut backoff = RECONNECT_MIN;
    let mut last_error = String::new();
    loop {
        match subscribe_once(&base_url, token.as_deref(), &handle).await {
            Ok(()) => backoff = RECONNECT_MIN,
            Err(e) => {
                // A wrong URL or token must leave a trace at the default log level, once per
                // distinct failure rather than on every retry. A controller without plugins
                // (404/501) stays quiet on purpose.
                let msg = format!("{e:#}");
                if msg != last_error
                    && !msg.contains("answered 404")
                    && !msg.contains("answered 501")
                {
                    tracing::warn!(error = %msg, "plugin route stream failed; retrying");
                } else {
                    tracing::debug!(error = %msg, "plugin route stream ended");
                }
                last_error = msg;
            }
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
        if withdraws_routes(resp.status()) && (set(Vec::new()) | set_web(Vec::new())) {
            tracing::info!("controller serves no plugin routes; withdrawing them");
            handle.request_reload();
        }
        anyhow::bail!("controller answered {} for {url}", resp.status());
    }
    let mut buf = wayhouse_http::sse::EventBuffer::new();
    loop {
        let Some(chunk) = tokio::time::timeout(wayhouse_http::sse::IDLE_TIMEOUT, resp.chunk())
            .await
            .map_err(|_| {
                anyhow::anyhow!(
                    "{url} sent nothing for {:?}; reconnecting",
                    wayhouse_http::sse::IDLE_TIMEOUT
                )
            })?
            .map_err(|e| anyhow::anyhow!("reading {url}: {}", wayhouse_http::error_chain(&e)))?
        else {
            return Ok(());
        };
        buf.push(&chunk)
            .map_err(|e| anyhow::anyhow!("stream from {url}: {e}"))?;
        while let Some(event) = buf.next_event() {
            if let Some(routes) = parse_event(&event) {
                let web = parse_web(&event);
                if set(routes) | set_web(web) {
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
    fn web_routes_are_read_and_kept_out_of_the_tcp_routes() {
        let ev = r#"data: {"routes":[],"web_routes":[{"host":"w.example.com","backend":"10.0.0.1:80","owner":"plugin:1","plugin":"p"}]}"#;
        assert!(parse_event(ev).unwrap().is_empty());
        assert_eq!(
            parse_web(ev),
            vec![("w.example.com".to_string(), "10.0.0.1:80".to_string())]
        );
        assert!(parse_web(r#"data: {"routes":[]}"#).is_empty());
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
    fn the_port_routes_list_of_a_document_is_read() {
        let r = parse_event(
            r#"data: {"routes":[],"port_routes":[{"key":"k","port":30003,"proto":"both","backend":"10.0.0.1:7777","owner":"plugin:1","plugin":"p"},{"key":"x","port":30004,"proto":"sctp","backend":"10.0.0.2:1","owner":"plugin:1","plugin":"p"}]}"#,
        )
        .unwrap();
        assert_eq!(
            r,
            vec![PluginRoute::port(30003, PortProto::Both, "10.0.0.1:7777")]
        );
    }

    #[test]
    fn port_routes_off_leaves_the_port_routes_out_of_the_merge() {
        let routes = vec![
            PluginRoute::host("a.example.com", "10.0.1.1:1"),
            PluginRoute::port(30003, PortProto::Udp, "10.0.0.1:7777"),
        ];
        assert_eq!(in_force(&routes, true).len(), 2);
        let off = in_force(&routes, false);
        assert_eq!(off, vec![PluginRoute::host("a.example.com", "10.0.1.1:1")]);
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
    fn a_config_file_is_loaded_with_the_routes_merged_in() {
        let text = "schema_version: 2\npools:\n  - name: p\n    targets: [\"10.9.0.1:1\"]\nlisteners:\n  - name: l\n    bind: \"0.0.0.0:1\"\n    protocol: tcp\n    pool: p\n    plugin_routes: { type: sni }\n";
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        std::fs::write(&path, text).unwrap();
        let routes = [PluginRoute::host("b.example.com", "10.0.1.2:1")];
        let cfg = load_with(&path, &routes).unwrap();
        assert!(format!("{:?}", cfg.listeners).contains("b.example.com"));
        // No routes: the file as written. A missing file is still a read error.
        let plain = load_with(&path, &[]).unwrap();
        assert!(!format!("{:?}", plain.listeners).contains("b.example.com"));
        assert!(load_with(&dir.path().join("nope.yaml"), &routes).is_err());
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

    #[test]
    fn an_overlay_file_gives_routes_port_routes_and_web_routes() {
        let (routes, web) = parse_file(Some(
            r#"{"routes":[{"host":"a.example.com","backend":"10.0.0.1:1"}],
                "port_routes":[{"port":7000,"proto":"tcp","backend":"10.0.0.2:7000"}],
                "web_routes":[{"host":"w.example.com","backend":"10.0.0.3:80"}]}"#,
        ))
        .unwrap();
        assert_eq!(routes.len(), 2);
        assert_eq!(routes[0], PluginRoute::host("a.example.com", "10.0.0.1:1"));
        assert_eq!(web, vec![("w.example.com".into(), "10.0.0.3:80".into())]);
    }

    #[test]
    fn a_missing_overlay_file_is_empty_and_a_malformed_one_is_an_error() {
        assert_eq!(parse_file(None).unwrap(), (Vec::new(), Vec::new()));
        assert!(parse_file(Some("routes: nope")).is_err());
        assert!(parse_file(Some(r#"{"routes":"x"}"#)).is_err());
    }

    #[test]
    fn an_unreadable_overlay_path_is_an_error_and_a_missing_one_is_not() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_file_text(&dir.path().join("none.json")).unwrap(), None);
        // A directory is not a readable file: that must not read as "no routes".
        assert!(read_file_text(dir.path()).is_err());
    }
}
