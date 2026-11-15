//! Reports this proxy's backend health to the controller (`POST /backend-health`), the feed
//! behind the plugin `backend_health` event (ABI 0.3, `docs/plugins.md`).
//!
//! Every [`CHECK_EVERY`] the proxy reads its backends' health; a changed picture is sent at
//! once and an unchanged one every [`REFRESH_EVERY`], so the controller (which forgets a
//! proxy after three minutes of silence) keeps it. Each report is a full snapshot. A
//! controller without plugins answers 501 or 404 and is left alone for a while.

use std::collections::BTreeMap;
use std::time::Duration;

use serde::Serialize;
use wayhouse_core::RuntimeHandle;

const CHECK_EVERY: Duration = Duration::from_secs(5);
const REFRESH_EVERY: Duration = Duration::from_secs(60);
const NOT_SUPPORTED_BACKOFF: Duration = Duration::from_secs(300);
/// The controller's per-report cap; more backends than this are left out of the report.
const MAX_REPORT: usize = 4096;

#[derive(Serialize)]
struct Report<'a> {
    proxy: &'a str,
    full: bool,
    backends: Vec<Item>,
}

#[derive(Serialize, PartialEq, Eq, Clone)]
struct Item {
    addr: String,
    up: bool,
}

/// This proxy's name on the controller: the host name when there is one, else a random id.
fn proxy_name() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .filter(|h| !h.is_empty() && h.len() <= 128)
        .unwrap_or_else(|| {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos());
            format!("proxy-{:x}-{nanos:x}", std::process::id())
        })
}

/// The health of every distinct backend, by address. A backend in several pools is down
/// only when it is down in all of them.
fn picture(handle: &RuntimeHandle) -> Vec<Item> {
    let snap = handle.snapshot();
    let mut by_addr: BTreeMap<String, bool> = BTreeMap::new();
    for pool in snap.pools.values() {
        for b in pool.backends() {
            let up = by_addr.entry(b.addr.to_string()).or_insert(false);
            *up |= b.is_healthy();
        }
    }
    by_addr
        .into_iter()
        .take(MAX_REPORT)
        .map(|(addr, up)| Item { addr, up })
        .collect()
}

/// Runs forever.
pub async fn run(base_url: String, token: Option<String>, handle: RuntimeHandle) {
    let name = proxy_name();
    let url = format!("{base_url}/backend-health");
    let mut last: Option<Vec<Item>> = None;
    let mut since_sent = REFRESH_EVERY;
    loop {
        tokio::time::sleep(CHECK_EVERY).await;
        since_sent += CHECK_EVERY;
        let now = picture(&handle);
        if last.as_ref() == Some(&now) && since_sent < REFRESH_EVERY {
            continue;
        }
        let body = Report {
            proxy: &name,
            full: true,
            backends: now.clone(),
        };
        let Ok(json) = serde_json::to_vec(&body) else {
            continue;
        };
        let mut req = wayhouse_http::client()
            .post(&url)
            .header("content-type", "application/json")
            .body(json);
        if let Some(token) = &token {
            req = req.bearer_auth(token);
        }
        match req.send().await {
            Ok(r) if r.status().is_success() => {
                last = Some(now);
                since_sent = Duration::ZERO;
            }
            Ok(r) if matches!(r.status().as_u16(), 501 | 404) => {
                // Plugins are off on this controller (or it is older): ask again much later.
                tracing::debug!("controller takes no backend health reports");
                tokio::time::sleep(NOT_SUPPORTED_BACKOFF).await;
            }
            Ok(r) => tracing::warn!(status = %r.status(), "backend health report refused"),
            Err(e) => {
                tracing::debug!(error = %wayhouse_http::error_chain(&e), "backend health report failed")
            }
        }
    }
}
