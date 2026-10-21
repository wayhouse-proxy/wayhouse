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
                name: "origin".into()
            },
            WebUpstream::Local {
                pool: "wings1".into(),
                addr: "127.0.0.1:19010".parse().unwrap()
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
    assert!(m.contains("is not a hostname"), "{m}");
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

fn site_with(extra: &str) -> String {
    format!("  sites:\n    - hosts: [panel.example.com]\n      upstreams:\n{OK_UP}{extra}")
}

#[test]
fn site_tls_upstream_tls_and_health_path_resolve() {
    use crate::{UpstreamTls, WebSiteTls};
    let cfg = parse(&site_with(
        "      tls: { cert: /certs/a.pem, key: /certs/a.key }\n      upstream_tls: skip\n      health_path: /healthz\n",
    ))
    .unwrap();
    let s = &cfg.web.unwrap().sites[0];
    assert_eq!(
        s.tls,
        Some(WebSiteTls {
            cert: "/certs/a.pem".into(),
            key: "/certs/a.key".into()
        })
    );
    assert_eq!(s.upstream_tls, Some(UpstreamTls::Skip));
    assert_eq!(s.health_path.as_deref(), Some("/healthz"));

    let cfg = parse(&site_with("      upstream_tls: verify\n")).unwrap();
    assert_eq!(
        cfg.web.unwrap().sites[0].upstream_tls,
        Some(UpstreamTls::Verify)
    );
    let cfg = parse(&site_with("      upstream_tls: { ca_file: /ca.pem }\n")).unwrap();
    assert_eq!(
        cfg.web.unwrap().sites[0].upstream_tls,
        Some(UpstreamTls::CaFile("/ca.pem".into()))
    );
    // Unset means plain HTTP and Caddy-side TLS comes from elsewhere.
    let s = &parse(&site(OK_UP)).unwrap().web.unwrap().sites[0];
    assert!(s.tls.is_none() && s.upstream_tls.is_none() && s.health_path.is_none());
}

#[test]
fn site_tls_needs_both_files_and_a_known_upstream_mode() {
    let m = err(&site_with("      tls: { cert: /a.pem, key: \"\" }\n"));
    assert!(
        m.contains("web.sites[0]: tls: key must not be empty"),
        "{m}"
    );
    let m = err(&site_with("      tls: { cert: /a.pem }\n"));
    assert!(m.contains("key"), "{m}");
    let m = err(&site_with("      upstream_tls: maybe\n"));
    assert!(m.contains("upstream_tls"), "{m}");
    let m = err(&site_with("      upstream_tls: { ca_file: \"\" }\n"));
    assert!(
        m.contains("web.sites[0]: upstream_tls: ca_file must not be empty"),
        "{m}"
    );
}

#[test]
fn health_path_must_be_an_absolute_path() {
    let m = err(&site_with("      health_path: healthz\n"));
    assert!(
        m.contains("web.sites[0]: health_path: must start with '/'"),
        "{m}"
    );
}

#[test]
fn view_serializes_the_resolved_section_for_the_sync() {
    let cfg = parse(&format!(
        "  trusted_proxy: [10.1.0.2]\n{}",
        site_with("      upstream_tls: skip\n")
    ))
    .unwrap();
    let v = cfg.web.as_ref().unwrap().view();
    let json = serde_json::to_value(&v).unwrap();
    assert_eq!(json["pool"], "web");
    assert_eq!(json["listen"]["http"], "127.0.0.1:8080");
    assert_eq!(json["listen"]["https"], "127.0.0.1:8443");
    assert_eq!(json["trusted_proxy"], serde_json::json!(["10.1.0.2/32"]));
    assert_eq!(
        json["sites"][0]["hosts"],
        serde_json::json!(["panel.example.com"])
    );
    assert_eq!(
        json["sites"][0]["upstreams"][0],
        serde_json::json!({"addr": "10.0.0.9:8080"})
    );
    assert_eq!(json["sites"][0]["upstream_tls"], "skip");
    let back: crate::WebView = serde_json::from_value(json).unwrap();
    assert_eq!(back, v);
}

fn via(pools: &[&str]) -> String {
    let ups: String = pools
        .iter()
        .map(|p| format!("        - {{ pool: {p}, via: wayhouse }}\n"))
        .collect();
    site(&ups)
}

fn local_addr(cfg: &Config, pool: &str) -> std::net::SocketAddr {
    cfg.web.as_ref().unwrap().sites[0]
        .upstreams
        .iter()
        .find_map(|u| match u {
            WebUpstream::Local { pool: p, addr } if p == pool => Some(*addr),
            _ => None,
        })
        .unwrap()
}

#[test]
fn via_wayhouse_appends_a_loopback_listener_per_pool() {
    let cfg = parse(&via(&["wings1", "wings2"])).unwrap();
    for pool in ["wings1", "wings2"] {
        let addr = local_addr(&cfg, pool);
        assert!(addr.ip().is_loopback());
        assert!((19000..=19999).contains(&addr.port()));
        let l = cfg
            .listeners
            .iter()
            .find(|l| l.name == format!("web-up-{pool}"))
            .unwrap();
        assert_eq!(l.protocol, Protocol::Tcp);
        assert_eq!(l.bind, addr);
        assert_eq!(l.routes.len(), 1);
        assert_eq!(l.routes[0].matcher, Matcher::Always);
        assert_eq!(l.routes[0].action, Action::Pool(pool.to_string()));
    }
    assert_eq!(cfg.listeners.len(), 3);
}

#[test]
fn a_pool_shared_by_sites_gets_one_listener() {
    let doc = format!(
        "{}    - hosts: [other.example.com]\n      upstreams:\n        - {{ pool: wings1, via: wayhouse }}\n",
        via(&["wings1"])
    );
    let cfg = parse(&doc).unwrap();
    assert_eq!(
        cfg.listeners
            .iter()
            .filter(|l| l.name == "web-up-wings1")
            .count(),
        1
    );
    let w = cfg.web.as_ref().unwrap();
    assert_eq!(w.sites[0].upstreams, w.sites[1].upstreams);
}

#[test]
fn direct_pools_and_addrs_add_no_listener() {
    let cfg = parse(&site(&format!("{OK_UP}        - pool: origin\n"))).unwrap();
    assert_eq!(cfg.listeners.len(), 1);
}

#[test]
fn a_pinned_port_is_honoured() {
    let cfg = parse(&site(
        "        - { pool: wings1, via: wayhouse, port: 19500 }\n",
    ))
    .unwrap();
    assert_eq!(local_addr(&cfg, "wings1").port(), 19500);
}

#[test]
fn ports_are_stable_when_other_pools_come_and_go() {
    let alone = local_addr(&parse(&via(&["wings1"])).unwrap(), "wings1");
    let with = parse(&via(&["origin", "wings1", "wings2"])).unwrap();
    assert_eq!(local_addr(&with, "wings1"), alone);
    let without = parse(&via(&["wings1", "wings2"])).unwrap();
    assert_eq!(local_addr(&without, "wings1"), alone);
}

#[test]
fn a_pin_on_the_hashed_port_moves_the_other_pool_to_the_next_free_one() {
    let alone = local_addr(&parse(&via(&["wings1"])).unwrap(), "wings1").port();
    let doc = site(&format!(
        "        - {{ pool: wings1, via: wayhouse }}\n        - {{ pool: wings2, via: wayhouse, port: {alone} }}\n"
    ));
    let cfg = parse(&doc).unwrap();
    assert_eq!(local_addr(&cfg, "wings2").port(), alone);
    assert_ne!(local_addr(&cfg, "wings1").port(), alone);
}

fn with_operator_listener(bind: &str) -> String {
    let doc = parse(&via(&["wings1"])).unwrap();
    let hashed = local_addr(&doc, "wings1").port();
    let extra = format!(
        "  - name: game\n    bind: \"{}\"\n    pool: origin\n",
        bind.replace("PORT", &hashed.to_string())
    );
    let base = doc_with("schema_version: 3\n", &via(&["wings1"]));
    base.replacen("listeners:\n", &format!("listeners:\n{extra}"), 1)
}

#[test]
fn operator_listeners_on_the_candidate_port_are_skipped() {
    for bind in ["127.0.0.1:PORT", "0.0.0.0:PORT"] {
        let cfg = parse_str(&with_operator_listener(bind)).unwrap();
        let hashed = {
            let d = parse(&via(&["wings1"])).unwrap();
            local_addr(&d, "wings1").port()
        };
        assert_ne!(local_addr(&cfg, "wings1").port(), hashed, "{bind}");
    }
}

#[test]
fn an_operator_listener_on_another_address_does_not_block_the_port() {
    let cfg = parse_str(&with_operator_listener("10.1.2.3:PORT")).unwrap();
    let d = parse(&via(&["wings1"])).unwrap();
    assert_eq!(local_addr(&cfg, "wings1"), local_addr(&d, "wings1"));
}

#[test]
fn a_pin_on_an_operator_bound_port_is_an_error() {
    let doc = doc_with(
        "schema_version: 3\n",
        &site("        - { pool: wings1, via: wayhouse, port: 19100 }\n"),
    )
    .replacen(
        "listeners:\n",
        "listeners:\n  - name: game\n    bind: \"127.0.0.1:19100\"\n    pool: origin\n",
        1,
    );
    let e = parse_str(&doc).unwrap_err().to_string();
    assert!(e.contains("already bound by another listener"), "{e}");
}

#[test]
fn the_admin_address_port_is_skipped() {
    let d = parse(&via(&["wings1"])).unwrap();
    let hashed = local_addr(&d, "wings1").port();
    let doc = doc_with("schema_version: 3\n", &via(&["wings1"])).replacen(
        "pools:\n",
        &format!("settings:\n  admin:\n    listen: \"127.0.0.1:{hashed}\"\npools:\n"),
        1,
    );
    let cfg = parse_str(&doc).unwrap();
    assert_ne!(local_addr(&cfg, "wings1").port(), hashed);
}

#[test]
fn an_operator_listener_cannot_use_a_generated_name() {
    let doc = doc_with("schema_version: 3\n", &via(&["wings1"])).replacen(
        "listeners:\n",
        "listeners:\n  - name: web-up-wings1\n    bind: \"0.0.0.0:7000\"\n    pool: origin\n",
        1,
    );
    let e = parse_str(&doc).unwrap_err().to_string();
    assert!(
        e.contains("listener name web-up-wings1 already used"),
        "{e}"
    );
}

#[test]
fn a_full_range_is_a_clear_error() {
    // Two pools, a one-port range.
    let doc = format!(
        "  internal_ports: \"19000-19000\"\n{}",
        via(&["wings1", "wings2"])
    );
    let e = err(&doc);
    assert!(
        e.contains("no free port in internal_ports 19000-19000"),
        "{e}"
    );
}

#[test]
fn a_game_listener_sharing_the_pool_is_untouched() {
    let base = doc_with("schema_version: 3\n", &via(&["wings1"]));
    let with_game = base.replacen(
        "listeners:\n",
        "listeners:\n  - name: game\n    bind: \"0.0.0.0:7000\"\n    pool: wings1\n",
        1,
    );
    let cfg = parse_str(&with_game).unwrap();
    let game = cfg.listeners.iter().find(|l| l.name == "game").unwrap();
    assert_eq!(game.routes.len(), 1);
    assert_eq!(game.routes[0].action, Action::Pool("wings1".into()));
    assert_eq!(cfg.pools.iter().filter(|p| p.name == "wings1").count(), 1);
}

#[test]
fn hosts_must_be_hostnames() {
    for bad in [
        "a.com:443",
        "https://x",
        "a b",
        "10.0.0.1",
        "x.",
        ".example.com",
        "a..b",
        "*.",
        "1234",
    ] {
        let m = err(&format!(
            "  sites:\n    - hosts: [{bad:?}]\n      upstreams:\n{OK_UP}"
        ));
        assert!(m.contains("web.sites[0]"), "{bad}: {m}");
    }
}

#[test]
fn health_path_rejects_placeholders_and_whitespace() {
    for bad in ["/a b", "/{http.request.host}", "/x\ty"] {
        let m = err(&format!(
            "  sites:\n    - hosts: [panel.example.com]\n      health_path: {bad:?}\n      upstreams:\n{OK_UP}"
        ));
        assert!(m.contains("health_path"), "{bad}: {m}");
    }
}

#[test]
fn internal_ports_may_not_cover_caddys_own_listen_ports() {
    let via = "        - pool: wings1\n          via: wayhouse\n          port: 8443\n";
    let m = err(&format!(
        "  listen: {{ http: \"127.0.0.1:8080\", https: \"127.0.0.1:8443\" }}\n  internal_ports: \"8000-8999\"\n{}",
        site(via)
    ));
    assert!(m.contains("pinned port 8443"), "{m}");
}
