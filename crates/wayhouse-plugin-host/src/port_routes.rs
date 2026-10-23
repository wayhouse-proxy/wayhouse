//! The `port_routes` capability: which entry ports a plugin may ask for and where they may
//! point (docs/plugins.md "Port routes", #403).
//!
//! A plugin hands the host its whole set of `{key, backend, proto, prefer?}` entries in one
//! `port_routes_set` call. The host checks each entry against the approval; the controller
//! assigns the entry ports, keeps `key -> port` stable and publishes the overlay. A plugin
//! reads its assignments back with `port_routes_get`.

use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::ops::RangeInclusive;

use serde::{Deserialize, Serialize};

use crate::routes::Net;

/// Most port ranges a declaration may list.
pub const MAX_PORT_RANGES: usize = 16;
/// Most backend networks a declaration may list.
pub const MAX_PORT_NETWORKS: usize = 16;
/// Host ceiling on `port_routes.max_entries`.
pub const MAX_PORT_ROUTE_ENTRIES: usize = 1024;
/// Longest entry key.
pub const MAX_PORT_KEY_BYTES: usize = 128;

/// A transport an approval may allow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PortTransport {
    Udp,
    Tcp,
}

/// What an entry serves: one transport, or both on the same port.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PortProtoSel {
    Udp,
    Tcp,
    Both,
}

impl PortProtoSel {
    fn needs(self) -> &'static [PortTransport] {
        match self {
            Self::Udp => &[PortTransport::Udp],
            Self::Tcp => &[PortTransport::Tcp],
            Self::Both => &[PortTransport::Udp, PortTransport::Tcp],
        }
    }
}

/// What a plugin may route by port.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PortRouteCap {
    /// Entry ports the host may assign: `"30001"` or `"lo-hi"`.
    pub ports: Vec<String>,
    /// Transports the plugin may use.
    pub proto: Vec<PortTransport>,
    /// Networks (`ip` or `ip/prefix`) its backends must be in.
    pub backends: Vec<String>,
    /// Most entries one set may hold.
    pub max_entries: usize,
}

/// One port route. `key` is the stable identity (a Pelican server uuid): the host assigns an
/// entry port per key and keeps it when `backend` changes. `prefer` is a hint; the host uses
/// it when free and otherwise picks the lowest free port.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PortRouteEntry {
    pub key: String,
    pub backend: String,
    pub proto: PortProtoSel,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefer: Option<u16>,
}

fn parse_range(s: &str) -> Result<RangeInclusive<u16>, String> {
    let bad = || format!("port range {s:?} must be `port` or `lo-hi` with ports 1 to 65535");
    let (lo, hi) = match s.split_once('-') {
        Some((lo, hi)) => (lo.trim(), hi.trim()),
        None => (s.trim(), s.trim()),
    };
    let (lo, hi): (u16, u16) = (
        lo.parse().map_err(|_| bad())?,
        hi.parse().map_err(|_| bad())?,
    );
    if lo == 0 || lo > hi {
        return Err(bad());
    }
    Ok(lo..=hi)
}

fn nets(list: &[String]) -> Vec<Net> {
    list.iter().filter_map(|n| Net::parse(n).ok()).collect()
}

impl PortRouteCap {
    /// The declared port ranges, parsed. Only call after [`validate`](Self::validate).
    pub fn ranges(&self) -> Vec<RangeInclusive<u16>> {
        self.ports
            .iter()
            .filter_map(|p| parse_range(p).ok())
            .collect()
    }

    /// Shape checks on a declaration.
    pub fn validate(&self) -> Result<(), String> {
        if self.ports.is_empty() || self.ports.len() > MAX_PORT_RANGES {
            return Err(format!(
                "port_routes.ports needs 1 to {MAX_PORT_RANGES} entries"
            ));
        }
        for p in &self.ports {
            parse_range(p)?;
        }
        if self.proto.is_empty()
            || self.proto.iter().collect::<BTreeSet<_>>().len() != self.proto.len()
        {
            return Err("port_routes.proto lists udp and/or tcp, each once".into());
        }
        if self.backends.is_empty() || self.backends.len() > MAX_PORT_NETWORKS {
            return Err(format!(
                "port_routes.backends needs 1 to {MAX_PORT_NETWORKS} entries"
            ));
        }
        for n in &self.backends {
            Net::parse(n)?;
        }
        if self.max_entries == 0 || self.max_entries > MAX_PORT_ROUTE_ENTRIES {
            return Err(format!(
                "port_routes.max_entries must be 1 to {MAX_PORT_ROUTE_ENTRIES}"
            ));
        }
        Ok(())
    }

