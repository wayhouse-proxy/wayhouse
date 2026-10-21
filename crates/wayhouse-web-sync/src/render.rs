//! `WebView` to one Caddy JSON document. A pure function: the inputs it needs from the
//! outside world (pool backends, certificate PEM) are passed in.
//!
//! Every choice here comes from the spike (#355, `caddy-spike-findings-2026-10-15.md`):
//!
//! * the `admin` block is identical on every render (a different one would move or
//!   expose the admin API) and sets `config.persist: false`, because Caddy otherwise
//!   writes inline private keys to `autosave.json`;
//! * the `proxy_protocol` wrapper uses `fallback_policy: REJECT` and an explicit
//!   `allow` list, since the default IGNORE accepts a spoofed header's peer as the client;
//! * `stream_close_delay` on every `reverse_proxy`, or each reload closes WebSockets;
//! * `protocols: [h1, h2]`, or Caddy opens an HTTP/3 UDP listener;
//! * a client-supplied `X-Real-IP` is overwritten (`X-Forwarded-For` is already
//!   rebuilt from the PROXY address, so `trusted_proxies` is not needed);
//! * certificates are inline PEM, so a renewal changes the document and reloads.

use serde_json::{json, Value};
use wayhouse_config::{UpstreamTlsView, WebSiteView, WebUpstreamView, WebVia, WebView};

use crate::admin_addr::AdminAddr;
use crate::certs::SiteCerts;
use crate::pools::PoolBackends;

/// How long an upgraded stream (WebSocket) outlives a reload.
const STREAM_CLOSE_DELAY: &str = "1h";
const READ_HEADER_TIMEOUT: &str = "10s";
const IDLE_TIMEOUT: &str = "2m";

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RenderError {
    #[error("site {site}: no certificate was loaded for its tls section")]
    MissingCert { site: usize },
    #[error("{what} {value:?} is not a valid {kind}")]
    Invalid {
        what: &'static str,
        kind: &'static str,
        value: String,
    },
}

pub struct RenderInput<'a> {
    pub web: &'a WebView,
    /// Enabled backends per pool, for `via: direct` upstreams.
    pub pools: &'a PoolBackends,
    pub certs: &'a SiteCerts,
    pub admin: &'a AdminAddr,
}

pub fn render(input: &RenderInput<'_>) -> Result<Value, RenderError> {
    let web = input.web;
    for (what, v) in [
        ("listen_http", &web.listen.http),
        ("listen_https", &web.listen.https),
    ] {
        v.parse::<std::net::SocketAddr>()
            .map_err(|_| RenderError::Invalid {
                what,
                kind: "socket address",
                value: v.clone(),
            })?;
    }
    // Loopback is always allowed: a local `wayhouse` forwards from it.
    let mut allow = vec!["127.0.0.1/32".to_string(), "::1/128".to_string()];
    for t in &web.trusted_proxy {
        if !allow.contains(t) {
            allow.push(t.clone());
        }
    }

    let routes = routes(input)?;
    let wrapper = json!({"wrapper": "proxy_protocol", "allow": allow, "fallback_policy": "REJECT"});
    let https = json!({
        "listen": [web.listen.https],
        "listener_wrappers": [wrapper, {"wrapper": "tls"}],
        "protocols": ["h1", "h2"],
        "tls_connection_policies": [{}],
        "automatic_https": {"disable": true},
        "read_header_timeout": READ_HEADER_TIMEOUT,
        "idle_timeout": IDLE_TIMEOUT,
        "routes": routes.https,
    });
    let http = json!({
        "listen": [web.listen.http],
        "listener_wrappers": [wrapper],
        "protocols": ["h1", "h2"],
        "automatic_https": {"disable": true},
        "read_header_timeout": READ_HEADER_TIMEOUT,
        "idle_timeout": IDLE_TIMEOUT,
        "routes": routes.http,
    });

    let mut apps = json!({"http": {"servers": {"https": https, "http": http}}});
    let certs: Vec<Value> = input
        .certs
        .values()
        .map(|p| json!({"certificate": p.certificate, "key": p.key}))
        .collect();
    if !certs.is_empty() {
        apps["tls"] = json!({"certificates": {"load_pem": certs}});
    }
    Ok(json!({
        "admin": {"listen": input.admin.caddy_listen(), "config": {"persist": false}},
        "apps": apps,
    }))
}

