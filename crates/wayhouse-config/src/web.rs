//! The `web:` section: sites published through a Caddy that sits behind the `web`
//! pool (spec `docs/superpowers/specs/2026-10-15-web-routing-design.md`).
//!
//! The data path ignores the section except for the listeners [`validate_web`]
//! generates: a pool used with `via: wayhouse` gets a loopback-only TCP listener
//! `web-up-<pool>`, so Caddy reaches the origin through the ordinary pool, health
//! check, drain, tunnel and cross-node path.

use std::collections::{BTreeMap, BTreeSet};
use std::net::{IpAddr, SocketAddr};

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::cidr::Acl;
use crate::matcher::{HostPattern, Matcher};
use crate::parse::parse_host_pattern;
use crate::resolved::{Action, ListenerConfig, PoolConfig, Protocol, ProxyProtocol, Route};
use crate::ConfigError;

/// Most ports `internal_ports` may span.
const MAX_INTERNAL_PORTS: u32 = 1000;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawWeb {
    /// Pool whose backends are Caddy listeners (default `web`).
    #[serde(default = "default_pool")]
    pub(crate) pool: String,
    /// Where Caddy listens for the connections Wayhouse forwards.
    #[serde(default)]
    pub(crate) listen: RawWebListen,
    /// Address of the generated `via: wayhouse` listeners; loopback only.
    #[serde(default = "default_internal_bind")]
    pub(crate) internal_bind: String,
    /// `lo-hi` ports for the generated listeners.
    #[serde(default = "default_internal_ports")]
    pub(crate) internal_ports: String,
    #[serde(default)]
    pub(crate) sites: Vec<RawSite>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawWebListen {
    #[serde(default = "default_http")]
    pub(crate) http: String,
    #[serde(default = "default_https")]
    pub(crate) https: String,
}

impl Default for RawWebListen {
    fn default() -> Self {
        Self {
            http: default_http(),
            https: default_https(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawSite {
    #[serde(default)]
    pub(crate) hosts: Vec<String>,
    #[serde(default)]
    pub(crate) upstreams: Vec<RawUpstream>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawUpstream {
    #[serde(default)]
    pub(crate) addr: Option<String>,
    #[serde(default)]
    pub(crate) pool: Option<String>,
    #[serde(default)]
    pub(crate) via: Option<RawVia>,
    /// `via: wayhouse` only: pin the generated listener's port.
    #[serde(default)]
    pub(crate) port: Option<u16>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum RawVia {
    Direct,
    Wayhouse,
}

fn default_pool() -> String {
    "web".to_string()
}
fn default_http() -> String {
    "127.0.0.1:8080".to_string()
}
fn default_https() -> String {
    "127.0.0.1:8443".to_string()
}
fn default_internal_bind() -> String {
    "127.0.0.1".to_string()
}
fn default_internal_ports() -> String {
    "19000-19999".to_string()
}

/// Resolved `web:` section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebConfig {
    /// The pool in front of Caddy (`proxy_protocol: v2`).
    pub pool: String,
    pub listen_http: SocketAddr,
    pub listen_https: SocketAddr,
    pub sites: Vec<WebSite>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebSite {
    /// Lowercase exact names or `*.suffix` patterns.
    pub hosts: Vec<String>,
    pub upstreams: Vec<WebUpstream>,
}

/// An upstream in its final form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebUpstream {
    /// A plain `ip:port`.
    Addr(SocketAddr),
    /// A pool reached `via: direct`: the renderer resolves its backends.
    Pool { name: String },
    /// A pool reached `via: wayhouse`: Caddy connects to `addr`, the generated
    /// loopback listener that routes to `pool`.
    Local { pool: String, addr: SocketAddr },
}

fn invalid(msg: String) -> ConfigError {
    ConfigError::Invalid(msg)
}

fn host_key(p: &HostPattern) -> String {
    match p {
        HostPattern::Exact(e) => e.clone(),
        HostPattern::Suffix(s) => format!("*{s}"),
    }
}

/// `"lo-hi"` to an inclusive port range.
fn parse_ports(raw: &str) -> Result<(u16, u16), ConfigError> {
    let bad = |why: &str| invalid(format!("web.internal_ports {raw:?}: {why} (use \"lo-hi\")"));
    let (lo, hi) = raw.split_once('-').ok_or_else(|| bad("not a range"))?;
    let lo: u16 = lo.trim().parse().map_err(|_| bad("bad lower bound"))?;
    let hi: u16 = hi.trim().parse().map_err(|_| bad("bad upper bound"))?;
    if lo == 0 {
        return Err(bad("includes port 0"));
    }
    if lo > hi {
        return Err(bad("is reversed"));
    }
    if u32::from(hi - lo) + 1 > MAX_INTERNAL_PORTS {
        return Err(bad(&format!(
            "spans more than {MAX_INTERNAL_PORTS}, at most {MAX_INTERNAL_PORTS} ports"
        )));
    }
    Ok((lo, hi))
}

/// Where in a range of `width` ports a pool prefers to sit: stable across edits to
/// other pools.
fn preferred_slot(pool: &str, width: u32) -> u32 {
    let d = Sha256::digest(pool.as_bytes());
    let n = u64::from_be_bytes(d[..8].try_into().expect("a SHA-256 digest has 32 bytes"));
    u32::try_from(n % u64::from(width)).expect("a remainder below a u32 fits a u32")
}

/// Validates `raw`, appends the generated listeners and returns the resolved section.
/// `admin` is the admin API's address, kept out of the allocation.
pub(crate) fn validate_web(
    raw: RawWeb,
    pools: &[PoolConfig],
    listeners: &mut Vec<ListenerConfig>,
    admin: SocketAddr,
) -> Result<WebConfig, ConfigError> {
    let web_pool = pools
        .iter()
        .find(|p| p.name == raw.pool)
        .ok_or_else(|| invalid(format!("web.pool: unknown pool {}", raw.pool)))?;
    if web_pool.proxy_protocol != ProxyProtocol::V2 {
        return Err(invalid(format!(
            "web.pool: pool {} must set proxy_protocol v2 (Caddy's PROXY protocol listener \
             rejects plain connections)",
            raw.pool
        )));
    }
    let addr = |field: &str, v: &str| -> Result<SocketAddr, ConfigError> {
        v.parse().map_err(|_| {
            invalid(format!(
                "web.listen.{field}: {v:?} is not a valid socket address"
            ))
        })
    };
    let listen_http = addr("http", &raw.listen.http)?;
    let listen_https = addr("https", &raw.listen.https)?;

    let bind_ip: IpAddr = raw
        .internal_bind
        .parse()
        .ok()
        .filter(IpAddr::is_loopback)
        .ok_or_else(|| {
            invalid(format!(
                "web.internal_bind: {:?} must be loopback (the generated listeners are \
                 for Caddy on this host only)",
                raw.internal_bind
            ))
        })?;
    let (lo, hi) = parse_ports(&raw.internal_ports)?;

    let mut seen_hosts: BTreeMap<String, usize> = BTreeMap::new();
    let mut parsed: Vec<(Vec<String>, Vec<Pending>)> = Vec::with_capacity(raw.sites.len());
    // `via: wayhouse` pool -> pinned port (None = pick one).
    let mut local: BTreeMap<String, Option<u16>> = BTreeMap::new();
    for (i, s) in raw.sites.iter().enumerate() {
        let at = |m: &str| invalid(format!("web.sites[{i}]: {m}"));
        if s.hosts.is_empty() {
            return Err(at("needs at least one host"));
        }
        if s.upstreams.is_empty() {
            return Err(at("needs at least one upstream"));
        }
        let mut hosts = Vec::with_capacity(s.hosts.len());
        for h in &s.hosts {
            let key = host_key(&parse_host_pattern(h).map_err(|e| at(&e))?);
            if let Some(first) = seen_hosts.insert(key.clone(), i) {
                return Err(at(&format!(
                    "host {key} is also in site {first} (a host cannot be in two sites)"
                )));
            }
            hosts.push(key);
        }
        let mut ups = Vec::with_capacity(s.upstreams.len());
        for (j, u) in s.upstreams.iter().enumerate() {
            let at = |m: &str| invalid(format!("web.sites[{i}].upstreams[{j}]: {m}"));
            match (&u.addr, &u.pool) {
                (Some(a), None) => {
                    if u.via.is_some() || u.port.is_some() {
                        return Err(at("`via` and `port` apply only to a pool"));
                    }
                    let a = a
                        .parse()
                        .map_err(|_| at(&format!("addr {a:?} is not a valid socket address")))?;
                    ups.push(Pending::Addr(a));
                }
                (None, Some(p)) => {
                    let pool = pools
                        .iter()
                        .find(|c| &c.name == p)
                        .ok_or_else(|| at(&format!("unknown pool {p}")))?;
                    if u.via != Some(RawVia::Wayhouse) {
                        if u.port.is_some() {
                            return Err(at("`port` needs via: wayhouse"));
                        }
                        ups.push(Pending::Direct(p.clone()));
                        continue;
                    }
                    if pool.proxy_protocol != ProxyProtocol::None {
                        return Err(at(&format!(
                            "pool {p} sets proxy_protocol, but a via: wayhouse upstream speaks \
                             plain HTTP to the origin"
                        )));
                    }
                    if let Some(port) = u.port {
                        if !(lo..=hi).contains(&port) {
                            return Err(at(&format!(
                                "port {port} is outside internal_ports {lo}-{hi}"
                            )));
                        }
                    }
                    let slot = local.entry(p.clone()).or_insert(None);
                    match (*slot, u.port) {
                        (Some(a), Some(b)) if a != b => {
                            return Err(at(&format!(
                                "pool {p} is pinned to different ports ({a} and {b})"
                            )))
                        }
                        (_, Some(b)) => *slot = Some(b),
                        _ => {}
                    }
                    ups.push(Pending::Local(p.clone()));
                }
                _ => return Err(at("needs exactly one of addr / pool")),
            }
        }
        parsed.push((hosts, ups));
    }

    let ports = allocate(&local, bind_ip, (lo, hi), listeners, admin)?;
    for pool in local.keys() {
        let name = format!("web-up-{pool}");
        if listeners.iter().any(|l| l.name == name) {
            return Err(invalid(format!(
                "listener name {name} is already used (web: generates it for pool {pool})"
            )));
        }
    }
    for (pool, port) in &ports {
        listeners.push(ListenerConfig {
            name: format!("web-up-{pool}"),
            bind: SocketAddr::new(bind_ip, *port),
            extra_binds: Vec::new(),
            protocol: Protocol::Tcp,
            routes: vec![Route {
                matcher: Matcher::Always,
                action: Action::Pool(pool.clone()),
            }],
            prefix: None,
            freebind: false,
            transparent: false,
            sniffers: Vec::new(),
            route_hint: false,
            first_packet_gate: false,
            acl: Acl::default(),
            geo: None,
            per_source: None,
            rate_limit: None,
        });
    }

    let sites = parsed
        .into_iter()
        .map(|(hosts, ups)| WebSite {
            hosts,
            upstreams: ups
                .into_iter()
                .map(|u| match u {
                    Pending::Addr(a) => WebUpstream::Addr(a),
                    Pending::Direct(name) => WebUpstream::Pool { name },
                    Pending::Local(pool) => WebUpstream::Local {
                        addr: SocketAddr::new(bind_ip, ports[&pool]),
                        pool,
                    },
                })
                .collect(),
        })
        .collect();
    Ok(WebConfig {
        pool: raw.pool,
        listen_http,
        listen_https,
        sites,
    })
}

/// An upstream before the generated listeners have ports.
enum Pending {
    Addr(SocketAddr),
    Direct(String),
    Local(String),
}

/// Assigns each `via: wayhouse` pool a port: its pin, or the first free one from its
/// hashed slot. Pins are reserved first so no pool can take another's; pools are
/// processed in name order so the result is deterministic.
fn allocate(
    local: &BTreeMap<String, Option<u16>>,
    ip: IpAddr,
    (lo, hi): (u16, u16),
    listeners: &[ListenerConfig],
    admin: SocketAddr,
) -> Result<BTreeMap<String, u16>, ConfigError> {
    let mut taken: BTreeSet<u16> = BTreeSet::new();
    for a in listeners
        .iter()
        .filter(|l| l.protocol == Protocol::Tcp)
        .flat_map(ListenerConfig::binds)
        .chain([admin])
    {
        if a.ip() == ip || a.ip().is_unspecified() {
            taken.insert(a.port());
        }
    }
    let mut out = BTreeMap::new();
    let mut pinned: BTreeMap<u16, &str> = BTreeMap::new();
    for (pool, pin) in local {
        if let Some(port) = pin {
            if let Some(other) = pinned.insert(*port, pool) {
                return Err(invalid(format!(
                    "web: port {port} is pinned twice (pools {other} and {pool})"
                )));
            }
            if taken.contains(port) {
                return Err(invalid(format!(
                    "web: pool {pool} is pinned to port {port}, which a listener already binds"
                )));
            }
            out.insert(pool.clone(), *port);
        }
    }
    taken.extend(out.values().copied());
    let width = u32::from(hi - lo) + 1;
    for (pool, pin) in local {
        if pin.is_some() {
            continue;
        }
        let start = preferred_slot(pool, width);
        let port = (0..width)
            .map(|k| lo + u16::try_from((start + k) % width).expect("below the range width"))
            .find(|p| !taken.contains(p))
            .ok_or_else(|| {
                invalid(format!(
                    "web.internal_ports: no free port in {lo}-{hi} for pool {pool}"
                ))
            })?;
        taken.insert(port);
        out.insert(pool.clone(), port);
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
