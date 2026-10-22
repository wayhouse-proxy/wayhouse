//! The certificate model and site-name validation.

use std::collections::HashSet;
use std::fmt;

use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

/// A secret string: prints `<redacted>`, zeroizes on drop. Serializes as the plain value
/// because its only serialized homes are the sealed store and the scoped bundle response.
#[derive(Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Redacted(Zeroizing<String>);

impl Redacted {
    pub fn new(s: impl Into<String>) -> Self {
        Self(Zeroizing::new(s.into()))
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Redacted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

impl fmt::Display for Redacted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

/// An issued certificate with its private key. Times are Unix seconds.
#[derive(Debug, Clone)]
pub struct CertBundle {
    pub chain_pem: String,
    pub key_pem: Redacted,
    pub not_after: u64,
    pub issued_at: u64,
    pub serial: String,
    pub names: Vec<String>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum NameError {
    #[error("no names")]
    Empty,
    #[error("invalid name {0:?}")]
    Invalid(String),
    #[error("duplicate name {0:?}")]
    Duplicate(String),
    #[error("too many names ({0}, at most {MAX_NAMES})")]
    TooMany(usize),
}

/// The most names one certificate may carry (the Let's Encrypt limit).
pub const MAX_NAMES: usize = 100;

/// A site's names: lowercase DNS hostnames, optionally `*.zone` (wildcard first label only).
pub fn validate_names(names: &[String]) -> Result<(), NameError> {
    if names.is_empty() {
        return Err(NameError::Empty);
    }
    if names.len() > MAX_NAMES {
        return Err(NameError::TooMany(names.len()));
    }
    let mut seen = HashSet::with_capacity(names.len());
    for n in names {
        if !valid_name(n) {
            return Err(NameError::Invalid(n.clone()));
        }
        if !seen.insert(n.as_str()) {
            return Err(NameError::Duplicate(n.clone()));
        }
    }
    Ok(())
}

/// A zone name for a delegated challenge: a hostname without a wildcard.
pub fn valid_zone(zone: &str) -> bool {
    !zone.starts_with("*.") && valid_name(zone)
}

fn valid_name(n: &str) -> bool {
    let host = n.strip_prefix("*.").unwrap_or(n);
    if n.len() > 253 || host.split('.').count() < 2 {
        return false;
    }
    host.split('.').all(|l| {
        !l.is_empty()
            && l.len() <= 63
            && !l.starts_with('-')
            && !l.ends_with('-')
            && l.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(names: &[&str]) -> Result<(), NameError> {
        validate_names(&names.iter().map(ToString::to_string).collect::<Vec<_>>())
    }

    #[test]
    fn accepts_hosts_and_wildcards() {
        assert_eq!(
            v(&["example.com", "*.example.com", "a-b.example.com"]),
            Ok(())
        );
    }

    #[test]
    fn rejects_bad_names() {
        assert_eq!(v(&[]), Err(NameError::Empty));
        for bad in [
            "Example.com",
            "",
            "*",
            "a.*.b",
            "*.com.*",
            "localhost",
            "-a.com",
            "a..com",
            "a_b.com",
        ] {
            assert_eq!(v(&[bad]), Err(NameError::Invalid(bad.into())), "{bad}");
        }
        assert_eq!(
            v(&["a.com", "a.com"]),
            Err(NameError::Duplicate("a.com".into()))
        );
    }

    #[test]
    fn caps_the_number_of_names() {
        let many =
            |n: usize| -> Vec<String> { (0..n).map(|i| format!("h{i}.example.com")).collect() };
        assert_eq!(validate_names(&many(MAX_NAMES)), Ok(()));
        assert_eq!(
            validate_names(&many(MAX_NAMES + 1)),
            Err(NameError::TooMany(MAX_NAMES + 1))
        );
    }

    #[test]
    fn redacted_never_prints() {
        let r = Redacted::new("sentinel-key");
        assert_eq!(format!("{r:?} {r}"), "<redacted> <redacted>");
        let b = CertBundle {
            chain_pem: String::new(),
            key_pem: r,
            not_after: 0,
            issued_at: 0,
            serial: String::new(),
            names: vec![],
        };
        assert!(!format!("{b:?}").contains("sentinel-key"));
    }
}
