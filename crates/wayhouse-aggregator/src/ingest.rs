//! The ingest payload shape and the in-memory per-instance store it lands
//! in. `wayhouse`'s push client (slice 7) builds one [`IngestPayload`] per tick
//! directly from its live `Snapshot` / `ConnTracker` — no scraping of its
//! own admin API's plaintext output, no intermediate format.
//!
//! Deliberately a summary, not the full live session registry (`GET
//! /sessions` already exists per-instance for that, break-glass style):
//! pool/backend health + admin state, and session *counts*, is what a fleet
//! dashboard actually renders per tick.

use std::collections::HashMap;
use std::sync::{PoisonError, RwLock};

use serde::{Deserialize, Serialize};

use crate::util::unix_ms;

/// What a proxy pushes about itself: a summary of its pools, backends and sessions, not its
/// full state. The aggregator keeps the latest push per `instance` and overwrites it on the next.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
#[schema(example = json!({
    "instance": "edge-fra-1",
    "admin_url": "http://10.0.1.4:9900",
    "pools": [{"name": "survival", "balancer": "round_robin",
               "backends": [{"addr": "10.0.2.4:25565", "healthy": true, "state": "enabled", "active": 12}]}],
    "sessions": {"tcp": 12, "udp": 0},
    "group": "eu/frankfurt/cluster-a",
    "version": "0.4.0",
    "protocol": "1.3",
    "protocol_mismatches": 0,
    "region": "eu-central",
    "tags": {"tier": "edge"},
    "listeners": [{"name": "minecraft", "accepted": 1042, "blocked": 7, "bytes_c2s": 5242880, "bytes_s2c": 20971520, "packets": 0}],
    "config_revision": 41,
    "features": [{"name": "gossip_verdicts", "value": true, "source": "default", "available": true}]
}))]
pub struct IngestPayload {
    /// The instance's own name (its hostname or an operator-assigned name). It is the key the
    /// push is stored and overwritten under, so two instances with one name overwrite each
    /// other. Must not be empty, at most 1024 bytes.
    #[schema(example = "edge-fra-1")]
    pub instance: String,
    /// The base URL of this instance's own admin API, as the aggregator can reach it (for
    /// example `http://10.0.1.4:9900`). Fan-out calls go to this URL with the instance token, so
    /// the aggregator accepts only a host that `--instance-url-allow` lists or, with no list,
    /// an IP address equal to the one the push came from (never a loopback address). The scheme
    /// must be `http` or `https`, with no credentials, query or fragment. A wildcard bind
    /// address (`0.0.0.0`) is not reachable from the aggregator.
    #[schema(example = "http://10.0.1.4:9900")]
    pub admin_url: String,
    /// The instance's pools and their backends. At most 1024 pools of at most 4096 backends.
    pub pools: Vec<PoolSummary>,
    /// Live session counts.
    #[serde(default)]
    pub sessions: SessionCounts,
    /// The instance's fleet group path (`settings.group`), such as `eu/frankfurt/cluster-a`. A
    /// display label for the console's fleet tree; an instance without one shows ungrouped.
    #[serde(default)]
    #[schema(example = "eu/frankfurt/cluster-a")]
    pub group: Option<String>,
    /// The product version the instance runs. Empty from a build that predates the field.
    #[serde(default)]
    #[schema(example = "0.4.0")]
    pub version: String,
    /// The component wire protocol (`major.minor`) the instance speaks. Empty from a build that
    /// predates the field.
    #[serde(default)]
    #[schema(example = "1.0")]
    pub protocol: String,
    /// How many requests the instance has refused for an incompatible protocol since it
    /// started. The aggregator notes when this rises, which turns a node's version skew red for
    /// five minutes.
    #[serde(default)]
    pub protocol_mismatches: u64,
    /// The instance's region (`--region`), a label for the fleet roll-ups.
    #[serde(default)]
    #[schema(example = "eu-central")]
    pub region: Option<String>,
    /// The instance's `key=value` labels (`--tag`), used to filter the node list.
    #[serde(default)]
    pub tags: std::collections::BTreeMap<String, String>,
    /// Cumulative per-listener counters since the instance started. The aggregator derives the
    /// rates in `GET /fleet/stats` from the difference between pushes. At most 1024 listeners.
    #[serde(default)]
    pub listeners: Vec<ListenerStats>,
    /// The controller revision the instance has applied; absent from a proxy that reads its
    /// config from a file.
    #[serde(default)]
    pub config_revision: Option<u64>,
    /// The instance's effective feature flags, so the console can show how many nodes run each
    /// flag off. At most 64, each name once.
    #[serde(default)]
    pub features: Vec<FeatureReport>,
}

