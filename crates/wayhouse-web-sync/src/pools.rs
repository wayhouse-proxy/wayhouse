//! `GET /pools` of the local `wayhouse` (plaintext, one pool per unindented line and one
//! backend per indented line) reduced to what `via: direct` needs: each pool's
//! enabled backends. A draining or disabled backend is left out, which is how a drain
//! reaches Caddy (it cannot see wayhouse's admin state itself).

use std::collections::BTreeMap;
use std::net::SocketAddr;

pub type PoolBackends = BTreeMap<String, Vec<SocketAddr>>;

/// Why a `/pools` body could not be read. Any of these fails the cycle: dropping a
/// backend we could not parse would silently remove it from Caddy.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("unreadable /pools line {line}: {text:?}")]
pub struct PoolsParseError {
    pub line: usize,
    pub text: String,
}

pub fn parse(body: &str) -> Result<PoolBackends, PoolsParseError> {
    let mut out = PoolBackends::new();
    let mut current: Option<&str> = None;
    for (i, raw) in body.lines().enumerate() {
        let bad = || PoolsParseError {
            line: i + 1,
            text: raw.to_string(),
        };
        if raw.trim().is_empty() {
            continue;
        }
        let mut cols = raw.split('\t');
        let first = cols.next().ok_or_else(bad)?;
        if !raw.starts_with(char::is_whitespace) {
            out.entry(first.to_string()).or_default();
            current = Some(first);
            continue;
        }
        let pool = current.ok_or_else(bad)?;
        let addr: SocketAddr = first.trim().parse().map_err(|_| bad())?;
        let state = cols
            .find_map(|c| c.strip_prefix("state="))
            .ok_or_else(bad)?;
        if state == "enabled" {
            out.get_mut(pool).ok_or_else(bad)?.push(addr);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BODY: &str = "web\tbalancer=RoundRobin\n  127.0.0.1:8443\thealthy\tstate=enabled\tactive=0\n\
origin\tbalancer=RoundRobin\n  10.0.0.5:8080\thealthy\tstate=enabled\tactive=3\n  \
10.0.0.6:8080\tunhealthy\tstate=draining\tactive=1\n  [fd00::7]:8080\thealthy\tstate=disabled\tactive=0\n\
empty\tbalancer=Random\n";

    #[test]
    fn keeps_only_enabled_backends_and_every_pool() {
        let p = parse(BODY).unwrap();
        assert_eq!(p["web"], vec!["127.0.0.1:8443".parse().unwrap()]);
        assert_eq!(p["origin"], vec!["10.0.0.5:8080".parse().unwrap()]);
        assert!(p["empty"].is_empty());
    }

    #[test]
    fn an_unreadable_backend_line_fails_instead_of_vanishing() {
        let e = parse("p\tbalancer=x\n  not-an-addr\thealthy\tstate=enabled\n").unwrap_err();
        assert_eq!(e.line, 2);
        assert!(parse("  10.0.0.1:1\thealthy\tstate=enabled\n").is_err());
        assert!(parse("p\tbalancer=x\n  10.0.0.1:1\thealthy\n").is_err());
    }
}
