//! Entry-port allocation for plugin port routes (`docs/plugins.md` "Port routes", #403).
//!
//! A plugin declares `{key, backend, proto, prefer?}` entries; the host owns the mapping
//! `key -> entry port`. Allocation runs in the replicated apply step, so it must be a pure
//! function of the stored ledger and the entry (the entry carries the clock): every replica
//! reaches the same table.
//!
//! One entry port belongs to one `(install, key)` whatever its protocol, so a `both` entry
//! needs no second port and two installs can never collide. A released port waits in
//! quarantine before anyone else gets it, so a player with an old address does not land on a
//! different server; the key that released it can take it back at any time.

use std::collections::BTreeMap;
use std::ops::RangeInclusive;

use serde::{Deserialize, Serialize};
use wayhouse_plugin_host::{PortProtoSel, PortRouteEntry};

/// How long a released port waits before another key may take it.
pub const QUARANTINE_SECS: u64 = 24 * 60 * 60;

/// Who owns an entry port, or owned it before it was released.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortOwner {
    pub install: String,
    pub key: String,
}

/// A released port and when it was released.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Quarantined {
    pub was: PortOwner,
    pub since: u64,
}

/// Every entry port in use or in quarantine, across installs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortLedger {
    pub owners: BTreeMap<u16, PortOwner>,
    pub quarantine: BTreeMap<u16, Quarantined>,
}

/// One declared port route and the entry port it got.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortAssignment {
    pub key: String,
    pub backend: String,
    pub proto: PortProtoSel,
    /// `None` when the approved ports were all taken (reported, never silently dropped).
    pub port: Option<u16>,
}

impl PortLedger {
    pub fn is_empty(&self) -> bool {
        self.owners.is_empty() && self.quarantine.is_empty()
    }

    fn owner_of(&self, install: &str, key: &str) -> Option<u16> {
        self.owners
            .iter()
            .find(|(_, o)| o.install == install && o.key == key)
            .map(|(p, _)| *p)
    }

    fn release(&mut self, port: u16, now: u64) {
        if let Some(was) = self.owners.remove(&port) {
            self.quarantine
                .insert(port, Quarantined { was, since: now });
        }
    }

    /// Releases every port `install` holds (a deleted install).
    pub fn release_install(&mut self, install: &str, now: u64) {
        let ports: Vec<u16> = self
            .owners
            .iter()
            .filter(|(_, o)| o.install == install)
            .map(|(p, _)| *p)
            .collect();
        for p in ports {
            self.release(p, now);
        }
    }
}

fn in_ranges(ranges: &[RangeInclusive<u16>], port: u16) -> bool {
    ranges.iter().any(|r| r.contains(&port))
}

/// Replaces `install`'s declared set with `declared` and returns the table: every key
/// keeps the port it had, new keys get `prefer` when free and otherwise the lowest free
/// port of `ranges`, keys that are gone release their port into quarantine.
///
/// `declared` is expected sorted by key (the host sorts a set it returns), which makes the
/// result independent of how the plugin ordered it.
///
/// `now` is the proposer's clock, carried in the log entry so replicas agree. A leader whose
/// clock runs far ahead can end a quarantine early; that trade-off is accepted.
pub fn allocate(
    ledger: &mut PortLedger,
    install: &str,
    ranges: &[RangeInclusive<u16>],
    declared: &[PortRouteEntry],
    now: u64,
) -> Vec<PortAssignment> {
    let expired = |q: &Quarantined| now.saturating_sub(q.since) >= QUARANTINE_SECS;
    let wanted: std::collections::BTreeSet<&str> =
        declared.iter().map(|d| d.key.as_str()).collect();

    // Keys that are gone, and ports that left the approval, go into quarantine.
    let held: Vec<(u16, String)> = ledger
        .owners
        .iter()
        .filter(|(_, o)| o.install == install)
        .map(|(p, o)| (*p, o.key.clone()))
        .collect();
    for (port, key) in held {
        if !wanted.contains(key.as_str()) || !in_ranges(ranges, port) {
            ledger.release(port, now);
        }
    }

    let mut table = Vec::with_capacity(declared.len());
    for d in declared {
        let port = ledger
            .owner_of(install, &d.key)
            .or_else(|| {
                // Its own quarantined port, if still inside the approval.
                let p = ledger
                    .quarantine
                    .iter()
                    .find(|(p, q)| {
                        q.was.install == install && q.was.key == d.key && in_ranges(ranges, **p)
                    })
                    .map(|(p, _)| *p)?;
                ledger.quarantine.remove(&p);
                Some(p)
            })
            .or_else(|| {
                let free = |l: &PortLedger, p: u16| {
                    !l.owners.contains_key(&p) && l.quarantine.get(&p).is_none_or(expired)
                };
                let pick = d
                    .prefer
                    .filter(|p| in_ranges(ranges, *p) && free(ledger, *p))
                    .or_else(|| {
                        ranges
                            .iter()
                            .flat_map(Clone::clone)
                            .find(|p| free(ledger, *p))
                    })
                    // Full: the quarantined port that waited longest.
                    .or_else(|| {
                        ledger
                            .quarantine
                            .iter()
                            .filter(|(p, _)| {
                                in_ranges(ranges, **p) && !ledger.owners.contains_key(*p)
                            })
                            .min_by_key(|(p, q)| (q.since, **p))
                            .map(|(p, _)| *p)
                    })?;
                ledger.quarantine.remove(&pick);
                Some(pick)
            });
        if let Some(p) = port {
            ledger.owners.insert(
                p,
                PortOwner {
                    install: install.to_string(),
                    key: d.key.clone(),
                },
            );
        }
        table.push(PortAssignment {
            key: d.key.clone(),
            backend: d.backend.clone(),
            proto: d.proto,
            port,
        });
    }
    ledger.quarantine.retain(|_, q| !expired(q));
    table
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)]
mod tests {
    use super::*;