/// One feature flag as an instance reports it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
pub struct FeatureReport {
    /// The flag's name.
    #[schema(example = "gossip_verdicts")]
    pub name: String,
    /// The flag's effective value on the instance.
    pub value: bool,
    /// Where the value comes from: `default`, `config`, `override` or `build`.
    #[schema(example = "default")]
    pub source: String,
    /// `false` when the instance's build lacks the code behind the flag.
    pub available: bool,
}

/// One listener's cumulative counters since the instance started.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
pub struct ListenerStats {
    /// The listener's name.
    #[schema(example = "minecraft")]
    pub name: String,
    /// TCP connections accepted (a UDP listener reports none).
    #[serde(default)]
    pub accepted: u64,
    /// Connections and new UDP sessions dropped by the filters that run before routing.
    #[serde(default)]
    pub blocked: u64,
    /// TCP bytes relayed from clients to backends.
    #[serde(default)]
    pub bytes_c2s: u64,
    /// TCP bytes relayed from backends to clients.
    #[serde(default)]
    pub bytes_s2c: u64,
    /// UDP datagrams forwarded in both directions (UDP is counted in packets, not bytes).
    #[serde(default)]
    pub packets: u64,
}

/// One pool and its backends.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
pub struct PoolSummary {
    /// The pool's name.
    #[schema(example = "survival")]
    pub name: String,
    /// The balancing strategy, such as `round_robin`.
    #[schema(example = "round_robin")]
    pub balancer: String,
    /// The pool's backends.
    pub backends: Vec<BackendSummary>,
}

/// One backend of a pool.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
pub struct BackendSummary {
    /// The backend's address.
    #[schema(example = "10.0.2.4:25565")]
    pub addr: String,
    /// Whether the last health check passed.
    pub healthy: bool,
    /// The admin state: `enabled`, `draining` or `disabled`.
    #[schema(example = "enabled")]
    pub state: String,
    /// Sessions currently open to this backend.
    pub active: usize,
}

/// Live session counts of one instance.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default, utoipa::ToSchema)]
pub struct SessionCounts {
    /// Open TCP sessions.
    #[serde(default)]
    pub tcp: usize,
    /// Open UDP sessions.
    #[serde(default)]
    pub udp: usize,
}

/// One instance's most recent ingest, plus when the aggregator itself
/// received it — timestamped by the aggregator's own clock, not the
/// proxy's, so this stays meaningful even with unsynchronised clocks across
/// a fleet (it only ever answers "how long since *we* last heard from it").
#[derive(Debug, Clone, PartialEq)]
pub struct InstanceState {
    pub payload: IngestPayload,
    pub received_at_ms: u64,
    /// When `protocol_mismatches` last went up between two pushes (aggregator
    /// clock), for the "red only after a recent increase" skew rule.
    pub mismatch_rose_at_ms: Option<u64>,
}

/// Most distinct instances the store holds; an instance silent for
/// [`STALE_AFTER_MS`] is evicted to make room once it is full.
pub(crate) const DEFAULT_MAX_INSTANCES: usize = 10_000;
/// How long an instance may stay silent before a full store may drop it.
const STALE_AFTER_MS: u64 = 60 * 60 * 1000;
/// Longest any self-reported string (instance, URL, names, version) may be.
const MAX_FIELD_BYTES: usize = 1024;
/// Most pools one push may carry, and most backends per pool.
const MAX_POOLS: usize = 1024;
const MAX_BACKENDS_PER_POOL: usize = 4096;
const MAX_LISTENERS: usize = 1024;
const MAX_FEATURES: usize = 64;

