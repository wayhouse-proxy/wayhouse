//! Per-listener token-bucket rate limiting on *new* connections / *new* UDP
//! sessions (phase 7). One [`RateLimiter`] is shared across all of a listener's
//! worker accept tasks; it is rebuilt whenever the listener respawns (a config
//! change), so the parameters always match the live [`ListenerConfig`].
//!
//! Keys: the client source IP, and its /24 (IPv4) or /64 (IPv6) network — the
//! network bucket catches floods spread thinly across many single IPs. A permit
//! is taken only when *both* configured buckets can afford it; on refusal
//! nothing is consumed. Excess is dropped silently (no error reply — the proxy
//! never reflects).
//!
//! First cut, each a drop-in replacement later: a single `Mutex<HashMap>` (brief
//! lock, never held across `.await`, like `Backend::observe`); lazy pruning of
//! idle buckets (those that would have refilled to capacity by now) once a map
//! grows past [`PRUNE_AT`], at most once per [`PRUNE_EVERY_MS`], and a hard
//! cap ([`HARD_CAP`]) past which new sources are refused (fail closed) so
//! memory stays bounded under a flood of distinct sources. A sharded map /
//! timing wheel can come in a perf pass.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Mutex, PoisonError};

use wayhouse_config::{RateLimit, TokenBucket as BucketCfg};

use crate::util::mono_ms;

/// Drop the network-bucket map / IP-bucket map once either exceeds this many
/// entries, evicting buckets that have refilled to capacity (idle sources).
const PRUNE_AT: usize = 100_000;
/// Minimum gap between two prune scans, so a map full of live buckets does not
/// cost a full `retain` on every permit.
const PRUNE_EVERY_MS: u64 = 1_000;
/// Hard bound on each map. At the cap (after a prune attempt) a source with no
/// bucket yet is refused rather than tracked.
const HARD_CAP: usize = 2 * PRUNE_AT;

#[derive(Clone, Copy)]
struct Bucket {
    tokens: f64,
    last_ms: u64,
}

impl Bucket {
    fn full(cfg: &BucketCfg, now: u64) -> Self {
        Self {
            tokens: cfg.burst as f64,
            last_ms: now,
        }
    }

    /// Refill for elapsed time, then report whether a whole permit is available.
    fn refill(&mut self, cfg: &BucketCfg, now: u64) {
        let dt = now.saturating_sub(self.last_ms) as f64 / 1000.0;
        self.last_ms = now;
        self.tokens = (self.tokens + dt * cfg.rate as f64).min(cfg.burst as f64);
    }

    /// Would this bucket be back at capacity at `now`? Evaluated without
    /// mutating, since the stored `tokens` lag behind until the next `refill`.
    fn is_idle(&self, cfg: &BucketCfg, now: u64) -> bool {
        let dt = now.saturating_sub(self.last_ms) as f64 / 1000.0;
        self.tokens + dt * cfg.rate as f64 >= cfg.burst as f64
    }
}

/// Network key: the leading bytes of the address (/24 for v4, /64 for v6).
#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy)]
pub(crate) enum NetKey {
    V4([u8; 3]),
    V6([u8; 8]),
}

impl NetKey {
    pub(crate) fn of(ip: IpAddr) -> Self {
        match ip {
            IpAddr::V4(a) => {
                let o = a.octets();
                NetKey::V4([o[0], o[1], o[2]])
            }
            IpAddr::V6(a) => {
                // A dual-stack listener reports IPv4 clients as ::ffff:a.b.c.d;
                // key them by their IPv4 /24, not the all-zero v6 /64.
                if let Some(v4) = a.to_ipv4_mapped() {
                    return Self::of(IpAddr::V4(v4));
                }
                let o = a.octets();
                let mut p = [0u8; 8];
                p.copy_from_slice(&o[..8]);
                NetKey::V6(p)
            }
        }
    }
}

#[derive(Default)]
struct State {
    ips: HashMap<IpAddr, Bucket>,
    nets: HashMap<NetKey, Bucket>,
    last_prune_ms: Option<u64>,
}

pub struct RateLimiter {
    per_ip: Option<BucketCfg>,
    per_net: Option<BucketCfg>,
    state: Mutex<State>,
}

