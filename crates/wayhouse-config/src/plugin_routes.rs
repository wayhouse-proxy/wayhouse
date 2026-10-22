//! Plugin routes merged into a config document (`docs/plugins.md` "Routes", #236, #403).
//!
//! The controller publishes the routes enabled plugins declare; a proxy applies them with
//! [`merge_text`] to the operator's document before parsing it. Only listeners that opted
//! in (`plugin_routes:`) get them, and they are appended after the listener's own routes,
//! so an operator route, including an `always` fallback, always wins. Each distinct backend
//! becomes a one-target pool named `plugin-<hash of the address>`.
//!
//! A route is keyed by a hostname (`sni` and `sniffer` listeners) or by an entry port
//! (`type: port` listeners, which bind a port range over one protocol): a listener takes
//! only the routes of its own kind, and a port route only where the port lies inside the
//! listener's bind range and the protocol fits.

use std::collections::{BTreeMap, BTreeSet};

use serde_norway::{Mapping, Value};
use sha2::{Digest, Sha256};

use crate::parse::parse_bind_spec;
use crate::ConfigError;

/// The transport(s) a port route serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortProto {
    Udp,
    Tcp,
    Both,
}

impl PortProto {
    /// `udp`, `tcp` or `both`.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "udp" => Some(Self::Udp),
            "tcp" => Some(Self::Tcp),
            "both" => Some(Self::Both),
            _ => None,
        }
    }

    fn serves(self, listener_protocol: &str) -> bool {
        matches!(
            (self, listener_protocol),
            (Self::Both, _) | (Self::Udp, "udp") | (Self::Tcp, "tcp")
        )
    }
}

/// What a plugin route is matched on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteKey {
    /// A hostname or `*.suffix` pattern.
    Host(String),
    /// A destination (entry) port.
    Port { port: u16, proto: PortProto },
}

/// One route a plugin declared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginRoute {
    pub key: RouteKey,
    /// `ip:port`.
    pub backend: String,
}

impl PluginRoute {
    pub fn host(host: impl Into<String>, backend: impl Into<String>) -> Self {
        Self {
            key: RouteKey::Host(host.into()),
            backend: backend.into(),
        }
    }

    pub fn port(port: u16, proto: PortProto, backend: impl Into<String>) -> Self {
        Self {
            key: RouteKey::Port { port, proto },
            backend: backend.into(),
        }
    }

    fn describe(&self) -> String {
        match &self.key {
            RouteKey::Host(h) => format!("{h} -> {}", self.backend),
            RouteKey::Port { port, .. } => format!("port {port} -> {}", self.backend),
        }
    }
}

/// What a merge did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Merged {
    pub text: String,
    /// Routes left out, with the reason (a pool name an operator already uses, a port no
    /// listener covers).
    pub skipped: Vec<String>,
    /// Plugin routes added to listeners, counted per listener.
    pub applied: usize,
}

/// The pool a plugin backend gets.
pub fn pool_name(backend: &str) -> String {
    let digest = Sha256::digest(backend.as_bytes());
    let hex: String = digest.iter().take(16).map(|b| format!("{b:02x}")).collect();
    format!("plugin-{hex}")
}

fn key(s: &str) -> Value {
    Value::String(s.to_string())
}

fn listeners_with_opt_in(doc: &Value) -> bool {
    doc.get("listeners")
        .and_then(Value::as_sequence)
        .is_some_and(|ls| {
            ls.iter()
                .any(|l| l.get("plugin_routes").is_some_and(Value::is_mapping))
        })
}

/// Whether any listener of `text` opted in to plugin routes. A document that does not parse
/// as YAML counts as not wanting them (the caller's own parse reports the error).
pub fn wants_plugin_routes(text: &str) -> bool {
    serde_norway::from_str::<Value>(text).is_ok_and(|d| listeners_with_opt_in(&d))
}

/// The ports a listener's `bind:` covers, or `None` for a bind that does not parse (the
/// caller's own parse reports it).
fn bind_ports(l: &Value) -> Option<std::ops::RangeInclusive<u16>> {
    let (first, extra) = parse_bind_spec("", l.get("bind")?.as_str()?).ok()?;
    let last = extra.last().unwrap_or(&first);
    Some(first.port()..=last.port())
}

