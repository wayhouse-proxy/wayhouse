//! `wayhouse-certsync`: keeps a directory of certificate bundles in step with the
//! controller's ACME store (`docs/acme.md`).
//!
//! It runs on a gateway host with a gateway token. Every interval it lists the sites the token
//! may read, fetches each one whose version differs from the local copy and writes
//! `<dir>/<site>/{fullchain.pem,privkey.pem,meta.json}` atomically. A reverse proxy such as
//! Caddy loads the files from there; `--reload-cmd` tells it when they change. When the
//! controller cannot be reached the last good files stay where they are.

pub mod client;
pub mod install;
pub mod sync;

use std::time::Duration;

/// Parses `60`, `60s`, `5m` or `2h` (a bare number is seconds).
pub fn parse_interval(text: &str) -> Result<Duration, String> {
    let text = text.trim();
    let (digits, unit) = match text.char_indices().find(|(_, c)| !c.is_ascii_digit()) {
        Some((i, _)) => text.split_at(i),
        None => (text, "s"),
    };
    let n: u64 = digits
        .parse()
        .map_err(|_| format!("{text:?} is not a duration like 60s, 5m or 2h"))?;
    let secs = match unit {
        "s" => n,
        "m" => n.saturating_mul(60),
        "h" => n.saturating_mul(3600),
        _ => return Err(format!("{text:?} is not a duration like 60s, 5m or 2h")),
    };
    if secs == 0 {
        return Err("the interval must be at least 1 second".into());
    }
    Ok(Duration::from_secs(secs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intervals_parse_with_an_optional_unit() {
        assert_eq!(parse_interval("60"), Ok(Duration::from_secs(60)));
        assert_eq!(parse_interval("60s"), Ok(Duration::from_secs(60)));
        assert_eq!(parse_interval("5m"), Ok(Duration::from_secs(300)));
        assert_eq!(parse_interval("2h"), Ok(Duration::from_secs(7200)));
        for bad in ["", "s", "0", "0s", "1d", "-5", "1.5s", "5 m x"] {
            assert!(parse_interval(bad).is_err(), "{bad}");
        }
    }
}
