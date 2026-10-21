//! The ACME admin API (opt-in with `--acme`): sites, their certificate status and the
//! account URL. No route here returns a private key.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use axum::{middleware, Json, Router};
use serde::Deserialize;

use super::types::{cert_slot, AcmeOp, Applied, SEAL_SCOPE};
use super::AcmeStore;
use crate::ha::client::{propose_write_as, ForwardHeaders};
use crate::ha::{HaHandle, WriteRequest, WriteResponse};
use crate::plugins::secrets::KeyringHandle;

#[derive(Clone)]
pub struct AcmeState {
    pub store: AcmeStore,
    pub auth_token: Option<Arc<str>>,
    /// `Some` under HA: writes go through Raft and `store` is the state machine's.
    pub ha: Option<Arc<HaHandle>>,
    pub keyring: KeyringHandle,
    pub directory_url: String,
    /// The plugin installs a site's `dns_plugin` is checked against; `None` when plugins are
    /// off (then no site can be stored, since nothing could solve its challenge).
    pub plugins: Option<crate::plugins::PluginStore>,
}

impl AcmeState {
    /// A site's `dns_plugin` must be an enabled install that approved `on_acme_dns`.
    fn check_dns_plugin(&self, id: &str) -> Result<(), String> {
        let Some(plugins) = &self.plugins else {
            return Err(
                "plugins are off; start the controller with --plugins to use a DNS plugin".into(),
            );
        };
        match plugins.get(id) {
            Ok(Some(rec)) if !rec.enabled => Err(format!("the plugin install {id} is disabled")),
            Ok(Some(rec)) if rec.approved.triggers.on_acme_dns => Ok(()),
            Ok(Some(_)) => Err(format!(
                "the plugin install {id} did not approve the on_acme_dns trigger"
            )),
            Ok(None) => Err(format!("no plugin install {id}")),
            Err(e) => Err(format!("could not read the plugin installs: {e}")),
        }
    }
}

fn err(status: StatusCode, msg: impl std::fmt::Display) -> Response {
    (
        status,
        Json(serde_json::json!({ "error": msg.to_string() })),
    )
        .into_response()
}

fn internal(e: impl std::fmt::Display) -> Response {
    tracing::error!(error = %e, "acme store failure");
    err(StatusCode::INTERNAL_SERVER_ERROR, "acme store failure")
}

pub fn router(state: AcmeState) -> Router {
    Router::new()
        .route("/acme/sites", get(list))
        .route(
            "/acme/sites/{name}",
            put(put_site).get(get_site).delete(delete_site),
        )
        .route("/acme/account", get(account))
        .route_layer(middleware::from_fn_with_state(
            wayhouse_http::server::BearerAuth::new(state.auth_token.as_deref()),
            wayhouse_http::server::require_bearer,
        ))
        .with_state(state)
}