/// `text` with `routes` appended to every opted-in listener. With no routes, or no opted-in
/// listener, the text comes back byte for byte.
pub fn merge_text(text: &str, routes: &[PluginRoute]) -> Result<Merged, ConfigError> {
    let unchanged = || Merged {
        text: text.to_string(),
        ..Merged::default()
    };
    if routes.is_empty() {
        return Ok(unchanged());
    }
    let mut doc: Value = serde_norway::from_str(text)?;
    if !listeners_with_opt_in(&doc) {
        return Ok(unchanged());
    }
    let mut out = Merged::default();
    let operator_pools: Vec<String> = doc
        .get("pools")
        .and_then(Value::as_sequence)
        .map(|ps| {
            ps.iter()
                .filter_map(|p| p.get("name").and_then(Value::as_str).map(str::to_string))
                .collect()
        })
        .unwrap_or_default();

    // One pool per distinct backend; a name an operator pool already has is not reused.
    let mut new_pools: Vec<(String, String)> = Vec::new();
    let mut usable: Vec<(&PluginRoute, String)> = Vec::new();
    for r in routes {
        let name = pool_name(&r.backend);
        if operator_pools.contains(&name) {
            out.skipped
                .push(format!("{}: pool name {name} is taken", r.describe()));
            continue;
        }
        if !new_pools.iter().any(|(n, _)| *n == name) {
            new_pools.push((name.clone(), r.backend.clone()));
        }
        usable.push((r, name));
    }

    let Some(listeners) = doc.get_mut("listeners").and_then(Value::as_sequence_mut) else {
        return Ok(unchanged());
    };
    // Port routes some listener took, by index into `usable`; the rest are reported.
    let mut port_taken: BTreeSet<usize> = BTreeSet::new();
    // Pool name -> whether a tcp listener uses it.
    let mut used_pools: BTreeMap<String, bool> = BTreeMap::new();
    for l in listeners {
        let Some(mode) = l.get("plugin_routes").filter(|m| m.is_mapping()).cloned() else {
            continue;
        };
        let ports = bind_ports(l);
        let listener_protocol = l
            .get("protocol")
            .and_then(Value::as_str)
            .unwrap_or("tcp")
            .to_string();
        let Some(map) = l.as_mapping_mut() else {
            continue;
        };
        // A bare `pool:` is shorthand for one `always` route: spell it out so the plugin
        // routes can follow it.
        if let Some(pool) = map.remove("pool") {
            let mut m = Mapping::new();
            m.insert(key("type"), key("always"));
            let mut a = Mapping::new();
            a.insert(key("pool"), pool);
            let mut route = Mapping::new();
            route.insert(key("match"), Value::Mapping(m));
            route.insert(key("action"), Value::Mapping(a));
            let routes = map
                .entry(key("routes"))
                .or_insert_with(|| Value::Sequence(vec![]));
            if let Some(seq) = routes.as_sequence_mut() {
                seq.insert(0, Value::Mapping(route));
            }
        }
        let kind = mode.get("type").cloned().unwrap_or_else(|| key("sni"));
        let by_port = kind.as_str() == Some("port");
        let sniffer = mode.get("sniffer").cloned();
        let seq = map
            .entry(key("routes"))
            .or_insert_with(|| Value::Sequence(vec![]));
        let Some(seq) = seq.as_sequence_mut() else {
            continue;
        };
        for (idx, (r, pool)) in usable.iter().enumerate() {
            let mut m = Mapping::new();
            m.insert(key("type"), kind.clone());
            match (&r.key, by_port) {
                (RouteKey::Host(h), false) => {
                    if let Some(s) = &sniffer {
                        m.insert(key("sniffer"), s.clone());
                    }
                    m.insert(key("host"), Value::Sequence(vec![key(h)]));
                }
                (RouteKey::Port { port, proto }, true) => {
                    if !proto.serves(&listener_protocol)
                        || !ports.as_ref().is_some_and(|p| p.contains(port))
                    {
                        continue;
                    }
                    port_taken.insert(idx);
                    m.insert(key("ports"), Value::Sequence(vec![Value::Number((*port).into())]));
                }
                _ => continue,
            }
            let mut a = Mapping::new();
            a.insert(key("pool"), key(pool));
            let mut route = Mapping::new();
            route.insert(key("match"), Value::Mapping(m));
            route.insert(key("action"), Value::Mapping(a));
            seq.push(Value::Mapping(route));
            *used_pools.entry(pool.clone()).or_insert(false) |= listener_protocol == "tcp";
            out.applied += 1;
        }
    }
    for (idx, (r, _)) in usable.iter().enumerate() {
        if matches!(r.key, RouteKey::Port { .. }) && !port_taken.contains(&idx) {
            out.skipped.push(format!(
                "{}: no port listener with plugin_routes covers that port and protocol",
                r.describe()
            ));
        }
    }

    if out.applied > 0 {
        let pools = doc
            .as_mapping_mut()
            .expect("a document with listeners is a mapping")
            .entry(key("pools"))
            .or_insert_with(|| Value::Sequence(vec![]));
        if let Some(seq) = pools.as_sequence_mut() {
            for (name, backend) in new_pools {
                let Some(&tcp) = used_pools.get(&name) else {
                    continue;
                };
                let mut p = Mapping::new();
                p.insert(key("name"), key(&name));
                p.insert(key("targets"), Value::Sequence(vec![key(&backend)]));
                if !tcp {
                    // Only udp listeners use it: a tcp connect probe would mark it down.
                    let mut hc = Mapping::new();
                    hc.insert(key("type"), key("none"));
                    p.insert(key("health_check"), Value::Mapping(hc));
                }
                seq.push(Value::Mapping(p));
            }
        }
    }
    out.text = serde_norway::to_string(&doc)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{parse_str, Action, Matcher};

    const BASE: &str = r#"
schema_version: 2
pools:
  - name: lobby
    targets: ["10.9.0.1:25565"]
listeners:
  - name: mc
    bind: "0.0.0.0:25565"
    protocol: tcp
    plugin_routes: { type: sni }
    routes:
      - match: { type: sni, host: ["lobby.example.com"] }
        action: { pool: lobby }
"#;

    fn r(host: &str, backend: &str) -> PluginRoute {
        PluginRoute::host(host, backend)
    }

    fn pr(port: u16, proto: PortProto, backend: &str) -> PluginRoute {
        PluginRoute::port(port, proto, backend)
    }

    const PORTS: &str = r#"
schema_version: 2
pools:
  - name: lobby
    targets: ["10.9.0.1:25565"]
    health_check: { type: none }
listeners:
  - name: games-udp
    bind: "0.0.0.0:30000-30009"
    protocol: udp
    plugin_routes: { type: port }
    routes:
      - match: { type: port, ports: [30000] }
        action: { pool: lobby }
  - name: games-tcp
    bind: "0.0.0.0:30000-30009"
    protocol: tcp
    plugin_routes: { type: port }
"#;

    fn pool_of(l: &crate::ListenerConfig, i: usize) -> String {
        let Action::Pool(p) = &l.routes[i].action else {
            panic!("route {i} is not a pool")
        };
        p.clone()
    }

    #[test]
    fn port_routes_follow_the_operators_per_protocol() {
        let m = merge_text(
            PORTS,
            &[
                pr(30003, PortProto::Udp, "10.0.1.1:7777"),
                pr(30004, PortProto::Both, "10.0.1.2:25565"),
                pr(30005, PortProto::Tcp, "10.0.1.3:25565"),
            ],
        )
        .unwrap();
        assert_eq!(m.applied, 4, "udp gets 2, tcp gets 2");
        assert!(m.skipped.is_empty(), "{:?}", m.skipped);
        let cfg = parse_str(&m.text).unwrap();
        let (udp, tcp) = (&cfg.listeners[0], &cfg.listeners[1]);
        assert_eq!(udp.routes.len(), 3);
        assert_eq!(pool_of(udp, 0), "lobby", "the operator's route stays first");
        assert_eq!(pool_of(udp, 1), pool_name("10.0.1.1:7777"));
        assert_eq!(pool_of(udp, 2), pool_name("10.0.1.2:25565"));
        assert_eq!(tcp.routes.len(), 2);
        assert_eq!(pool_of(tcp, 0), pool_name("10.0.1.2:25565"));
        assert_eq!(pool_of(tcp, 1), pool_name("10.0.1.3:25565"));
        let Matcher::DstPort(r) = &udp.routes[1].matcher else {
            panic!()
        };
        assert_eq!(r, &vec![30003..=30003]);
    }

    #[test]
    fn a_port_outside_the_bind_range_is_skipped_and_reported() {
        let m = merge_text(
            PORTS,
            &[
                pr(31000, PortProto::Both, "10.0.1.1:7777"),
                pr(30001, PortProto::Udp, "10.0.1.2:7777"),
            ],
        )
        .unwrap();
        assert_eq!(m.applied, 1);
        assert_eq!(m.skipped.len(), 1, "{:?}", m.skipped);
        assert!(m.skipped[0].contains("31000"), "{:?}", m.skipped);
        let cfg = parse_str(&m.text).unwrap();
        assert_eq!(cfg.pools.len(), 2, "lobby plus the one pool in use");
    }

    #[test]
    fn host_and_port_routes_do_not_cross_listener_types() {
        let mixed = format!(
            "{}  - name: mc\n    bind: \"0.0.0.0:25565\"\n    protocol: tcp\n    plugin_routes: {{ type: sni }}\n",
            PORTS
        );
        let m = merge_text(
            &mixed,
            &[
                r("a.example.com", "10.0.1.1:25565"),
                pr(30002, PortProto::Tcp, "10.0.1.2:25565"),
            ],
        )
        .unwrap();
        let cfg = parse_str(&m.text).unwrap();
        assert_eq!(cfg.listeners[0].routes.len(), 1, "udp: operator route only");
        assert_eq!(cfg.listeners[1].routes.len(), 1, "tcp port listener: the port route");
        let mc = &cfg.listeners[2];
        assert_eq!(mc.routes.len(), 1);
        assert!(matches!(mc.routes[0].matcher, Matcher::Sni { .. }));
    }

    #[test]
    fn an_unclaimed_port_in_the_range_has_no_route() {
        let m = merge_text(PORTS, &[pr(30003, PortProto::Udp, "10.0.1.1:7777")]).unwrap();
        let cfg = parse_str(&m.text).unwrap();
        let matches = |port: u16| {
            cfg.listeners[0]
                .routes
                .iter()
                .any(|r| matches!(&r.matcher, Matcher::DstPort(rs) if rs.iter().any(|x| x.contains(&port))))
        };
        assert!(matches(30003));
        assert!(!matches(30007), "no route, the datagram is dropped as no_route");
        assert!(!cfg.listeners[0]
            .routes
            .iter()
            .any(|r| matches!(r.matcher, Matcher::Always)));
    }

    #[test]
    fn the_port_opt_in_is_checked() {
        for bad in [
            "plugin_routes: { type: port, sniffer: minecraft }",
        ] {
            let text = PORTS.replace("plugin_routes: { type: port }", bad);
            assert!(parse_str(&text).is_err(), "{bad}");
        }
        // A tcp or udp listener can opt in; a listener of either protocol takes it.
        assert!(parse_str(PORTS).is_ok());
        // The port variant needs schema 2 like the others.
        let old = PORTS.replace("schema_version: 2\n", "schema_version: 1\n");
        assert!(parse_str(&old).is_err());
    }

    #[test]
    fn plugin_routes_follow_the_operators_and_get_one_pool_per_backend() {
        let m = merge_text(
            BASE,
            &[
                r("a.example.com", "10.0.1.1:25565"),
                r("b.example.com", "10.0.1.1:25565"),
                r("*.eu.example.com", "10.0.1.2:25565"),
            ],
        )
        .unwrap();
        assert_eq!(m.applied, 3);
        let cfg = parse_str(&m.text).unwrap();
        assert_eq!(cfg.pools.len(), 3, "lobby plus two plugin pools");
        let l = &cfg.listeners[0];
        assert_eq!(l.routes.len(), 4);
        let Action::Pool(first) = &l.routes[0].action else {
            panic!()
        };
        assert_eq!(first, "lobby", "the operator's route stays first");
        let Action::Pool(second) = &l.routes[1].action else {
            panic!()
        };
        assert_eq!(*second, pool_name("10.0.1.1:25565"));
        assert!(matches!(l.routes[1].matcher, Matcher::Sni { .. }));
    }

    #[test]
    fn a_bare_pool_listener_keeps_its_pool_ahead_of_the_plugin_routes() {
        let text = r#"
schema_version: 2
pools:
  - name: lobby
    targets: ["10.9.0.1:25565"]
listeners:
  - name: mc
    bind: "0.0.0.0:25565"
    protocol: tcp
    pool: lobby
    plugin_routes: { type: sni }
"#;
        let m = merge_text(text, &[r("a.example.com", "10.0.1.1:25565")]).unwrap();
        let cfg = parse_str(&m.text).unwrap();
        let routes = &cfg.listeners[0].routes;
        assert_eq!(routes.len(), 2);
        assert!(matches!(routes[0].matcher, Matcher::Always));
    }

    #[test]
    fn nothing_changes_without_routes_or_an_opted_in_listener() {
        assert_eq!(merge_text(BASE, &[]).unwrap().text, BASE);
        let plain = BASE.replace("    plugin_routes: { type: sni }\n", "");
        let m = merge_text(&plain, &[r("a.example.com", "10.0.1.1:1")]).unwrap();
        assert_eq!((m.text.as_str(), m.applied), (plain.as_str(), 0));
        assert!(wants_plugin_routes(BASE));
        assert!(!wants_plugin_routes(&plain));
    }

    #[test]
    fn a_null_plugin_routes_value_is_not_an_opt_in() {
        let nulled = BASE.replace("plugin_routes: { type: sni }", "plugin_routes: ~");
        assert!(!wants_plugin_routes(&nulled));
        let m = merge_text(&nulled, &[r("a.example.com", "10.0.1.1:1")]).unwrap();
        assert_eq!((m.text.as_str(), m.applied), (nulled.as_str(), 0));
    }

    #[test]
    fn pool_names_keep_128_bits_of_the_hash() {
        let name = pool_name("10.0.1.1:25565");
        assert_eq!(name.len(), "plugin-".len() + 32, "{name}");
    }

    #[test]
    fn a_pool_name_an_operator_already_uses_is_not_reused() {
        let taken = pool_name("10.0.1.1:25565");
        let text = BASE
            .replace("name: lobby", &format!("name: {taken}"))
            .replace("pool: lobby", &format!("pool: {taken}"));
        let m = merge_text(&text, &[r("a.example.com", "10.0.1.1:25565")]).unwrap();
        assert_eq!(m.applied, 0);
        assert_eq!(m.skipped.len(), 1);
    }

    #[test]
    fn the_opt_in_field_is_checked() {
        for bad in [
            "plugin_routes: { type: nope }",
            "plugin_routes: { type: sniffer }",
            "plugin_routes: { type: sni, sniffer: minecraft }",
            // A typo that is not a mapping is an error, never a silent opt-out (#317).
            "plugin_routes: sni",
            "plugin_routes: true",
            "plugin_routes: [sni]",
        ] {
            let text = BASE.replace("plugin_routes: { type: sni }", bad);
            assert!(parse_str(&text).is_err(), "{bad}");
        }
        let udp = "schema_version: 2\npools:\n  - name: p\n    targets: [\"10.9.0.1:1\"]\nlisteners:\n  - name: u\n    bind: \"0.0.0.0:1\"\n    protocol: udp\n    pool: p\n    plugin_routes: { type: sni }\n";
        assert!(parse_str(udp)
            .unwrap_err()
            .to_string()
            .contains("tcp listeners"));
        // The field needs a schema that has it.
        let old = BASE.replace("schema_version: 2\n", "schema_version: 1\n");
        assert!(parse_str(&old)
            .unwrap_err()
            .to_string()
            .contains("schema_version 2"));
    }
}
