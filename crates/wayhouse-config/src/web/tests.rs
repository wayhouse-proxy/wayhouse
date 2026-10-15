use crate::{parse_str, Action, Config, ConfigError, Matcher, Protocol, WebUpstream};

const POOLS: &str = "\
pools:
  - name: web
    targets: [\"127.0.0.1:8443\"]
    proxy_protocol: v2
  - name: origin
    targets: [\"10.0.0.5:8080\"]
  - name: wings1
    targets: [\"10.0.0.6:8443\"]
  - name: wings2
    targets: [\"10.0.0.7:8443\"]
";

const LISTENERS: &str = "\
listeners:
  - name: https
    bind: \"0.0.0.0:443\"
    routes:
      - match: { type: sni, host: [\"*.example.com\"] }
        action: { pool: web }
";

fn doc(web: &str) -> String {
    format!("schema_version: 3\n{POOLS}{LISTENERS}web:\n{web}")
}

fn parse(web: &str) -> Result<Config, ConfigError> {
    parse_str(&doc(web))
}

fn err(web: &str) -> String {
    match parse(web) {
        Ok(_) => panic!("expected an error for:\n{web}"),
        Err(e) => e.to_string(),
    }
}

fn site(upstreams: &str) -> String {
    format!("  sites:\n    - hosts: [panel.example.com]\n      upstreams:\n{upstreams}")
}

fn generated<'a>(cfg: &'a Config, pool: &str) -> &'a crate::ListenerConfig {
    let name = format!("web-up-{pool}");
    cfg.listeners
        .iter()
        .find(|l| l.name == name)
        .unwrap_or_else(|| panic!("no listener {name}"))
}

#[test]
fn defaults_resolve_and_plain_upstreams_stay_as_written() {
    let cfg = parse(&site(
        "        - addr: 10.0.0.9:8080\n        - pool: origin\n",
    ))
    .unwrap();
    let web = cfg.web.as_ref().unwrap();
    assert_eq!(web.pool, "web");
    assert_eq!(web.listen_http.to_string(), "127.0.0.1:8080");
    assert_eq!(web.listen_https.to_string(), "127.0.0.1:8443");
    assert_eq!(
        web.sites[0].upstreams,
        vec![
            WebUpstream::Addr("10.0.0.9:8080".parse().unwrap()),
            WebUpstream::Pool {
                name: "origin".into()
            },
        ]
    );
    assert!(cfg.listeners.iter().all(|l| !l.name.starts_with("web-up-")));
}

#[test]
fn absent_web_section_changes_nothing() {
    let cfg = parse_str(&format!("{POOLS}{LISTENERS}")).unwrap();
    assert!(cfg.web.is_none());
    assert_eq!(cfg.listeners.len(), 1);
}

#[test]
fn via_wayhouse_adds_a_loopback_tcp_listener_routed_to_the_pool() {
    let cfg = parse(&site("        - pool: wings1\n          via: wayhouse\n")).unwrap();
    let l = generated(&cfg, "wings1");
    assert_eq!(l.protocol, Protocol::Tcp);
    assert!(l.bind.ip().is_loopback());
    assert!((19000..=19999).contains(&l.bind.port()));
    assert_eq!(l.routes.len(), 1);
    assert!(matches!(l.routes[0].matcher, Matcher::Always));
    assert!(matches!(&l.routes[0].action, Action::Pool(p) if p == "wings1"));
    let web = cfg.web.as_ref().unwrap();
    assert_eq!(
        web.sites[0].upstreams,
        vec![WebUpstream::Local {
            pool: "wings1".into(),
            addr: l.bind
        }]
    );
}

#[test]
fn a_pool_used_by_two_sites_gets_one_listener() {
    let web = "  sites:
    - hosts: [a.example.com]
      upstreams:
        - { pool: wings1, via: wayhouse }
    - hosts: [b.example.com]
      upstreams:
        - { pool: wings1, via: wayhouse }
";
    let cfg = parse(web).unwrap();
    let n = cfg
        .listeners
        .iter()
        .filter(|l| l.name == "web-up-wings1")
        .count();
    assert_eq!(n, 1);
}

#[test]
fn a_pinned_port_is_used() {
    let cfg = parse(&site(
        "        - { pool: wings1, via: wayhouse, port: 19010 }\n",
    ))
    .unwrap();
    assert_eq!(generated(&cfg, "wings1").bind.port(), 19010);
}

#[test]
fn ports_do_not_move_when_another_pool_is_removed() {
    let both = "  sites:
    - hosts: [a.example.com]
      upstreams:
        - { pool: wings1, via: wayhouse }
        - { pool: wings2, via: wayhouse }
";
    let one = "  sites:
    - hosts: [a.example.com]
      upstreams:
        - { pool: wings2, via: wayhouse }
";
    let a = parse(both).unwrap();
    let b = parse(one).unwrap();
    assert_ne!(
        generated(&a, "wings1").bind.port(),
        generated(&a, "wings2").bind.port()
    );
    assert_eq!(
        generated(&a, "wings2").bind.port(),
        generated(&b, "wings2").bind.port()
    );
}

#[test]
fn a_tiny_range_forces_collisions_to_the_next_free_port() {
    let web = "  internal_ports: \"19000-19001\"
  sites:
    - hosts: [a.example.com]
      upstreams:
        - { pool: wings1, via: wayhouse }
        - { pool: wings2, via: wayhouse }
";
    let cfg = parse(web).unwrap();
    let mut ports = [
        generated(&cfg, "wings1").bind.port(),
        generated(&cfg, "wings2").bind.port(),
    ];
    ports.sort_unstable();
    assert_eq!(ports, [19000, 19001]);
}

#[test]
fn a_full_range_is_an_error() {
    let web = "  internal_ports: \"19000-19000\"
  sites:
    - hosts: [a.example.com]
      upstreams:
        - { pool: wings1, via: wayhouse }
        - { pool: wings2, via: wayhouse }
";
    assert!(err(web).contains("no free port"));
}

#[test]
fn an_operator_listener_on_the_candidate_port_is_skipped() {
    let web = "  internal_ports: \"19000-19001\"
  sites:
    - hosts: [a.example.com]
      upstreams:
        - { pool: wings1, via: wayhouse }
";
    let free = parse(web).unwrap();
    let taken = generated(&free, "wings1").bind;
    let other = if taken.port() == 19000 { 19001 } else { 19000 };
    let y = format!(
        "schema_version: 3\n{POOLS}{LISTENERS}  - name: squat\n    bind: \"127.0.0.1:{}\"\n    pool: origin\nweb:\n{web}",
        taken.port()
    );
    let cfg = parse_str(&y).unwrap();
    assert_eq!(generated(&cfg, "wings1").bind.port(), other);
}

#[test]
fn the_web_pool_must_exist_and_use_proxy_protocol_v2() {
    assert!(err(&format!(
        "  pool: nope\n{}",
        site("        - addr: 10.0.0.9:80\n")
    ))
    .contains("unknown pool nope"));
    let y = doc(&site("        - addr: 10.0.0.9:80\n")).replace("proxy_protocol: v2", "");
    let e = parse_str(&y).unwrap_err().to_string();
    assert!(e.contains("must set proxy_protocol v2"), "{e}");
}

#[test]
fn a_host_may_belong_to_one_site_only() {
    let web = "  sites:
    - hosts: [a.example.com, \"*.apps.example.com\"]
      upstreams: [{ addr: \"10.0.0.9:80\" }]
    - hosts: [\"*.APPS.example.com\"]
      upstreams: [{ addr: \"10.0.0.9:81\" }]
";
    let e = err(web);
    assert!(
        e.contains("host *.apps.example.com") && e.contains("two sites"),
        "{e}"
    );
}

#[test]
fn sites_and_upstreams_need_content() {
    assert!(
        err("  sites:\n    - hosts: []\n      upstreams: [{ addr: \"10.0.0.9:80\" }]\n")
            .contains("at least one host")
    );
    assert!(
        err("  sites:\n    - hosts: [a.example.com]\n      upstreams: []\n")
            .contains("at least one upstream")
    );
    assert!(err(
        "  sites:\n    - hosts: [\"a.*\"]\n      upstreams: [{ addr: \"10.0.0.9:80\" }]\n"
    )
    .contains("invalid host pattern"));
}

#[test]
fn an_upstream_is_exactly_one_of_addr_or_pool() {
    assert!(err(&site("        - {}\n")).contains("exactly one of addr / pool"));
    assert!(
        err(&site("        - { addr: \"10.0.0.9:80\", pool: origin }\n"))
            .contains("exactly one of addr / pool")
    );
    assert!(err(&site(
        "        - { addr: \"10.0.0.9:80\", via: wayhouse }\n"
    ))
    .contains("`via` and `port` apply only to a pool"));
    assert!(err(&site("        - { pool: origin, port: 19010 }\n"))
        .contains("`port` needs via: wayhouse"));
    assert!(err(&site("        - { pool: ghost }\n")).contains("unknown pool ghost"));
    assert!(err(&site("        - { addr: \"not-an-address\" }\n")).contains("not a valid"));
}

#[test]
fn generated_listeners_stay_on_loopback() {
    let e = err(&format!(
        "  internal_bind: 0.0.0.0\n{}",
        site("        - { pool: wings1, via: wayhouse }\n")
    ));
    assert!(e.contains("must be loopback"), "{e}");
}

#[test]
fn port_range_and_pins_are_checked() {
    let s = |ports: &str, up: &str| format!("  internal_ports: \"{ports}\"\n{}", site(up));
    assert!(err(&s("19000-20500", "        - addr: 10.0.0.9:80\n")).contains("at most 1000"));
    assert!(err(&s("20000-19000", "        - addr: 10.0.0.9:80\n")).contains("internal_ports"));
    assert!(err(&s(
        "19000-19999",
        "        - { pool: wings1, via: wayhouse, port: 5000 }\n"
    ))
    .contains("outside internal_ports"));
    let dup = "        - { pool: wings1, via: wayhouse, port: 19010 }\n        - { pool: wings2, via: wayhouse, port: 19010 }\n";
    assert!(err(&s("19000-19999", dup)).contains("pinned twice"));
    let diff = "        - { pool: wings1, via: wayhouse, port: 19010 }\n";
    let two = format!("{diff}        - {{ pool: wings1, via: wayhouse, port: 19011 }}\n");
    assert!(err(&s("19000-19999", &two)).contains("different ports"));
}

#[test]
fn a_via_wayhouse_pool_must_not_prepend_proxy_protocol() {
    let y = doc(&site("        - { pool: wings1, via: wayhouse }\n")).replace(
        "targets: [\"10.0.0.6:8443\"]",
        "targets: [\"10.0.0.6:8443\"]\n    proxy_protocol: v1",
    );
    let e = parse_str(&y).unwrap_err().to_string();
    assert!(
        e.contains("pool wings1") && e.contains("proxy_protocol"),
        "{e}"
    );
}

#[test]
fn a_generated_listener_may_not_clash_with_an_operator_listener() {
    let y = format!(
        "schema_version: 3\n{POOLS}{LISTENERS}  - name: web-up-wings1\n    bind: \"127.0.0.1:7000\"\n    pool: origin\nweb:\n{}",
        site("        - { pool: wings1, via: wayhouse }\n")
    );
    let e = parse_str(&y).unwrap_err().to_string();
    assert!(
        e.contains("listener name web-up-wings1 is already used"),
        "{e}"
    );
}

#[test]
fn web_needs_schema_version_3() {
    let y = doc(&site("        - addr: 10.0.0.9:80\n")).replacen(
        "schema_version: 3",
        "schema_version: 2",
        1,
    );
    let e = parse_str(&y).unwrap_err().to_string();
    assert!(e.contains("`web` needs schema_version 3"), "{e}");
}

#[test]
fn warns_when_no_tcp_listener_routes_to_the_web_pool() {
    let y = format!(
        "schema_version: 3\n{POOLS}listeners:\n  - name: other\n    bind: \"0.0.0.0:25565\"\n    pool: origin\nweb:\n{}",
        site("        - addr: 10.0.0.9:80\n")
    );
    let cfg = parse_str(&y).unwrap();
    let w = cfg.warnings();
    assert!(
        w.iter()
            .any(|m| m.contains("no tcp listener routes to pool web")),
        "{w:?}"
    );
    let ok = parse(&site("        - addr: 10.0.0.9:80\n")).unwrap();
    assert!(ok.warnings().is_empty());
}
