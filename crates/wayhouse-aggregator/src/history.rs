//! Rolling fleet stats (#461): a ring buffer of one-minute buckets, about an
//! hour deep, fed by every accepted push.
//!
//! Proxies push *cumulative* counters ([`crate::ingest::ListenerStats`]); this
//! module keeps the previous sample per instance and adds the difference to the
//! minute buckets it covers. A counter that went down means the instance
//! restarted, so its new value is the whole increase. A gap between pushes
//! spreads the increase over the minutes it spans, so a silent instance never
//! shows as a spike. Series are aggregates only (listener, pool, region, fleet);
//! there is no per-node series, which keeps memory independent of fleet size.
//! Gauges (pool backends, sessions) are sampled once per instance per minute,
//! from its first push in that minute.
//!
//! Memory only, like the rest of the aggregator: a restart starts the charts
//! empty.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Mutex, PoisonError};

use serde::Serialize;

use crate::ingest::{IngestPayload, ListenerStats};

pub const BUCKET_MS: u64 = 60_000;
/// An hour of closed minutes plus the open one.
const MAX_BUCKETS: usize = 61;
/// Distinct names per bucket for each kind of series; more are ignored.
const MAX_SERIES: usize = 1024;
/// A gap longer than this rebaselines instead of spreading.
const MAX_SPREAD_MS: u64 = (MAX_BUCKETS as u64 - 1) * BUCKET_MS;

#[derive(Default, Clone, Copy)]
struct Flow {
    accepted: f64,
    blocked: f64,
    bytes_c2s: f64,
    bytes_s2c: f64,
    packets: f64,
}

impl Flow {
    fn add(&mut self, o: &Flow) {
        self.accepted += o.accepted;
        self.blocked += o.blocked;
        self.bytes_c2s += o.bytes_c2s;
        self.bytes_s2c += o.bytes_s2c;
        self.packets += o.packets;
    }
    fn scaled(&self, f: f64) -> Flow {
        Flow {
            accepted: self.accepted * f,
            blocked: self.blocked * f,
            bytes_c2s: self.bytes_c2s * f,
            bytes_s2c: self.bytes_s2c * f,
            packets: self.packets * f,
        }
    }
    fn bytes(&self) -> f64 {
        self.bytes_c2s + self.bytes_s2c
    }
}

struct Bucket {
    start_ms: u64,
    listeners: HashMap<String, Flow>,
    regions: HashMap<String, Flow>,
    /// pool -> (healthy backends, backends) summed over the sampled instances.
    pools: HashMap<String, (u32, u32)>,
    sessions: (u64, u64),
    sampled: HashSet<String>,
}

impl Bucket {
    fn new(start_ms: u64) -> Self {
        Bucket {
            start_ms,
            listeners: HashMap::new(),
            regions: HashMap::new(),
            pools: HashMap::new(),
            sessions: (0, 0),
            sampled: HashSet::new(),
        }
    }
}

struct Last {
    at_ms: u64,
    counters: HashMap<String, ListenerStats>,
}

#[derive(Default)]
struct Inner {
    buckets: VecDeque<Bucket>,
    last: HashMap<String, Last>,
    /// When the newest push arrived: the open minute's data covers up to here.
    last_record_ms: u64,
}

#[derive(Default)]
pub struct History(Mutex<Inner>);

fn minute(ms: u64) -> u64 {
    ms - ms % BUCKET_MS
}

fn delta(now: u64, prev: u64) -> u64 {
    // Lower than before: the instance restarted, the new value is all increase.
    if now >= prev {
        now - prev
    } else {
        now
    }
}

impl Inner {
    /// The bucket for the minute at `start_ms`, created at its sorted place if missing. `None`
    /// when that minute is older than everything the ring still holds.
    fn bucket(&mut self, start_ms: u64) -> Option<&mut Bucket> {
        let at = match self.buckets.binary_search_by_key(&start_ms, |b| b.start_ms) {
            Ok(i) => i,
            Err(i) => {
                self.buckets.insert(i, Bucket::new(start_ms));
                if self.buckets.len() > MAX_BUCKETS {
                    self.buckets.pop_front();
                    i.checked_sub(1)?
                } else {
                    i
                }
            }
        };
        self.buckets.get_mut(at)
    }

