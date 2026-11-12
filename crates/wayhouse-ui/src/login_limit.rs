//! In-process rate limit for `POST /ui/login`. Every attempt costs an Argon2
//! verification in `--users-file` mode, so an unthrottled login is both a
//! brute-force target and a cheap CPU DoS. Two limits per attempt:
//! a token bucket per client address (IPv6 by /64), and a growing delay per
//! username, so a botnet can't spread guesses at one account. Every attempt
//! at a name pushes its next admission out (100 ms after the first, doubling
//! up to a 5 s cap); a successful login clears the name. The cap keeps the
//! owner's worst wait short while still holding guessing to one try per 5 s.
//! Names hash into a fixed set of slots, real or not, so the delay reveals
//! nothing about which accounts exist and the memory is bounded. Behind a
//! reverse proxy every client shares the proxy's address and the per-IP bucket
//! degrades to a global one; the per-username delay still holds.
//!
//! Ephemeral and per replica, like the session store: a restart forgets
//! everything and replicas do not share state.

use std::collections::hash_map::RandomState;
use std::collections::HashMap;
use std::hash::BuildHasher;
use std::net::{IpAddr, Ipv6Addr};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

/// Bucket shape: `burst` attempts at once, then one per `refill`.
#[derive(Debug, Clone, Copy)]
pub struct Bucket {
    pub burst: u32,
    pub refill: Duration,
}

/// Per-username delay: `base` after the first attempt, doubling up to `cap`;
/// a name idle for `forget` starts over.
#[derive(Debug, Clone, Copy)]
pub struct Backoff {
    pub base: Duration,
    pub cap: Duration,
    pub forget: Duration,
}

#[derive(Debug, Clone, Copy)]
pub struct LoginLimits {
    pub per_ip: Bucket,
    pub per_username: Backoff,
    /// Most tracked keys per map; at the cap new keys are refused (fail
    /// closed) once idle ones have been pruned.
    pub max_keys: usize,
}

impl Default for LoginLimits {
    fn default() -> Self {
        LoginLimits {
            per_ip: Bucket {
                burst: 10,
                refill: Duration::from_secs(3),
            },
            per_username: Backoff {
                base: Duration::from_millis(100),
                cap: Duration::from_secs(5),
                forget: Duration::from_secs(600),
            },
            max_keys: 10_000,
        }
    }
}

/// Fixed number of username slots: every name (account or not) hashes into
/// one of them, so the map is bounded and the delay looks the same for
/// names that exist and names that don't.
const USERNAME_BUCKETS: usize = 1024;

struct State {
    tokens: f64,
    at: Instant,
}

/// One username slot: attempts since the last success, when the next is
/// allowed, and when the last was admitted.
struct Delay {
    attempts: u32,
    until: Instant,
    last: Instant,
}

#[derive(Default)]
struct Delays {
    map: HashMap<usize, Delay>,
}

impl Delays {
    /// Admits an attempt and schedules the next, or says how long to wait.
    /// A refusal changes nothing, so a flood of refused requests can't push
    /// the owner's wait past the cap.
    fn take(&mut self, key: usize, b: Backoff, now: Instant) -> Result<(), Duration> {
        if self
            .map
            .get(&key)
            .is_some_and(|d| now.saturating_duration_since(d.last) >= b.forget)
        {
            self.map.remove(&key);
        }
        let d = self.map.entry(key).or_insert(Delay {
            attempts: 0,
            until: now,
            last: now,
        });
        if now < d.until {
            return Err(d.until - now);
        }
        d.attempts = d.attempts.saturating_add(1);
        let doublings = (d.attempts - 1).min(16);
        d.until = now + b.base.saturating_mul(1 << doublings).min(b.cap);
        d.last = now;
        Ok(())
    }
}

struct Buckets<K> {
    map: HashMap<K, State>,
}

impl<K> Default for Buckets<K> {
    fn default() -> Self {
        Buckets {
            map: HashMap::new(),
        }
    }
}

impl<K: std::hash::Hash + Eq + Clone> Buckets<K> {
    /// Takes one token for `key`; `Err(wait)` when the bucket is empty.
    /// Does not mutate on refusal, so a flood of refused requests costs
    /// the victim key nothing extra.
    fn take(&mut self, key: &K, b: Bucket, max_keys: usize, now: Instant) -> Result<(), Duration> {
        let rate = 1.0 / b.refill.as_secs_f64();
        if !self.map.contains_key(key) && self.map.len() >= max_keys {
            // Full buckets are indistinguishable from unseen keys; drop them.
            self.map
                .retain(|_, s| Self::level(s, rate, b, now) < f64::from(b.burst));
            if self.map.len() >= max_keys {
                return Err(b.refill);
            }
        }
        let s = self.map.entry(key.clone()).or_insert(State {
            tokens: f64::from(b.burst),
            at: now,
        });
        let level = Self::level(s, rate, b, now);
        if level < 1.0 {
            return Err(Duration::from_secs_f64((1.0 - level) / rate));
        }
        s.tokens = level - 1.0;
        s.at = now;
        Ok(())
    }