    const DAY: u64 = QUARANTINE_SECS;

    fn e(key: &str, prefer: Option<u16>) -> PortRouteEntry {
        PortRouteEntry {
            key: key.into(),
            backend: "10.0.0.1:7777".into(),
            proto: PortProtoSel::Udp,
            prefer,
        }
    }

    fn ports(table: &[PortAssignment]) -> Vec<(&str, Option<u16>)> {
        table.iter().map(|a| (a.key.as_str(), a.port)).collect()
    }

    #[test]
    fn new_keys_take_the_lowest_free_port_or_their_preferred_one() {
        let mut l = PortLedger::default();
        let t = allocate(
            &mut l,
            "a",
            &[30000..=30009],
            &[e("one", None), e("three", Some(30005)), e("two", None)],
            100,
        );
        assert_eq!(
            ports(&t),
            vec![
                ("one", Some(30000)),
                ("three", Some(30005)),
                ("two", Some(30001))
            ]
        );
        assert_eq!(l.owners.len(), 3);
    }

    #[test]
    fn a_taken_preferred_port_falls_back_to_the_lowest_free_one() {
        let mut l = PortLedger::default();
        allocate(&mut l, "a", &[30000..=30009], &[e("x", Some(30000))], 1);
        let t = allocate(&mut l, "b", &[30000..=30009], &[e("y", Some(30000))], 2);
        assert_eq!(ports(&t), vec![("y", Some(30001))]);
    }

    #[test]
    fn a_preferred_port_outside_the_ranges_is_ignored() {
        let mut l = PortLedger::default();
        let t = allocate(&mut l, "a", &[30000..=30009], &[e("x", Some(40000))], 1);
        assert_eq!(ports(&t), vec![("x", Some(30000))]);
    }

    #[test]
    fn a_key_keeps_its_port_when_the_backend_changes_or_other_keys_come_and_go() {
        let mut l = PortLedger::default();
        let r = [30000..=30009];
        allocate(&mut l, "a", &r, &[e("m", None), e("z", None)], 1);
        let mut moved = e("z", None);
        moved.backend = "10.0.9.9:7777".into();
        let t = allocate(&mut l, "a", &r, &[e("a1", None), moved], 2);
        // "z" stays on 30001; "m" is gone; the new "a1" takes the lowest free port.
        assert_eq!(ports(&t), vec![("a1", Some(30002)), ("z", Some(30001))]);
        assert_eq!(t[1].backend, "10.0.9.9:7777");
    }

    #[test]
    fn a_removed_key_waits_in_quarantine_and_its_return_reclaims_the_port() {
        let mut l = PortLedger::default();
        let r = [30000..=30001];
        allocate(&mut l, "a", &r, &[e("gone", None)], 10);
        allocate(&mut l, "a", &r, &[], 20);
        assert_eq!(l.quarantine[&30000].since, 20);
        assert!(l.owners.is_empty());
        // Another key does not get the quarantined port while a free one exists.
        let other = allocate(&mut l, "b", &r, &[e("other", None)], 30);
        assert_eq!(ports(&other), vec![("other", Some(30001))]);
        // The key that released it takes it back.
        let back = allocate(&mut l, "a", &r, &[e("gone", None)], 40);
        assert_eq!(ports(&back), vec![("gone", Some(30000))]);
        assert!(l.quarantine.is_empty());
    }