    /// Spreads `flow` (the increase between `from` and `to`) over the minutes
    /// the interval covers, in proportion to the time in each.
    fn spread(&mut self, from: u64, to: u64, region: Option<&str>, name: &str, flow: Flow) {
        let span = (to - from).max(1) as f64;
        let mut t = from;
        while t < to {
            let m = minute(t);
            let end = (m + BUCKET_MS).min(to);
            let part = flow.scaled((end - t) as f64 / span);
            if let Some(b) = self.bucket(m) {
                if b.listeners.len() < MAX_SERIES || b.listeners.contains_key(name) {
                    b.listeners.entry(name.to_string()).or_default().add(&part);
                }
                if let Some(r) = region {
                    if b.regions.len() < MAX_SERIES || b.regions.contains_key(r) {
                        b.regions.entry(r.to_string()).or_default().add(&part);
                    }
                }
            }
            t = end;
        }
    }
}

impl History {
    /// Folds one accepted push, received at `now` (aggregator clock, ms).
    pub fn record(&self, p: &IngestPayload, now: u64) {
        let mut h = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        h.last_record_ms = h.last_record_ms.max(now);
        let region = p.region.as_deref();
        let prev = h.last.remove(&p.instance);
        if let Some(prev) = prev.filter(|l| now > l.at_ms && now - l.at_ms <= MAX_SPREAD_MS) {
            for l in &p.listeners {
                // A listener new since the last push has no baseline yet.
                let Some(old) = prev.counters.get(&l.name) else {
                    continue;
                };
                let flow = Flow {
                    accepted: delta(l.accepted, old.accepted) as f64,
                    blocked: delta(l.blocked, old.blocked) as f64,
                    bytes_c2s: delta(l.bytes_c2s, old.bytes_c2s) as f64,
                    bytes_s2c: delta(l.bytes_s2c, old.bytes_s2c) as f64,
                    packets: delta(l.packets, old.packets) as f64,
                };
                h.spread(prev.at_ms, now, region, &l.name, flow);
            }
        }
        h.last.insert(
            p.instance.clone(),
            Last {
                at_ms: now,
                counters: p
                    .listeners
                    .iter()
                    .map(|l| (l.name.clone(), l.clone()))
                    .collect(),
            },
        );
        if h.last.len() > 2 * MAX_SERIES {
            h.last
                .retain(|_, l| now.saturating_sub(l.at_ms) <= MAX_SPREAD_MS);
        }
        if let Some(b) = h.bucket(minute(now)) {
            if b.sampled.insert(p.instance.clone()) {
                for pool in &p.pools {
                    if b.pools.len() < MAX_SERIES || b.pools.contains_key(&pool.name) {
                        let e = b.pools.entry(pool.name.clone()).or_default();
                        e.0 += pool.backends.iter().filter(|x| x.healthy).count() as u32;
                        e.1 += pool.backends.len() as u32;
                    }
                }
                b.sessions.0 += p.sessions.tcp as u64;
                b.sessions.1 += p.sessions.udp as u64;
            }
        }
    }
}

