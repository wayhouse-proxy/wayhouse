//! A shared secret that can be rotated while the process runs.
//!
//! A rotation keeps the old value usable for an overlap window, because the parties holding
//! a secret (HA replicas, the clients of the admin API) reload at different moments. The old
//! value is *signed with* for one period and *accepted* for another, set per call: a secret
//! that peers sign with (the gossip key) accepts the old value for twice as long as it signs
//! with it, because a replica that reloads late still signs with the old value after an early
//! one has switched. A secret nobody signs with (the admin token) accepts it for just the
//! overlap. After that only the new value is accepted. Restarting drops the old value at once.

use std::sync::{Arc, PoisonError, RwLock};
use std::time::{Duration, Instant};

use crate::token_eq;

struct State {
    current: Arc<str>,
    /// The value this one replaced, when we stop signing with it, and when we stop accepting it.
    previous: Option<(Arc<str>, Instant, Instant)>,
}

pub struct RotatingSecret(RwLock<State>);

impl RotatingSecret {
    #[must_use]
    pub fn new(value: &str) -> Self {
        Self(RwLock::new(State {
            current: Arc::from(value),
            previous: None,
        }))
    }

    /// Makes `new` the current value, keeps signing with the old one for `sign_for` and accepting
    /// it for `accept_for`. Returns
    /// `false`, changing nothing, when `new` is already the current value.
    pub fn rotate(&self, new: &str, sign_for: Duration, accept_for: Duration) -> bool {
        self.rotate_at(new, sign_for, accept_for, Instant::now())
    }

    fn rotate_at(&self, new: &str, sign_for: Duration, accept_for: Duration, now: Instant) -> bool {
        let mut s = self.0.write().unwrap_or_else(PoisonError::into_inner);
        if token_eq(new, &s.current) {
            return false;
        }
        let old = std::mem::replace(&mut s.current, Arc::from(new));
        s.previous = (!accept_for.is_zero()).then(|| (old, now + sign_for, now + accept_for));
        true
    }

    /// The value to present or sign with: the old one while the window is open, so a peer that
    /// has not reloaded yet still accepts it, and the new one after.
    #[must_use]
    pub fn signing(&self) -> Arc<str> {
        self.signing_at(Instant::now())
    }

    fn signing_at(&self, now: Instant) -> Arc<str> {
        let s = self.0.read().unwrap_or_else(PoisonError::into_inner);
        match &s.previous {
            Some((old, sign_until, _)) if now < *sign_until => old.clone(),
            _ => s.current.clone(),
        }
    }

    /// Every value that is acceptable now: the current one, then the old one inside its window.
    #[must_use]
    pub fn accepted(&self) -> Vec<Arc<str>> {
        self.accepted_at(Instant::now())
    }

    fn accepted_at(&self, now: Instant) -> Vec<Arc<str>> {
        let s = self.0.read().unwrap_or_else(PoisonError::into_inner);
        let mut v = vec![s.current.clone()];
        if let Some((old, _, accept_until)) = &s.previous {
            if now < *accept_until {
                v.push(old.clone());
            }
        }
        v
    }

    /// Whether `presented` is an acceptable value. Both are always compared, so the time does
    /// not tell a caller which one matched.
    #[must_use]
    pub fn accepts(&self, presented: &str) -> bool {
        self.accepted()
            .iter()
            .fold(false, |hit, k| token_eq(presented, k) | hit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OLD: &str = "old-secret-0123456789";
    const NEW: &str = "new-secret-0123456789";

    #[test]
    fn both_values_work_inside_the_window_and_only_the_new_one_after() {
        let s = RotatingSecret::new(OLD);
        let t0 = Instant::now();
        assert!(s.rotate_at(NEW, Duration::from_secs(60), Duration::from_secs(120), t0));
        assert_eq!(s.accepted_at(t0).len(), 2);
        assert!(s
            .accepted_at(t0 + Duration::from_secs(119))
            .iter()
            .any(|k| &**k == OLD));
        let later = s.accepted_at(t0 + Duration::from_secs(120));
        assert_eq!(later.len(), 1);
        assert_eq!(&*later[0], NEW);
    }

    /// A replica that reloads late signs with the old value for a window after its own reload,
    /// which can be a whole window after an early replica rotated; the early one still accepts it.
    #[test]
    fn the_old_value_is_signed_with_for_one_window_but_accepted_for_two() {
        let s = RotatingSecret::new(OLD);
        let t0 = Instant::now();
        s.rotate_at(NEW, Duration::from_secs(60), Duration::from_secs(120), t0);
        assert_eq!(&*s.signing_at(t0 + Duration::from_secs(59)), OLD);
        assert_eq!(&*s.signing_at(t0 + Duration::from_secs(60)), NEW);
        // At the end of the signing window a replica that reloaded just then still signs with OLD
        // for 60 more seconds, so OLD must be acceptable until twice the window.
        assert!(s
            .accepted_at(t0 + Duration::from_secs(60))
            .iter()
            .any(|k| &**k == OLD));
    }

    #[test]
    fn a_zero_window_revokes_the_old_value_at_once() {
        let s = RotatingSecret::new(OLD);
        assert!(s.rotate(NEW, Duration::ZERO, Duration::ZERO));
        assert!(s.accepts(NEW));
        assert!(!s.accepts(OLD));
    }

    #[test]
    fn rotating_to_the_same_value_changes_nothing() {
        let s = RotatingSecret::new(OLD);
        assert!(!s.rotate(OLD, Duration::from_secs(60), Duration::from_secs(60)));
        assert_eq!(s.accepted().len(), 1);
    }

    #[test]
    fn a_second_rotation_forgets_the_value_before_the_last() {
        let s = RotatingSecret::new(OLD);
        s.rotate(NEW, Duration::from_secs(60), Duration::from_secs(60));
        s.rotate(
            "third-secret-0123456789",
            Duration::from_secs(60),
            Duration::from_secs(60),
        );
        assert!(!s.accepts(OLD));
        assert!(s.accepts(NEW));
        assert!(s.accepts("third-secret-0123456789"));
    }

    #[test]
    fn signing_uses_the_old_value_until_the_window_closes() {
        let s = RotatingSecret::new(OLD);
        s.rotate(NEW, Duration::from_secs(3600), Duration::from_secs(3600));
        assert_eq!(&*s.signing(), OLD);
        let s = RotatingSecret::new(OLD);
        s.rotate(NEW, Duration::ZERO, Duration::ZERO);
        assert_eq!(&*s.signing(), NEW);
    }
}