/// The `/acme` routes of a controller that does not run ACME: 501 with a reason.
pub fn disabled_router(reason: &'static str) -> Router {
    let refuse = move || async move { err(StatusCode::NOT_IMPLEMENTED, reason) };
    Router::new()
        .route("/acme/sites", axum::routing::any(refuse))
        .route("/acme/{*rest}", axum::routing::any(refuse))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SiteBody {
    names: Vec<String>,
    #[serde(default)]
    gateways: Vec<String>,
    dns_plugin: String,
    propagation_timeout_secs: Option<u64>,
    enabled: Option<bool>,
}

fn status_of(
    st: &AcmeState,
    site: &super::Site,
) -> Result<serde_json::Value, super::AcmeStoreError> {
    let cert = st.store.cert(&site.name)?;
    let attempt = st.store.attempt(&site.name)?;
    let cert_json = cert.map(|c| {
        let held = crate::plugins::api::unreadable(
            &st.keyring,
            SEAL_SCOPE,
            &cert_slot(&site.name, c.version),
            &c.sealed_key,
        );
        serde_json::json!({
            "version": c.version, "not_after": c.not_after, "issued_at": c.issued_at,
            "serial": c.serial, "names": c.names, "key_id": c.sealed_key.key_id, "held": held,
            "untrusted": c.untrusted,
        })
    });
    Ok(serde_json::json!({
        "name": site.name, "names": site.names, "gateways": site.gateways,
        "dns_plugin": site.dns_plugin, "enabled": site.enabled,
        "propagation_timeout_secs": site.propagation_timeout_secs,
        "certificate": cert_json,
        "attempt": attempt.map(|a| serde_json::json!({
            "failures": a.failures, "next_attempt_at": a.next_attempt_at, "last_error": a.last_error,
        })),
    }))
}

async fn list(State(st): State<AcmeState>) -> Response {
    let sites = match st.store.sites() {
        Ok(s) => s,
        Err(e) => return internal(e),
    };
    let mut out = Vec::new();
    for s in &sites {
        match status_of(&st, s) {
            Ok(v) => out.push(v),
            Err(e) => return internal(e),
        }
    }
    Json(serde_json::json!({ "sites": out })).into_response()
}

async fn get_site(State(st): State<AcmeState>, Path(name): Path<String>) -> Response {
    match st.store.site(&name) {
        Ok(Some(s)) => match status_of(&st, &s) {
            Ok(v) => Json(v).into_response(),
            Err(e) => internal(e),
        },
        Ok(None) => err(StatusCode::NOT_FOUND, "no such site"),
        Err(e) => internal(e),
    }
}

async fn account(State(st): State<AcmeState>) -> Response {
    match st.store.account() {
        Ok(Some(a)) => Json(serde_json::json!({
            "directory_url": a.directory_url, "account_url": a.account_url,
            "key_id": a.sealed.key_id,
        }))
        .into_response(),
        Ok(None) => Json(serde_json::json!({
            "directory_url": st.directory_url, "account_url": null,
        }))
        .into_response(),
        Err(e) => internal(e),
    }
}

/// Maps what applying a site write did to a response.
fn answer(applied: &Applied, ok: impl FnOnce() -> Response) -> Response {
    match applied {
        Applied::Done => ok(),
        Applied::NameTaken(n) => err(
            StatusCode::CONFLICT,
            format!("{n} already belongs to another site"),
        ),
        Applied::NoSuchSite => err(StatusCode::NOT_FOUND, "no such site"),
        Applied::Invalid(why) => err(StatusCode::BAD_REQUEST, why),
        other => err(
            StatusCode::CONFLICT,
            format!("the write was refused: {other:?}"),
        ),
    }
}

async fn put_site(
    State(st): State<AcmeState>,
    headers: HeaderMap,
    Path(name): Path<String>,
    body: Bytes,
) -> Response {
    let Ok(parsed) = serde_json::from_slice::<SiteBody>(&body) else {
        return err(
            StatusCode::BAD_REQUEST,
            "the body must be {\"names\": [...], \"dns_plugin\": \"...\", \"gateways\": [...]}",
        );
    };
    let site = super::Site {
        name: name.clone(),
        names: parsed.names,
        gateways: parsed.gateways,
        dns_plugin: parsed.dns_plugin,
        propagation_timeout_secs: parsed.propagation_timeout_secs.unwrap_or(120),
        enabled: parsed.enabled.unwrap_or(true),
    };
    if let Err(why) = site.validate() {
        let wildcard = why.contains("wildcard");
        return err(
            if wildcard {
                StatusCode::UNPROCESSABLE_ENTITY
            } else {
                StatusCode::BAD_REQUEST
            },
            why,
        );
    }
    if let Err(why) = st.check_dns_plugin(&site.dns_plugin) {
        return err(StatusCode::BAD_REQUEST, why);
    }
    let op = AcmeOp::SiteSet(site);
    write(
        &st,
        op,
        Method::PUT,
        &format!("/acme/sites/{name}"),
        &headers,
        body,
        |_| Json(serde_json::json!({ "name": name, "stored": true })).into_response(),
    )
    .await
}

async fn delete_site(
    State(st): State<AcmeState>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Response {
    let op = AcmeOp::SiteDelete { name: name.clone() };
    write(
        &st,
        op,
        Method::DELETE,
        &format!("/acme/sites/{name}"),
        &headers,
        Bytes::new(),
        |_| StatusCode::NO_CONTENT.into_response(),
    )
    .await
}

/// Applies `op` locally, or under HA proposes it (forwarding to the leader when this
/// replica is not it).
async fn write(
    st: &AcmeState,
    op: AcmeOp,
    method: Method,
    path: &str,
    headers: &HeaderMap,
    body: Bytes,
    ok: impl FnOnce(&Applied) -> Response + Send,
) -> Response {
    let Some(ha) = &st.ha else {
        return match st.store.apply_local(&op) {
            Ok(applied) => answer(&applied, || ok(&applied)),
            Err(e) => internal(e),
        };
    };
    if let Err(why) = super::check_support(ha).await {
        return err(StatusCode::CONFLICT, why);
    }
    let forward = ForwardHeaders::from_headers(headers);
    let text = String::from_utf8_lossy(&body).into_owned();
    propose_write_as(
        ha,
        WriteRequest::Acme(op),
        method,
        path,
        text,
        &forward,
        |resp| match resp {
            WriteResponse::AcmeResult(applied) => answer(&applied, || ok(&applied)),
            other => internal(format!("unexpected answer {other:?}")),
        },
    )
    .await
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    use super::*;
    use crate::acme::{StoredAccount, StoredCert};
    use crate::plugins::secrets::Sealed;

    fn dns_install(id: &str, enabled: bool, approved: bool) -> crate::plugins::InstallRecord {
        let approved_caps = wayhouse_plugin_host::Capabilities::parse(
            format!(
                r#"{{"triggers":{{"on_timer":true,"on_acme_dns":{approved}}},"tick_interval_secs":30}}"#
            )
            .as_bytes(),
        )
        .unwrap();
        crate::plugins::InstallRecord {
            id: id.into(),
            name: id.into(),
            sha256: "0".repeat(64),
            size: 1,
            approved: approved_caps,
            config: serde_json::Value::Null,
            enabled,
            created_at: 0,
            created_by: None,
            webhook: None,
        }
    }

    fn app(token: Option<&str>) -> (Router, AcmeStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db = sled::open(dir.path()).unwrap();
        let store = AcmeStore::open(&db).unwrap();
        let plugins = crate::plugins::PluginStore::open(&db).unwrap();
        plugins.put_blob(&"0".repeat(64), b"module").unwrap();
        plugins
            .create(&dns_install("0123456789abcdef", true, true))
            .unwrap();
        plugins
            .create(&dns_install("notapproved000000", true, false))
            .unwrap();
        plugins
            .create(&dns_install("disabled00000000", false, true))
            .unwrap();
        let state = AcmeState {
            store: store.clone(),
            auth_token: token.map(Arc::from),
            ha: None,
            keyring: KeyringHandle::default(),
            directory_url: "https://ca.example/dir".into(),
            plugins: Some(plugins.clone()),
        };
        (router(state), store, dir)
    }

    async fn call(
        app: &Router,
        method: &str,
        uri: &str,
        body: &str,
        token: Option<&str>,
    ) -> (StatusCode, serde_json::Value) {
        let mut req = Request::builder().method(method).uri(uri);
        if let Some(t) = token {
            req = req.header("authorization", format!("Bearer {t}"));
        }
        let resp = app
            .clone()
            .oneshot(req.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
        )
    }

    const SITE: &str =
        r#"{"names":["panel.example.com"],"gateways":["gw1"],"dns_plugin":"0123456789abcdef"}"#;

    #[tokio::test]
    async fn sites_are_created_listed_read_and_deleted() {
        let (app, _s, _d) = app(None);
        let (st, _) = call(&app, "PUT", "/acme/sites/web", SITE, None).await;
        assert_eq!(st, StatusCode::OK);
        let (_, list) = call(&app, "GET", "/acme/sites", "", None).await;
        assert_eq!(list["sites"][0]["name"], "web");
        assert!(list["sites"][0]["certificate"].is_null());
        let (st, one) = call(&app, "GET", "/acme/sites/web", "", None).await;
        assert_eq!(
            (st, one["names"][0].as_str()),
            (StatusCode::OK, Some("panel.example.com"))
        );
        assert_eq!(
            call(&app, "DELETE", "/acme/sites/web", "", None).await.0,
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            call(&app, "GET", "/acme/sites/web", "", None).await.0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            call(&app, "DELETE", "/acme/sites/web", "", None).await.0,
            StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn a_site_needs_an_enabled_install_that_approved_on_acme_dns() {
        let (app, _s, _d) = app(None);
        for plugin in ["missing0000000000", "notapproved000000", "disabled00000000"] {
            let body = format!(r#"{{"names":["a.example.com"],"dns_plugin":"{plugin}"}}"#);
            let (st, e) = call(&app, "PUT", "/acme/sites/a", &body, None).await;
            assert_eq!(st, StatusCode::BAD_REQUEST, "{plugin}: {e}");
        }
    }

    #[tokio::test]
    async fn with_plugins_off_no_site_can_be_stored() {
        let dir = tempfile::tempdir().unwrap();
        let db = sled::open(dir.path()).unwrap();
        let state = AcmeState {
            store: AcmeStore::open(&db).unwrap(),
            auth_token: None,
            ha: None,
            keyring: KeyringHandle::default(),
            directory_url: "https://ca.example/dir".into(),
            plugins: None,
        };
        let (st, e) = call(&router(state), "PUT", "/acme/sites/a", SITE, None).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        assert!(e["error"].as_str().unwrap().contains("--plugins"), "{e}");
    }

    #[tokio::test]
    async fn invalid_sites_are_refused_with_the_right_status() {
        let (app, _s, _d) = app(None);
        let wild = r#"{"names":["*.example.com"],"dns_plugin":"0123456789abcdef"}"#;
        assert_eq!(
            call(&app, "PUT", "/acme/sites/w", wild, None).await.0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
        let upper = r#"{"names":["Panel.example.com"],"dns_plugin":"x"}"#;
        assert_eq!(
            call(&app, "PUT", "/acme/sites/w", upper, None).await.0,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            call(&app, "PUT", "/acme/sites/w", "{}", None).await.0,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            call(
                &app,
                "PUT",
                "/acme/sites/w",
                r#"{"names":["a.example.com"],"dns_plugin":"x","surprise":1}"#,
                None
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            call(&app, "PUT", "/acme/sites/Bad_Name", SITE, None)
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
    }

    #[tokio::test]
    async fn a_hostname_belongs_to_one_site() {
        let (app, _s, _d) = app(None);
        assert_eq!(
            call(&app, "PUT", "/acme/sites/one", SITE, None).await.0,
            StatusCode::OK
        );
        let (st, v) = call(&app, "PUT", "/acme/sites/two", SITE, None).await;
        assert_eq!(st, StatusCode::CONFLICT);
        assert!(v["error"].as_str().unwrap().contains("panel.example.com"));
    }

    #[tokio::test]
    async fn status_shows_the_certificate_but_never_key_material() {
        let (app, store, _d) = app(None);
        call(&app, "PUT", "/acme/sites/web", SITE, None).await;
        store
            .apply_local(&AcmeOp::CertIssued {
                site: "web".into(),
                tagged_term: 0,
                cert: StoredCert {
                    version: 1,
                    chain_pem: "-----BEGIN CERTIFICATE-----".into(),
                    sealed_key: Sealed {
                        key_id: "k1".into(),
                        nonce: "NONCE".into(),
                        ciphertext: "CIPHERTEXT".into(),
                    },
                    not_after: 2_000_000_000,
                    issued_at: 1,
                    serial: "0a".into(),
                    names: vec!["panel.example.com".into()],
                    untrusted: true,
                },
            })
            .unwrap();
        let (_, raw) = call(&app, "GET", "/acme/sites/web", "", None).await;
        assert_eq!(raw["certificate"]["version"], 1);
        assert_eq!(raw["certificate"]["held"], "held: no secret key");
        assert_eq!(raw["certificate"]["untrusted"], true);
        let text = raw.to_string();
        for forbidden in ["CIPHERTEXT", "NONCE", "BEGIN CERTIFICATE"] {
            assert!(
                !text.contains(forbidden),
                "{forbidden} leaked into the status"
            );
        }
    }

    #[tokio::test]
    async fn the_account_endpoint_gives_the_url_for_caa_pinning_and_no_key() {
        let (app, store, _d) = app(None);
        let (_, v) = call(&app, "GET", "/acme/account", "", None).await;
        assert!(v["account_url"].is_null());
        assert_eq!(v["directory_url"], "https://ca.example/dir");
        store
            .apply_local(&AcmeOp::AccountSet(StoredAccount {
                directory_url: "https://ca.example/dir".into(),
                account_url: "https://ca.example/acct/7".into(),
                sealed: Sealed {
                    key_id: "k1".into(),
                    nonce: "n".into(),
                    ciphertext: "SECRETCT".into(),
                },
            }))
            .unwrap();
        let (_, v) = call(&app, "GET", "/acme/account", "", None).await;
        assert_eq!(v["account_url"], "https://ca.example/acct/7");
        assert!(!v.to_string().contains("SECRETCT"));
    }

    #[tokio::test]
    async fn the_bearer_token_gates_every_route() {
        let (app, _s, _d) = app(Some("a-long-enough-token"));
        assert_eq!(
            call(&app, "GET", "/acme/sites", "", None).await.0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            call(&app, "PUT", "/acme/sites/web", SITE, Some("wrong"))
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            call(&app, "GET", "/acme/sites", "", Some("a-long-enough-token"))
                .await
                .0,
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn a_controller_without_acme_answers_501() {
        let app = disabled_router("off");
        assert_eq!(
            call(&app, "GET", "/acme/sites", "", None).await.0,
            StatusCode::NOT_IMPLEMENTED
        );
        assert_eq!(
            call(&app, "PUT", "/acme/sites/x", SITE, None).await.0,
            StatusCode::NOT_IMPLEMENTED
        );
    }
}