/// A push the store will not take.
#[derive(Debug, PartialEq, Eq)]
pub enum IngestRejected {
    /// The store is full of instances that are all still reporting.
    Full,
}

impl IngestPayload {
    /// Size limits on what an instance may self-report: the whole payload is
    /// cloned and serialised on every `/fleet` read, so it must stay bounded.
    pub fn check_bounds(&self) -> Result<(), String> {
        let long = |s: &str| s.len() > MAX_FIELD_BYTES;
        if long(&self.instance)
            || long(&self.admin_url)
            || long(&self.version)
            || long(&self.protocol)
            || self.group.as_deref().is_some_and(long)
            || self.region.as_deref().is_some_and(long)
        {
            return Err(format!("a field is longer than {MAX_FIELD_BYTES} bytes"));
        }
        wayhouse_http::labels::check_labels(self.region.as_deref(), &self.tags)?;
        if self.pools.len() > MAX_POOLS {
            return Err(format!("more than {MAX_POOLS} pools"));
        }
        if self.listeners.len() > MAX_LISTENERS {
            return Err(format!("more than {MAX_LISTENERS} listeners"));
        }
        if self.features.len() > MAX_FEATURES {
            return Err(format!("more than {MAX_FEATURES} feature flags"));
        }
        let mut names = std::collections::HashSet::new();
        if let Some(f) = self.features.iter().find(|f| !names.insert(&f.name)) {
            return Err(format!("feature flag {:?} listed twice", f.name));
        }
        if self
            .features
            .iter()
            .any(|f| long(&f.name) || long(&f.source))
        {
            return Err(format!("a field is longer than {MAX_FIELD_BYTES} bytes"));
        }
        if self.listeners.iter().any(|l| long(&l.name)) {
            return Err(format!("a field is longer than {MAX_FIELD_BYTES} bytes"));
        }
        for p in &self.pools {
            if long(&p.name) || long(&p.balancer) {
                return Err(format!("a field is longer than {MAX_FIELD_BYTES} bytes"));
            }
            if p.backends.len() > MAX_BACKENDS_PER_POOL {
                return Err(format!(
                    "pool {} has more than {MAX_BACKENDS_PER_POOL} backends",
                    p.name
                ));
            }
            if p.backends.iter().any(|b| long(&b.addr) || long(&b.state)) {
                return Err(format!("a field is longer than {MAX_FIELD_BYTES} bytes"));
            }
        }
        Ok(())
    }
}

/// The whole store: latest-write-wins per `instance`. No history; an instance
/// that stops pushing just goes stale in place (slice 8's `/fleet/healthz` is
/// where "stale" gets a meaning and a threshold), and is only dropped to make
/// room when the store is full.
pub struct IngestStore {
    instances: RwLock<HashMap<String, InstanceState>>,
    max_instances: usize,
    history: crate::history::History,
}

impl Default for IngestStore {
    fn default() -> Self {
        Self::with_max_instances(DEFAULT_MAX_INSTANCES)
    }
}

