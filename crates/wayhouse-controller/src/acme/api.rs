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

use super::types::{
    cert_slot, AcmeOp, Applied, RewrapAccount, RewrapCert, ACCOUNT_SLOT, SEAL_SCOPE,
};
use super::AcmeStore;
use crate::ha::client::{forward_to_current_leader, propose_write_as, ForwardHeaders};
use crate::ha::{HaHandle, WriteRequest, WriteResponse};
use crate::plugins::secrets::{KeyringHandle, Sealed};

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
        .route("/admin/acme/rewrap", axum::routing::post(rewrap))
        .route("/acme/gateways", get(list_gateways).post(create_gateway))
        .route(
            "/acme/gateways/{name}",
            axum::routing::delete(delete_gateway),
        )
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
        .route("/admin/acme/rewrap", axum::routing::any(refuse))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SiteBody {
    names: Vec<String>,
    #[serde(default)]
    gateways: Vec<String>,
    /// `dns-01` (the default) or `http-01`.
    #[serde(default)]
    challenge: super::ChallengeKind,
    #[serde(default)]
    dns_plugin: String,
    #[serde(default)]
    challenge_zone: Option<String>,
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
        "challenge": site.challenge.as_str(),
        "dns_plugin": site.dns_plugin, "enabled": site.enabled,
        "challenge_zone": site.challenge_zone,
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
        Applied::NoSuchGateway => err(StatusCode::NOT_FOUND, "no such gateway"),
        Applied::Exists => err(
            StatusCode::CONFLICT,
            "already exists; delete the gateway first to issue it a new token",
        ),
        Applied::Invalid(why) => err(StatusCode::BAD_REQUEST, why),
        other => err(
            StatusCode::CONFLICT,
            format!("the write was refused: {other:?}"),
        ),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GatewayBody {
    name: String,
}

/// The gateways (never their tokens or hashes) and the sites each may fetch.
async fn list_gateways(State(st): State<AcmeState>) -> Response {
    let (gateways, sites) = match (st.store.gateways(), st.store.sites()) {
        (Ok(g), Ok(s)) => (g, s),
        (Err(e), _) | (_, Err(e)) => return internal(e),
    };
    let out: Vec<_> = gateways
        .iter()
        .map(|g| {
            let reach: Vec<&str> = sites
                .iter()
                .filter(|s| s.gateways.contains(&g.name))
                .map(|s| s.name.as_str())
                .collect();
            serde_json::json!({ "name": g.name, "created_at": g.created_at, "sites": reach })
        })
        .collect();
    Json(serde_json::json!({ "gateways": out })).into_response()
}

/// Registers a gateway. The token is in this response only; the store keeps its hash.
async fn create_gateway(State(st): State<AcmeState>, headers: HeaderMap, body: Bytes) -> Response {
    let Ok(parsed) = serde_json::from_slice::<GatewayBody>(&body) else {
        return err(
            StatusCode::BAD_REQUEST,
            "the body must be {\"name\": \"...\"}",
        );
    };
    if !super::Site::valid_name(&parsed.name) {
        return err(
            StatusCode::BAD_REQUEST,
            "a gateway name is 1 to 63 of a-z, 0-9 and '-'",
        );
    }
    let token = super::bundles::new_token();
    let created_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let name = parsed.name;
    let op = AcmeOp::GatewaySet(super::Gateway {
        name: name.clone(),
        token_hash: super::bundles::hash_token(&token),
        created_at,
    });
    write(
        &st,
        op,
        Method::POST,
        "/acme/gateways",
        &headers,
        body,
        |_| {
            (
                StatusCode::CREATED,
                Json(serde_json::json!({ "name": name, "token": token })),
            )
                .into_response()
        },
    )
    .await
}

/// Revokes a gateway: its token stops working at once on every replica. Each renewal creates
/// a new key, so a key the gateway held is replaced at the next renewal.
async fn delete_gateway(
    State(st): State<AcmeState>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Response {
    let op = AcmeOp::GatewayDelete { name: name.clone() };
    write(
        &st,
        op,
        Method::DELETE,
        &format!("/acme/gateways/{name}"),
        &headers,
        Bytes::new(),
        |_| StatusCode::NO_CONTENT.into_response(),
    )
    .await
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
            "the body must be {\"names\": [...], \"dns_plugin\": \"...\", \"gateways\": [...]} (dns-01) or {\"names\": [...], \"challenge\": \"http-01\"}",
        );
    };
    let site = super::Site {
        name: name.clone(),
        names: parsed.names,
        gateways: parsed.gateways,
        challenge: parsed.challenge,
        dns_plugin: parsed.dns_plugin,
        challenge_zone: parsed.challenge_zone,
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
    if site.challenge == super::ChallengeKind::Dns01 {
        if let Err(why) = st.check_dns_plugin(&site.dns_plugin) {
            return err(StatusCode::BAD_REQUEST, why);
        }
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

/// Re-encrypts the account key and every certificate key under the active secret key (step 2
/// of a rotation, next to `/admin/plugins/secrets/rewrap`). Runs on the leader; refuses
/// while a member lacks the active key.
async fn rewrap(State(st): State<AcmeState>, headers: HeaderMap) -> Response {
    const PATH: &str = "/admin/acme/rewrap";
    let forward = ForwardHeaders::from_headers(&headers);
    if let Some(ha) = &st.ha {
        if !ha.is_leader() {
            if forward.forwarded {
                return err(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "the leader changed while the request was forwarded; retry shortly",
                );
            }
            return forward_to_current_leader(ha, Method::POST, PATH, Bytes::new(), &forward).await;
        }
    }
    let ring = st.keyring.current();
    let Some(active) = ring.active_id().map(str::to_string) else {
        return err(
            StatusCode::CONFLICT,
            "no secret key configured on this controller",
        );
    };
    if let Some(ha) = &st.ha {
        if let Err(why) = crate::plugins::peer::check_active_key(ha, &active).await {
            return err(
                StatusCode::CONFLICT,
                format!("every node needs the active key {active} first: {why}"),
            );
        }
        if let Err(why) = super::check_support(ha).await {
            return err(StatusCode::CONFLICT, why);
        }
    }
    let (mut account, mut certs, mut unreadable) = (None, Vec::new(), Vec::new());
    let reseal = |slot: &str, sealed: &Sealed| -> Option<Sealed> {
        let value = ring.open(SEAL_SCOPE, slot, sealed).ok()?;
        ring.seal(SEAL_SCOPE, slot, value.expose()).ok()
    };
    match st.store.account() {
        Ok(Some(a)) if a.sealed.key_id != active => match reseal(ACCOUNT_SLOT, &a.sealed) {
            Some(sealed) => {
                account = Some(RewrapAccount {
                    from_nonce: a.sealed.nonce,
                    sealed,
                });
            }
            None => unreadable.push("account".to_string()),
        },
        Ok(_) => {}
        Err(e) => return internal(e),
    }
    let sites = match st.store.sites() {
        Ok(s) => s,
        Err(e) => return internal(e),
    };
    for site in sites {
        let stored = match st.store.certs(&site.name) {
            Ok(c) => c,
            Err(e) => return internal(e),
        };
        for c in stored.into_iter().filter(|c| c.sealed_key.key_id != active) {
            match reseal(&cert_slot(&site.name, c.version), &c.sealed_key) {
                Some(sealed) => certs.push(RewrapCert {
                    site: site.name.clone(),
                    version: c.version,
                    from_nonce: c.sealed_key.nonce,
                    sealed,
                }),
                None => unreadable.push(format!("{}@{}", site.name, c.version)),
            }
        }
    }
    let total = usize::from(account.is_some()) + certs.len();
    let done = |skipped: Vec<String>| {
        Json(serde_json::json!({
            "rewrapped": total - skipped.len(),
            "skipped": skipped,
            "unreadable": unreadable,
            "active_key_id": active,
        }))
        .into_response()
    };
    if total == 0 {
        return done(Vec::new());
    }
    let op = AcmeOp::Rewrap { account, certs };
    let Some(ha) = &st.ha else {
        return match st.store.apply_local(&op) {
            Ok(Applied::Rewrapped(skipped)) => done(skipped),
            Ok(other) => err(
                StatusCode::CONFLICT,
                format!("the write was refused: {other:?}"),
            ),
            Err(e) => internal(e),
        };
    };
    let response = propose_write_as(
        ha,
        WriteRequest::Acme(op),
        Method::POST,
        PATH,
        String::new(),
        &forward,
        |resp| match resp {
            WriteResponse::AcmeResult(Applied::Rewrapped(skipped)) => done(skipped),
            WriteResponse::AcmeResult(other) => err(
                StatusCode::CONFLICT,
                format!("the write was refused: {other:?}"),
            ),
            other => internal(format!("unexpected answer {other:?}")),
        },
    )
    .await;
    if response.status().is_success() {
        crate::plugins::peer::request_purge(ha);
    }
    response
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
        let db = crate::kv::Db::open(dir.path()).unwrap();
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

    #[tokio::test]
    async fn gateways_are_created_once_listed_without_secrets_and_revoked() {
        let (app, store, _d) = app(None);
        call(&app, "PUT", "/acme/sites/web", SITE, None).await;
        let (st, made) = call(&app, "POST", "/acme/gateways", r#"{"name":"gw1"}"#, None).await;
        assert_eq!(st, StatusCode::CREATED);
        let token = made["token"].as_str().unwrap().to_string();
        assert!(token.starts_with("gwt_"));
        // Only the hash is stored.
        let stored = store.gateway("gw1").unwrap().unwrap();
        assert_eq!(stored.token_hash, crate::acme::bundles::hash_token(&token));
        assert!(!serde_json::to_string(&stored).unwrap().contains(&token));
        let (_, list) = call(&app, "GET", "/acme/gateways", "", None).await;
        assert_eq!(list["gateways"][0]["name"], "gw1");
        assert_eq!(list["gateways"][0]["sites"][0], "web");
        assert!(!list.to_string().contains(&token));
        assert!(!list.to_string().contains(&stored.token_hash));
        // A second POST does not rotate silently.
        assert_eq!(
            call(&app, "POST", "/acme/gateways", r#"{"name":"gw1"}"#, None)
                .await
                .0,
            StatusCode::CONFLICT
        );
        assert_eq!(
            call(&app, "DELETE", "/acme/gateways/gw1", "", None).await.0,
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            call(&app, "DELETE", "/acme/gateways/gw1", "", None).await.0,
            StatusCode::NOT_FOUND
        );
        for bad in [r#"{"name":"Bad_Name"}"#, r#"{}"#, r#"{"name":"a","x":1}"#] {
            assert_eq!(
                call(&app, "POST", "/acme/gateways", bad, None).await.0,
                StatusCode::BAD_REQUEST,
                "{bad}"
            );
        }
    }

    #[tokio::test]
    async fn gateway_routes_need_the_admin_token_and_the_gateway_token_is_not_one() {
        let (app, _s, _d) = app(Some("admin"));
        assert_eq!(
            call(&app, "POST", "/acme/gateways", r#"{"name":"gw1"}"#, None)
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
        let (st, made) = call(
            &app,
            "POST",
            "/acme/gateways",
            r#"{"name":"gw1"}"#,
            Some("admin"),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED);
        let gw = made["token"].as_str().unwrap();
        assert_eq!(
            call(&app, "GET", "/acme/gateways", "", Some(gw)).await.0,
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn a_site_may_name_a_challenge_zone_that_must_be_a_hostname() {
        let (app, _s, _d) = app(None);
        let with = |zone: &str| {
            format!(
                r#"{{"names":["panel.example.com"],"dns_plugin":"0123456789abcdef","challenge_zone":"{zone}"}}"#
            )
        };
        for bad in ["*.acme.other.net", "not a zone", "x", "-bad.example", ""] {
            let (st, e) = call(&app, "PUT", "/acme/sites/web", &with(bad), None).await;
            assert_eq!(st, StatusCode::BAD_REQUEST, "{bad:?}: {e}");
        }
        let (st, _) = call(
            &app,
            "PUT",
            "/acme/sites/web",
            &with("acme.other.net"),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        let (_, one) = call(&app, "GET", "/acme/sites/web", "", None).await;
        assert_eq!(one["challenge_zone"], "acme.other.net");
        let (_, plain) = call(&app, "PUT", "/acme/sites/web", SITE, None).await;
        assert_eq!(plain["stored"], true);
        let (_, one) = call(&app, "GET", "/acme/sites/web", "", None).await;
        assert!(one["challenge_zone"].is_null());
    }

    #[tokio::test]
    async fn a_site_is_limited_in_names_and_in_delegated_challenge_name_length() {
        let (app, _s, _d) = app(None);
        let names = |n: usize| {
            (0..n)
                .map(|i| format!("\"h{i}.example.com\""))
                .collect::<Vec<_>>()
                .join(",")
        };
        let body = |n: usize| {
            format!(
                r#"{{"names":[{}],"dns_plugin":"0123456789abcdef"}}"#,
                names(n)
            )
        };
        let (st, _) = call(&app, "PUT", "/acme/sites/web", &body(100), None).await;
        assert_eq!(st, StatusCode::OK);
        let (st, e) = call(&app, "PUT", "/acme/sites/web", &body(101), None).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{e}");
        assert!(e["error"].as_str().unwrap().contains("too many"), "{e}");

        // `_acme-challenge.` + 207-character host + `.` + 54-character zone is over 253.
        let dns = |labels: usize| format!("{}.com", vec!["a".repeat(50); labels].join("."));
        let (host, zone) = (dns(4), dns(1));
        let b = format!(
            r#"{{"names":["{host}"],"dns_plugin":"0123456789abcdef","challenge_zone":"{zone}"}}"#
        );
        let (st, e) = call(&app, "PUT", "/acme/sites/long", &b, None).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{e}");
        assert!(e["error"].as_str().unwrap().contains("253"), "{e}");
    }

    const SITE: &str =
        r#"{"names":["panel.example.com"],"gateways":["gw1"],"dns_plugin":"0123456789abcdef"}"#;

    #[tokio::test]
    async fn an_http_01_site_needs_no_dns_plugin_and_cannot_cover_a_wildcard() {
        let (app, _s, _d) = app(None);
        let (st, _) = call(
            &app,
            "PUT",
            "/acme/sites/web",
            r#"{"names":["a.example.com","b.example.com"],"challenge":"http-01"}"#,
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        let (_, one) = call(&app, "GET", "/acme/sites/web", "", None).await;
        assert_eq!(one["challenge"], "http-01");
        let (_, dns) = call(&app, "PUT", "/acme/sites/dns", SITE, None).await;
        assert_eq!(dns["stored"], true);
        let (_, one) = call(&app, "GET", "/acme/sites/dns", "", None).await;
        assert_eq!(one["challenge"], "dns-01", "the default");
        for (why, body, expect) in [
            (
                "wildcard",
                r#"{"names":["*.example.com"],"challenge":"http-01","gateways":["gw1"]}"#,
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
            (
                "plugin",
                r#"{"names":["x.example.com"],"challenge":"http-01","dns_plugin":"0123456789abcdef"}"#,
                StatusCode::BAD_REQUEST,
            ),
            (
                "zone",
                r#"{"names":["x.example.com"],"challenge":"http-01","challenge_zone":"acme.other.net"}"#,
                StatusCode::BAD_REQUEST,
            ),
            (
                "unknown challenge",
                r#"{"names":["x.example.com"],"challenge":"tls-alpn-01"}"#,
                StatusCode::BAD_REQUEST,
            ),
            (
                "dns-01 without a plugin",
                r#"{"names":["x.example.com"]}"#,
                StatusCode::BAD_REQUEST,
            ),
        ] {
            let (st, e) = call(&app, "PUT", "/acme/sites/bad", body, None).await;
            assert_eq!(st, expect, "{why}: {e}");
        }
    }

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
        let db = crate::kv::Db::open(dir.path()).unwrap();
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

    const KEY_A: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
    const KEY_B: &str = "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=";

    #[tokio::test]
    async fn rotation_rewraps_the_account_and_certificate_keys_under_the_new_key() {
        use crate::plugins::secrets::{Keyring, KeyringHandle};
        let dir = tempfile::tempdir().unwrap();
        let db = crate::kv::Db::open(dir.path()).unwrap();
        let store = AcmeStore::open(&db).unwrap();
        let ring_a = Keyring::parse(KEY_A).unwrap();
        let old_id = ring_a.active_id().unwrap().to_string();
        let keyring = KeyringHandle::new(Keyring::parse(KEY_A).unwrap());
        let state = |keyring: &KeyringHandle| AcmeState {
            store: store.clone(),
            auth_token: None,
            ha: None,
            keyring: keyring.clone(),
            directory_url: "https://ca.example/dir".into(),
            plugins: None,
        };
        let app = router(state(&keyring));
        call(
            &app,
            "PUT",
            "/acme/sites/web",
            r#"{"names":["a.example.com"],"challenge":"http-01"}"#,
            None,
        )
        .await;
        let ring = keyring.current();
        let sealed_key = ring
            .seal(SEAL_SCOPE, &cert_slot("web", 1), b"cert-key")
            .unwrap();
        let sealed_acct = ring
            .seal(SEAL_SCOPE, crate::acme::types::ACCOUNT_SLOT, b"acct-key")
            .unwrap();
        store
            .apply_local(&AcmeOp::AccountSet(StoredAccount {
                directory_url: "https://ca.example/dir".into(),
                account_url: "https://ca.example/acct/1".into(),
                sealed: sealed_acct,
            }))
            .unwrap();
        store
            .apply_local(&AcmeOp::CertIssued {
                site: "web".into(),
                tagged_term: 0,
                cert: StoredCert {
                    version: 1,
                    chain_pem: "-----BEGIN CERTIFICATE-----".into(),
                    sealed_key,
                    not_after: 2_000_000_000,
                    issued_at: 1,
                    serial: "0a".into(),
                    names: vec!["a.example.com".into()],
                    untrusted: false,
                },
            })
            .unwrap();
        // Step 1: add the new key (it becomes active), keep the old one.
        let both = KeyringHandle::new(Keyring::parse(&format!("{KEY_A}\n{KEY_B}")).unwrap());
        let app = router(state(&both));
        // Step 2: rewrap.
        let (status, r) = call(&app, "POST", "/admin/acme/rewrap", "", None).await;
        assert_eq!(status, StatusCode::OK, "{r}");
        assert_eq!(r["rewrapped"], 2, "{r}");
        let new_id = both.current().active_id().unwrap().to_string();
        assert_ne!(old_id, new_id);
        assert_eq!(store.account().unwrap().unwrap().sealed.key_id, new_id);
        assert_eq!(
            store.cert("web").unwrap().unwrap().sealed_key.key_id,
            new_id
        );
        // Step 3: the old key is retired; both secrets still open.
        let only_new = Keyring::parse(KEY_B).unwrap();
        let acct = store.account().unwrap().unwrap();
        let cert = store.cert("web").unwrap().unwrap();
        assert_eq!(
            only_new
                .open(SEAL_SCOPE, crate::acme::types::ACCOUNT_SLOT, &acct.sealed)
                .unwrap()
                .expose(),
            b"acct-key"
        );
        assert_eq!(
            only_new
                .open(SEAL_SCOPE, &cert_slot("web", 1), &cert.sealed_key)
                .unwrap()
                .expose(),
            b"cert-key"
        );
        // Rewrapping again is a no-op.
        let (_, r) = call(&app, "POST", "/admin/acme/rewrap", "", None).await;
        assert_eq!(r["rewrapped"], 0, "{r}");
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
        assert_eq!(
            call(&app, "POST", "/admin/acme/rewrap", "", None).await.0,
            StatusCode::NOT_IMPLEMENTED
        );
    }
}