    fn level(s: &State, rate: f64, b: Bucket, now: Instant) -> f64 {
        let gained = now.saturating_duration_since(s.at).as_secs_f64() * rate;
        (s.tokens + gained).min(f64::from(b.burst))
    }

    /// Hands a token back (used when the username delay refuses).
    fn give_back(&mut self, key: &K, b: Bucket) {
        if let Some(s) = self.map.get_mut(key) {
            s.tokens = (s.tokens + 1.0).min(f64::from(b.burst));
        }
    }
}

pub struct LoginLimiter {
    limits: LoginLimits,
    inner: Mutex<Inner>,
}

struct Inner {
    ips: Buckets<IpAddr>,
    users: Delays,
    /// Randomly keyed, so an attacker cannot pick names that collide with a
    /// chosen account's bucket.
    hasher: RandomState,
}

impl Default for Inner {
    fn default() -> Self {
        Inner {
            ips: Buckets::default(),
            users: Delays::default(),
            hasher: RandomState::new(),
        }
    }
}

impl Inner {
    fn username_bucket(&self, name: &str) -> usize {
        let name = name.to_lowercase();
        (self.hasher.hash_one(&name) as usize) % USERNAME_BUCKETS
    }
}

impl Default for LoginLimiter {
    fn default() -> Self {
        Self::new(LoginLimits::default())
    }
}

/// An IPv4-mapped IPv6 address is its IPv4 address; other IPv6 addresses
/// collapse to their /64 so one host can't dodge the limit by rotating
/// addresses inside its prefix.
fn source_key(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V4(_) => ip,
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => {
                let o = v6.octets();
                let mut masked = [0u8; 16];
                masked[..8].copy_from_slice(&o[..8]);
                IpAddr::V6(Ipv6Addr::from(masked))
            }
        },
    }
}

impl LoginLimiter {
    pub fn new(limits: LoginLimits) -> Self {
        LoginLimiter {
            limits,
            inner: Mutex::new(Inner::default()),
        }
    }

    /// Admits one login attempt from `ip` for `username` (if any), or says
    /// how long to wait.
    pub fn check(&self, ip: IpAddr, username: Option<&str>) -> Result<(), Duration> {
        self.check_at(ip, username, Instant::now())
    }

    /// Clears the delay of a username whose login succeeded, so correct
    /// passwords never count towards one.
    pub fn forgive_username(&self, username: &str) {
        let mut g = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        let slot = g.username_bucket(username);
        g.users.map.remove(&slot);
    }