#[derive(Serialize, Debug, PartialEq)]
pub struct Stats {
    pub generated_at_ms: u64,
    pub range_sec: u64,
    pub fleet: Series<FleetPoint>,
    pub listeners: Vec<Named<ListenerPoint>>,
    pub pools: Vec<Named<PoolPoint>>,
    pub regions: Vec<Named<RegionPoint>>,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct Series<P> {
    /// Seconds between points; widgets use it with `last_sample_ms` for freshness.
    pub resolution_sec: u64,
    pub last_sample_ms: Option<u64>,
    pub points: Vec<P>,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct Named<P> {
    pub name: String,
    #[serde(flatten)]
    pub series: Series<P>,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct FleetPoint {
    pub t_ms: u64,
    /// The open minute: rates cover the time elapsed in it so far.
    pub partial: bool,
    pub accepted_per_sec: f64,
    pub blocked_per_sec: f64,
    pub bytes_per_sec: f64,
    pub packets_per_sec: f64,
    /// `None` for a minute no instance pushed in (only a gap spread reached it).
    pub tcp_sessions: Option<u64>,
    pub udp_sessions: Option<u64>,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct ListenerPoint {
    pub t_ms: u64,
    pub partial: bool,
    pub accepted_per_sec: f64,
    pub blocked_per_sec: f64,
    pub bytes_c2s_per_sec: f64,
    pub bytes_s2c_per_sec: f64,
    pub packets_per_sec: f64,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct PoolPoint {
    pub t_ms: u64,
    pub partial: bool,
    pub healthy_backends: u32,
    pub backends: u32,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct RegionPoint {
    pub t_ms: u64,
    pub partial: bool,
    pub bytes_per_sec: f64,
}

fn series<P>(points: Vec<P>, last_sample_ms: Option<u64>) -> Series<P> {
    Series {
        resolution_sec: BUCKET_MS / 1000,
        last_sample_ms,
        points,
    }
}

impl History {
    /// The last `range_sec` seconds (whole minutes, at least one) as of `now`.
    pub fn stats(&self, range_sec: u64, now: u64) -> Stats {
        let h = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let open = minute(now);
        let since = open.saturating_sub(range_sec * 1000);
        let buckets: Vec<&Bucket> = h.buckets.iter().filter(|b| b.start_ms >= since).collect();
        // Seconds of data a bucket holds: a whole minute, or the elapsed part of the open one.
        let secs = |b: &Bucket| {
            if b.start_ms >= open {
                (h.last_record_ms.saturating_sub(b.start_ms).max(1000)) as f64 / 1000.0
            } else {
                BUCKET_MS as f64 / 1000.0
            }
        };
        let partial = |b: &Bucket| b.start_ms >= open;

        let names = |f: &dyn Fn(&Bucket) -> Vec<&String>| {
            let mut v: Vec<&String> = buckets.iter().flat_map(|b| f(b)).collect();
            v.sort();
            v.dedup();
            v
        };
        let last = buckets.last().map(|b| b.start_ms);

        let fleet = buckets
            .iter()
            .map(|b| {
                let mut f = Flow::default();
                b.listeners.values().for_each(|l| f.add(l));
                let s = secs(b);
                let sampled = !b.sampled.is_empty();
                FleetPoint {
                    t_ms: b.start_ms,
                    partial: partial(b),
                    accepted_per_sec: f.accepted / s,
                    blocked_per_sec: f.blocked / s,
                    bytes_per_sec: f.bytes() / s,
                    packets_per_sec: f.packets / s,
                    tcp_sessions: sampled.then_some(b.sessions.0),
                    udp_sessions: sampled.then_some(b.sessions.1),
                }
            })
            .collect();

        let listeners = names(&|b| b.listeners.keys().collect())
            .into_iter()
            .map(|n| Named {
                name: n.clone(),
                series: series(
                    buckets
                        .iter()
                        .map(|b| {
                            let f = b.listeners.get(n).copied().unwrap_or_default();
                            let s = secs(b);
                            ListenerPoint {
                                t_ms: b.start_ms,
                                partial: partial(b),
                                accepted_per_sec: f.accepted / s,
                                blocked_per_sec: f.blocked / s,
                                bytes_c2s_per_sec: f.bytes_c2s / s,
                                bytes_s2c_per_sec: f.bytes_s2c / s,
                                packets_per_sec: f.packets / s,
                            }
                        })
                        .collect(),
                    last,
                ),
            })
            .collect();

        let pools = names(&|b| b.pools.keys().collect())
            .into_iter()
            .map(|n| Named {
                name: n.clone(),
                series: series(
                    buckets
                        .iter()
                        .filter_map(|b| {
                            b.pools.get(n).map(|&(healthy, total)| PoolPoint {
                                t_ms: b.start_ms,
                                partial: partial(b),
                                healthy_backends: healthy,
                                backends: total,
                            })
                        })
                        .collect(),
                    last,
                ),
            })
            .collect();

        let regions = names(&|b| b.regions.keys().collect())
            .into_iter()
            .map(|n| Named {
                name: n.clone(),
                series: series(
                    buckets
                        .iter()
                        .map(|b| RegionPoint {
                            t_ms: b.start_ms,
                            partial: partial(b),
                            bytes_per_sec: b.regions.get(n).map_or(0.0, Flow::bytes) / secs(b),
                        })
                        .collect(),
                    last,
                ),
            })
            .collect();

        Stats {
            generated_at_ms: now,
            range_sec,
            fleet: series(fleet, last),
            listeners,
            pools,
            regions,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::{BackendSummary, PoolSummary, SessionCounts};

    const M: u64 = BUCKET_MS;

    fn push(instance: &str, accepted: u64, bytes: u64) -> IngestPayload {
        IngestPayload {
            instance: instance.into(),
            admin_url: "http://127.0.0.1:0".into(),
            pools: vec![PoolSummary {
                name: "p".into(),
                balancer: "rr".into(),
                backends: vec![
                    BackendSummary {
                        addr: "a:1".into(),
                        healthy: true,
                        state: "enabled".into(),
                        active: 0,
                    },
                    BackendSummary {
                        addr: "b:1".into(),
                        healthy: false,
                        state: "enabled".into(),
                        active: 0,
                    },
                ],
            }],
            sessions: SessionCounts { tcp: 2, udp: 1 },
            group: None,
            version: String::new(),
            protocol: String::new(),
            protocol_mismatches: 0,
            region: Some("eu".into()),
            listeners: vec![ListenerStats {
                name: "main".into(),
                accepted,
                bytes_c2s: bytes,
                ..Default::default()
            }],
        }
    }

    fn accepted(h: &History, now: u64) -> Vec<f64> {
        let s = h.stats(3600, now);
        s.listeners[0]
            .series
            .points
            .iter()
            .map(|p| p.accepted_per_sec * if p.partial { 1.0 } else { 60.0 })
            .collect()
    }

    #[test]
    fn rates_come_from_counter_differences_not_totals() {
        let h = History::default();
        let t0 = 10 * M;
        h.record(&push("a", 1000, 0), t0);
        h.record(&push("a", 1060, 0), t0 + 30_000);
        let s = h.stats(300, t0 + 30_000);
        let p = &s.fleet.points.last().unwrap();
        assert!(p.partial);
        // 60 connections in the 30 s since the first push.
        assert_eq!(p.accepted_per_sec, 2.0);
    }

    #[test]
    fn a_counter_reset_counts_the_new_value_as_the_increase() {
        let h = History::default();
        h.record(&push("a", 5000, 0), 10 * M);
        h.record(&push("a", 40, 0), 10 * M + 10_000);
        assert_eq!(accepted(&h, 10 * M + 10_000), vec![40.0 / 10.0]);
    }

    #[test]
    fn a_gap_is_spread_over_the_minutes_it_spans() {
        let h = History::default();
        h.record(&push("a", 0, 0), 10 * M);
        h.record(&push("a", 300, 0), 15 * M);
        let closed: Vec<f64> = accepted(&h, 15 * M + 1000).into_iter().take(5).collect();
        assert_eq!(closed, vec![60.0; 5], "no spike in the last minute");
    }

    #[test]
    fn a_gap_longer_than_the_buffer_rebaselines() {
        let h = History::default();
        h.record(&push("a", 0, 0), M);
        h.record(&push("a", 10_000, 0), 100 * M);
        assert!(h.stats(3600, 100 * M).listeners.is_empty());
    }

    #[test]
    fn an_instance_restart_does_not_lose_or_double_count_the_others() {
        let h = History::default();
        let t = 10 * M;
        h.record(&push("a", 100, 0), t);
        h.record(&push("b", 100, 0), t);
        h.record(&push("a", 160, 0), t + 20_000);
        h.record(&push("b", 10, 0), t + 20_000);
        let total = h
            .stats(300, t + 20_000)
            .fleet
            .points
            .last()
            .unwrap()
            .accepted_per_sec
            * 20.0;
        assert_eq!(total, 70.0);
    }

    #[test]
    fn a_new_listener_waits_for_a_baseline() {
        let h = History::default();
        h.record(&push("a", 5, 0), 10 * M);
        let mut p = push("a", 5, 0);
        p.listeners.push(ListenerStats {
            name: "late".into(),
            accepted: 999,
            ..Default::default()
        });
        h.record(&p, 10 * M + 10_000);
        let s = h.stats(300, 10 * M + 10_000);
        assert!(s.listeners.iter().all(|l| l.name != "late"));
    }

    #[test]
    fn buckets_evict_after_an_hour() {
        let h = History::default();
        for i in 0..70 {
            h.record(&push("a", i, 0), (10 + i) * M);
        }
        let s = h.stats(3600, 80 * M);
        // The hour ending at the open minute (80 M, which has no data yet).
        assert_eq!(s.fleet.points.len(), MAX_BUCKETS - 1);
        assert_eq!(s.fleet.points[0].t_ms, 20 * M);
    }

    #[test]
    fn a_range_is_a_span_of_time_not_a_count_of_buckets() {
        let h = History::default();
        h.record(&push("a", 0, 0), 10 * M);
        let s = h.stats(300, 100 * M);
        assert!(s.fleet.points.is_empty(), "{:?}", s.fleet.points);
        assert!(s.pools.is_empty());
    }

    #[test]
    fn a_gap_spread_lands_in_minutes_another_instance_has_not_opened_yet() {
        let h = History::default();
        h.record(&push("a", 0, 0), 10 * M);
        // A new instance opens minute 15 first; a's catch-up for 10..15 comes after.
        let mut b = push("b", 0, 0);
        b.listeners.clear();
        h.record(&b, 15 * M);
        h.record(&push("a", 300, 0), 15 * M + 1000);
        let closed: Vec<f64> = accepted(&h, 15 * M + 1000).into_iter().take(5).collect();
        assert_eq!(closed.len(), 5);
        assert!(closed.iter().all(|v| (v - 60.0).abs() < 0.5), "{closed:?}");
    }

    #[test]
    fn the_open_minute_divides_by_the_time_the_data_covers() {
        let h = History::default();
        h.record(&push("a", 0, 0), 10 * M);
        h.record(&push("a", 20, 0), 10 * M + 2_000);
        // Asked 10 s after the last push: still 20 connections in 2 s.
        let s = h.stats(300, 10 * M + 12_000);
        assert_eq!(s.fleet.points.last().unwrap().accepted_per_sec, 10.0);
    }

    #[test]
    fn minutes_nobody_sampled_have_no_session_counts() {
        let h = History::default();
        h.record(&push("a", 0, 0), 10 * M);
        h.record(&push("a", 300, 0), 15 * M);
        let s = h.stats(3600, 15 * M + 1000);
        let by_t = |t: u64| s.fleet.points.iter().find(|p| p.t_ms == t).unwrap();
        assert_eq!(by_t(10 * M).tcp_sessions, Some(2));
        assert_eq!(by_t(12 * M).tcp_sessions, None, "a spread-only minute");
        assert_eq!(by_t(12 * M).udp_sessions, None);
    }

    #[test]
    fn gauges_are_sampled_once_per_instance_per_minute() {
        let h = History::default();
        let t = 10 * M;
        for (inst, at) in [("a", t), ("a", t + 10_000), ("b", t + 20_000)] {
            h.record(&push(inst, 0, 0), at);
        }
        let s = h.stats(300, t + 30_000);
        let p = &s.pools[0].series.points[0];
        assert_eq!((p.healthy_backends, p.backends), (2, 4));
        let f = &s.fleet.points[0];
        assert_eq!((f.tcp_sessions, f.udp_sessions), (Some(4), Some(2)));
    }

    #[test]
    fn region_series_sum_instances_and_skip_unlabelled_ones() {
        let h = History::default();
        let t = 10 * M;
        for inst in ["a", "b"] {
            h.record(&push(inst, 0, 0), t);
            h.record(&push(inst, 0, 600), t + 10_000);
        }
        h.record(
            &{
                let mut p = push("c", 0, 0);
                p.region = None;
                p
            },
            t,
        );
        h.record(
            &{
                let mut p = push("c", 0, 600);
                p.region = None;
                p
            },
            t + 10_000,
        );
        let s = h.stats(300, t + 10_000);
        assert_eq!(s.regions.len(), 1);
        assert_eq!(s.regions[0].series.points[0].bytes_per_sec, 120.0);
    }

    #[test]
    fn an_old_proxy_without_counters_still_feeds_the_gauges() {
        let h = History::default();
        let mut p = push("old", 0, 0);
        p.listeners.clear();
        p.region = None;
        h.record(&p, 10 * M);
        h.record(&p, 10 * M + 10_000);
        let s = h.stats(300, 10 * M + 10_000);
        assert!(s.listeners.is_empty() && s.regions.is_empty());
        assert_eq!(s.pools[0].series.points[0].backends, 2);
    }
}
