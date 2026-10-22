//! Certificate distribution to gateways: `GET /acme/bundles` and `GET /acme/bundles/{site}`.
//!
//! A gateway authenticates with its own bearer token (see [`new_token`]); the admin token does
//! not open these routes. A token reaches only the sites that list its gateway name, and
//! anything else answers 404 so a site's existence is not revealed. Any controller node serves
//! bundles, followers included, from its replicated store. The private key leaves the
//! controller only in the body of a successful bundle response, and is never logged.

use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::RngCore;
use sha2::{Digest, Sha256};

use super::types::{cert_slot, Gateway, SEAL_SCOPE};
use super::AcmeStore;
use crate::plugins::secrets::KeyringHandle;

#[derive(Clone)]
pub struct BundleState {
    pub store: AcmeStore,
    pub keyring: KeyringHandle,
}

/// A new gateway token: 256 random bits, URL-safe.
pub fn new_token() -> String {
    let mut raw = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut raw);
    format!("gwt_{}", URL_SAFE_NO_PAD.encode(raw))
}

/// The hash stored for a gateway token (lowercase hex SHA-256).
pub fn hash_token(token: &str) -> String {
    Sha256::digest(token.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub fn router(state: BundleState) -> Router {
    Router::new()
        .route("/acme/bundles", get(list))
        .route("/acme/bundles/{site}", get(fetch))
        .with_state(state)
}

fn err(status: StatusCode, msg: &str) -> Response {
    (status, Json(serde_json::json!({ "error": msg }))).into_response()
}

fn internal(e: impl std::fmt::Display) -> Response {
    tracing::error!(error = %e, "acme bundle failure");
    err(StatusCode::INTERNAL_SERVER_ERROR, "acme store failure")
}

/// The token in an `Authorization: Bearer <token>` value; the scheme is case-insensitive.
fn bearer_token(value: &str) -> Option<&str> {
    let (scheme, token) = value.split_once(' ')?;
    scheme.eq_ignore_ascii_case("bearer").then_some(token)
}

/// True for a string shaped like a token from [`new_token`], so a malformed one is refused
/// without reading the gateway table.
fn well_formed(token: &str) -> bool {
    token.len() == 4 + 43 && token.starts_with("gwt_")
}

/// The gateway the bearer token belongs to. Every stored hash is compared, so the time taken
/// does not depend on which gateway (if any) matched.
fn authenticate(st: &BundleState, headers: &HeaderMap) -> Result<Gateway, Box<Response>> {
    let denied = || Box::new(err(StatusCode::UNAUTHORIZED, "unauthorized"));
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(bearer_token)
        .filter(|t| well_formed(t))
        .ok_or_else(denied)?;
    let presented = hash_token(token);
    let gateways = st.store.gateways().map_err(|e| Box::new(internal(e)))?;
    let mut found = None;
    for gw in gateways {
        if wayhouse_http::token_eq(&presented, &gw.token_hash) {
            found = Some(gw);
        }
    }
    found.ok_or_else(denied)
}

async fn list(State(st): State<BundleState>, headers: HeaderMap) -> Response {
    let gw = match authenticate(&st, &headers) {
        Ok(g) => g,
        Err(r) => return *r,
    };
    let sites = match st.store.sites() {
        Ok(s) => s,
        Err(e) => return internal(e),
    };
    let mut out = Vec::new();
    for s in sites.iter().filter(|s| s.gateways.contains(&gw.name)) {
        match st.store.cert(&s.name) {
            Ok(Some(c)) => out.push(serde_json::json!({
                "site": s.name, "version": c.version, "not_after": c.not_after,
            })),
            Ok(None) => {}
            Err(e) => return internal(e),
        }
    }
    Json(serde_json::json!({ "sites": out })).into_response()
}

/// True when an `If-None-Match` value names `version` (or `*`).
fn matches_etag(headers: &HeaderMap, version: u64) -> bool {
    let Some(v) = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
    else {
        return false;
    };
    v.split(',').any(|t| {
        let t = t.trim();
        let t = t.strip_prefix("W/").unwrap_or(t).trim_matches('"');
        t == "*" || t == version.to_string()
    })
}

async fn fetch(
    State(st): State<BundleState>,
    headers: HeaderMap,
    Path(site): Path<String>,
) -> Response {
    let gw = match authenticate(&st, &headers) {
        Ok(g) => g,
        Err(r) => return *r,
    };
    let missing = || err(StatusCode::NOT_FOUND, "no such bundle");
    let found = match st.store.site(&site) {
        Ok(s) => s,
        Err(e) => return internal(e),
    };
    // A site this gateway is not listed on looks the same as one that does not exist.
    if !found.is_some_and(|s| s.gateways.contains(&gw.name)) {
        return missing();
    }
    let cert = match st.store.cert(&site) {
        Ok(Some(c)) => c,
        Ok(None) => return err(StatusCode::NOT_FOUND, "no certificate has been issued yet"),
        Err(e) => return internal(e),
    };
    let etag = HeaderValue::from_str(&format!("\"{}\"", cert.version)).expect("digits and quotes");
    if matches_etag(&headers, cert.version) {
        tracing::debug!(site = %site, gateway = %gw.name, version = cert.version, "bundle not modified");
        let mut resp = StatusCode::NOT_MODIFIED.into_response();
        resp.headers_mut().insert(header::ETAG, etag);
        return resp;
    }
    let key = match st.keyring.current().open(
        SEAL_SCOPE,
        &cert_slot(&site, cert.version),
        &cert.sealed_key,
    ) {
        Ok(k) => k,
        Err(e) => {
            tracing::error!(site = %site, error = %e, "cannot open the certificate key");
            return err(
                StatusCode::SERVICE_UNAVAILABLE,
                "this controller cannot read the certificate key (is the cluster secret key loaded?)",
            );
        }
    };
    let Ok(key_pem) = std::str::from_utf8(key.expose()) else {
        return internal("certificate key is not UTF-8");
    };
    // The audit line names who fetched which version, never the key.
    tracing::info!(site = %site, gateway = %gw.name, version = cert.version, "certificate bundle served");
    let mut resp = Json(serde_json::json!({
        "version": cert.version,
        "chain_pem": cert.chain_pem,
        "key_pem": key_pem,
        "not_after": cert.not_after,
        "serial": cert.serial,
        "untrusted": cert.untrusted,
    }))
    .into_response();
    resp.headers_mut().insert(header::ETAG, etag);
    resp.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    use super::*;
    use crate::acme::{AcmeOp, Applied, Site, StoredCert};

    #[test]
    fn the_bearer_scheme_is_case_insensitive_and_tokens_are_checked_for_shape() {
        assert_eq!(bearer_token("Bearer abc"), Some("abc"));
        assert_eq!(bearer_token("bearer abc"), Some("abc"));
        assert_eq!(bearer_token("BEARER abc"), Some("abc"));
        assert_eq!(bearer_token("Basic abc"), None);
        assert_eq!(bearer_token("Bearer"), None);
        assert!(well_formed(&new_token()));
        assert!(!well_formed("gwt_short"));
        assert!(!well_formed(&"x".repeat(47)));
    }
    use crate::plugins::secrets::Keyring;

    const KEY: &str = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=";
    const KEY_PEM: &str =
        "-----BEGIN PRIVATE KEY-----\nTOPSECRETKEYMATERIAL\n-----END PRIVATE KEY-----\n";

    struct Fixture {
        app: Router,
        store: AcmeStore,
        keyring: KeyringHandle,
        _dir: tempfile::TempDir,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let db = sled::open(dir.path()).unwrap();
        let store = AcmeStore::open(&db).unwrap();
        let keyring = KeyringHandle::new(Keyring::parse(KEY).unwrap());
        let app = router(BundleState {
            store: store.clone(),
            keyring: keyring.clone(),
        });
        Fixture {
            app,
            store,
            keyring,
            _dir: dir,
        }
    }

    fn site(name: &str, host: &str, gateways: &[&str]) -> Site {
        Site {
            name: name.into(),
            names: vec![host.into()],
            gateways: gateways.iter().map(|g| (*g).into()).collect(),
            challenge: crate::acme::ChallengeKind::Dns01,
            dns_plugin: "p".into(),
            challenge_zone: None,
            propagation_timeout_secs: 120,
            enabled: true,
        }
    }

    impl Fixture {
        fn site(&self, name: &str, host: &str, gateways: &[&str]) {
            let op = AcmeOp::SiteSet(site(name, host, gateways));
            assert_eq!(self.store.apply_local(&op).unwrap(), Applied::Done);
        }

        fn cert(&self, site: &str, version: u64) {
            let host = self.store.site(site).unwrap().unwrap().names;
            let sealed = self
                .keyring
                .current()
                .seal(SEAL_SCOPE, &cert_slot(site, version), KEY_PEM.as_bytes())
                .unwrap();
            let op = AcmeOp::CertIssued {
                site: site.into(),
                tagged_term: 0,
                cert: StoredCert {
                    version,
                    chain_pem: format!("CHAIN-{site}-{version}"),
                    sealed_key: sealed,
                    not_after: 2_000_000_000,
                    issued_at: 1,
                    serial: "01".into(),
                    names: host,
                    untrusted: false,
                },
            };
            assert_eq!(self.store.apply_local(&op).unwrap(), Applied::Done);
        }

        fn gateway(&self, name: &str) -> String {
            let token = new_token();
            let op = AcmeOp::GatewaySet(Gateway {
                name: name.into(),
                token_hash: hash_token(&token),
                created_at: 1,
            });
            assert_eq!(self.store.apply_local(&op).unwrap(), Applied::Done);
            token
        }

        async fn get(
            &self,
            uri: &str,
            token: Option<&str>,
            extra: &[(&str, &str)],
        ) -> (StatusCode, HeaderMap, String) {
            let mut req = Request::builder().uri(uri);
            if let Some(t) = token {
                req = req.header("authorization", format!("Bearer {t}"));
            }
            for (k, v) in extra {
                req = req.header(*k, *v);
            }
            let resp = self
                .app
                .clone()
                .oneshot(req.body(Body::empty()).unwrap())
                .await
                .unwrap();
            let (status, headers) = (resp.status(), resp.headers().clone());
            let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
                .await
                .unwrap();
            (status, headers, String::from_utf8(bytes.to_vec()).unwrap())
        }
    }

    #[tokio::test]
    async fn a_gateway_gets_the_chain_and_the_key_of_its_site() {
        let f = fixture();
        f.site("web", "web.example.com", &["edge"]);
        f.cert("web", 1);
        let token = f.gateway("edge");
        let (st, h, body) = f.get("/acme/bundles/web", Some(&token), &[]).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(h[header::ETAG], "\"1\"");
        assert_eq!(h[header::CACHE_CONTROL], "no-store");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["version"], 1);
        assert_eq!(v["chain_pem"], "CHAIN-web-1");
        assert_eq!(v["key_pem"], KEY_PEM);
        assert_eq!(v["not_after"], 2_000_000_000u64);
    }

    #[tokio::test]
    async fn a_token_reaches_only_the_sites_that_list_its_gateway() {
        let f = fixture();
        f.site("a", "a.example.com", &["gw-a"]);
        f.site("b", "b.example.com", &["gw-b"]);
        f.cert("a", 1);
        f.cert("b", 1);
        let token_a = f.gateway("gw-a");
        let _token_b = f.gateway("gw-b");
        assert_eq!(
            f.get("/acme/bundles/a", Some(&token_a), &[]).await.0,
            StatusCode::OK
        );
        let (st, _, body) = f.get("/acme/bundles/b", Some(&token_a), &[]).await;
        assert_eq!(st, StatusCode::NOT_FOUND);
        assert!(!body.contains("TOPSECRET"));
        // A site that does not exist answers the same.
        let (st2, _, body2) = f.get("/acme/bundles/nope", Some(&token_a), &[]).await;
        assert_eq!((st2, body2), (StatusCode::NOT_FOUND, body));
        let (_, _, list) = f.get("/acme/bundles", Some(&token_a), &[]).await;
        let list: serde_json::Value = serde_json::from_str(&list).unwrap();
        assert_eq!(list["sites"].as_array().unwrap().len(), 1);
        assert_eq!(list["sites"][0]["site"], "a");
        assert_eq!(list["sites"][0]["version"], 1);
        assert!(!list.to_string().contains("TOPSECRET"));
    }

    #[tokio::test]
    async fn unknown_missing_and_revoked_tokens_get_401() {
        let f = fixture();
        f.site("web", "web.example.com", &["edge"]);
        f.cert("web", 1);
        let token = f.gateway("edge");
        for t in [None, Some("gwt_nothing"), Some("")] {
            assert_eq!(
                f.get("/acme/bundles/web", t, &[]).await.0,
                StatusCode::UNAUTHORIZED,
                "{t:?}"
            );
            assert_eq!(
                f.get("/acme/bundles", t, &[]).await.0,
                StatusCode::UNAUTHORIZED
            );
        }
        assert_eq!(
            f.get("/acme/bundles/web", Some(&token), &[]).await.0,
            StatusCode::OK
        );
        let del = AcmeOp::GatewayDelete {
            name: "edge".into(),
        };
        assert_eq!(f.store.apply_local(&del).unwrap(), Applied::Done);
        assert_eq!(
            f.get("/acme/bundles/web", Some(&token), &[]).await.0,
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn if_none_match_gives_304_until_a_new_version_is_issued() {
        let f = fixture();
        f.site("web", "web.example.com", &["edge"]);
        f.cert("web", 1);
        let token = f.gateway("edge");
        let (st, h, body) = f
            .get(
                "/acme/bundles/web",
                Some(&token),
                &[("if-none-match", "\"1\"")],
            )
            .await;
        assert_eq!((st, body.as_str()), (StatusCode::NOT_MODIFIED, ""));
        assert_eq!(h[header::ETAG], "\"1\"");
        f.cert("web", 2);
        let (st, _, body) = f
            .get(
                "/acme/bundles/web",
                Some(&token),
                &[("if-none-match", "\"1\"")],
            )
            .await;
        assert_eq!(st, StatusCode::OK);
        assert!(body.contains("CHAIN-web-2"));
    }

    #[tokio::test]
    async fn a_site_without_a_certificate_yet_is_404_and_not_listed() {
        let f = fixture();
        f.site("web", "web.example.com", &["edge"]);
        let token = f.gateway("edge");
        assert_eq!(
            f.get("/acme/bundles/web", Some(&token), &[]).await.0,
            StatusCode::NOT_FOUND
        );
        let (_, _, list) = f.get("/acme/bundles", Some(&token), &[]).await;
        assert!(list.contains("\"sites\":[]"), "{list}");
    }

    #[tokio::test]
    async fn a_controller_without_the_secret_key_cannot_serve_the_key() {
        let f = fixture();
        f.site("web", "web.example.com", &["edge"]);
        f.cert("web", 1);
        let token = f.gateway("edge");
        // The same store, behind a node that holds a different cluster key.
        let other = router(BundleState {
            store: f.store.clone(),
            keyring: KeyringHandle::new(
                Keyring::parse("AQECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=").unwrap(),
            ),
        });
        let resp = other
            .oneshot(
                Request::builder()
                    .uri("/acme/bundles/web")
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    /// Collects log output so a test can scan it.
    #[derive(Clone, Default)]
    struct Capture(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for Capture {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Capture {
        type Writer = Capture;
        fn make_writer(&'a self) -> Capture {
            self.clone()
        }
    }

    /// The log of every test in this process. A process-wide subscriber (rather than a scoped
    /// one) keeps `tracing`'s per-callsite cache from hiding events when tests run in parallel.
    fn process_log() -> &'static Capture {
        static LOG: std::sync::OnceLock<Capture> = std::sync::OnceLock::new();
        LOG.get_or_init(|| {
            let log = Capture::default();
            let subscriber = tracing_subscriber::fmt()
                .with_writer(log.clone())
                .with_ansi(false)
                .with_max_level(tracing::Level::TRACE)
                .finish();
            tracing::subscriber::set_global_default(subscriber)
                .expect("no other test installs a global subscriber");
            log
        })
    }

    #[tokio::test]
    async fn the_audit_line_names_the_fetch_and_no_log_holds_the_key() {
        let log = process_log();
        let f = fixture();
        f.site("audit-site", "audit.example.com", &["audit-gw"]);
        f.cert("audit-site", 1);
        let token = f.gateway("audit-gw");
        let (st, _, body) = f.get("/acme/bundles/audit-site", Some(&token), &[]).await;
        assert_eq!(st, StatusCode::OK);
        assert!(body.contains("TOPSECRET"), "the key is in the response");
        // And the other paths: a refused token and a 304.
        f.get("/acme/bundles/audit-site", Some("gwt_bad"), &[])
            .await;
        f.get(
            "/acme/bundles/audit-site",
            Some(&token),
            &[("if-none-match", "\"1\"")],
        )
        .await;
        let text = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
        let audit: Vec<&str> = text
            .lines()
            .filter(|l| l.contains("certificate bundle served") && l.contains("audit-site"))
            .collect();
        assert_eq!(audit.len(), 1, "one line per served bundle: {text}");
        assert!(
            audit[0].contains("gateway=audit-gw") && audit[0].contains("version=1"),
            "{}",
            audit[0]
        );
        assert!(
            !text.contains("TOPSECRET") && !text.contains("PRIVATE KEY"),
            "no log line holds the key"
        );
        assert!(!text.contains(&token), "no log line holds the token");
    }

    #[test]
    fn tokens_are_distinct_and_only_their_hash_is_comparable() {
        let (a, b) = (new_token(), new_token());
        assert_ne!(a, b);
        assert!(a.starts_with("gwt_") && a.len() > 40);
        assert_eq!(hash_token(&a).len(), 64);
        assert_ne!(hash_token(&a), hash_token(&b));
    }
}
