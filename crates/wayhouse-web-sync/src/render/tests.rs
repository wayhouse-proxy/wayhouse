use serde_json::{json, Value};
use wayhouse_config::{
    UpstreamTlsView, WebSiteTlsView, WebSiteView, WebUpstreamView, WebVia, WebView,
};

use super::*;
use crate::certs::Pem;

fn site(hosts: &[&str], upstreams: Vec<WebUpstreamView>) -> WebSiteView {
    WebSiteView {
        hosts: hosts.iter().map(ToString::to_string).collect(),
        upstreams,
        tls: None,
        upstream_tls: None,
        health_path: None,
    }
}

fn addr(a: &str) -> WebUpstreamView {
    WebUpstreamView::Addr { addr: a.into() }
}

fn web(sites: Vec<WebSiteView>) -> WebView {
    WebView {
        pool: "web".into(),
        listen_http: "127.0.0.1:8080".into(),
        listen_https: "127.0.0.1:8443".into(),
        trusted_proxy: vec![],
        sites,
    }
}

fn render_with(web: &WebView, pools: &PoolBackends, certs: &SiteCerts) -> Value {
    let admin = AdminAddr::parse("127.0.0.1:2019").unwrap();
    render(&RenderInput {
        web,
        pools,
        certs,
        admin: &admin,
    })
    .unwrap()
}

fn render_web(web: &WebView) -> Value {
    render_with(web, &PoolBackends::new(), &SiteCerts::new())
}

fn pem() -> Pem {
    Pem {
        certificate: "CERT".into(),
        key: "KEY".into(),
    }
}

#[test]
fn golden_plain_http_site() {
    let doc = render_web(&web(vec![site(
        &["panel.example.com"],
        vec![addr("10.0.0.5:8080")],
    )]));
    let proxy = json!({
        "handler": "reverse_proxy",
        "upstreams": [{"dial": "10.0.0.5:8080"}],
        "stream_close_delay": "1h",
        "headers": {"request": {"set": {"X-Real-IP": ["{http.request.remote.host}"]}}},
        "health_checks": {"passive": {"fail_duration": "30s", "max_fails": 3}},
    });
    let route = json!({
        "match": [{"host": ["panel.example.com"]}],
        "terminal": true,
        "handle": [proxy],
    });
    let wrapper = json!({
        "wrapper": "proxy_protocol",
        "allow": ["127.0.0.1/32", "::1/128"],
        "fallback_policy": "REJECT",
    });
    let want = json!({
        "admin": {"listen": "127.0.0.1:2019", "config": {"persist": false}},
        "apps": {"http": {"servers": {
            "https": {
                "listen": ["127.0.0.1:8443"],
                "listener_wrappers": [wrapper, {"wrapper": "tls"}],
                "protocols": ["h1", "h2"],
                "tls_connection_policies": [{}],
                "automatic_https": {"disable": true},
                "read_header_timeout": "10s",
                "idle_timeout": "2m",
                "routes": [route],
            },
            "http": {
                "listen": ["127.0.0.1:8080"],
                "listener_wrappers": [wrapper],
                "protocols": ["h1", "h2"],
                "automatic_https": {"disable": true},
                "read_header_timeout": "10s",
                "idle_timeout": "2m",
                "routes": [route],
            },
        }}},
    });
    assert_eq!(doc, want);
}

#[test]
fn a_site_with_a_certificate_redirects_on_http_and_loads_inline_pem() {
    let mut s = site(&["a.example.com"], vec![addr("10.0.0.5:8080")]);
    s.tls = Some(WebSiteTlsView {
        cert: "/c.pem".into(),
        key: "/k.pem".into(),
    });
    let certs = SiteCerts::from([(0, pem())]);
    let doc = render_with(&web(vec![s]), &PoolBackends::new(), &certs);
    assert_eq!(
        doc["apps"]["tls"],
        json!({"certificates": {"load_pem": [{"certificate": "CERT", "key": "KEY"}]}})
    );
    let http_route = &doc["apps"]["http"]["servers"]["http"]["routes"][0];
    assert_eq!(
        http_route["handle"][0],
        json!({
            "handler": "static_response",
            "status_code": 308,
            "headers": {"Location": ["https://{http.request.host}{http.request.uri}"]},
        })
    );
    let https_route = &doc["apps"]["http"]["servers"]["https"]["routes"][0];
    assert_eq!(https_route["handle"][0]["handler"], "reverse_proxy");
}