    #[test]
    fn a_quarantined_port_is_reused_by_others_only_when_the_range_is_otherwise_full_or_after_a_day()
    {
        let mut l = PortLedger::default();
        let r = [30000..=30001];
        allocate(&mut l, "a", &r, &[e("x", None), e("y", None)], 100);
        allocate(&mut l, "a", &r, &[e("y", None)], 200); // x released at 200
                                                         // Full otherwise: the oldest quarantined port is reused.
        let t = allocate(&mut l, "b", &r, &[e("n", None)], 300);
        assert_eq!(ports(&t), vec![("n", Some(30000))]);
        // After a day a quarantined port counts as free again, lowest first.
        let mut l = PortLedger::default();
        let wide = [30000..=30003];
        allocate(&mut l, "a", &wide, &[e("x", None)], 100);
        allocate(&mut l, "a", &wide, &[], 200);
        let early = allocate(&mut l, "b", &wide, &[e("n", None)], 300);
        assert_eq!(ports(&early), vec![("n", Some(30001))], "30000 still waits");
        let late = allocate(&mut l, "c", &wide, &[e("m", None)], 200 + DAY);
        assert_eq!(ports(&late), vec![("m", Some(30000))]);
    }

    #[test]
    fn the_oldest_quarantined_port_is_reused_first() {
        let mut l = PortLedger::default();
        let r = [30000..=30002];
        allocate(
            &mut l,
            "a",
            &r,
            &[e("p", None), e("q", None), e("r", None)],
            1,
        );
        allocate(&mut l, "a", &r, &[e("p", None)], 500); // q, r released together
        l.quarantine.get_mut(&30002).unwrap().since = 400; // r is older than q
        let t = allocate(&mut l, "b", &r, &[e("n", None)], 600);
        assert_eq!(ports(&t), vec![("n", Some(30002))]);
    }

    #[test]
    fn exhaustion_leaves_the_entry_unassigned() {
        let mut l = PortLedger::default();
        let r = [30000..=30000];
        let t = allocate(&mut l, "a", &r, &[e("a1", None), e("a2", None)], 1);
        assert_eq!(ports(&t), vec![("a1", Some(30000)), ("a2", None)]);
        assert_eq!(l.owners.len(), 1);
        // Freed room is used by the next declaration.
        let t = allocate(&mut l, "a", &r, &[e("a2", None)], 2);
        assert_eq!(ports(&t), vec![("a2", Some(30000))]);
    }

    #[test]
    fn a_port_that_left_the_approved_ranges_is_released_and_reallocated() {
        let mut l = PortLedger::default();
        allocate(&mut l, "a", &[30000..=30009], &[e("x", Some(30008))], 1);
        let t = allocate(&mut l, "a", &[30000..=30003], &[e("x", None)], 2);
        assert_eq!(ports(&t), vec![("x", Some(30000))]);
        assert_eq!(l.quarantine[&30008].was.key, "x");
    }

    #[test]
    fn installs_use_separate_key_spaces() {
        let mut l = PortLedger::default();
        let r = [30000..=30009];
        let a = allocate(&mut l, "a", &r, &[e("same", None)], 1);
        let b = allocate(&mut l, "b", &r, &[e("same", None)], 1);
        assert_eq!(ports(&a), vec![("same", Some(30000))]);
        assert_eq!(ports(&b), vec![("same", Some(30001))]);
    }

    #[test]
    fn deleting_an_install_quarantines_all_its_ports() {
        let mut l = PortLedger::default();
        allocate(
            &mut l,
            "a",
            &[30000..=30009],
            &[e("x", None), e("y", None)],
            1,
        );
        l.release_install("a", 50);
        assert!(l.owners.is_empty());
        assert_eq!(l.quarantine.len(), 2);
    }

    #[test]
    fn the_same_inputs_give_the_same_table() {
        let run = || {
            let mut l = PortLedger::default();
            allocate(
                &mut l,
                "a",
                &[30000..=30009],
                &[e("k1", None), e("k2", Some(30004))],
                5,
            );
            allocate(&mut l, "b", &[30000..=30009], &[e("k1", Some(30004))], 6);
            serde_json::to_string(&l).unwrap()
        };
        assert_eq!(run(), run());
    }
}