impl RateLimiter {
    /// Build from a listener's resolved config. Returns a limiter that always
    /// admits when `cfg` is `None` (cheap: `permit` short-circuits).
    pub fn new(cfg: Option<&RateLimit>) -> Self {
        Self {
            per_ip: cfg.and_then(|c| c.per_ip),
            per_net: cfg.and_then(|c| c.per_net),
            state: Mutex::new(State::default()),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.per_ip.is_some() || self.per_net.is_some()
    }

    /// Try to take one permit for a connection / new session from `ip`.
    ///
    /// Returns `None` when admitted, or `Some("rate_ip" | "rate_net")` naming
    /// the bucket that refused (for the `wayhouse_filter_blocked_total` label). On
    /// refusal no tokens are consumed from either bucket.
    pub fn permit(&self, ip: IpAddr) -> Option<&'static str> {
        if !self.is_enabled() {
            return None;
        }
        let now = mono_ms();
        let ip = ip.to_canonical();
        let mut st = self.state.lock().unwrap_or_else(PoisonError::into_inner);

        // At the hard cap a source with no bucket yet is refused (after one
        // prune attempt) instead of growing the map.
        if self.per_ip.is_some() && !st.ips.contains_key(&ip) && st.ips.len() >= HARD_CAP {
            self.prune(&mut st, now, true);
            if st.ips.len() >= HARD_CAP {
                return Some("rate_ip");
            }
        }
        if self.per_net.is_some() {
            let key = NetKey::of(ip);
            if !st.nets.contains_key(&key) && st.nets.len() >= HARD_CAP {
                self.prune(&mut st, now, true);
                if st.nets.len() >= HARD_CAP {
                    return Some("rate_net");
                }
            }
        }

        // Refill (creating a full bucket on first sight) and check affordability
        // for both buckets before consuming from either.
        let ip_ok = match &self.per_ip {
            None => true,
            Some(cfg) => {
                let b = st.ips.entry(ip).or_insert_with(|| Bucket::full(cfg, now));
                b.refill(cfg, now);
                b.tokens >= 1.0
            }
        };
        let net_ok = match &self.per_net {
            None => true,
            Some(cfg) => {
                let key = NetKey::of(ip);
                let b = st.nets.entry(key).or_insert_with(|| Bucket::full(cfg, now));
                b.refill(cfg, now);
                b.tokens >= 1.0
            }
        };

        if !ip_ok {
            self.prune(&mut st, now, false);
            return Some("rate_ip");
        }
        if !net_ok {
            self.prune(&mut st, now, false);
            return Some("rate_net");
        }

        if self.per_ip.is_some() {
            if let Some(b) = st.ips.get_mut(&ip) {
                b.tokens -= 1.0;
            }
        }
        if self.per_net.is_some() {
            if let Some(b) = st.nets.get_mut(&NetKey::of(ip)) {
                b.tokens -= 1.0;
            }
        }
        self.prune(&mut st, now, false);
        None
    }

