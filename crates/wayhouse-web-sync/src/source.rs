//! The local `wayhouse` admin API: `GET /web` (the resolved `web:` section) and
//! `GET /pools` (backends for `via: direct`).

use std::time::Duration;

use wayhouse_config::WebView;

use crate::pools::{self, PoolBackends};

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct SourceError(pub String);

pub trait Source {
    /// `Ok(None)` when `wayhouse` answers but has no `web:` section.
    fn web(&self) -> impl std::future::Future<Output = Result<Option<WebView>, SourceError>>;
    fn pools(&self) -> impl std::future::Future<Output = Result<PoolBackends, SourceError>>;
}

/// Refuses a plain-http, non-loopback `base` when a bearer token would travel to it.
pub fn check_transport(base: &str, has_token: bool, allow_insecure: bool) -> Result<(), String> {
    let url = reqwest::Url::parse(base).map_err(|e| format!("--wayhouse-admin {base}: {e}"))?;
    match url.scheme() {
        "https" => Ok(()),
        "http" if !has_token || allow_insecure || is_loopback(&url) => Ok(()),
        "http" => Err(format!(
            "--wayhouse-admin {base} is plain http to a non-loopback host, which would send the \
             admin token in clear text: use https, or pass --allow-insecure-secrets on a network \
             you fully trust"
        )),
        other => Err(format!(
            "--wayhouse-admin {base}: unsupported scheme {other}"
        )),
    }
}

fn is_loopback(url: &reqwest::Url) -> bool {
    let host = url.host_str().unwrap_or("").trim_matches(['[', ']']);
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

pub struct HttpSource {
    base: String,
    token: Option<String>,
    client: reqwest::Client,
}

impl HttpSource {
    pub fn new(base: &str, token: Option<String>, timeout: Duration) -> Result<Self, SourceError> {
        let client = wayhouse_http::builder()
            .timeout(timeout)
            .build()
            .map_err(|e| SourceError(format!("http client: {e}")))?;
        Ok(Self {
            base: base.trim_end_matches('/').to_string(),
            token,
            client,
        })
    }

    async fn get(&self, path: &str) -> Result<reqwest::Response, SourceError> {
        let mut req = self.client.get(format!("{}{path}", self.base));
        if let Some(t) = &self.token {
            req = req.bearer_auth(t);
        }
        req.send()
            .await
            .map_err(|e| SourceError(format!("GET {path}: {}", wayhouse_http::error_chain(&e))))
    }
}

impl Source for HttpSource {
    async fn web(&self) -> Result<Option<WebView>, SourceError> {
        let r = self.get("/web").await?;
        match r.status() {
            s if s == reqwest::StatusCode::NOT_FOUND => Ok(None),
            s if s.is_success() => r
                .json::<WebView>()
                .await
                .map(Some)
                .map_err(|e| SourceError(format!("GET /web: bad body: {e}"))),
            s => Err(SourceError(format!("GET /web: status {s}"))),
        }
    }

    async fn pools(&self) -> Result<PoolBackends, SourceError> {
        let r = self.get("/pools").await?;
        if !r.status().is_success() {
            return Err(SourceError(format!("GET /pools: status {}", r.status())));
        }
        let body = r
            .text()
            .await
            .map_err(|e| SourceError(format!("GET /pools: {e}")))?;
        pools::parse(&body).map_err(|e| SourceError(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_never_goes_over_plain_http_to_a_remote_host() {
        for ok in [
            "http://127.0.0.1:9900",
            "http://localhost:9900",
            "http://[::1]:9900",
            "https://wayhouse.example:9900",
        ] {
            assert!(check_transport(ok, true, false).is_ok(), "{ok}");
        }
        let err = check_transport("http://10.0.0.5:9900", true, false).unwrap_err();
        assert!(err.contains("--allow-insecure-secrets"), "{err}");
        assert!(check_transport("http://10.0.0.5:9900", true, true).is_ok());
        assert!(
            check_transport("http://10.0.0.5:9900", false, false).is_ok(),
            "no token, nothing to leak"
        );
        assert!(check_transport("ftp://x", false, false).is_err());
        assert!(check_transport("not a url", false, false).is_err());
    }
}
