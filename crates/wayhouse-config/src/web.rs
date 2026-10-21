//! The `web:` section: sites published through a Caddy that sits behind the `web`
//! pool (spec `docs/superpowers/specs/2026-10-15-web-routing-design.md`).
//!
//! This module validates and resolves the section and appends one loopback
//! listener `web-up-<pool>` per pool used with `via: wayhouse`; the data path
//! otherwise ignores it. The Caddy renderer is a separate slice.

use std::collections::{BTreeMap, BTreeSet};
use std::net::{IpAddr, SocketAddr};

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::cidr::Acl;
use crate::cidr::Cidr;
use crate::matcher::HostPattern;
use crate::matcher::Matcher;
use crate::parse::parse_host_pattern;
use crate::resolved::{Action, ListenerConfig, PoolConfig, Protocol, ProxyProtocol, Route};
use crate::ConfigError;

pub(crate) mod view;

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
    /// Addresses or CIDRs whose PROXY headers Caddy trusts (loopback, peer gateway nodes).
    #[serde(default)]
    pub(crate) trusted_proxy: Vec<String>,
    /// Address of the `via: wayhouse` listeners; loopback only.
    #[serde(default = "default_internal_bind")]
    pub(crate) internal_bind: String,
    /// `lo-hi` ports for the `via: wayhouse` listeners.
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
    /// Operator-supplied certificate files; unset means Caddy gets no certificate
    /// for these hosts from this section.
    #[serde(default)]
    pub(crate) tls: Option<RawSiteTls>,
    /// How Caddy talks to the upstreams; unset is plain HTTP.
    #[serde(default)]
    pub(crate) upstream_tls: Option<serde_norway::Value>,
    /// Path Caddy probes on each upstream; unset means passive checks only.
    #[serde(default)]
    pub(crate) health_path: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawSiteTls {
    pub(crate) cert: String,
    pub(crate) key: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawUpstream {
    #[serde(default)]
    pub(crate) addr: Option<String>,
    #[serde(default)]
    pub(crate) pool: Option<String>,
    #[serde(default)]
    pub(crate) via: Option<WebVia>,
    /// `via: wayhouse` only: pin the generated listener's port.
    #[serde(default)]
    pub(crate) port: Option<u16>,
}

/// How Caddy reaches a pool upstream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum WebVia {
    /// Caddy connects to the pool's backends itself.
    Direct,
    /// Caddy connects to a loopback listener that routes to the pool.
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
    /// Sources Caddy trusts PROXY headers from.
    pub trusted_proxy: Vec<Cidr>,
    /// Loopback address of the `via: wayhouse` listeners.
    pub internal_bind: IpAddr,
    /// Inclusive port range for the `via: wayhouse` listeners.
    pub internal_ports: (u16, u16),
    pub sites: Vec<WebSite>,
}

impl WebConfig {
    /// Does Caddy listen on an address other than loopback?
    pub(crate) fn listens_off_loopback(&self) -> bool {
        !(self.listen_http.ip().is_loopback() && self.listen_https.ip().is_loopback())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebSite {
    /// Lowercase exact names or `*.suffix` patterns.
    pub hosts: Vec<String>,
    pub upstreams: Vec<WebUpstream>,
    /// Certificate and key files the sync loads into Caddy for these hosts.
    pub tls: Option<WebSiteTls>,
    /// `None` is plain HTTP to the upstreams.
    pub upstream_tls: Option<UpstreamTls>,
    /// Active health-check path on each upstream.
    pub health_path: Option<String>,
}

/// Paths as the sync sees them (it reads the files, not `wayhouse`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebSiteTls {
    pub cert: String,
    pub key: String,
}

/// HTTPS from Caddy to a site's upstreams.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpstreamTls {
    /// Verify against the system roots.
    Verify,
    /// Do not verify (for a link that is already authenticated, such as a tunnel).
    Skip,
    /// Verify against this CA file.
    CaFile(String),
}

/// An upstream in its resolved form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebUpstream {
    /// A plain `ip:port`.
    Addr(SocketAddr),
    /// A defined pool reached with `via: direct`: the renderer resolves its backends.
    Pool { name: String },
    /// A pool reached with `via: wayhouse`: Caddy connects to the generated loopback
    /// listener `web-up-<pool>` at `addr`.
    Local { pool: String, addr: SocketAddr },
}

/// Name of the generated listener for a `via: wayhouse` pool.
pub fn local_listener_name(pool: &str) -> String {
    format!("web-up-{pool}")
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
            "spans more than {MAX_INTERNAL_PORTS} ports, at most {MAX_INTERNAL_PORTS} ports"
        )));
    }
    Ok((lo, hi))
}

/// `verify`, `skip`, or `{ ca_file: path }`.
fn parse_upstream_tls(v: &serde_norway::Value) -> Result<UpstreamTls, String> {
    const HINT: &str = "use verify, skip, or { ca_file: <path> }";
    if let Some(s) = v.as_str() {
        return match s {
            "verify" => Ok(UpstreamTls::Verify),
            "skip" => Ok(UpstreamTls::Skip),
            other => Err(format!("unknown mode {other:?} ({HINT})")),
        };
    }
    let ca = v
        .as_mapping()
        .filter(|m| m.len() == 1)
        .and_then(|m| m.get("ca_file"))
        .and_then(serde_norway::Value::as_str)
        .ok_or_else(|| HINT.to_string())?;
    if ca.trim().is_empty() {
        return Err("ca_file must not be empty".to_string());
    }
    Ok(UpstreamTls::CaFile(ca.to_string()))
}

