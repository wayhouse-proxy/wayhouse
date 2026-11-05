//! `GET /fleet/features`: how each feature flag stands across the fleet, from the flags the
//! proxies report in their push ([`IngestPayload::features`]). The Features page reads it to
//! say "gossip_verdicts is off on 2 of 14 nodes" and to show a flag a node's build lacks.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::ingest::InstanceState;

/// Most instance names listed per flag; the counts are always complete.
const MAX_LISTED: usize = 50;

#[derive(Debug, Default, Serialize, PartialEq, Eq)]
pub struct FlagRollup {
    pub name: String,
    /// Instances whose build has the flag and whose effective value is on / off.
    pub on: usize,
    pub off: usize,
    /// Instances whose build lacks the code behind the flag.
    pub unavailable: usize,
    /// Up to [`MAX_LISTED`] instances that run the flag off, by name.
    pub off_instances: Vec<String>,
    /// Up to [`MAX_LISTED`] instances whose value comes from an override.
    pub overridden_instances: Vec<String>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct FeatureRollup {
    /// Instances that reported flags (an older build reports none) out of `total`.
    pub reporting: usize,
    pub total: usize,
    pub flags: Vec<FlagRollup>,
}

#[must_use]
pub fn rollup(states: &[InstanceState]) -> FeatureRollup {
    let mut flags: BTreeMap<&str, FlagRollup> = BTreeMap::new();
    let mut reporting = 0;
    for s in states {
        let p = &s.payload;
        reporting += usize::from(!p.features.is_empty());
        for f in &p.features {
            let r = flags.entry(&f.name).or_insert_with(|| FlagRollup {
                name: f.name.clone(),
                ..FlagRollup::default()
            });
            match (f.available, f.value) {
                (false, _) => r.unavailable += 1,
                (true, true) => r.on += 1,
                (true, false) => {
                    r.off += 1;
                    if r.off_instances.len() < MAX_LISTED {
                        r.off_instances.push(p.instance.clone());
                    }
                }
            }
            if f.source == "override" && r.overridden_instances.len() < MAX_LISTED {
                r.overridden_instances.push(p.instance.clone());
            }
        }
    }
    FeatureRollup {
        reporting,
        total: states.len(),
        flags: flags.into_values().collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::IngestPayload;
    use serde_json::json;

    fn node(name: &str, features: &serde_json::Value) -> InstanceState {
        let payload: IngestPayload = serde_json::from_value(
            json!({"instance": name, "admin_url": "http://x", "pools": [], "features": features}),
        )
        .unwrap();
        InstanceState {
            payload,
            received_at_ms: 0,
            mismatch_rose_at_ms: None,
        }
    }

    fn flag(value: bool, source: &str, available: bool) -> serde_json::Value {
        json!([{"name": "acl", "value": value, "source": source, "available": available}])
    }

    #[test]
    fn counts_on_off_and_missing_builds_and_names_the_nodes_that_run_it_off() {
        let states = [
            node("a", &flag(true, "default", true)),
            node("b", &flag(false, "override", true)),
            node("c", &flag(false, "build", false)),
            node("old", &json!([])),
        ];
        let r = rollup(&states);
        assert_eq!((r.reporting, r.total), (3, 4));
        let acl = &r.flags[0];
        assert_eq!((acl.on, acl.off, acl.unavailable), (1, 1, 1));
        assert_eq!(acl.off_instances, ["b"]);
        assert_eq!(acl.overridden_instances, ["b"]);
    }

    #[test]
    fn the_listed_names_are_capped_but_the_counts_are_not() {
        let states: Vec<_> = (0..MAX_LISTED + 5)
            .map(|i| node(&format!("n{i}"), &flag(false, "config", true)))
            .collect();
        let acl = &rollup(&states).flags[0];
        assert_eq!(acl.off, MAX_LISTED + 5);
        assert_eq!(acl.off_instances.len(), MAX_LISTED);
    }
}