struct Routes {
    https: Vec<Value>,
    http: Vec<Value>,
}

/// Exact hosts are matched before wildcards, whatever site they belong to, so a
/// `*.example.com` site cannot shadow `panel.example.com` of another site.
fn routes(input: &RenderInput<'_>) -> Result<Routes, RenderError> {
    let mut exact = Routes {
        https: vec![],
        http: vec![],
    };
    let mut wild = Routes {
        https: vec![],
        http: vec![],
    };
    for (i, site) in input.web.sites.iter().enumerate() {
        let has_tls = site.tls.is_some();
        if has_tls && !input.certs.contains_key(&i) {
            return Err(RenderError::MissingCert { site: i });
        }
        let proxy = reverse_proxy(site, input.pools);
        let (w, e): (Vec<_>, Vec<_>) = site
            .hosts
            .iter()
            .cloned()
            .partition(|h| h.starts_with("*."));
        for (hosts, bucket) in [(e, &mut exact), (w, &mut wild)] {
            if hosts.is_empty() {
                continue;
            }
            let m = json!([{"host": hosts}]);
            // Without a certificate there is no https for the host, so it is served on http.
            bucket.https.push(route(&m, &proxy));
            bucket.http.push(if has_tls {
                route(&m, &redirect())
            } else {
                route(&m, &proxy)
            });
        }
    }
    exact.https.append(&mut wild.https);
    exact.http.append(&mut wild.http);
    Ok(exact)
}

fn route(matcher: &Value, handler: &Value) -> Value {
    json!({"match": matcher, "terminal": true, "handle": [handler]})
}

fn redirect() -> Value {
    json!({
        "handler": "static_response",
        "status_code": 308,
        "headers": {"Location": ["https://{http.request.host}{http.request.uri}"]},
    })
}

fn reverse_proxy(site: &WebSiteView, pools: &PoolBackends) -> Value {
    let mut dials: Vec<String> = Vec::new();
    for u in &site.upstreams {
        match u {
            WebUpstreamView::Addr { addr } => dials.push(addr.clone()),
            WebUpstreamView::Pool {
                pool,
                via: WebVia::Direct,
                ..
            } => dials.extend(
                pools
                    .get(pool)
                    .into_iter()
                    .flatten()
                    .map(ToString::to_string),
            ),
            WebUpstreamView::Pool {
                via: WebVia::Wayhouse,
                local,
                ..
            } => dials.extend(local.clone()),
        }
    }
    // Keep the first occurrence: a repeated address would double its weight in Caddy's balancer.
    let mut seen = std::collections::HashSet::new();
    dials.retain(|d| seen.insert(d.clone()));
    // An empty list is a valid document: the route answers 503 (spike item 9).
    let upstreams: Vec<Value> = dials.iter().map(|d| json!({"dial": d})).collect();
    let mut rp = json!({
        "handler": "reverse_proxy",
        "upstreams": upstreams,
        "stream_close_delay": STREAM_CLOSE_DELAY,
        "headers": {"request": {"set": {"X-Real-IP": ["{http.request.remote.host}"]}}},
        "health_checks": {"passive": {"fail_duration": "30s", "max_fails": 3}},
    });
    if let Some(path) = &site.health_path {
        rp["health_checks"]["active"] = json!({"path": path, "interval": "10s", "timeout": "3s"});
    }
    if let Some(t) = &site.upstream_tls {
        let tls = match t {
            UpstreamTlsView::Verify => json!({}),
            UpstreamTlsView::Skip => json!({"insecure_skip_verify": true}),
            UpstreamTlsView::CaFile(p) => json!({"root_ca_pem_files": [p]}),
        };
        rp["transport"] = json!({"protocol": "http", "tls": tls});
    }
    rp
}

#[cfg(test)]
mod tests;