    /// Bounded-memory guard: drop buckets that have refilled to capacity
    /// (sources that went quiet). Runs when a map exceeds [`PRUNE_AT`] (or is at
    /// [`HARD_CAP`], `force`), at most once per [`PRUNE_EVERY_MS`].
    fn prune(&self, st: &mut State, now: u64, force: bool) {
        let over = |n: usize| n > PRUNE_AT;
        let needed = force || over(st.ips.len()) || over(st.nets.len());
        if !needed {
            return;
        }
        if let Some(last) = st.last_prune_ms {
            if now.saturating_sub(last) < PRUNE_EVERY_MS {
                return;
            }
        }
        st.last_prune_ms = Some(now);
        if let Some(cfg) = &self.per_ip {
            st.ips.retain(|_, b| !b.is_idle(cfg, now));
        }
        if let Some(cfg) = &self.per_net {
            st.nets.retain(|_, b| !b.is_idle(cfg, now));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rl(per_ip: Option<(u32, u32)>, per_net: Option<(u32, u32)>) -> RateLimiter {
        RateLimiter::new(Some(&RateLimit {
            per_ip: per_ip.map(|(rate, burst)| BucketCfg { rate, burst }),
            per_net: per_net.map(|(rate, burst)| BucketCfg { rate, burst }),
        }))
    }

    #[test]
    fn disabled_limiter_always_admits() {
        let l = RateLimiter::new(None);
        assert!(!l.is_enabled());
        for _ in 0..1000 {
            assert_eq!(l.permit("1.2.3.4".parse().unwrap()), None);
        }
    }

    #[test]
    fn per_ip_bucket_exhausts_then_refills() {
        // 10/s, burst 3.
        let l = rl(Some((10, 3)), None);
        let ip: IpAddr = "9.9.9.9".parse().unwrap();
        assert_eq!(l.permit(ip), None);
        assert_eq!(l.permit(ip), None);
        assert_eq!(l.permit(ip), None);
        assert_eq!(l.permit(ip), Some("rate_ip"));

        // Hand the bucket ~2 tokens back (as a refill would) and confirm it
        // grants exactly two more permits before refusing again.
        {
            let mut st = l.state.lock().unwrap_or_else(PoisonError::into_inner);
            let b = st.ips.get_mut(&ip).unwrap();
            b.tokens = 2.0;
        }
        assert_eq!(l.permit(ip), None);
        assert_eq!(l.permit(ip), None);
        assert_eq!(l.permit(ip), Some("rate_ip"));
    }

    #[test]
    fn per_net_groups_addresses_in_the_same_24() {
        // No per-ip limit; /24 bucket of burst 2.
        let l = rl(None, Some((1, 2)));
        assert_eq!(l.permit("10.0.0.1".parse().unwrap()), None);
        assert_eq!(l.permit("10.0.0.2".parse().unwrap()), None);
        // Third distinct IP in 10.0.0.0/24 is refused by the shared net bucket.
        assert_eq!(l.permit("10.0.0.9".parse().unwrap()), Some("rate_net"));
        // A different /24 is unaffected.
        assert_eq!(l.permit("10.0.1.1".parse().unwrap()), None);
    }

    #[test]
    fn refusal_consumes_nothing() {
        // per_ip burst 1; per_net burst 1. First call drains both.
        let l = rl(Some((1, 1)), Some((1, 1)));
        let a: IpAddr = "172.16.5.5".parse().unwrap();
        assert_eq!(l.permit(a), None);
        assert_eq!(l.permit(a), Some("rate_ip"));
        // The /24 bucket still has its single token: a fresh IP in another /24
        // but same... no — use same /24, different ip: net refuses now too,
        // proving the earlier refusal didn't also consume the net token twice.
        let b: IpAddr = "172.16.5.6".parse().unwrap();
        assert_eq!(l.permit(b), Some("rate_net"));
    }

    #[test]
    fn ipv4_mapped_clients_get_their_own_ipv4_net_bucket() {
        // Two different /24s reaching a dual-stack listener as ::ffff:a.b.c.d
        // must not share a bucket (they used to collapse into NetKey::V6([0;8])).
        let l = rl(None, Some((1, 1)));
        assert_eq!(l.permit("::ffff:10.0.0.1".parse().unwrap()), None);
        assert_eq!(l.permit("::ffff:10.0.1.1".parse().unwrap()), None);
        // Same /24 as the first, whether spelled mapped or plain, is refused.
        assert_eq!(
            l.permit("::ffff:10.0.0.2".parse().unwrap()),
            Some("rate_net")
        );
        assert_eq!(l.permit("10.0.0.3".parse().unwrap()), Some("rate_net"));
    }

    #[test]
    fn netkey_of_mapped_equals_netkey_of_plain_ipv4() {
        assert_eq!(
            NetKey::of("::ffff:192.0.2.7".parse().unwrap()),
            NetKey::of("192.0.2.7".parse().unwrap())
        );
    }

    #[test]
    fn prune_evicts_buckets_that_have_refilled_since_last_use() {
        let l = rl(Some((1, 2)), Some((1, 2)));
        for i in 0..(PRUNE_AT as u32 + 10) {
            let ip = IpAddr::from(std::net::Ipv4Addr::from(0x0100_0000 + i * 256));
            assert_eq!(l.permit(ip), None);
        }
        // Everything above was debited once; a minute later all of it has
        // refilled to capacity.
        {
            let mut st = l.state.lock().unwrap_or_else(PoisonError::into_inner);
            l.prune(&mut st, mono_ms() + 60_000, true);
        }
        let st = l.state.lock().unwrap_or_else(PoisonError::into_inner);
        assert_eq!(st.ips.len(), 0, "idle ip buckets pruned");
        assert_eq!(st.nets.len(), 0, "idle net buckets pruned");
    }

    #[test]
    fn maps_stay_bounded_under_a_flood_of_distinct_sources() {
        let l = rl(Some((1, 1)), Some((1, 1)));
        let n = HARD_CAP as u32 + 5_000;
        for i in 0..n {
            let ip = IpAddr::from(std::net::Ipv4Addr::from(0x0100_0000 + i * 256));
            let _ = l.permit(ip);
        }
        let st = l.state.lock().unwrap_or_else(PoisonError::into_inner);
        assert!(st.ips.len() <= HARD_CAP, "ips {}", st.ips.len());
        assert!(st.nets.len() <= HARD_CAP, "nets {}", st.nets.len());
    }
}