/// A bare address is a /32 or /128.
fn parse_trusted(raw: &str) -> Result<Cidr, ConfigError> {
    let s = raw.trim();
    let full = match s.parse::<IpAddr>() {
        Ok(IpAddr::V4(_)) => format!("{s}/32"),
        Ok(IpAddr::V6(_)) => format!("{s}/128"),
        Err(_) => s.to_string(),
    };
    Cidr::parse(&full).map_err(|e| invalid(format!("web.trusted_proxy: {e}")))
}

/// A site host is a plain hostname (`a.example.com`) or `*.suffix`: labels of `[a-z0-9_-]`, no
/// port, no scheme, no empty label, no trailing dot and no IP literal. The leading-dot suffix
/// form accepted by route matchers is not accepted here. Hosts reach Caddy matchers and
/// later ACME names, so a typo should fail at `--check`, not at render time.
fn check_web_host(h: &str) -> Result<(), String> {
    let h = h.trim().to_ascii_lowercase();
    let name = h.strip_prefix("*.").unwrap_or(&h);
    let bad =
        || format!("host {h:?} is not a hostname (use `name.example.com` or `*.example.com`)");
    let labels: Vec<&str> = name.split('.').collect();
    let ok = labels.iter().all(|l| {
        !l.is_empty()
            && l.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
    });
    if !ok
        || name.parse::<IpAddr>().is_ok()
        || labels.iter().all(|l| l.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err(bad());
    }
    Ok(())
}

/// Whether `l` binds `port` on `ip` itself or on the matching wildcard address.
fn bind_covers(bind: SocketAddr, ip: IpAddr, port: u16) -> bool {
    bind.port() == port
        && (bind.ip() == ip || (bind.ip().is_unspecified() && bind.is_ipv4() == ip.is_ipv4()))
}

/// First 8 bytes of the SHA-256 of the pool name, big-endian.
fn name_hash(pool: &str) -> u64 {
    let d = Sha256::digest(pool.as_bytes());
    u64::from_be_bytes(d[..8].try_into().expect("sha256 digest has 32 bytes"))
}