    /// Whether this declaration asks for nothing `approved` does not grant.
    pub fn within(&self, approved: &PortRouteCap) -> bool {
        let granted_ports = approved.ranges();
        let granted_nets = nets(&approved.backends);
        self.max_entries <= approved.max_entries
            && self.proto.iter().all(|t| approved.proto.contains(t))
            && self.ranges().iter().all(|r| {
                granted_ports
                    .iter()
                    .any(|g| g.start() <= r.start() && r.end() <= g.end())
            })
            && self.backends.iter().all(|n| {
                Net::parse(n)
                    .ok()
                    .is_some_and(|want| granted_nets.iter().any(|a| a.covers(&want)))
            })
    }

    /// Checks one set against the capability: the count, each entry's key, backend,
    /// transport and `prefer`, and that no key appears twice.
    pub fn check_set(&self, set: &[PortRouteEntry]) -> Result<(), String> {
        if set.len() > self.max_entries {
            return Err(format!(
                "{} port routes, above the cap of {}",
                set.len(),
                self.max_entries
            ));
        }
        let granted_nets = nets(&self.backends);
        let granted_ports = self.ranges();
        let mut seen = BTreeSet::new();
        for e in set {
            if e.key.is_empty()
                || e.key.len() > MAX_PORT_KEY_BYTES
                || !e
                    .key
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'))
            {
                return Err(format!(
                    "port route key {:?} must be 1 to {MAX_PORT_KEY_BYTES} characters of letters, digits and -_.:",
                    e.key
                ));
            }
            if !seen.insert(e.key.as_str()) {
                return Err(format!("port route key {} appears twice", e.key));
            }
            if let Some(missing) = e.proto.needs().iter().find(|t| !self.proto.contains(t)) {
                return Err(format!(
                    "port route {} uses {missing:?}, which the approval does not allow",
                    e.key
                ));
            }
            let addr: SocketAddr = e
                .backend
                .parse()
                .map_err(|_| format!("port route backend {:?} is not ip:port", e.backend))?;
            if addr.port() == 0 {
                return Err(format!("port route backend {} has port 0", e.backend));
            }
            if !granted_nets.iter().any(|n| n.contains(addr.ip())) {
                return Err(format!(
                    "port route backend {} is outside the approved networks",
                    e.backend
                ));
            }
            if let Some(p) = e.prefer {
                if !granted_ports.iter().any(|r| r.contains(&p)) {
                    return Err(format!(
                        "port route {} prefers {p}, outside the approved ports",
                        e.key
                    ));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cap() -> PortRouteCap {
        PortRouteCap {
            ports: vec!["30000-30099".into(), "31000".into()],
            proto: vec![PortTransport::Udp],
            backends: vec!["10.0.0.0/16".into()],
            max_entries: 4,
        }
    }

    #[test]
    fn ranges_parse_and_bad_ones_are_refused() {
        assert_eq!(cap().ranges(), vec![30000..=30099, 31000..=31000]);
        cap().validate().unwrap();
        for bad in ["0", "5-3", "70000", "a-b", "30000-"] {
            let c = PortRouteCap {
                ports: vec![bad.into()],
                ..cap()
            };
            assert!(c.validate().is_err(), "{bad}");
        }
    }

    #[test]
    fn a_declaration_must_be_within_the_approval() {
        let ok = cap();
        let mut want = cap();
        want.ports = vec!["30010-30020".into()];
        want.max_entries = 1;
        assert!(want.within(&ok));
        for widen in [
            |c: &mut PortRouteCap| c.ports.push("32000".into()),
            |c: &mut PortRouteCap| c.ports = vec!["29990-30010".into()],
            |c: &mut PortRouteCap| c.proto.push(PortTransport::Tcp),
            |c: &mut PortRouteCap| c.backends = vec!["10.0.0.0/8".into()],
            |c: &mut PortRouteCap| c.max_entries = 5,
        ] {
            let mut w = cap();
            widen(&mut w);
            assert!(!w.within(&ok));
        }
    }
}
