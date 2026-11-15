//! The backend-health board (ABI 0.3, `backends` capability).
//!
//! Proxies report which backends they see up or down; the board keeps the latest word of
//! each proxy and derives one verdict per backend: **down when a majority of the proxies
//! that reported it say down** (the gossip domain-quorum rule), otherwise up. A proxy that
//! has not reported for [`STALE_AFTER`] stops counting. The board is in memory on the leader
//! only: a new leader starts empty and refills from the proxies' next full reports, and
//! events are hints (docs/plugins.md "Events"), so a short gap is acceptable.

use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use wayhouse_plugin_host::BackendStatus;

/// A proxy's report stops counting after this long without news from it.
pub const STALE_AFTER: Duration = Duration::from_secs(180);
/// Most backends the board holds; a report that would exceed it is refused.
pub const MAX_BACKENDS: usize = 16 * 1024;
/// Most distinct proxies the board holds.
pub const MAX_PROXIES: usize = 64;
/// Most entries in one report.
pub const MAX_REPORT: usize = 4096;

#[derive(Default)]
struct Inner {
    /// proxy -> (when it last reported, backend -> up).
    reports: BTreeMap<String, (Instant, BTreeMap<String, bool>)>,
    /// The verdict per backend and when it last changed (unix seconds).
    verdict: BTreeMap<String, (bool, u64)>,
}

/// Shared handle to the board.
#[derive(Clone, Default)]
pub struct HealthBoard(Arc<Mutex<Inner>>);

/// Why a report was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    Proxy,
    Backend(String),
    TooMany,
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Proxy => write!(
                f,
                "the proxy name is empty, over 128 bytes or the board is full"
            ),
            Self::Backend(b) => write!(f, "{b:?} is not an ip:port"),
            Self::TooMany => write!(f, "too many backends"),
        }
    }
}

impl HealthBoard {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Records a proxy's report: `full` replaces everything it said before, otherwise the
    /// entries are merged. Returns the backends whose verdict changed.
    pub fn report(
        &self,
        proxy: &str,
        full: bool,
        items: &[(String, bool)],
        now: Instant,
        unix: u64,
    ) -> Result<Vec<BackendStatus>, Refused> {
        if proxy.is_empty() || proxy.len() > 128 {
            return Err(Refused::Proxy);
        }
        if items.len() > MAX_REPORT {
            return Err(Refused::TooMany);
        }
        for (b, _) in items {
            if b.parse::<SocketAddr>().is_err() {
                return Err(Refused::Backend(b.clone()));
            }
        }
        let mut inner = self.lock();
        if !inner.reports.contains_key(proxy) && inner.reports.len() >= MAX_PROXIES {
            return Err(Refused::Proxy);
        }
        // Count the distinct backends the board would hold *before* touching it, so a
        // refused report (a `full` one included) leaves the earlier state alone.
        let mut known: BTreeSet<&String> = inner
            .reports
            .iter()
            .filter(|(p, _)| p.as_str() != proxy)
            .flat_map(|(_, (_, m))| m.keys())
            .collect();
        if !full {
            if let Some((_, mine)) = inner.reports.get(proxy) {
                known.extend(mine.keys());
            }
        }
        known.extend(items.iter().map(|(b, _)| b));
        if known.len() > MAX_BACKENDS {
            return Err(Refused::TooMany);
        }
        let mut touched: BTreeSet<String> = BTreeSet::new();
        let entry = inner
            .reports
            .entry(proxy.to_string())
            .or_insert_with(|| (now, BTreeMap::new()));
        entry.0 = now;
        if full {
            touched.extend(entry.1.keys().cloned());
            entry.1.clear();
        }
        for (b, up) in items {
            entry.1.insert(b.clone(), *up);
            touched.insert(b.clone());
        }
        Ok(Self::recompute(&mut inner, &touched, now, unix))
    }

    /// Drops proxies that have gone quiet and returns the verdicts that changed because of it.
    pub fn sweep(&self, now: Instant, unix: u64) -> Vec<BackendStatus> {
        let mut inner = self.lock();
        let mut touched = BTreeSet::new();
        inner.reports.retain(|_, (at, map)| {
            let fresh = now.duration_since(*at) < STALE_AFTER;
            if !fresh {
                touched.extend(map.keys().cloned());
            }
            fresh
        });
        Self::recompute(&mut inner, &touched, now, unix)
    }

    fn recompute(
        inner: &mut Inner,
        touched: &BTreeSet<String>,
        _now: Instant,
        unix: u64,
    ) -> Vec<BackendStatus> {
        let mut changed = Vec::new();
        for b in touched {
            let (mut down, mut total) = (0usize, 0usize);
            for (_, map) in inner.reports.values() {
                if let Some(up) = map.get(b) {
                    total += 1;
                    down += usize::from(!up);
                }
            }
            if total == 0 {
                // Nobody reports it any more: forget it.
                inner.verdict.remove(b);
                continue;
            }
            let up = down * 2 <= total;
            match inner.verdict.get(b) {
                Some((was, _)) if *was == up => {}
                prev => {
                    // The first sighting of a healthy backend is not news.
                    let news = prev.is_some() || !up;
                    inner.verdict.insert(b.clone(), (up, unix));
                    if news {
                        changed.push(BackendStatus {
                            backend: b.clone(),
                            up,
                            since: unix,
                        });
                    }
                }
            }
        }
        changed
    }