    fn check_at(&self, ip: IpAddr, username: Option<&str>, now: Instant) -> Result<(), Duration> {
        let ip = source_key(ip);
        let l = self.limits;
        let mut g = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        g.ips.take(&ip, l.per_ip, l.max_keys, now)?;
        if let Some(name) = username {
            let slot = g.username_bucket(name);
            if let Err(wait) = g.users.take(slot, l.per_username, now) {
                g.ips.give_back(&ip, l.per_ip);
                return Err(wait);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// 100 ms first delay doubling to a 400 ms cap, forgotten after 10 s.
    fn backoff() -> Backoff {
        Backoff {
            base: ms(100),
            cap: ms(400),
            forget: Duration::from_secs(10),
        }
    }

    fn limiter(ip_burst: u32, user: Backoff, max_keys: usize) -> LoginLimiter {
        LoginLimiter::new(LoginLimits {
            per_ip: Bucket {
                burst: ip_burst,
                refill: Duration::from_secs(10),
            },
            per_username: user,
            max_keys,
        })
    }

    /// A per-username delay too small to matter, for tests about the IP side.
    fn no_backoff() -> Backoff {
        Backoff {
            base: Duration::ZERO,
            cap: Duration::ZERO,
            forget: Duration::ZERO,
        }
    }

    #[test]
    fn an_ip_is_refused_after_its_burst_and_admitted_again_after_a_refill() {
        let l = limiter(2, no_backoff(), 100);
        let t0 = Instant::now();
        assert!(l.check_at(ip("192.0.2.1"), None, t0).is_ok());
        assert!(l.check_at(ip("192.0.2.1"), None, t0).is_ok());
        let wait = l.check_at(ip("192.0.2.1"), None, t0).unwrap_err();
        assert!(wait > Duration::ZERO && wait <= Duration::from_secs(10));
        assert!(l.check_at(ip("192.0.2.2"), None, t0).is_ok(), "other IP");
        let later = t0 + Duration::from_secs(10);
        assert!(l.check_at(ip("192.0.2.1"), None, later).is_ok());
    }

    #[test]
    fn a_username_waits_longer_after_each_attempt_up_to_the_cap() {
        let l = limiter(100, backoff(), 100);
        let mut t = Instant::now();
        // 100, 200, 400 ms, then pinned at the cap.
        for want in [100, 200, 400, 400, 400] {
            assert!(l.check_at(ip("192.0.2.1"), Some("alice"), t).is_ok());
            assert_eq!(
                l.check_at(ip("192.0.2.1"), Some("alice"), t).unwrap_err(),
                ms(want)
            );
            t += ms(want);
        }
    }

    #[test]
    fn the_delay_is_per_username_across_ips_and_case() {
        let l = limiter(100, backoff(), 100);
        let t0 = Instant::now();
        assert!(l.check_at(ip("192.0.2.1"), Some("Alice"), t0).is_ok());
        assert!(l.check_at(ip("192.0.2.2"), Some("ALICE"), t0).is_err());
        assert!(l.check_at(ip("192.0.2.3"), Some("bob"), t0).is_ok());
    }

    #[test]
    fn a_refused_attempt_neither_extends_the_delay_nor_burns_the_ip_budget() {
        let l = limiter(2, backoff(), 100);
        let t0 = Instant::now();
        assert!(l.check_at(ip("192.0.2.1"), Some("alice"), t0).is_ok());
        for _ in 0..50 {
            assert!(l.check_at(ip("192.0.2.1"), Some("alice"), t0).is_err());
        }
        // One IP token is left, and alice is admitted right after her 100 ms.
        assert!(l
            .check_at(ip("192.0.2.1"), Some("alice"), t0 + ms(100))
            .is_ok());
    }

    #[test]
    fn a_successful_login_clears_the_delay() {
        let l = limiter(100, backoff(), 100);
        let t0 = Instant::now();
        for _ in 0..3 {
            assert!(l.check_at(ip("192.0.2.1"), Some("alice"), t0).is_ok());
            l.forgive_username("ALICE");
        }
        assert!(l.check_at(ip("192.0.2.1"), Some("alice"), t0).is_ok());
        // The failure that follows restarts at the base delay, not the cap.
        assert_eq!(
            l.check_at(ip("192.0.2.1"), Some("alice"), t0).unwrap_err(),
            ms(100)
        );
    }

    #[test]
    fn an_idle_username_is_forgiven_after_the_forget_window() {
        let l = limiter(100, backoff(), 100);
        let t0 = Instant::now();
        for i in 0..4 {
            let _ = l.check_at(ip("192.0.2.1"), Some("alice"), t0 + ms(400 * i));
        }
        let later = t0 + Duration::from_secs(30);
        assert!(l.check_at(ip("192.0.2.1"), Some("alice"), later).is_ok());
        assert_eq!(
            l.check_at(ip("192.0.2.1"), Some("alice"), later)
                .unwrap_err(),
            ms(100)
        );
    }

    #[test]
    fn an_ipv6_prefix_shares_one_bucket_and_mapped_v4_is_its_v4() {
        let l = limiter(1, no_backoff(), 100);
        let t0 = Instant::now();
        assert!(l.check_at(ip("2001:db8:1:2::1"), None, t0).is_ok());
        assert!(l.check_at(ip("2001:db8:1:2:ffff::9"), None, t0).is_err());
        assert!(l.check_at(ip("2001:db8:1:3::1"), None, t0).is_ok());
        assert!(l.check_at(ip("::ffff:192.0.2.1"), None, t0).is_ok());
        assert!(l.check_at(ip("192.0.2.1"), None, t0).is_err());
    }

    #[test]
    fn usernames_share_a_fixed_set_of_buckets() {
        let l = limiter(1_000_000, no_backoff(), 1_000_000);
        let t0 = Instant::now();
        for i in 0..5_000 {
            let name = format!("user-{i}");
            assert!(l.check_at(ip("192.0.2.1"), Some(&name), t0).is_ok());
        }
        let tracked = l.inner.lock().unwrap().users.map.len();
        assert!(tracked <= USERNAME_BUCKETS, "{tracked} username buckets");
    }

    #[test]
    fn tracked_keys_are_capped_and_idle_ones_pruned() {
        let l = limiter(5, no_backoff(), 2);
        let t0 = Instant::now();
        assert!(l.check_at(ip("192.0.2.1"), None, t0).is_ok());
        assert!(l.check_at(ip("192.0.2.2"), None, t0).is_ok());
        // Map full of keys still below burst: a new key is refused.
        assert!(l.check_at(ip("192.0.2.3"), None, t0).is_err());
        // Once the old buckets have refilled they are pruned to make room.
        let later = t0 + Duration::from_secs(60);
        assert!(l.check_at(ip("192.0.2.3"), None, later).is_ok());
    }
}
