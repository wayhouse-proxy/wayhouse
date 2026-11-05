//! The effective feature flags the data path reads (`wayhouse_config::features`).
//!
//! The binary computes each flag's value (override over config over default, build
//! availability) and publishes the data-plane ones here with [`set_all`]; the listeners, the
//! rate limiters, the sniffers and the health checker ask [`on`]. One `AtomicU64` holds a bit
//! per registry position, so a read is one relaxed load and nothing locks on the accept path.
//! Every flag starts on, which is also what a library user who never calls [`set_all`] gets.

use std::sync::atomic::{AtomicU64, Ordering};

use wayhouse_config::features::REGISTRY;

static ON: AtomicU64 = AtomicU64::new(u64::MAX);

// The mask holds one bit per registry entry.
const _: () = assert!(REGISTRY.len() <= 64);

#[cfg(test)]
thread_local! {
    /// A unit test's own mask, so tests that flip a flag cannot race the others.
    static LOCAL: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}

/// Whether the flag at registry position `flag` (see `wayhouse_config::features::ACL` and
/// friends) is on.
#[inline]
#[must_use]
pub fn on(flag: usize) -> bool {
    #[cfg(test)]
    if let Some(mask) = LOCAL.with(std::cell::Cell::get) {
        return mask >> flag & 1 == 1;
    }
    ON.load(Ordering::Relaxed) >> flag & 1 == 1
}

/// Publishes the effective value of every flag; `value` answers by name.
pub fn set_all(value: impl Fn(&str) -> bool) {
    ON.store(mask(value), Ordering::Relaxed);
}

fn mask(value: impl Fn(&str) -> bool) -> u64 {
    REGISTRY
        .iter()
        .enumerate()
        .filter(|(_, f)| value(f.name))
        .fold(0, |m, (i, _)| m | 1 << i)
}

/// Switches `off` off for this thread until the guard drops (unit tests).
#[cfg(test)]
pub(crate) fn off(off: &[&str]) -> OffGuard {
    LOCAL.with(|l| l.set(Some(mask(|n| !off.contains(&n)))));
    OffGuard
}

#[cfg(test)]
pub(crate) struct OffGuard;

#[cfg(test)]
impl Drop for OffGuard {
    fn drop(&mut self) {
        LOCAL.with(|l| l.set(None));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wayhouse_config::features::{ACL, GEO};

    #[test]
    fn a_flag_follows_the_published_value() {
        let m = mask(|n| n != "acl");
        assert_eq!(m >> ACL & 1, 0);
        assert_eq!(m >> GEO & 1, 1);
        {
            let _g = off(&["geo"]);
            assert!(on(ACL));
            assert!(!on(GEO));
        }
        assert!(on(GEO), "the switch ends with the guard");
    }
}
