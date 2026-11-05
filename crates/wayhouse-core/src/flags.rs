//! The effective feature flags the data path reads (`wayhouse_config::features`).
//!
//! The binary computes each flag's value (override over config over default, build
//! availability) and publishes it here with [`set_all`]; the listeners, the rate limiters, the
//! sniffers and the health checker ask [`on`]. See [`wayhouse_config::features::Mask`].

use wayhouse_config::features::Mask;

static ON: Mask = Mask::new();

/// Whether the flag at registry position `flag` (see `wayhouse_config::features::ACL` and
/// friends) is on.
#[inline]
#[must_use]
pub fn on(flag: usize) -> bool {
    ON.on(flag)
}

/// Publishes the effective value of every flag; `value` answers by name.
pub fn set_all(value: impl Fn(&str) -> bool) {
    ON.set_all(value);
}

/// Switches `off` off for this thread until the guard drops (unit tests).
#[cfg(test)]
pub(crate) use wayhouse_config::features::off_here as off;

#[cfg(test)]
mod tests {
    use super::*;
    use wayhouse_config::features::{ACL, GEO};

    #[test]
    fn a_flag_follows_the_published_value() {
        {
            let _g = off(&["geo"]);
            assert!(on(ACL));
            assert!(!on(GEO));
        }
        assert!(on(GEO), "the switch ends with the guard");
    }
}