impl IngestStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_max_instances(max_instances: usize) -> Self {
        IngestStore {
            instances: RwLock::default(),
            max_instances,
            history: crate::history::History::default(),
        }
    }

    /// The rolling stats the pushes feed (`GET /fleet/stats`).
    pub fn history(&self) -> &crate::history::History {
        &self.history
    }

    /// Records `payload` as the latest state for its `instance`, overwriting
    /// whatever was there before. A push the store refuses ([`Self::try_ingest`])
    /// is dropped.
    pub fn ingest(&self, payload: IngestPayload) {
        let _ = self.try_ingest(payload);
    }

    /// [`Self::ingest`], but says when a *new* instance does not fit. A known
    /// instance always updates; when the store is full, instances silent for
    /// [`STALE_AFTER_MS`] are evicted first.
    pub fn try_ingest(&self, payload: IngestPayload) -> Result<(), IngestRejected> {
        self.try_ingest_at(payload, unix_ms())
    }

    fn try_ingest_at(&self, payload: IngestPayload, now: u64) -> Result<(), IngestRejected> {
        let mut instances = self
            .instances
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        if !instances.contains_key(&payload.instance) && instances.len() >= self.max_instances {
            instances.retain(|_, s| now.saturating_sub(s.received_at_ms) <= STALE_AFTER_MS);
            if instances.len() >= self.max_instances {
                return Err(IngestRejected::Full);
            }
        }
        let prev = instances.get(&payload.instance);
        let mismatch_rose_at_ms = match prev {
            Some(p) if payload.protocol_mismatches > p.payload.protocol_mismatches => Some(now),
            Some(p) => p.mismatch_rose_at_ms,
            None => None,
        };
        self.history.record(&payload, now);
        let state = InstanceState {
            payload,
            received_at_ms: now,
            mismatch_rose_at_ms,
        };
        instances.insert(state.payload.instance.clone(), state);
        Ok(())
    }

    /// One instance's latest state, if it has ever pushed.
    pub fn get(&self, instance: &str) -> Option<InstanceState> {
        self.instances
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(instance)
            .cloned()
    }

    /// Every instance's latest state, instance name ascending (a stable
    /// order for a dashboard table, not a performance concern at fleet
    /// sizes this serves).
    pub fn snapshot(&self) -> Vec<InstanceState> {
        let map = self
            .instances
            .read()
            .unwrap_or_else(PoisonError::into_inner);
        let mut out: Vec<InstanceState> = map.values().cloned().collect();
        out.sort_by(|a, b| a.payload.instance.cmp(&b.payload.instance));
        out
    }
}