#[test]
fn a_changed_certificate_changes_the_document() {
    let mut s = site(&["a.example.com"], vec![addr("10.0.0.5:8080")]);
    s.tls = Some(WebSiteTlsView {
        cert: "/c.pem".into(),
        key: "/k.pem".into(),
    });
    let w = web(vec![s]);
    let old = render_with(&w, &PoolBackends::new(), &SiteCerts::from([(0, pem())]));
    let renewed = Pem {
        certificate: "CERT2".into(),
        key: "KEY".into(),
    };
    let new = render_with(&w, &PoolBackends::new(), &SiteCerts::from([(0, renewed)]));
    assert_ne!(old, new);
}

#[test]
fn a_tls_site_without_a_loaded_certificate_is_an_error_not_a_partial_document() {
    let mut s = site(&["a.example.com"], vec![addr("10.0.0.5:8080")]);
    s.tls = Some(WebSiteTlsView {
        cert: "/c.pem".into(),
        key: "/k.pem".into(),
    });
    let admin = AdminAddr::parse("127.0.0.1:2019").unwrap();
    let e = render(&RenderInput {
        web: &web(vec![s]),
        pools: &PoolBackends::new(),
        certs: &SiteCerts::new(),
        admin: &admin,
    })
    .unwrap_err();
    assert_eq!(e, RenderError::MissingCert { site: 0 });
}

#[test]
fn upstream_tls_variants() {
    let cases = [
        (UpstreamTlsView::Verify, json!({})),
        (UpstreamTlsView::Skip, json!({"insecure_skip_verify": true})),
        (
            UpstreamTlsView::CaFile("/ca.pem".into()),
            json!({"root_ca_pem_files": ["/ca.pem"]}),
        ),
    ];
    for (mode, tls) in cases {
        let mut s = site(&["w.example.com"], vec![addr("10.0.0.6:8443")]);
        s.upstream_tls = Some(mode);
        let doc = render_web(&web(vec![s]));
        let h = &doc["apps"]["http"]["servers"]["https"]["routes"][0]["handle"][0];
        assert_eq!(h["transport"], json!({"protocol": "http", "tls": tls}));
    }
    let doc = render_web(&web(vec![site(
        &["p.example.com"],
        vec![addr("10.0.0.6:80")],
    )]));
    let h = &doc["apps"]["http"]["servers"]["https"]["routes"][0]["handle"][0];
    assert!(h.get("transport").is_none(), "plain HTTP has no transport");
}

#[test]
fn health_path_adds_an_active_check() {
    let mut s = site(&["h.example.com"], vec![addr("10.0.0.6:80")]);
    s.health_path = Some("/healthz".into());
    let doc = render_web(&web(vec![s]));
    let h = &doc["apps"]["http"]["servers"]["https"]["routes"][0]["handle"][0];
    assert_eq!(
        h["health_checks"]["active"],
        json!({"path": "/healthz", "interval": "10s", "timeout": "3s"})
    );
    assert!(h["health_checks"]["passive"].is_object());
}

fn direct(pool: &str) -> WebUpstreamView {
    WebUpstreamView::Pool {
        pool: pool.into(),
        via: WebVia::Direct,
        local: None,
    }
}

#[test]
fn direct_pool_upstreams_are_the_enabled_backends() {
    let w = web(vec![site(
        &["w.example.com"],
        vec![direct("wings"), addr("10.0.0.9:80")],
    )]);
    let pools = PoolBackends::from([(
        "wings".to_string(),
        vec![
            "10.0.0.6:8443".parse().unwrap(),
            "[fd00::7]:8443".parse().unwrap(),
        ],
    )]);
    let doc = render_with(&w, &pools, &SiteCerts::new());
    let dials: Vec<&str> = doc["apps"]["http"]["servers"]["https"]["routes"][0]["handle"][0]
        ["upstreams"]
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u["dial"].as_str().unwrap())
        .collect();
    assert_eq!(dials, ["10.0.0.6:8443", "[fd00::7]:8443", "10.0.0.9:80"]);
}

#[test]
fn a_drained_pool_renders_an_empty_upstream_list_not_an_error() {
    let w = web(vec![site(&["w.example.com"], vec![direct("wings")])]);
    let pools = PoolBackends::from([("wings".to_string(), vec![])]);
    let doc = render_with(&w, &pools, &SiteCerts::new());
    let h = &doc["apps"]["http"]["servers"]["https"]["routes"][0]["handle"][0];
    assert_eq!(h["upstreams"], json!([]));
}

