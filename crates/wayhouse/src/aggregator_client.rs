//! `--aggregator <url>` push client (phase 10+11 slice 7, `docs/10` "The
//! aggregator"). Optional and independent of `--controller` — a proxy can
//! push its state to an aggregator while still reading its config from a
//! file, or vice versa; the two are unrelated axes.
//!
//! Builds an `IngestPayload` straight from the live `RuntimeHandle` (no
//! self-scraping of the admin API's plaintext output) and `POST`s it on a
//! fixed interval. The wire shape is duplicated here rather than pulling in
//! `wayhouse-aggregator` as a dependency — `wayhouse` never depends on an optional
//! operational tool, the same reason `controller_client` hand-parses the
//! controller's SSE JSON instead of depending on `wayhouse-controller`.
//!
//! **No retry buffer.** Each push is a full current-state summary, not an
//! event: a payload the aggregator's `IngestStore` accepts overwrites
//! whatever it had before, so replaying an old failed push after a fresher
//! one has already landed would make the aggregator's view *older*, not more
//! complete. A failed push is logged and simply superseded by the next
//! tick's fresher snapshot once the aggregator is reachable again — the
//! roadmap's "ring buffer" framing predates settling on a latest-write-wins
//! store design; this is the correct behavior for that design, not a cut
//! corner.

use std::time::Duration;

use metrics_exporter_prometheus::PrometheusHandle;
use serde::Serialize;
use wayhouse_core::metrics_defs as m;
use wayhouse_core::{Proto, RuntimeHandle};

#[derive(Serialize)]
struct IngestPayload {
    instance: String,
    /// This instance's own admin API base URL — the address the aggregator
    /// fans slice-9 intent verbs out to. See `PushConfig::admin_url`.
    admin_url: String,
    pools: Vec<PoolSummary>,
    sessions: SessionCounts,
    /// Self-reported fleet organization path (`settings.group`), so the
    /// admin GUI can render a grouped/tree fleet view. Never consulted by
    /// routing/forwarding.
    #[serde(skip_serializing_if = "Option::is_none")]
    group: Option<String>,
    /// Product version and wire protocol, so the fleet view can flag skew (#185).
    version: String,
    protocol: String,
    /// Requests refused for an incompatible protocol so far; the aggregator
    /// notes when it rises.
    protocol_mismatches: u64,
    /// Cumulative per-listener counters; the aggregator derives rates (#461).
    listeners: Vec<ListenerStats>,
}

#[derive(Serialize, Default, Debug, PartialEq)]
struct ListenerStats {
    name: String,
    accepted: u64,
    blocked: u64,
    bytes_c2s: u64,
    bytes_s2c: u64,
    packets: u64,
}

#[derive(Serialize)]
struct PoolSummary {
    name: String,
    balancer: String,
    backends: Vec<BackendSummary>,
}

#[derive(Serialize)]
struct BackendSummary {
    addr: String,
    healthy: bool,
    state: String,
    active: usize,
}

#[derive(Serialize, Default)]
struct SessionCounts {
    tcp: usize,
    udp: usize,
}

/// `--aggregator`'s resolved settings, built once at startup in `main.rs`.
pub struct PushConfig {
    pub base_url: String,
    pub instance: String,
    /// This instance's own admin API base URL, self-reported so the
    /// aggregator's slice-9 intent-verb fan-out has somewhere to send calls
    /// for this instance. `http://{settings.admin.listen}`, or `https://` with
    /// `settings.admin.tls` — a
    /// `0.0.0.0`/wildcard bind isn't reachable from the aggregator's side,
    /// same pre-existing caveat any admin-API client already has, not
    /// something this introduces.
    pub admin_url: String,
    pub interval: Duration,
    /// Bearer token to present on every push, if `--aggregator-token` set
    /// one — the aggregator side of the pair `settings.admin.auth_token` is
    /// for calls coming the other way.
    pub token: Option<String>,
}

/// Runs forever, pushing one summary per `cfg.interval`. `tokio::spawn`ed
/// from `main.rs`; never returns under normal operation.
pub async fn run(cfg: PushConfig, handle: RuntimeHandle, prometheus: PrometheusHandle) {
    let PushConfig {
        base_url,
        instance,
        admin_url,
        interval,
        token,
    } = cfg;
    let client = wayhouse_http::client();
    let url = format!("{base_url}/ingest");
    let mut ticker = tokio::time::interval(interval);
    // The first tick fires immediately; skip straight to a real interval so
    // startup doesn't race the runtime's very first snapshot for no reason.
    ticker.tick().await;

    loop {
        ticker.tick().await;
        let payload = build_payload(&instance, &admin_url, &handle, &prometheus);
        let mut req = client.post(&url).json(&payload);
        if let Some(token) = &token {
            req = req.bearer_auth(token);
        }
        match req.send().await {
            Ok(resp) if resp.status().is_success() => {
                tracing::debug!(aggregator = %base_url, "pushed a fleet-state summary")
            }
            Ok(resp) => tracing::warn!(
                aggregator = %base_url, status = %resp.status(),
                "aggregator rejected the pushed summary"
            ),
            Err(e) => tracing::warn!(
                aggregator = %base_url, error = ?e,
                "pushing to the aggregator failed; the next tick will retry with a fresher summary"
            ),
        }
    }
}

