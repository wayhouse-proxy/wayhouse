use crate::{parse_str, Config, ConfigError, WebUpstream, WebVia};

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
  - name: pp
    targets: [\"10.0.0.8:8443\"]
    proxy_protocol: v1
";

const LISTENERS: &str = "\
listeners:
  - name: https
    bind: \"0.0.0.0:443\"
    routes:
      - match: { type: sni, host: [\"*.example.com\"] }
        action: { pool: web }
";

fn doc_with(schema: &str, web: &str) -> String {
    format!("{schema}{POOLS}{LISTENERS}web:\n{web}")
}

fn parse(web: &str) -> Result<Config, ConfigError> {
    parse_str(&doc_with("schema_version: 3\n", web))
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

const OK_UP: &str = "        - addr: 10.0.0.9:8080\n";

#[test]
fn defaults_resolve_and_upstreams_keep_their_form() {
    let cfg = parse(&site(&format!(
        "{OK_UP}        - pool: origin\n        - {{ pool: wings1, via: wayhouse, port: 19010 }}\n"
    )))
    .unwrap();
    let web = cfg.web.as_ref().unwrap();
    assert_eq!(web.pool, "web");
    assert_eq!(web.listen_http.to_string(), "127.0.0.1:8080");
    assert_eq!(web.listen_https.to_string(), "127.0.0.1:8443");
    assert!(web.trusted_proxy.is_empty());
    assert_eq!(web.internal_bind.to_string(), "127.0.0.1");
    assert_eq!(web.internal_ports, (19000, 19999));
    assert_eq!(
        web.sites[0].upstreams,
        vec![
            WebUpstream::Addr("10.0.0.9:8080".parse().unwrap()),
            WebUpstream::Pool {
                name: "origin".into(),
                via: WebVia::Direct,
                port: None
            },
            WebUpstream::Pool {
                name: "wings1".into(),
                via: WebVia::Wayhouse,
                port: Some(19010)
            },
        ]
    );
}

#[test]
fn absent_web_section_changes_nothing() {
    let cfg = parse_str(&format!("{POOLS}{LISTENERS}")).unwrap();
    assert!(cfg.web.is_none());
    assert!(cfg.warnings().is_empty());
}

#[test]
fn web_needs_schema_version_3() {
    let e = parse_str(&doc_with("schema_version: 2\n", &site(OK_UP))).unwrap_err();
    assert!(
        e.to_string().contains("`web` needs schema_version 3"),
        "{e}"
    );
    let e = parse_str(&doc_with("", &site(OK_UP))).unwrap_err();
    assert!(
        e.to_string().contains("`web` needs schema_version 3"),
        "{e}"
    );
}

#[test]
fn web_pool_must_exist() {
    let m = err(&format!("  pool: nope\n{}", site(OK_UP)));
    assert!(m.contains("web.pool: unknown pool nope"), "{m}");
}

#[test]
fn web_pool_must_set_proxy_protocol_v2() {
    for p in ["origin", "pp"] {
        let m = err(&format!("  pool: {p}\n{}", site(OK_UP)));
        assert!(m.contains("must set proxy_protocol v2"), "{m}");
    }
}

#[test]
fn hosts_are_lowercased_and_may_be_wildcards() {
    let cfg = parse(&format!(
        "  sites:\n    - hosts: [Panel.Example.com, \"*.Apps.example.com\"]\n      upstreams:\n{OK_UP}"
    ))
    .unwrap();
    assert_eq!(
        cfg.web.unwrap().sites[0].hosts,
        vec!["panel.example.com", "*.apps.example.com"]
    );
}

#[test]
fn a_site_needs_a_host_and_an_upstream() {
    let m = err(&format!(
        "  sites:\n    - hosts: []\n      upstreams:\n{OK_UP}"
    ));
    assert!(m.contains("web.sites[0]: needs at least one host"), "{m}");
    let m = err("  sites:\n    - hosts: [a.example.com]\n      upstreams: []\n");
    assert!(
        m.contains("web.sites[0]: needs at least one upstream"),
        "{m}"
    );
}

#[test]
fn bad_host_patterns_are_rejected() {
    let m = err(&format!(
        "  sites:\n    - hosts: [\"a*.example.com\"]\n      upstreams:\n{OK_UP}"
    ));
    assert!(m.contains("web.sites[0]"), "{m}");
    assert!(m.contains("`*` only as `*.suffix`"), "{m}");
}

#[test]
fn a_host_cannot_be_in_two_sites_even_by_case_or_wildcard() {
    for (a, b) in [
        ("a.example.com", "A.example.com"),
        ("*.x.example.com", "*.X.example.com"),
    ] {
        let m = err(&format!(
            "  sites:\n    - hosts: [\"{a}\"]\n      upstreams:\n{OK_UP}    - hosts: [\"{b}\"]\n      upstreams:\n{OK_UP}"
        ));
        assert!(
            m.contains("web.sites[1]") && m.contains("also in site 0"),
            "{m}"
        );
    }
}

#[test]
fn an_upstream_is_exactly_one_of_addr_or_pool() {
    for ups in [
        "        - {}\n",
        "        - { addr: 10.0.0.9:80, pool: origin }\n",
    ] {
        let m = err(&site(ups));
        assert!(m.contains("needs exactly one of addr / pool"), "{m}");
        assert!(m.contains("web.sites[0].upstreams[0]"), "{m}");
    }
}

#[test]
fn addr_upstream_must_be_a_socket_address_without_via_or_port() {
    let m = err(&site("        - addr: not-an-address\n"));
    assert!(m.contains("is not a valid socket address"), "{m}");
    let m = err(&site("        - { addr: 10.0.0.9:80, via: direct }\n"));
    assert!(m.contains("`via` and `port` apply only to a pool"), "{m}");
    let m = err(&site("        - { addr: 10.0.0.9:80, port: 19010 }\n"));
    assert!(m.contains("`via` and `port` apply only to a pool"), "{m}");
}

#[test]
fn unknown_upstream_pool_is_rejected() {
    let m = err(&site("        - pool: ghost\n"));
    assert!(m.contains("unknown pool ghost"), "{m}");
}

#[test]
fn port_needs_via_wayhouse() {
    for ups in [
        "        - { pool: origin, port: 19010 }\n",
        "        - { pool: origin, via: direct, port: 19010 }\n",
    ] {
        let m = err(&site(ups));
        assert!(m.contains("`port` needs via: wayhouse"), "{m}");
    }
}

#[test]
fn via_wayhouse_pool_must_not_set_proxy_protocol() {
    let m = err(&site("        - { pool: pp, via: wayhouse }\n"));
    assert!(m.contains("sets proxy_protocol"), "{m}");
    // via: direct leaves that choice to the renderer.
    parse(&site("        - { pool: pp, via: direct }\n")).unwrap();
}

#[test]
fn internal_bind_must_be_loopback() {
    for b in ["10.0.0.1", "0.0.0.0", "localhost", "::"] {
        let m = err(&format!("  internal_bind: {b:?}\n{}", site(OK_UP)));
        assert!(
            m.contains("web.internal_bind") && m.contains("must be loopback"),
            "{m}"
        );
    }
    parse(&format!("  internal_bind: \"::1\"\n{}", site(OK_UP))).unwrap();
}

#[test]
fn internal_ports_must_be_a_small_lo_hi_range() {
    for (r, why) in [
        ("19000", "not a range"),
        ("a-b", "bad lower bound"),
        ("19000-x", "bad upper bound"),
        ("0-10", "includes port 0"),
        ("19999-19000", "is reversed"),
        ("19000-20000", "at most 1000 ports"),
        ("1-70000", "bad upper bound"),
    ] {
        let m = err(&format!("  internal_ports: {r:?}\n{}", site(OK_UP)));
        assert!(
            m.contains("web.internal_ports") && m.contains(why),
            "{r}: {m}"
        );
    }
    let cfg = parse(&format!(
        "  internal_ports: \"20000-20999\"\n{}",
        site(OK_UP)
    ))
    .unwrap();
    assert_eq!(cfg.web.unwrap().internal_ports, (20000, 20999));
}

#[test]
fn pins_must_lie_in_the_range() {
    let m = err(&site(
        "        - { pool: wings1, via: wayhouse, port: 18999 }\n",
    ));
    assert!(
        m.contains("port 18999 is outside internal_ports 19000-19999"),
        "{m}"
    );
}

#[test]
fn pins_must_be_unique_per_port_and_consistent_per_pool() {
    let m = err(&site(
        "        - { pool: wings1, via: wayhouse, port: 19010 }\n        - { pool: wings2, via: wayhouse, port: 19010 }\n",
    ));
    assert!(m.contains("port 19010 is pinned twice"), "{m}");
    let m = err(&site(
        "        - { pool: wings1, via: wayhouse, port: 19010 }\n        - { pool: wings1, via: wayhouse, port: 19011 }\n",
    ));
    assert!(
        m.contains("pool wings1 is pinned to different ports"),
        "{m}"
    );
    // The same pool pinned to the same port twice is fine.
    parse(&site(
        "        - { pool: wings1, via: wayhouse, port: 19010 }\n        - { pool: wings1, via: wayhouse, port: 19010 }\n",
    ))
    .unwrap();
}

#[test]
fn trusted_proxy_is_a_list_of_ips_and_cidrs() {
    let cfg = parse(&format!(
        "  trusted_proxy: [\"10.1.0.2/32\", \"fd00::/8\", \"192.0.2.7\"]\n{}",
        site(OK_UP)
    ))
    .unwrap();
    assert_eq!(cfg.web.unwrap().trusted_proxy.len(), 3);
    let m = err(&format!("  trusted_proxy: [\"nope\"]\n{}", site(OK_UP)));
    assert!(m.contains("web.trusted_proxy") && m.contains("nope"), "{m}");
}

#[test]
fn listen_may_be_non_loopback_but_must_be_a_socket_address() {
    let cfg = parse(&format!(
        "  listen: {{ http: \"0.0.0.0:80\", https: \"10.0.0.3:443\" }}\n{}",
        site(OK_UP)
    ))
    .unwrap();
    assert_eq!(cfg.web.unwrap().listen_https.to_string(), "10.0.0.3:443");
    let m = err(&format!("  listen: {{ http: \"nope\" }}\n{}", site(OK_UP)));
    assert!(
        m.contains("web.listen.http") && m.contains("not a valid socket address"),
        "{m}"
    );
}

#[test]
fn unknown_web_fields_are_rejected() {
    assert!(parse(&format!("  caddy_admin: x\n{}", site(OK_UP))).is_err());
}

#[test]
fn warns_when_no_tcp_listener_routes_to_the_web_pool() {
    let doc = format!(
        "schema_version: 3\n{POOLS}listeners:\n  - name: game\n    bind: \"0.0.0.0:7000\"\n    routes:\n      - match: {{ type: always }}\n        action: {{ pool: origin }}\nweb:\n{}",
        site(OK_UP)
    );
    let w = parse_str(&doc).unwrap().warnings();
    assert!(
        w.iter()
            .any(|m| m.contains("no tcp listener routes to pool web")),
        "{w:?}"
    );
    // Routed: no such warning.
    let w = parse(&site(OK_UP)).unwrap().warnings();
    assert!(w.iter().all(|m| !m.contains("no tcp listener")), "{w:?}");
}

#[test]
fn warns_on_a_routable_listen_without_trusted_proxy() {
    let w = parse(&format!(
        "  listen: {{ http: \"0.0.0.0:80\" }}\n{}",
        site(OK_UP)
    ))
    .unwrap()
    .warnings();
    assert!(w.iter().any(|m| m.contains("trusted_proxy")), "{w:?}");
    let w = parse(&format!(
        "  listen: {{ http: \"0.0.0.0:80\" }}\n  trusted_proxy: [\"10.1.0.2/32\"]\n{}",
        site(OK_UP)
    ))
    .unwrap()
    .warnings();
    assert!(w.iter().all(|m| !m.contains("trusted_proxy")), "{w:?}");
    // Loopback defaults never warn.
    let w = parse(&site(OK_UP)).unwrap().warnings();
    assert!(w.iter().all(|m| !m.contains("trusted_proxy")), "{w:?}");
}

#[test]
fn an_upstream_may_not_be_the_web_pool_itself() {
    for ups in [
        "        - pool: web\n",
        "        - { pool: web, via: direct }\n",
        "        - { pool: web, via: wayhouse }\n",
    ] {
        let m = err(&site(ups));
        assert!(m.contains("is web.pool"), "{m}");
        assert!(m.contains("web.sites[0].upstreams[0]"), "{m}");
    }
}
