//! When to renew and how long to wait after a failure.

use std::time::Duration;

/// Renewal starts when less than this fraction of the lifetime is left.
const RENEW_FRACTION: f64 = 1.0 / 3.0;
/// The renewal point moves by at most this fraction of the lifetime, so a fleet issued
/// together does not renew together.
pub const JITTER_MAX: f64 = 0.05;

const BACKOFF_MIN: Duration = Duration::from_secs(5 * 60);
const BACKOFF_MAX: Duration = Duration::from_secs(6 * 3600);

/// True when the certificate should be renewed at `now` (Unix seconds). `jitter_frac` is in
/// `[0, 1]` and moves the renewal point earlier by up to [`JITTER_MAX`] of the lifetime.
pub fn due(issued_at: u64, not_after: u64, now: u64, jitter_frac: f64) -> bool {
    let life = not_after.saturating_sub(issued_at) as f64;
    let left = not_after.saturating_sub(now) as f64;
    let threshold = life * (RENEW_FRACTION + JITTER_MAX * jitter_frac.clamp(0.0, 1.0));
    left <= threshold
}

/// Wait after `failures` consecutive failures: 5 minutes, doubling, capped at 6 hours.
pub fn backoff(failures: u32) -> Duration {
    let mult = 1u32.checked_shl(failures.min(16)).unwrap_or(u32::MAX);
    BACKOFF_MIN.saturating_mul(mult).min(BACKOFF_MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: u64 = 86_400;

    #[test]
    fn not_due_at_half_life_due_at_a_third() {
        let (i, n) = (0, 90 * DAY);
        assert!(!due(i, n, 45 * DAY, 0.0));
        assert!(!due(i, n, 59 * DAY, 0.0));
        assert!(due(i, n, 60 * DAY, 0.0));
        assert!(due(i, n, 89 * DAY, 0.0));
        assert!(due(i, n, 91 * DAY, 0.0), "expired is due");
    }

    #[test]
    fn jitter_moves_the_edge_by_at_most_five_percent() {
        let (i, n) = (0, 90 * DAY);
        // 5 % of 90 days = 4.5 days earlier at most.
        assert!(!due(i, n, 55 * DAY, 0.0));
        assert!(due(i, n, 55 * DAY + DAY / 2, 1.0));
        assert!(!due(i, n, 55 * DAY - DAY, 1.0));
        assert!(due(i, n, 60 * DAY, 5.0), "jitter is clamped");
    }

    #[test]
    fn short_lived_certs_scale() {
        assert!(due(0, 6 * DAY, 4 * DAY + 1, 0.0));
        assert!(!due(0, 6 * DAY, 3 * DAY, 0.0));
    }

    #[test]
    fn backoff_doubles_and_caps() {
        assert_eq!(backoff(0), Duration::from_secs(300));
        assert_eq!(backoff(1), Duration::from_secs(600));
        assert_eq!(backoff(2), Duration::from_secs(1200));
        assert_eq!(backoff(5), Duration::from_secs(9600));
        assert_eq!(backoff(7), Duration::from_secs(6 * 3600));
        assert_eq!(backoff(1000), Duration::from_secs(6 * 3600));
    }
}