/// Per-listener totals read back from the recorder's own exposition text. The
/// `metrics` crate has no typed read API, and the counters are the one source of
/// truth the `/metrics` endpoint already serves.
/// shortcut: text parsing costs one registry render per push, move to a typed
/// recorder if the interval ever drops well below seconds.
fn listener_stats(exposition: &str) -> Vec<ListenerStats> {
    let mut by_name = std::collections::BTreeMap::<String, ListenerStats>::default();
    for line in exposition.lines() {
        let Some((series, value)) = line.rsplit_once(' ') else {
            continue;
        };
        let Some((metric, labels)) = series.split_once('{') else {
            continue;
        };
        let label = |key: &str| {
            let pat = format!("{key}=\"");
            let rest = &labels[labels.find(&pat)? + pat.len()..];
            Some(&rest[..rest.find('"')?])
        };
        let (Some(name), Ok(v)) = (label("listener"), value.parse::<f64>()) else {
            continue;
        };
        let v = v as u64;
        let l = by_name
            .entry(name.to_string())
            .or_insert_with(|| ListenerStats {
                name: name.to_string(),
                ..Default::default()
            });
        match (metric, label("result"), label("dir")) {
            (m::LISTENER_CONNECTIONS, Some("accepted"), _) => l.accepted += v,
            (m::FILTER_BLOCKED, ..) => l.blocked += v,
            (m::BYTES, _, Some("c2s")) => l.bytes_c2s += v,
            (m::BYTES, _, Some("s2c")) => l.bytes_s2c += v,
            (m::PACKETS, _, _) => l.packets += v,
            _ => {}
        }
    }
    by_name.into_values().collect()
}

fn build_payload(
    instance: &str,
    admin_url: &str,
    handle: &RuntimeHandle,
    prometheus: &PrometheusHandle,
) -> IngestPayload {
    let snapshot = handle.snapshot();
    let pools = snapshot
        .pools
        .iter()
        .map(|(name, pool)| PoolSummary {
            name: name.clone(),
            balancer: format!("{:?}", pool.balancer),
            backends: pool
                .backends()
                .iter()
                .map(|b| BackendSummary {
                    addr: b.addr.to_string(),
                    healthy: b.is_healthy(),
                    state: b.admin_state().as_str().to_string(),
                    active: b.active(),
                })
                .collect(),
        })
        .collect();

    let sessions = handle.sessions();
    let tcp = sessions.iter().filter(|s| s.proto == Proto::Tcp).count();
    let udp = sessions.iter().filter(|s| s.proto == Proto::Udp).count();

    IngestPayload {
        instance: instance.to_string(),
        admin_url: admin_url.to_string(),
        pools,
        sessions: SessionCounts { tcp, udp },
        group: snapshot.group.clone(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        protocol: wayhouse_http::protocol::ProtocolVersion::current().to_string(),
        protocol_mismatches: wayhouse_http::protocol::mismatches_total(),
        listeners: listener_stats(&prometheus.render()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wayhouse_core::{Runtime, Snapshot};

    const YAML: &str = r#"
settings:
  group: "eu/frankfurt"
pools:
  - name: local
    targets: ["127.0.0.1:9001"]
listeners:
  - name: main
    bind: "127.0.0.1:0"
    protocol: tcp
    pool: local
"#;

    #[tokio::test]
    async fn build_payload_reports_the_live_pool_and_backend_state() {
        let cfg = wayhouse_config::parse_str(YAML).unwrap();
        let runtime = Runtime::start(Snapshot::from_config(&cfg), std::sync::Arc::default(), 1);

        let prometheus = metrics_exporter_prometheus::PrometheusBuilder::new()
            .build_recorder()
            .handle();
        let payload = build_payload(
            "test-instance",
            "http://127.0.0.1:9900",
            &runtime.handle(),
            &prometheus,
        );

        assert_eq!(payload.instance, "test-instance");
        assert_eq!(payload.admin_url, "http://127.0.0.1:9900");
        assert_eq!(payload.pools.len(), 1);
        assert_eq!(payload.pools[0].name, "local");
        assert_eq!(payload.pools[0].backends.len(), 1);
        assert_eq!(payload.pools[0].backends[0].addr, "127.0.0.1:9001");
        // Optimistically healthy before the first health-check sweep runs
        // (docs/09 ADR 3 / AGENTS.md's "reload carries health by address").
        assert!(payload.pools[0].backends[0].healthy);
        assert_eq!(payload.pools[0].backends[0].state, "enabled");
        assert_eq!(payload.sessions.tcp, 0);
        assert_eq!(payload.sessions.udp, 0);
        assert_eq!(payload.group.as_deref(), Some("eu/frankfurt"));
        assert_eq!(payload.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(
            payload.protocol,
            wayhouse_http::protocol::ProtocolVersion::CURRENT.to_string()
        );
    }

    #[test]
    fn listener_stats_sum_the_counters_per_listener() {
        let text = r#"
# TYPE wayhouse_listener_connections_total counter
wayhouse_listener_connections_total{listener="main",result="accepted"} 12
wayhouse_listener_connections_total{listener="main",result="no_route"} 3
wayhouse_filter_blocked_total{listener="main",filter="acl"} 2
wayhouse_filter_blocked_total{listener="main",filter="geo"} 1
wayhouse_bytes_total{listener="main",dir="c2s"} 100
wayhouse_bytes_total{listener="main",dir="s2c"} 900
wayhouse_packets_total{listener="game",dir="c2s"} 5
wayhouse_packets_total{listener="game",dir="s2c"} 7
wayhouse_active_connections{listener="main"} 4
wayhouse_fd_open 31
"#;
        assert_eq!(
            listener_stats(text),
            vec![
                ListenerStats {
                    name: "game".into(),
                    packets: 12,
                    ..Default::default()
                },
                ListenerStats {
                    name: "main".into(),
                    accepted: 12,
                    blocked: 3,
                    bytes_c2s: 100,
                    bytes_s2c: 900,
                    packets: 0,
                },
            ]
        );
    }
}
