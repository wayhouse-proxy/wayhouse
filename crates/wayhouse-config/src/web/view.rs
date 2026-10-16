//! The JSON form of the resolved `web:` section, served by `wayhouse` at `GET /web`
//! and read by `wayhouse-web-sync`. Plain strings and no behaviour, so the sync does
//! not need the validation types.

use serde::{Deserialize, Serialize};

use super::{UpstreamTls, WebConfig, WebUpstream, WebVia};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebView {
    pub pool: String,
    pub listen_http: String,
    pub listen_https: String,
    /// CIDRs Caddy trusts PROXY headers from, besides loopback.
    pub trusted_proxy: Vec<String>,
    pub sites: Vec<WebSiteView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebSiteView {
    pub hosts: Vec<String>,
    pub upstreams: Vec<WebUpstreamView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls: Option<WebSiteTlsView>,
    /// `"verify"`, `"skip"`, or `{"ca_file": path}`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_tls: Option<UpstreamTlsView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebSiteTlsView {
    pub cert: String,
    pub key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UpstreamTlsView {
    Verify,
    Skip,
    #[serde(rename = "ca_file")]
    CaFile(String),
}

/// `{"addr": "ip:port"}` or `{"pool": name, "via": "direct"|"wayhouse"}`.
/// `local` is the loopback listener a `via: wayhouse` pool is reached on, once
/// the routing slice has generated one (until then it is absent and the sync
/// skips the upstream).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum WebUpstreamView {
    Addr {
        addr: String,
    },
    Pool {
        pool: String,
        via: WebVia,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        local: Option<String>,
    },
}

impl WebConfig {
    /// The form `GET /web` serves.
    pub fn view(&self) -> WebView {
        WebView {
            pool: self.pool.clone(),
            listen_http: self.listen_http.to_string(),
            listen_https: self.listen_https.to_string(),
            trusted_proxy: self.trusted_proxy.iter().map(ToString::to_string).collect(),
            sites: self
                .sites
                .iter()
                .map(|s| WebSiteView {
                    hosts: s.hosts.clone(),
                    upstreams: s
                        .upstreams
                        .iter()
                        .map(|u| match u {
                            WebUpstream::Addr(a) => WebUpstreamView::Addr {
                                addr: a.to_string(),
                            },
                            WebUpstream::Pool { name, via, .. } => WebUpstreamView::Pool {
                                pool: name.clone(),
                                via: *via,
                                local: None,
                            },
                        })
                        .collect(),
                    tls: s.tls.as_ref().map(|t| WebSiteTlsView {
                        cert: t.cert.clone(),
                        key: t.key.clone(),
                    }),
                    upstream_tls: s.upstream_tls.as_ref().map(|t| match t {
                        UpstreamTls::Verify => UpstreamTlsView::Verify,
                        UpstreamTls::Skip => UpstreamTlsView::Skip,
                        UpstreamTls::CaFile(p) => UpstreamTlsView::CaFile(p.clone()),
                    }),
                    health_path: s.health_path.clone(),
                })
                .collect(),
        }
    }
}