#[cfg(test)]
impl IngestStore {
    /// Test-only seam: insert a fully-formed [`InstanceState`], letting a
    /// test control `received_at_ms` directly instead of always stamping
    /// with the real clock (`ingest`) — needed to test [`crate::api`]'s
    /// staleness threshold without an actual multi-second sleep.
    pub(crate) fn insert_state(&self, state: InstanceState) {
        self.instances
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(state.payload.instance.clone(), state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(instance: &str) -> IngestPayload {
        IngestPayload {
            instance: instance.to_string(),
            admin_url: "http://127.0.0.1:0".to_string(),
            pools: vec![],
            sessions: SessionCounts::default(),
            group: None,
            version: String::new(),
            protocol: String::new(),
            protocol_mismatches: 0,
            region: None,
            tags: std::collections::BTreeMap::default(),
            listeners: vec![],
            config_revision: None,
            features: vec![],
        }
    }

    #[test]
    fn a_full_store_refuses_a_new_instance_but_still_updates_a_known_one() {
        let store = IngestStore::with_max_instances(2);
        store.try_ingest_at(payload("a"), 1_000).unwrap();
        store.try_ingest_at(payload("b"), 1_000).unwrap();
        assert!(store.try_ingest_at(payload("c"), 1_000).is_err());
        store.try_ingest_at(payload("a"), 2_000).unwrap();
        assert!(store.get("c").is_none());
    }

    #[test]
    fn a_full_store_evicts_instances_silent_past_the_ttl() {
        let store = IngestStore::with_max_instances(2);
        store.try_ingest_at(payload("a"), 1_000).unwrap();
        store
            .try_ingest_at(payload("b"), 1_000 + STALE_AFTER_MS / 2)
            .unwrap();
        // `a` has been silent for STALE_AFTER_MS + 1: it makes room, `b` does not.
        store
            .try_ingest_at(payload("c"), 1_000 + STALE_AFTER_MS + 1)
            .unwrap();
        assert!(store.get("a").is_none());
        assert!(store.get("b").is_some() && store.get("c").is_some());
    }

    #[test]
    fn oversized_payloads_are_rejected() {
        assert!(payload("ok").check_bounds().is_ok());
        assert!(payload(&"x".repeat(MAX_FIELD_BYTES + 1))
            .check_bounds()
            .is_err());
        let mut p = payload("many-pools");
        p.pools = (0..=MAX_POOLS)
            .map(|i| PoolSummary {
                name: format!("p{i}"),
                balancer: "rr".into(),
                backends: vec![],
            })
            .collect();
        assert!(p.check_bounds().is_err());
        let mut p = payload("many-backends");
        p.pools = vec![PoolSummary {
            name: "p".into(),
            balancer: "rr".into(),
            backends: (0..=MAX_BACKENDS_PER_POOL)
                .map(|i| BackendSummary {
                    addr: format!("10.0.0.1:{i}"),
                    healthy: true,
                    state: "enabled".into(),
                    active: 0,
                })
                .collect(),
        }];
        assert!(p.check_bounds().is_err());
    }

    #[test]
    fn a_payload_without_a_group_field_still_deserializes() {
        let json = r#"{"instance":"a","admin_url":"http://127.0.0.1:0","pools":[]}"#;
        let payload: IngestPayload = serde_json::from_str(json).unwrap();
        assert!(payload.group.is_none());
    }

    #[test]
    fn a_payload_without_version_fields_still_deserializes() {
        let json = r#"{"instance":"a","admin_url":"http://127.0.0.1:0","pools":[]}"#;
        let payload: IngestPayload = serde_json::from_str(json).unwrap();
        assert_eq!(
            (
                payload.version.as_str(),
                payload.protocol.as_str(),
                payload.protocol_mismatches
            ),
            ("", "", 0)
        );
    }

    #[test]
    fn an_old_proxy_payload_without_stats_fields_still_ingests() {
        let json =
            r#"{"instance":"old","admin_url":"http://127.0.0.1:0","pools":[],"version":"0.3.0"}"#;
        let store = IngestStore::new();
        store.ingest(serde_json::from_str(json).unwrap());
        let state = store.get("old").unwrap();
        assert!(state.payload.region.is_none() && state.payload.listeners.is_empty());
        assert!(store.history().stats(300, unix_ms()).listeners.is_empty());
    }

    #[test]
    fn a_payload_with_too_many_listeners_is_rejected() {
        let mut p = payload("l");
        p.listeners = (0..=MAX_LISTENERS)
            .map(|i| ListenerStats {
                name: format!("l{i}"),
                ..Default::default()
            })
            .collect();
        assert!(p.check_bounds().is_err());
    }

    #[test]
    fn a_payload_repeating_a_feature_name_is_rejected() {
        let mut p = payload("f");
        let one = FeatureReport {
            name: "acl".into(),
            value: true,
            source: "default".into(),
            available: true,
        };
        p.features = vec![one.clone(), one];
        assert!(p.check_bounds().unwrap_err().contains("twice"));
    }

    #[test]
    fn the_store_notes_when_the_mismatch_counter_rises() {
        let store = IngestStore::new();
        let mut p = payload("a");
        store.ingest(p.clone());
        assert_eq!(store.get("a").unwrap().mismatch_rose_at_ms, None);
        p.protocol_mismatches = 2;
        store.ingest(p.clone());
        let rose = store.get("a").unwrap().mismatch_rose_at_ms;
        assert!(rose.is_some(), "a rise is noted");
        store.ingest(p);
        assert_eq!(
            store.get("a").unwrap().mismatch_rose_at_ms,
            rose,
            "a flat counter keeps the earlier timestamp"
        );
    }

    #[test]
    fn an_unknown_instance_is_none() {
        let store = IngestStore::new();
        assert!(store.get("nope").is_none());
        assert!(store.snapshot().is_empty());
    }

    #[test]
    fn ingest_then_get_round_trips() {
        let store = IngestStore::new();
        store.ingest(payload("a"));
        let state = store.get("a").unwrap();
        assert_eq!(state.payload.instance, "a");
    }

    #[test]
    fn a_second_ingest_from_the_same_instance_overwrites_the_first() {
        let store = IngestStore::new();
        store.ingest(payload("a"));
        let mut second = payload("a");
        second.sessions = SessionCounts { tcp: 5, udp: 2 };
        store.ingest(second);

        assert_eq!(store.snapshot().len(), 1, "still one instance, not two");
        assert_eq!(store.get("a").unwrap().payload.sessions.tcp, 5);
    }

    #[test]
    fn snapshot_lists_every_instance_sorted_by_name() {
        let store = IngestStore::new();
        store.ingest(payload("b"));
        store.ingest(payload("a"));
        store.ingest(payload("c"));

        let names: Vec<String> = store
            .snapshot()
            .into_iter()
            .map(|s| s.payload.instance)
            .collect();
        assert_eq!(names, vec!["a", "b", "c"]);
    }
}