#[test]
fn via_wayhouse_uses_the_generated_listener_and_is_skipped_until_there_is_one() {
    let via = |local: Option<&str>| WebUpstreamView::Pool {
        pool: "wings".into(),
        via: WebVia::Wayhouse,
        local: local.map(Into::into),
    };
    let doc = render_web(&web(vec![site(
        &["w.example.com"],
        vec![via(Some("127.0.0.1:19010"))],
    )]));
    let h = &doc["apps"]["http"]["servers"]["https"]["routes"][0]["handle"][0];
    assert_eq!(h["upstreams"], json!([{"dial": "127.0.0.1:19010"}]));
    let doc = render_web(&web(vec![site(&["w.example.com"], vec![via(None)])]));
    let h = &doc["apps"]["http"]["servers"]["https"]["routes"][0]["handle"][0];
    assert_eq!(h["upstreams"], json!([]));
}

#[test]
fn exact_hosts_are_routed_before_wildcards_across_sites() {
    let w = web(vec![
        site(&["*.example.com"], vec![addr("10.0.0.1:80")]),
        site(
            &["panel.example.com", "*.apps.example.com"],
            vec![addr("10.0.0.2:80")],
        ),
    ]);
    let doc = render_web(&w);
    let matches: Vec<Value> = doc["apps"]["http"]["servers"]["https"]["routes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["match"][0]["host"].clone())
        .collect();
    assert_eq!(
        matches,
        [
            json!(["panel.example.com"]),
            json!(["*.example.com"]),
            json!(["*.apps.example.com"]),
        ]
    );
}

#[test]
fn trusted_proxies_extend_the_allow_list_without_duplicates() {
    let mut w = web(vec![site(&["a.example.com"], vec![addr("10.0.0.1:80")])]);
    w.trusted_proxy = vec!["10.1.0.2/32".into(), "127.0.0.1/32".into()];
    let doc = render_web(&w);
    for server in ["https", "http"] {
        assert_eq!(
            doc["apps"]["http"]["servers"][server]["listener_wrappers"][0]["allow"],
            json!(["127.0.0.1/32", "::1/128", "10.1.0.2/32"])
        );
    }
}

#[test]
fn an_empty_site_list_is_still_a_complete_document() {
    let doc = render_web(&web(vec![]));
    assert_eq!(doc["apps"]["http"]["servers"]["https"]["routes"], json!([]));
    assert!(doc["admin"].is_object());
}

#[test]
fn the_unix_socket_admin_block_matches_what_the_client_connects_to() {
    let admin = AdminAddr::parse("/run/caddy/admin.sock").unwrap();
    let doc = render(&RenderInput {
        web: &web(vec![]),
        pools: &PoolBackends::new(),
        certs: &SiteCerts::new(),
        admin: &admin,
    })
    .unwrap();
    assert_eq!(
        doc["admin"],
        json!({"listen": "unix//run/caddy/admin.sock", "config": {"persist": false}})
    );
}

#[test]
fn spike_invariants_hold_for_every_variant() {
    let mut s = site(
        &["a.example.com", "*.b.example.com"],
        vec![direct("p"), addr("[::1]:80")],
    );
    s.upstream_tls = Some(UpstreamTlsView::Skip);
    s.health_path = Some("/".into());
    let doc = render_web(&web(vec![
        s,
        site(&["c.example.com"], vec![addr("10.0.0.1:1")]),
    ]));
    assert_eq!(doc["admin"]["config"]["persist"], false);
    for server in ["https", "http"] {
        let srv = &doc["apps"]["http"]["servers"][server];
        assert_eq!(srv["protocols"], json!(["h1", "h2"]));
        assert_eq!(srv["listener_wrappers"][0]["fallback_policy"], "REJECT");
        assert_eq!(srv["automatic_https"]["disable"], true);
        for r in srv["routes"].as_array().unwrap() {
            let h = &r["handle"][0];
            if h["handler"] == "reverse_proxy" {
                assert_eq!(h["stream_close_delay"], "1h");
                assert_eq!(
                    h["headers"]["request"]["set"]["X-Real-IP"],
                    json!(["{http.request.remote.host}"])
                );
            }
        }
    }
}

#[test]
fn rendering_is_deterministic() {
    let w = web(vec![site(&["a.example.com"], vec![direct("p")])]);
    let pools = PoolBackends::from([("p".to_string(), vec!["10.0.0.1:1".parse().unwrap()])]);
    assert_eq!(
        render_with(&w, &pools, &SiteCerts::new()),
        render_with(&w, &pools, &SiteCerts::new())
    );
}

#[test]
fn a_listen_address_that_does_not_parse_is_refused() {
    let mut w = web(vec![]);
    w.listen_https = "not-an-address".into();
    let admin = AdminAddr::parse("127.0.0.1:2019").unwrap();
    let e = render(&RenderInput {
        web: &w,
        pools: &PoolBackends::new(),
        certs: &SiteCerts::new(),
        admin: &admin,
    })
    .unwrap_err();
    assert!(matches!(
        e,
        RenderError::Invalid {
            what: "listen_https",
            ..
        }
    ));
}
