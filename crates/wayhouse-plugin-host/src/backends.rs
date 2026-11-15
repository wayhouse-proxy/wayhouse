//! The `backends` capability (ABI 0.3): which backend addresses a plugin may hear about.
//!
//! The controller gathers backend health from the proxies and hands each plugin only the
//! backends inside its approved networks, so a plugin cannot learn the rest of the topology.
//! A plugin reads the current picture with `backends_get` and gets `backend_health` events
//! for changes (docs/plugins.md "Backend health").

use std::net::SocketAddr;

use serde::{Deserialize, Serialize};

use crate::routes::Net;

/// Most networks a `backends` declaration may list.
pub const MAX_BACKEND_NETWORKS: usize = 16;
/// Host ceiling on `backends.max_entries`.
pub const MAX_BACKEND_ENTRIES: usize = 1024;

/// What a plugin may learn about.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BackendsCap {
    /// Networks (`ip` or `ip/prefix`) of the backends it may hear about.
    pub networks: Vec<String>,
    /// Most backends one `backends_get` answer lists.
    pub max_entries: usize,
}

/// The health of one backend as a plugin sees it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct BackendStatus {
    /// `ip:port`.
    pub backend: String,
    pub up: bool,
    /// Unix seconds of the last change.
    pub since: u64,
}

impl BackendsCap {
    /// Shape checks on a declaration.
    pub fn validate(&self) -> Result<(), String> {
        if self.networks.is_empty() || self.networks.len() > MAX_BACKEND_NETWORKS {
            return Err(format!(
                "backends.networks needs 1 to {MAX_BACKEND_NETWORKS} entries"
            ));
        }
        for n in &self.networks {
            Net::parse(n)?;
        }
        if self.max_entries == 0 || self.max_entries > MAX_BACKEND_ENTRIES {
            return Err(format!(
                "backends.max_entries must be 1 to {MAX_BACKEND_ENTRIES}"
            ));
        }
        Ok(())
    }

    /// Whether this declaration asks for nothing `approved` does not grant.
    pub fn within(&self, approved: &BackendsCap) -> bool {
        let nets: Vec<Net> = approved
            .networks
            .iter()
            .filter_map(|n| Net::parse(n).ok())
            .collect();
        self.max_entries <= approved.max_entries
            && self.networks.iter().all(|n| {
                Net::parse(n)
                    .ok()
                    .is_some_and(|want| nets.iter().any(|a| a.covers(&want)))
            })
    }

    /// Whether `backend` (`ip:port`) is inside the approved networks.
    pub fn covers(&self, backend: &str) -> bool {
        let Ok(addr) = backend.parse::<SocketAddr>() else {
            return false;
        };
        self.networks
            .iter()
            .filter_map(|n| Net::parse(n).ok())
            .any(|n| n.contains(addr.ip()))
    }

    /// The statuses a plugin may see: those inside the networks, at most `max_entries`
    /// (the ones that changed last come first, so a cut keeps the news).
    pub fn visible(&self, all: &[BackendStatus]) -> Vec<BackendStatus> {
        let mut seen: Vec<BackendStatus> = all
            .iter()
            .filter(|s| self.covers(&s.backend))
            .cloned()
            .collect();
        seen.sort_by(|a, b| {
            b.since
                .cmp(&a.since)
                .then_with(|| a.backend.cmp(&b.backend))
        });
        seen.truncate(self.max_entries);
        seen
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cap() -> BackendsCap {
        BackendsCap {
            networks: vec!["10.0.0.0/16".into()],
            max_entries: 2,
        }
    }

    fn st(backend: &str, up: bool, since: u64) -> BackendStatus {
        BackendStatus {
            backend: backend.into(),
            up,
            since,
        }
    }

    #[test]
    fn only_backends_inside_the_networks_are_visible_newest_first_and_capped() {
        let all = [
            st("10.0.0.1:25565", true, 1),
            st("192.168.0.1:25565", false, 9),
            st("10.0.0.2:25565", false, 5),
            st("10.0.0.3:25565", true, 3),
        ];
        let seen = cap().visible(&all);
        assert_eq!(
            seen.iter().map(|s| s.backend.as_str()).collect::<Vec<_>>(),
            ["10.0.0.2:25565", "10.0.0.3:25565"]
        );
    }

    #[test]
    fn a_declaration_beyond_the_approval_is_refused() {
        let wide = BackendsCap {
            networks: vec!["10.0.0.0/8".into()],
            max_entries: 2,
        };
        assert!(cap().within(&cap()));
        assert!(!wide.within(&cap()));
        let more = BackendsCap {
            max_entries: 3,
            ..cap()
        };
        assert!(!more.within(&cap()));
    }

    #[test]
    fn a_bad_declaration_is_refused() {
        assert!(cap().validate().is_ok());
        assert!(BackendsCap {
            networks: vec![],
            ..cap()
        }
        .validate()
        .is_err());
        assert!(BackendsCap {
            networks: vec!["0.0.0.0/0".into()],
            ..cap()
        }
        .validate()
        .is_err());
        assert!(BackendsCap {
            max_entries: 0,
            ..cap()
        }
        .validate()
        .is_err());
    }
}