    /// Every verdict, for `backends_get`.
    pub fn snapshot(&self) -> Arc<Vec<BackendStatus>> {
        Arc::new(
            self.lock()
                .verdict
                .iter()
                .map(|(b, (up, since))| BackendStatus {
                    backend: b.clone(),
                    up: *up,
                    since: *since,
                })
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(b: &str, up: bool) -> (String, bool) {
        (b.to_string(), up)
    }

    #[test]
    fn a_backend_is_down_only_when_a_majority_of_proxies_say_so() {
        let board = HealthBoard::default();
        let t = Instant::now();
        let b = "10.0.0.5:25565";
        assert!(board
            .report("p1", true, &[r(b, true)], t, 100)
            .unwrap()
            .is_empty());
        assert!(board
            .report("p2", true, &[r(b, true)], t, 100)
            .unwrap()
            .is_empty());
        assert!(board
            .report("p3", true, &[r(b, true)], t, 100)
            .unwrap()
            .is_empty());
        // One of three says down: still up.
        assert!(board
            .report("p1", false, &[r(b, false)], t, 110)
            .unwrap()
            .is_empty());
        // Two of three: down, and that is news.
        let news = board.report("p2", false, &[r(b, false)], t, 120).unwrap();
        assert_eq!(
            news,
            vec![BackendStatus {
                backend: b.into(),
                up: false,
                since: 120
            }]
        );
        // One recovers: back to up, and that is news too.
        let news = board.report("p1", false, &[r(b, true)], t, 130).unwrap();
        assert!(news[0].up);
        assert_eq!(board.snapshot()[0].since, 130);
    }

    #[test]
    fn a_proxy_that_goes_quiet_stops_counting() {
        let board = HealthBoard::default();
        let t = Instant::now();
        let b = "10.0.0.5:25565";
        board.report("p1", true, &[r(b, false)], t, 100).unwrap();
        board
            .report(
                "p2",
                true,
                &[r(b, false)],
                t + Duration::from_secs(100),
                200,
            )
            .unwrap();
        // p1 is 200 s old (stale), p2 is 100 s old: only p2 counts, still down.
        let later = t + Duration::from_secs(200);
        assert!(board.sweep(later, 300).is_empty());
        assert_eq!(board.snapshot().len(), 1);
        // Nobody left: the backend is forgotten.
        let much_later = t + Duration::from_secs(400);
        assert!(board.sweep(much_later, 500).is_empty());
        assert!(board.snapshot().is_empty());
    }

    #[test]
    fn a_full_report_replaces_what_the_proxy_said_before() {
        let board = HealthBoard::default();
        let t = Instant::now();
        board
            .report(
                "p1",
                true,
                &[r("10.0.0.1:1", false), r("10.0.0.2:1", false)],
                t,
                1,
            )
            .unwrap();
        board
            .report("p1", true, &[r("10.0.0.2:1", false)], t, 2)
            .unwrap();
        let snap = board.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].backend, "10.0.0.2:1");
    }

    #[test]
    fn a_report_over_the_backend_cap_is_refused_and_leaves_the_board_alone() {
        let board = HealthBoard::default();
        let t = Instant::now();
        let block = |n: usize| -> Vec<(String, bool)> {
            (0..MAX_REPORT)
                .map(|i| {
                    let k = n * MAX_REPORT + i;
                    r(&format!("10.{}.{}.{}:1", n, k / 250 % 250, k % 250), false)
                })
                .collect()
        };
        for n in 0..MAX_BACKENDS / MAX_REPORT {
            board
                .report(&format!("p{n}"), true, &block(n), t, 1)
                .unwrap();
        }
        assert_eq!(board.snapshot().len(), MAX_BACKENDS);
        // A new proxy with one more address, and a `full` report that would add one.
        let extra = [r("192.0.2.1:1", true)];
        assert_eq!(
            board.report("new", true, &extra, t, 2),
            Err(Refused::TooMany)
        );
        assert_eq!(
            board.report("p0", false, &extra, t, 2),
            Err(Refused::TooMany)
        );
        let mut grown = block(0);
        grown.truncate(MAX_REPORT - 1);
        grown.extend(extra.clone());
        grown.push(r("192.0.2.2:1", true));
        assert_eq!(
            board.report("p0", true, &grown, t, 2),
            Err(Refused::TooMany)
        );
        // Nothing moved: same verdicts, the refused proxy is not on the board.
        let snap = board.snapshot();
        assert_eq!(snap.len(), MAX_BACKENDS);
        assert!(snap.iter().all(|b| !b.up));
        // And p0 still replaces its own set within the cap.
        board.report("p0", true, &block(0)[..10], t, 3).unwrap();
        assert_eq!(board.snapshot().len(), MAX_BACKENDS - MAX_REPORT + 10);
    }

    #[test]
    fn only_a_bounded_number_of_proxy_names_can_vote() {
        let board = HealthBoard::default();
        let t = Instant::now();
        for i in 0..MAX_PROXIES {
            board
                .report(&format!("p{i}"), true, &[r("10.0.0.1:1", true)], t, 1)
                .unwrap();
        }
        assert_eq!(
            board.report("one-too-many", true, &[r("10.0.0.1:1", false)], t, 2),
            Err(Refused::Proxy)
        );
        // A name already on the board still reports.
        board
            .report("p0", true, &[r("10.0.0.1:1", false)], t, 3)
            .unwrap();
    }

    #[test]
    fn bad_reports_are_refused() {
        let board = HealthBoard::default();
        let t = Instant::now();
        assert_eq!(board.report("", true, &[], t, 1), Err(Refused::Proxy));
        assert_eq!(
            board.report("p", true, &[r("not-an-addr", true)], t, 1),
            Err(Refused::Backend("not-an-addr".into()))
        );
        let many: Vec<_> = (0..=MAX_REPORT)
            .map(|i| r(&format!("10.0.{}.{}:1", i / 250, i % 250), true))
            .collect();
        assert_eq!(board.report("p", true, &many, t, 1), Err(Refused::TooMany));
    }
}