/// Validates `raw` against the already-resolved `pools`, appends the `via: wayhouse`
/// listeners to `listeners` and returns the section. `admin` is the admin API's address.
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
    let trusted_proxy = raw
        .trusted_proxy
        .iter()
        .map(|t| parse_trusted(t))
        .collect::<Result<Vec<_>, _>>()?;

    let internal_bind: IpAddr = raw
        .internal_bind
        .parse()
        .ok()
        .filter(IpAddr::is_loopback)
        .ok_or_else(|| {
            invalid(format!(
                "web.internal_bind: {:?} must be loopback (the via: wayhouse listeners are \
                 for Caddy on this host only)",
                raw.internal_bind
            ))
        })?;
    let (lo, hi) = parse_ports(&raw.internal_ports)?;

    let mut seen_hosts: BTreeMap<String, usize> = BTreeMap::new();
    // Pinned port per `via: wayhouse` pool, and the pool owning each pinned port.
    let mut pool_pin: BTreeMap<&str, u16> = BTreeMap::new();
    let mut pin_owner: BTreeMap<u16, &str> = BTreeMap::new();
    let mut sites = Vec::with_capacity(raw.sites.len());
    let mut wayhouse_pools: BTreeSet<&str> = BTreeSet::new();
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
            check_web_host(h).map_err(|e| at(&e))?;
            let key = host_key(&parse_host_pattern(h).map_err(|e| at(&e))?);
            if let Some(first) = seen_hosts.insert(key.clone(), i) {
                return Err(at(&format!(
                    "host {key} is also in site {first} (a host cannot be in two sites)"
                )));
            }
            hosts.push(key);
        }
        let mut upstreams = Vec::with_capacity(s.upstreams.len());
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
                    upstreams.push(WebUpstream::Addr(a));
                }
                (None, Some(p)) => {
                    if *p == raw.pool {
                        return Err(at(&format!(
                            "pool {p} is web.pool; Caddy would forward requests to itself"
                        )));
                    }
                    let pool = pools
                        .iter()
                        .find(|c| &c.name == p)
                        .ok_or_else(|| at(&format!("unknown pool {p}")))?;
                    let via = u.via.unwrap_or(WebVia::Direct);
                    if via == WebVia::Direct {
                        if u.port.is_some() {
                            return Err(at("`port` needs via: wayhouse"));
                        }
                    } else {
                        if pool.proxy_protocol != ProxyProtocol::None {
                            return Err(at(&format!(
                                "pool {p} sets proxy_protocol, but a via: wayhouse upstream \
                                 speaks plain HTTP to the origin"
                            )));
                        }
                        if let Some(port) = u.port {
                            if !(lo..=hi).contains(&port) {
                                return Err(at(&format!(
                                    "port {port} is outside internal_ports {lo}-{hi}"
                                )));
                            }
                            if let Some(&prev) = pool_pin.get(p.as_str()) {
                                if prev != port {
                                    return Err(at(&format!(
                                        "pool {p} is pinned to different ports ({prev} and {port})"
                                    )));
                                }
                            }
                            if let Some(&other) = pin_owner.get(&port) {
                                if other != p {
                                    return Err(at(&format!(
                                        "port {port} is pinned twice (pools {other} and {p})"
                                    )));
                                }
                            }
                            pool_pin.insert(p, port);
                            pin_owner.insert(port, p);
                        }
                        wayhouse_pools.insert(p);
                    }
                    upstreams.push(match via {
                        WebVia::Direct => WebUpstream::Pool { name: p.clone() },
                        // Address filled in once ports are allocated below.
                        WebVia::Wayhouse => WebUpstream::Local {
                            pool: p.clone(),
                            addr: SocketAddr::new(internal_bind, 0),
                        },
                    });
                }
                _ => return Err(at("needs exactly one of addr / pool")),
            }
        }
        let tls = match &s.tls {
            None => None,
            Some(t) => {
                for (field, v) in [("cert", &t.cert), ("key", &t.key)] {
                    if v.trim().is_empty() {
                        return Err(at(&format!("tls: {field} must not be empty")));
                    }
                }
                Some(WebSiteTls {
                    cert: t.cert.clone(),
                    key: t.key.clone(),
                })
            }
        };
        let upstream_tls = s
            .upstream_tls
            .as_ref()
            .map(|v| parse_upstream_tls(v).map_err(|m| at(&format!("upstream_tls: {m}"))))
            .transpose()?;
        if let Some(p) = &s.health_path {
            if !p.starts_with('/') {
                return Err(at("health_path: must start with '/'"));
            }
            // Caddy expands `{placeholders}` in the path; whitespace and control characters
            // would corrupt the probe request line.
            if p.chars()
                .any(|c| c.is_whitespace() || c.is_control() || c == '{' || c == '}')
            {
                return Err(at(
                    "health_path: must not contain whitespace, control characters, '{' or '}'",
                ));
            }
        }
        sites.push(WebSite {
            hosts,
            upstreams,
            tls,
            upstream_tls,
            health_path: s.health_path.clone(),
        });
    }

    // Reserve what the generated listeners must not collide with, then allocate. Caddy's own
    // ports count: `internal_ports` may legitimately span 8080 or 8443.
    for pool in &wayhouse_pools {
        let name = local_listener_name(pool);
        if listeners.iter().any(|l| l.name == name) {
            return Err(invalid(format!(
                "web: listener name {name} already used (it is reserved for the via: wayhouse \
                 listener of pool {pool})"
            )));
        }
    }
    let operator_taken = |port: u16| {
        bind_covers(admin, internal_bind, port)
            || bind_covers(listen_http, internal_bind, port)
            || bind_covers(listen_https, internal_bind, port)
            || listeners
                .iter()
                .filter(|l| l.protocol == Protocol::Tcp)
                .any(|l| l.binds().any(|b| bind_covers(b, internal_bind, port)))
    };
    let mut ports: BTreeMap<&str, u16> = BTreeMap::new();
    let mut used: BTreeSet<u16> = BTreeSet::new();
    for (&pool, &port) in &pool_pin {
        if operator_taken(port) {
            return Err(invalid(format!(
                "web: pinned port {port} of pool {pool} is already bound by another listener \
                 or the admin API"
            )));
        }
        ports.insert(pool, port);
        used.insert(port);
    }
    let size = u64::from(hi - lo) + 1;
    for &pool in wayhouse_pools
        .iter()
        .filter(|p| !pool_pin.contains_key(**p))
    {
        let start = name_hash(pool) % size;
        let port = (0..size)
            .map(|k| lo + ((start + k) % size) as u16)
            .find(|p| !used.contains(p) && !operator_taken(*p))
            .ok_or_else(|| {
                invalid(format!(
                    "web: no free port in internal_ports {lo}-{hi} for pool {pool}"
                ))
            })?;
        ports.insert(pool, port);
        used.insert(port);
    }
    for site in &mut sites {
        for u in &mut site.upstreams {
            if let WebUpstream::Local { pool, addr } = u {
                *addr = SocketAddr::new(internal_bind, ports[pool.as_str()]);
            }
        }
    }
    for (&pool, &port) in &ports {
        listeners.push(ListenerConfig {
            name: local_listener_name(pool),
            bind: SocketAddr::new(internal_bind, port),
            extra_binds: Vec::new(),
            protocol: Protocol::Tcp,
            routes: vec![Route {
                matcher: Matcher::Always,
                action: Action::Pool(pool.to_string()),
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

    Ok(WebConfig {
        pool: raw.pool,
        listen_http,
        listen_https,
        trusted_proxy,
        internal_bind,
        internal_ports: (lo, hi),
        sites,
    })
}

#[cfg(test)]
mod tests;
