//! The controller's bundle API (`GET /acme/bundles`, `GET /acme/bundles/{site}`), tried
//! across the configured controller URLs.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use reqwest::{header, StatusCode, Url};
use serde::Deserialize;

/// Largest response accepted; a bundle is a few kilobytes.
const MAX_BODY: u64 = 1 << 20;

#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    /// No controller answered usefully; the reasons, one per URL.
    #[error("no controller could be reached: {0}")]
    Unreachable(String),
    #[error(
        "the controller refused the gateway token (HTTP {0}); check --token-file, the gateway \
         may have been revoked"
    )]
    Unauthorized(u16),
    #[error("{0}")]
    Bad(String),
}

/// One line of the listing: a site this gateway may fetch and its newest version.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Entry {
    pub site: String,
    pub version: u64,
    pub not_after: u64,
}

/// A certificate chain and its private key.
#[derive(Clone, Deserialize)]
pub struct Bundle {
    pub version: u64,
    pub chain_pem: String,
    pub key_pem: zeroize::Zeroizing<String>,
    pub not_after: u64,
    #[serde(default)]
    pub serial: String,
    #[serde(default)]
    pub untrusted: bool,
}

impl std::fmt::Debug for Bundle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Bundle")
            .field("version", &self.version)
            .field("not_after", &self.not_after)
            .field("key_pem", &"<redacted>")
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub enum Fetched {
    NotModified,
    Bundle(Box<Bundle>),
    /// The site is no longer readable with this token.
    Gone,
}

pub struct Controller {
    urls: Vec<Url>,
    token: String,
    client: reqwest::Client,
    /// The URL that answered last; tried first next time.
    preferred: AtomicUsize,
}

impl Controller {
    pub fn new(urls: Vec<Url>, token: String, timeout: Duration) -> Result<Self, FetchError> {
        // No redirects: the bearer token and the private key must go only where they were sent.
        let client = wayhouse_http::builder()
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| FetchError::Bad(format!("http client: {e}")))?;
        Ok(Self {
            urls,
            token,
            client,
            preferred: AtomicUsize::new(0),
        })
    }

    /// GETs `path` from the first controller that answers with something other than a network
    /// error or a 5xx. A 401 or 403 stops the search: every controller holds the same gateways.
    async fn get(
        &self,
        path: &str,
        if_none_match: Option<u64>,
    ) -> Result<reqwest::Response, FetchError> {
        let n = self.urls.len();
        let start = self.preferred.load(Ordering::Relaxed) % n;
        let mut reasons = Vec::new();
        for i in 0..n {
            let idx = (start + i) % n;
            let url = &self.urls[idx];
            let target = format!("{}{path}", url.as_str().trim_end_matches('/'));
            let mut req = self.client.get(&target).bearer_auth(&self.token);
            if let Some(v) = if_none_match {
                req = req.header(header::IF_NONE_MATCH, format!("\"{v}\""));
            }
            match req.send().await {
                Err(e) => reasons.push(format!(
                    "{}: {}",
                    url.host_str().unwrap_or("?"),
                    wayhouse_http::error_chain(&e)
                )),
                Ok(r) if r.status().is_server_error() => reasons.push(format!(
                    "{}: HTTP {}",
                    url.host_str().unwrap_or("?"),
                    r.status().as_u16()
                )),
                Ok(r) if matches!(r.status(), StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) => {
                    return Err(FetchError::Unauthorized(r.status().as_u16()));
                }
                Ok(r) => {
                    self.preferred.store(idx, Ordering::Relaxed);
                    return Ok(r);
                }
            }
        }
        Err(FetchError::Unreachable(reasons.join("; ")))
    }

    async fn body(resp: reqwest::Response) -> Result<Vec<u8>, FetchError> {
        if resp.content_length().is_some_and(|l| l > MAX_BODY) {
            return Err(FetchError::Bad("the response is too large".into()));
        }
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| FetchError::Bad(format!("reading the response: {e}")))?;
        if bytes.len() as u64 > MAX_BODY {
            return Err(FetchError::Bad("the response is too large".into()));
        }
        Ok(bytes.to_vec())
    }

    /// The sites this token may read and have a certificate.
    pub async fn list(&self) -> Result<Vec<Entry>, FetchError> {
        #[derive(Deserialize)]
        struct Listing {
            sites: Vec<Entry>,
        }
        let resp = self.get("/acme/bundles", None).await?;
        if !resp.status().is_success() {
            return Err(FetchError::Bad(format!(
                "GET /acme/bundles: HTTP {}",
                resp.status().as_u16()
            )));
        }
        let body = Self::body(resp).await?;
        serde_json::from_slice::<Listing>(&body)
            .map(|l| l.sites)
            .map_err(|e| FetchError::Bad(format!("GET /acme/bundles: bad body: {e}")))
    }

    /// The bundle of `site`; `have` is the version already on disk, if any.
    pub async fn fetch(&self, site: &str, have: Option<u64>) -> Result<Fetched, FetchError> {
        let resp = self.get(&format!("/acme/bundles/{site}"), have).await?;
        match resp.status() {
            StatusCode::NOT_MODIFIED => Ok(Fetched::NotModified),
            StatusCode::NOT_FOUND => Ok(Fetched::Gone),
            s if s.is_success() => {
                let body = Self::body(resp).await?;
                serde_json::from_slice::<Bundle>(&body)
                    .map(|b| Fetched::Bundle(Box::new(b)))
                    .map_err(|e| FetchError::Bad(format!("site {site}: bad body: {e}")))
            }
            s => Err(FetchError::Bad(format!(
                "GET /acme/bundles/{site}: HTTP {}",
                s.as_u16()
            ))),
        }
    }
}
