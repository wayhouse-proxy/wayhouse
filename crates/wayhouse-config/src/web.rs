//! The `web:` section: sites published through a Caddy that sits behind the `web`
//! pool (spec `docs/superpowers/specs/2026-10-15-web-routing-design.md`).
//!
//! This module only validates and resolves the section; the data path ignores it.
//! Generated `via: wayhouse` listeners and the renderer are separate slices.

use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr};

use serde::Deserialize;

use crate::cidr::Cidr;
use crate::matcher::HostPattern;
use crate::parse::parse_host_pattern;
use crate::resolved::{PoolConfig, ProxyProtocol};
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
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
}

/// An upstream in its resolved form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebUpstream {
    /// A plain `ip:port`.
    Addr(SocketAddr),
    /// A defined pool, reached as `via` says; `port` pins a `via: wayhouse` listener.
    Pool {
        name: String,
        via: WebVia,
        port: Option<u16>,
    },
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

/// Validates `raw` against the already-resolved `pools` and returns the section.
pub(crate) fn validate_web(raw: RawWeb, pools: &[PoolConfig]) -> Result<WebConfig, ConfigError> {
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
                    }
                    upstreams.push(WebUpstream::Pool {
                        name: p.clone(),
                        via,
                        port: u.port,
                    });
                }
                _ => return Err(at("needs exactly one of addr / pool")),
            }
        }
        sites.push(WebSite { hosts, upstreams });
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
