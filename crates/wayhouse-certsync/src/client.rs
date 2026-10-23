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
    /// error, a 5xx or a 429 (each controller has its own limiter). A 401 or 403 stops the search: every controller holds the same gateways.
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
                Ok(r)
                    if r.status().is_server_error()
                        || r.status() == StatusCode::TOO_MANY_REQUESTS =>
                {
                    reasons.push(format!(
                        "{}: HTTP {}",
                        url.host_str().unwrap_or("?"),
                        r.status().as_u16()
                    ))
                }
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

    /// The body, read in chunks so a response without a `Content-Length` cannot grow past the cap.
    async fn body(mut resp: reqwest::Response) -> Result<Vec<u8>, FetchError> {
        let too_large = || FetchError::Bad("the response is too large".into());
        if resp.content_length().is_some_and(|l| l > MAX_BODY) {
            return Err(too_large());
        }
        let mut out = Vec::new();
        while let Some(chunk) = resp
            .chunk()
            .await
            .map_err(|e| FetchError::Bad(format!("reading the response: {e}")))?
        {
            if (out.len() + chunk.len()) as u64 > MAX_BODY {
                return Err(too_large());
            }
            out.extend_from_slice(&chunk);
        }
        Ok(out)
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

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    /// A controller that answers with `chunks` chunks of 64 KiB and no `Content-Length`.
    async fn streaming(chunks: usize) -> Url {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = Url::parse(&format!("http://{}", l.local_addr().unwrap())).unwrap();
        tokio::spawn(async move {
            let (mut sock, _) = l.accept().await.unwrap();
            let mut req = [0u8; 1024];
            let _ = sock.read(&mut req).await;
            let head = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n";
            sock.write_all(head.as_bytes()).await.unwrap();
            for _ in 0..chunks {
                let mut chunk = format!("{:x}\r\n", 64 * 1024).into_bytes();
                chunk.extend(std::iter::repeat_n(b'x', 64 * 1024));
                chunk.extend_from_slice(b"\r\n");
                if sock.write_all(&chunk).await.is_err() {
                    return; // the client gave up, as it should
                }
            }
            let _ = sock.write_all(b"0\r\n\r\n").await;
        });
        url
    }

    #[tokio::test]
    async fn a_chunked_body_past_the_cap_is_refused_while_reading() {
        let url = streaming(17).await; // 17 * 64 KiB > 1 MiB, and no Content-Length
        let c = Controller::new(vec![url], "t".into(), Duration::from_secs(5)).unwrap();
        let err = c.fetch("web", None).await.unwrap_err();
        assert!(err.to_string().contains("too large"), "{err}");
    }

    /// A controller that answers one request with the raw `response`.
    async fn canned(response: &'static str) -> Url {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = Url::parse(&format!("http://{}", l.local_addr().unwrap())).unwrap();
        tokio::spawn(async move {
            let (mut sock, _) = l.accept().await.unwrap();
            let mut req = [0u8; 1024];
            let _ = sock.read(&mut req).await;
            let _ = sock.write_all(response.as_bytes()).await;
        });
        url
    }

    #[tokio::test]
    async fn a_429_from_one_controller_falls_through_to_the_next() {
        let limited = canned("HTTP/1.1 429 Too Many Requests\r\nContent-Length: 0\r\n\r\n").await;
        let body = r#"{"sites":[]}"#;
        let ok = canned(Box::leak(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            )
            .into_boxed_str(),
        ))
        .await;
        let c = Controller::new(vec![limited, ok], "t".into(), Duration::from_secs(5)).unwrap();
        assert!(c.list().await.unwrap().is_empty());
    }
}
