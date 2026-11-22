//! Proxies `wayhouse-controller`'s config API — `GET`/`POST /config`, revision
//! history/diff, rollback, promote — and its read-only tunnel address table
//! (`GET /tunnel/addresses`, `docs/11` "Address authority", plus the registry
//! `DELETE`s that release an address) to the browser, via
//! `--controller-url`/`--controller-token`. Same shape as
//! `crate::aggregator_proxy`: thin, stateless, the browser's session cookie
//! never becomes a bearer token, `wayhouse-ui` holds the controller's own
//! credential and presents it server-side. [`viewer_router`]'s reads are
//! `Role::Viewer`; [`admin_router`]'s writes (submit, rollback, promote,
//! release —
//! phase 10's "full management" GUI level, `docs/10`) are `Role::Admin`.
//!
//! `GET /api/config/pending` (viewer) shows the revision on trial and its countdown;
//! `POST /api/config/confirm/{revision}` and `POST /api/config/revert` (admin) end the trial.
//!
//! `GET /api/config/schema` (viewer) and `POST /api/config/validate` (admin) front the controller's
//! editor endpoints; `POST /api/config?base=<rev>` passes the base-revision guard (and its `409`)
//! through untouched.
//!
//! The controller's `POST /config` body is raw YAML text, not JSON (it
//! accepts a `String` extractor, content-type-agnostic) — forwarded
//! byte-for-byte with no content-type forced on it, unlike
//! `aggregator_proxy`'s JSON bodies. [`admin_router`]'s writes each add an
//! `X-Actor` header (phase 12 slice 8) naming the session's username, if
//! any — `wayhouse-controller` records it per revision (`GET /config/revisions`'
//! `actor` field), the durable half of this release's audit trail.

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Extension, Path, RawQuery, State};
use axum::http::{Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use serde::Serialize;

use crate::api::AppState;
use crate::auth::Actor;

/// `GET` config reads — `Role::Viewer`.
pub fn viewer_router() -> Router<AppState> {
    Router::new()
        .route("/api/config", get(get_config))
        .route("/api/config/schema", get(config_schema))
        .route("/api/config/revisions", get(list_revisions))
        .route("/api/config/revisions/{revision}", get(get_revision))
        .route("/api/config/revisions/{revision}/diff", get(diff_revision))
        .route("/api/config/pending", get(pending_config))
        .route("/api/tunnel/addresses", get(tunnel_addresses))
        .route("/api/plugins", get(list_plugins))
        .route("/api/plugin-routes", get(plugin_routes))
        .route("/api/plugins/{id}", get(get_plugin))
        .route("/api/plugins/{id}/status", get(plugin_status))
        .route("/api/acme/sites", get(acme_sites))
        .route("/api/acme/gateways", get(acme_gateways))
        .route("/api/acme/account", get(acme_account))
        .route("/api/features", get(features))
        .route("/api/features/overrides", get(feature_overrides))
}

/// Config-changing writes — `Role::Admin`.
pub fn admin_router() -> Router<AppState> {
    Router::new()
        .route("/api/config", post(submit_config))
        .route("/api/config/validate", post(validate_config))
        .route("/api/config/rollback/{revision}", post(rollback))
        .route("/api/config/promote/{revision}", post(promote))
        .route("/api/config/confirm/{revision}", post(confirm_revision))
        .route("/api/config/revert", post(revert_config))
        .route(
            "/api/acme/sites/{name}",
            put(put_acme_site)
                .delete(delete_acme_site)
                .layer(DefaultBodyLimit::max(ACME_SITE_BODY_BYTES)),
        )
        .route(
            "/api/acme/gateways",
            post(create_acme_gateway).layer(DefaultBodyLimit::max(ACME_GATEWAY_BODY_BYTES)),
        )
        .route(
            "/api/acme/gateways/{name}",
            axum::routing::delete(revoke_acme_gateway),
        )
        // Enrollment codes (docs/12 "Enrolling an origin agent"): admin-only, reads included,
        // because a pending list names every node being added.
        .route(
            "/api/enrollments",
            post(mint_enrollment)
                .get(list_enrollments)
                .layer(DefaultBodyLimit::max(ENROLLMENT_BODY_BYTES)),
        )
        .route("/api/enrollments/{id}", delete(revoke_enrollment))
        .route("/api/tunnel/origins/{name}", delete(release_origin))
        .route("/api/tunnel/proxies/{name}", delete(release_proxy))
        .route(
            "/api/tunnel/origins/{name}/psk/rotate",
            post(rotate_origin_psk),
        )
        .route(
            "/api/plugins/modules",
            post(upload_plugin_module).layer(DefaultBodyLimit::max(
                wayhouse_http::MAX_SNIFFER_MODULE_BYTES,
            )),
        )
        .route("/api/plugins", post(install_plugin))
        .route("/api/plugins/{id}", delete(delete_plugin))
        .route("/api/plugins/{id}/enable", post(enable_plugin))
        .route("/api/plugins/{id}/disable", post(disable_plugin))
        .route(
            "/api/plugins/{id}/config",
            put(put_plugin_config).layer(DefaultBodyLimit::max(PLUGIN_CONFIG_BODY_BYTES)),
        )
        // The plugins-file dry run: it prints the approval hashes to paste into the file.
        .route("/api/plugins/file/plan", get(plugins_file_plan))
        .route("/api/plugins/secrets/keys", get(plugin_secret_keys))
        .route("/api/plugins/secrets/rewrap", post(rewrap_plugin_secrets))
        .route("/api/acme/rewrap", post(rewrap_acme_keys))
        .route("/api/secrets/rewrap", post(rewrap_cluster_secrets))
        .route("/api/plugins/{id}/secrets", get(list_plugin_secrets))
        .route(
            "/api/plugins/{id}/secrets/{slot}",
            put(put_plugin_secret)
                .delete(delete_plugin_secret)
                .layer(DefaultBodyLimit::max(PLUGIN_SECRET_BODY_BYTES)),
        )
        .route(
            "/api/plugins/{id}/webhook",
            post(enable_plugin_webhook).delete(revoke_plugin_webhook),
        )
        // Names, key ids and times only. The sealed form (`/secrets/{name}/sealed`) is for
        // proxies and is not reachable through the UI.
        .route("/api/secrets", get(list_secrets))
        .route(
            "/api/secrets/{name}",
            put(put_secret)
                .delete(delete_secret)
                .layer(DefaultBodyLimit::max(SECRET_BODY_BYTES)),
        )
        .route(
            "/api/features/overrides/{name}",
            put(put_feature_override)
                .delete(delete_feature_override)
                .layer(DefaultBodyLimit::max(FEATURE_OVERRIDE_BODY_BYTES)),
        )
}

/// A reason, a boolean and an expiry; the controller reads little more.
const FEATURE_OVERRIDE_BODY_BYTES: usize = 8 * 1024;

/// The controller refuses a bigger cluster secret body (8 KiB of value plus the JSON wrapper).
const SECRET_BODY_BYTES: usize = 16 * 1024;

/// A site names at most 100 names of at most 253 bytes, plus a few short fields.
const ACME_SITE_BODY_BYTES: usize = 64 * 1024;
/// A gateway registration is one name.
const ACME_GATEWAY_BODY_BYTES: usize = 1024;
/// A mint body is a role, a name, a few numbers, a URL and the step-up secret.
const ENROLLMENT_BODY_BYTES: usize = 4 * 1024;

/// The controller's own cap on an install's config is 64 KiB; the rest is the approved set.
const PLUGIN_CONFIG_BODY_BYTES: usize = 80 * 1024;
/// The controller refuses a bigger secret body (16 KiB) too.
const PLUGIN_SECRET_BODY_BYTES: usize = 16 * 1024;

/// The ACME status reads: sites with their certificate and last error, the gateways that may fetch
/// them, and the CA account. The writes below are admin-only.
async fn acme_sites(State(state): State<AppState>) -> Response {
    proxy(&state, Method::GET, "/acme/sites".to_string(), None, None).await
}

/// `PUT /api/acme/sites/{name}`: create or replace a site (docs/acme.md); the controller checks the body.
async fn put_acme_site(
    State(state): State<AppState>,
    Extension(Actor(actor)): Extension<Actor>,
    Path(name): Path<String>,
    body: Bytes,
) -> Response {
    let Some(segment) = crate::proxy_util::path_segment(&name) else {
        return bad_acme_name();
    };
    let who = actor.clone();
    let resp = proxy_typed(
        &state,
        Method::PUT,
        format!("/acme/sites/{segment}"),
        Some(body),
        Some("application/json"),
        actor,
    )
    .await;
    audit_acme("put site", who.as_deref(), Some(&name), &resp);
    resp
}

/// `DELETE /api/acme/sites/{name}`: removes the site, its certificates and its retry state.
async fn delete_acme_site(
    State(state): State<AppState>,
    Extension(Actor(actor)): Extension<Actor>,
    Path(name): Path<String>,
) -> Response {
    let Some(segment) = crate::proxy_util::path_segment(&name) else {
        return bad_acme_name();
    };
    let who = actor.clone();
    let resp = proxy(
        &state,
        Method::DELETE,
        format!("/acme/sites/{segment}"),
        None,
        actor,
    )
    .await;
    audit_acme("delete site", who.as_deref(), Some(&name), &resp);
    resp
}

/// `POST /api/acme/gateways`: registers a gateway; the answer carries its token, once, and is passed
/// through without being logged.
async fn create_acme_gateway(
    State(state): State<AppState>,
    Extension(Actor(actor)): Extension<Actor>,
    body: Bytes,
) -> Response {
    // No name in the line on purpose: reading it would mean parsing the request body, and the answer carries the token.
    let who = actor.clone();
    let resp = proxy_typed(
        &state,
        Method::POST,
        "/acme/gateways".to_string(),
        Some(body),
        Some("application/json"),
        actor,
    )
    .await;
    audit_acme("register gateway", who.as_deref(), None, &resp);
    resp
}

/// `POST /api/enrollments`: mints a one-time enrollment code. The body may carry the step-up
/// **passphrase** as `step_up` (a string): this backend fetches the controller's salt, derives the
/// key, signs the exact request with a short expiry and forwards an assertion instead; the
/// passphrase is dropped here and never logged or stored. The answer carries the code, so it is
/// never cached.
async fn mint_enrollment(
    State(state): State<AppState>,
    Extension(Actor(actor)): Extension<Actor>,
    body: Bytes,
) -> Response {
    tracing::info!(
        actor = ?actor.as_deref().unwrap_or("-"),
        "enrollment: mint code"
    );
    let body = match sign_step_up(&state, body).await {
        Ok(b) => b,
        Err(resp) => return resp,
    };
    let mut resp = proxy_typed(
        &state,
        Method::POST,
        "/enrollments".to_string(),
        Some(body),
        Some("application/json"),
        actor,
    )
    .await;
    resp.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    resp
}

fn mint_error(status: StatusCode, error: impl Into<String>) -> Response {
    (
        status,
        Json(ErrorResponse {
            error: error.into(),
        }),
    )
        .into_response()
}

/// Replaces a `step_up` passphrase in a mint body with a signed assertion for that body. A body
/// without a string `step_up` is forwarded untouched (the controller decides whether it needs one).
#[allow(clippy::result_large_err)] // the Err is the HTTP answer to send as is
async fn sign_step_up(state: &AppState, body: Bytes) -> Result<Bytes, Response> {
    use zeroize::Zeroizing;
    let Ok(mut v) = serde_json::from_slice::<serde_json::Value>(&body) else {
        return Ok(body);
    };
    let Some(pass) = v
        .get("step_up")
        .and_then(|p| p.as_str())
        .map(|p| Zeroizing::new(p.to_owned()))
    else {
        return Ok(body);
    };
    let Some(controller) = &state.controller else {
        return Ok(body);
    };
    let mut req = state
        .http
        .get(format!("{}/enrollments/step-up", controller.base_url));
    if let Some(token) = &controller.token {
        req = req.bearer_auth(token);
    }
    let answer = req.send().await.map_err(|e| {
        tracing::warn!(error = %wayhouse_http::error_chain(&e), "step-up salt: controller unreachable");
        mint_error(StatusCode::BAD_GATEWAY, "the controller could not be reached")
    })?;
    if !answer.status().is_success() {
        return Err(mint_error(
            StatusCode::CONFLICT,
            "this controller has no step-up key (--enroll-step-up-pubkey-file)",
        ));
    }
    let salt = answer
        .json::<serde_json::Value>()
        .await
        .ok()
        .and_then(|j| j["salt"].as_str().map(str::to_owned))
        .ok_or_else(|| {
            mint_error(
                StatusCode::BAD_GATEWAY,
                "the controller sent no step-up salt",
            )
        })
        .and_then(|s| {
            wayhouse_enroll::step_up::decode_salt(&s)
                .map_err(|e| mint_error(StatusCode::BAD_GATEWAY, e))
        })?;
    let text = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_owned);
    let num = |k: &str| v.get(k).and_then(serde_json::Value::as_u64);
    let (role, name) = (
        text("role").unwrap_or_else(|| "agent".into()),
        text("name").or_else(|| text("origin")).unwrap_or_default(),
    );
    let (name_prefix, parent_url) = (text("name_prefix"), text("parent_url"));
    let (ttl_secs, uses) = (num("ttl_secs"), num("uses"));
    let assertion = tokio::task::spawn_blocking(move || {
        // argon2id is deliberately slow: off the async threads.
        let key = wayhouse_enroll::step_up::derive(&pass, &salt)?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let r = wayhouse_enroll::step_up::Request {
            role: &role,
            name: &name,
            ttl_secs,
            uses,
            name_prefix: name_prefix.as_deref(),
            parent_url: parent_url.as_deref(),
        };
        Ok::<_, String>(wayhouse_enroll::step_up::sign(&key, &r, now))
    })
    .await
    .map_err(|_| mint_error(StatusCode::INTERNAL_SERVER_ERROR, "signing failed"))?
    .map_err(|e| mint_error(StatusCode::BAD_REQUEST, e))?;
    v["step_up"] = serde_json::to_value(assertion)
        .map_err(|_| mint_error(StatusCode::INTERNAL_SERVER_ERROR, "signing failed"))?;
    Ok(Bytes::from(v.to_string()))
}

/// `GET /api/enrollments`: codes with their state, never the code itself.
async fn list_enrollments(State(state): State<AppState>) -> Response {
    proxy(&state, Method::GET, "/enrollments".to_string(), None, None).await
}

/// `DELETE /api/enrollments/{id}`: revokes a code, spent or not.
async fn revoke_enrollment(
    State(state): State<AppState>,
    Extension(Actor(actor)): Extension<Actor>,
    Path(id): Path<String>,
) -> Response {
    let Some(segment) = crate::proxy_util::path_segment(&id) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "invalid code id".into(),
            }),
        )
            .into_response();
    };
    tracing::info!(actor = ?actor.as_deref().unwrap_or("-"), enrollment = %id, "enrollment: revoke code");
    proxy(
        &state,
        Method::DELETE,
        format!("/enrollments/{segment}"),
        None,
        actor,
    )
    .await
}

/// `DELETE /api/acme/gateways/{name}`: revokes a gateway; its token answers `401` at once (docs/acme.md).
async fn revoke_acme_gateway(
    State(state): State<AppState>,
    Extension(Actor(actor)): Extension<Actor>,
    Path(name): Path<String>,
) -> Response {
    let Some(segment) = crate::proxy_util::path_segment(&name) else {
        return bad_acme_name();
    };
    let who = actor.clone();
    let resp = proxy(
        &state,
        Method::DELETE,
        format!("/acme/gateways/{segment}"),
        None,
        actor,
    )
    .await;
    audit_acme("revoke gateway", who.as_deref(), Some(&name), &resp);
    resp
}

/// One audit line per ACME write, after the controller answered, so it says what happened and not only what was tried.
/// `subject` is a site or gateway name, never a token.
fn audit_acme(what: &str, actor: Option<&str>, subject: Option<&str>, resp: &Response) {
    tracing::info!(actor = ?actor.unwrap_or("-"), subject = ?subject, status = resp.status().as_u16(), "acme: {what}");
}

fn bad_acme_name() -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(ErrorResponse {
            error: "invalid site name".into(),
        }),
    )
        .into_response()
}

async fn acme_gateways(State(state): State<AppState>) -> Response {
    proxy(
        &state,
        Method::GET,
        "/acme/gateways".to_string(),
        None,
        None,
    )
    .await
}

async fn features(State(state): State<AppState>) -> Response {
    proxy(&state, Method::GET, "/features".to_string(), None, None).await
}

async fn feature_overrides(State(state): State<AppState>) -> Response {
    proxy(
        &state,
        Method::GET,
        "/features/overrides".to_string(),
        None,
        None,
    )
    .await
}

async fn put_feature_override(
    State(state): State<AppState>,
    Extension(Actor(actor)): Extension<Actor>,
    Path(name): Path<String>,
    body: Bytes,
) -> Response {
    let Some(name) = crate::proxy_util::path_segment(&name) else {
        return bad_feature_name();
    };
    proxy_typed(
        &state,
        Method::PUT,
        format!("/features/overrides/{name}"),
        Some(body),
        Some("application/json"),
        actor,
    )
    .await
}

async fn delete_feature_override(
    State(state): State<AppState>,
    Extension(Actor(actor)): Extension<Actor>,
    Path(name): Path<String>,
) -> Response {
    let Some(name) = crate::proxy_util::path_segment(&name) else {
        return bad_feature_name();
    };
    proxy(
        &state,
        Method::DELETE,
        format!("/features/overrides/{name}"),
        None,
        actor,
    )
    .await
}

async fn list_secrets(State(state): State<AppState>) -> Response {
    proxy(&state, Method::GET, "/secrets".to_string(), None, None).await
}

/// The value rides in the body to the controller, which seals it; the BFF never logs or keeps it.
async fn put_secret(
    State(state): State<AppState>,
    Extension(Actor(actor)): Extension<Actor>,
    Path(name): Path<String>,
    body: Bytes,
) -> Response {
    let Some(name) = crate::proxy_util::path_segment(&name) else {
        return bad_secret_name();
    };
    proxy_typed(
        &state,
        Method::PUT,
        format!("/secrets/{name}"),
        Some(body),
        Some("application/json"),
        actor,
    )
    .await
}

async fn delete_secret(
    State(state): State<AppState>,
    Extension(Actor(actor)): Extension<Actor>,
    Path(name): Path<String>,
) -> Response {
    let Some(name) = crate::proxy_util::path_segment(&name) else {
        return bad_secret_name();
    };
    proxy(
        &state,
        Method::DELETE,
        format!("/secrets/{name}"),
        None,
        actor,
    )
    .await
}

fn bad_secret_name() -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(ErrorResponse {
            error: "invalid secret name".into(),
        }),
    )
        .into_response()
}

fn bad_feature_name() -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(ErrorResponse {
            error: "invalid feature name".into(),
        }),
    )
        .into_response()
}

async fn acme_account(State(state): State<AppState>) -> Response {
    proxy(&state, Method::GET, "/acme/account".to_string(), None, None).await
}

/// The route overlay plugins publish: hostname routes and the entry ports of port routes.
async fn plugin_routes(State(state): State<AppState>) -> Response {
    proxy(
        &state,
        Method::GET,
        "/plugin-routes".to_string(),
        None,
        None,
    )
    .await
}

async fn list_plugins(State(state): State<AppState>) -> Response {
    proxy(&state, Method::GET, "/plugins".to_string(), None, None).await
}

/// The module bytes go through untouched; the controller inspects and stores them.
async fn upload_plugin_module(
    State(state): State<AppState>,
    Extension(Actor(actor)): Extension<Actor>,
    body: Bytes,
) -> Response {
    proxy(
        &state,
        Method::POST,
        "/plugins/modules".to_string(),
        Some(body),
        actor,
    )
    .await
}

/// The controller's `Json` extractor insists on the content type, so it is forwarded.
async fn install_plugin(
    State(state): State<AppState>,
    Extension(Actor(actor)): Extension<Actor>,
    body: Bytes,
) -> Response {
    proxy_typed(
        &state,
        Method::POST,
        "/plugins".to_string(),
        Some(body),
        Some("application/json"),
        actor,
    )
    .await
}

async fn enable_plugin(
    State(state): State<AppState>,
    Extension(Actor(actor)): Extension<Actor>,
    Path(id): Path<String>,
) -> Response {
    plugin_action(&state, Method::POST, &id, "/enable", actor).await
}

async fn disable_plugin(
    State(state): State<AppState>,
    Extension(Actor(actor)): Extension<Actor>,
    Path(id): Path<String>,
) -> Response {
    plugin_action(&state, Method::POST, &id, "/disable", actor).await
}

async fn delete_plugin(
    State(state): State<AppState>,
    Extension(Actor(actor)): Extension<Actor>,
    Path(id): Path<String>,
) -> Response {
    plugin_action(&state, Method::DELETE, &id, "", actor).await
}

async fn get_plugin(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    plugin_action(&state, Method::GET, &id, "", None).await
}

async fn plugin_status(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    plugin_action(&state, Method::GET, &id, "/status", None).await
}

async fn plugins_file_plan(State(state): State<AppState>) -> Response {
    proxy(
        &state,
        Method::GET,
        "/plugins/file/plan".to_string(),
        None,
        None,
    )
    .await
}

/// The key ids this controller and, under HA, every member holds (never key material).
async fn plugin_secret_keys(State(state): State<AppState>) -> Response {
    proxy(
        &state,
        Method::GET,
        "/admin/plugins/secrets/keys".to_string(),
        None,
        None,
    )
    .await
}

/// Re-encrypts every plugin secret under the active key (step two of a key rotation).
async fn rewrap_plugin_secrets(
    State(state): State<AppState>,
    Extension(Actor(actor)): Extension<Actor>,
) -> Response {
    proxy(
        &state,
        Method::POST,
        "/admin/plugins/secrets/rewrap".to_string(),
        None,
        actor,
    )
    .await
}

/// Re-encrypts the ACME account key and every certificate key under the active key (step two of
/// a key rotation). A controller without `--acme` answers 404.
async fn rewrap_acme_keys(
    State(state): State<AppState>,
    Extension(Actor(actor)): Extension<Actor>,
) -> Response {
    proxy(
        &state,
        Method::POST,
        "/admin/acme/rewrap".to_string(),
        None,
        actor,
    )
    .await
}

/// Re-encrypts every cluster secret (`{ secret: name }`) under the active key (step two of a key
/// rotation).
async fn rewrap_cluster_secrets(
    State(state): State<AppState>,
    Extension(Actor(actor)): Extension<Actor>,
) -> Response {
    proxy(
        &state,
        Method::POST,
        "/secrets/rewrap".to_string(),
        None,
        actor,
    )
    .await
}

async fn list_plugin_secrets(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    plugin_action(&state, Method::GET, &id, "/secrets", None).await
}

async fn enable_plugin_webhook(
    State(state): State<AppState>,
    Extension(Actor(actor)): Extension<Actor>,
    Path(id): Path<String>,
) -> Response {
    plugin_action(&state, Method::POST, &id, "/webhook", actor).await
}

async fn revoke_plugin_webhook(
    State(state): State<AppState>,
    Extension(Actor(actor)): Extension<Actor>,
    Path(id): Path<String>,
) -> Response {
    plugin_action(&state, Method::DELETE, &id, "/webhook", actor).await
}

async fn put_plugin_config(
    State(state): State<AppState>,
    Extension(Actor(actor)): Extension<Actor>,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    plugin_body(&state, Method::PUT, &id, "/config".into(), body, actor).await
}

/// The value is forwarded and never logged or echoed: the controller answers `204` or an
/// error that does not quote the body.
async fn put_plugin_secret(
    State(state): State<AppState>,
    Extension(Actor(actor)): Extension<Actor>,
    Path((id, slot)): Path<(String, String)>,
    body: Bytes,
) -> Response {
    let Some(slot) = crate::proxy_util::path_segment(&slot) else {
        return bad_plugin_id();
    };
    plugin_body(
        &state,
        Method::PUT,
        &id,
        format!("/secrets/{slot}"),
        body,
        actor,
    )
    .await
}

async fn delete_plugin_secret(
    State(state): State<AppState>,
    Extension(Actor(actor)): Extension<Actor>,
    Path((id, slot)): Path<(String, String)>,
) -> Response {
    let Some(slot) = crate::proxy_util::path_segment(&slot) else {
        return bad_plugin_id();
    };
    plugin_action(
        &state,
        Method::DELETE,
        &id,
        &format!("/secrets/{slot}"),
        actor,
    )
    .await
}

fn bad_plugin_id() -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(ErrorResponse {
            error: "invalid plugin id".into(),
        }),
    )
        .into_response()
}

/// A JSON body to `/plugins/{id}{suffix}`; the controller's `Json` extractors insist on the type.
async fn plugin_body(
    state: &AppState,
    method: Method,
    id: &str,
    suffix: String,
    body: Bytes,
    actor: Option<String>,
) -> Response {
    let Some(segment) = crate::proxy_util::path_segment(id) else {
        return bad_plugin_id();
    };
    proxy_typed(
        state,
        method,
        format!("/plugins/{segment}{suffix}"),
        Some(body),
        Some("application/json"),
        actor,
    )
    .await
}

async fn plugin_action(
    state: &AppState,
    method: Method,
    id: &str,
    suffix: &str,
    actor: Option<String>,
) -> Response {
    let Some(segment) = crate::proxy_util::path_segment(id) else {
        return bad_plugin_id();
    };
    proxy(
        state,
        method,
        format!("/plugins/{segment}{suffix}"),
        None,
        actor,
    )
    .await
}

/// The proxy the config editor talks to when there is no controller (`--proxy-url`).
fn file_proxy(state: &AppState) -> Option<&crate::api::ControllerTarget> {
    state
        .controller
        .as_ref()
        .map_or(state.proxy_admin.as_ref(), |_| None)
}

async fn get_config(State(state): State<AppState>) -> Response {
    if let Some(target) = file_proxy(&state) {
        return send(
            &state,
            target,
            "proxy",
            Method::GET,
            "/config/file".into(),
            None,
            None,
            None,
        )
        .await;
    }
    proxy(&state, Method::GET, "/config".to_string(), None, None).await
}

async fn config_schema(State(state): State<AppState>) -> Response {
    if let Some(target) = file_proxy(&state) {
        return send(
            &state,
            target,
            "proxy",
            Method::GET,
            "/config/schema".into(),
            None,
            None,
            None,
        )
        .await;
    }
    proxy(
        &state,
        Method::GET,
        "/config/schema".to_string(),
        None,
        None,
    )
    .await
}

/// Read-only on the controller, but admin-only here: it is the editor's save-time check.
async fn validate_config(State(state): State<AppState>, body: Bytes) -> Response {
    if let Some(target) = file_proxy(&state) {
        return send(
            &state,
            target,
            "proxy",
            Method::POST,
            "/config/validate".into(),
            Some(body),
            None,
            None,
        )
        .await;
    }
    proxy(
        &state,
        Method::POST,
        "/config/validate".to_string(),
        Some(body),
        None,
    )
    .await
}

async fn tunnel_addresses(State(state): State<AppState>) -> Response {
    proxy(
        &state,
        Method::GET,
        "/tunnel/addresses".to_string(),
        None,
        None,
    )
    .await
}

async fn submit_config(
    State(state): State<AppState>,
    Extension(Actor(actor)): Extension<Actor>,
    RawQuery(query): RawQuery,
    body: Bytes,
) -> Response {
    if let Some(target) = file_proxy(&state) {
        // A file has no canary stage or confirm window: only the revision the edit is based on goes on.
        let base = query
            .as_deref()
            .and_then(|q| q.split('&').find_map(|kv| kv.strip_prefix("base=")))
            .filter(|n| n.chars().all(|c| c.is_ascii_digit()));
        let suffix = base.map_or_else(
            || "/config/file".to_string(),
            |n| format!("/config/file?base={n}"),
        );
        return send(
            &state,
            target,
            "proxy",
            Method::PUT,
            suffix,
            Some(body),
            None,
            actor,
        )
        .await;
    }
    let suffix = match query {
        Some(q) => format!("/config?{q}"),
        None => "/config".to_string(),
    };
    proxy(&state, Method::POST, suffix, Some(body), actor).await
}

async fn list_revisions(State(state): State<AppState>) -> Response {
    proxy(
        &state,
        Method::GET,
        "/config/revisions".to_string(),
        None,
        None,
    )
    .await
}

async fn get_revision(State(state): State<AppState>, Path(revision): Path<u64>) -> Response {
    proxy(
        &state,
        Method::GET,
        format!("/config/revisions/{revision}"),
        None,
        None,
    )
    .await
}

async fn diff_revision(
    State(state): State<AppState>,
    Path(revision): Path<u64>,
    RawQuery(query): RawQuery,
) -> Response {
    let suffix = match query {
        Some(q) => format!("/config/revisions/{revision}/diff?{q}"),
        None => format!("/config/revisions/{revision}/diff"),
    };
    proxy(&state, Method::GET, suffix, None, None).await
}

async fn rollback(
    State(state): State<AppState>,
    Extension(Actor(actor)): Extension<Actor>,
    Path(revision): Path<u64>,
) -> Response {
    proxy(
        &state,
        Method::POST,
        format!("/config/rollback/{revision}"),
        None,
        actor,
    )
    .await
}

/// The revision on trial with its remaining time, and what counts as risky.
async fn pending_config(State(state): State<AppState>) -> Response {
    proxy(
        &state,
        Method::GET,
        "/config/pending".to_string(),
        None,
        None,
    )
    .await
}

async fn confirm_revision(
    State(state): State<AppState>,
    Extension(Actor(actor)): Extension<Actor>,
    Path(revision): Path<u64>,
) -> Response {
    proxy(
        &state,
        Method::POST,
        format!("/config/confirm/{revision}"),
        None,
        actor,
    )
    .await
}

/// `?revision=N` names the trial the operator saw; only that number is forwarded.
#[derive(serde::Deserialize)]
struct RevertParams {
    revision: Option<u64>,
}

async fn revert_config(
    State(state): State<AppState>,
    Extension(Actor(actor)): Extension<Actor>,
    axum::extract::Query(params): axum::extract::Query<RevertParams>,
) -> Response {
    let suffix = match params.revision {
        Some(revision) => format!("/config/revert?revision={revision}"),
        None => "/config/revert".to_string(),
    };
    proxy(&state, Method::POST, suffix, None, actor).await
}

async fn promote(
    State(state): State<AppState>,
    Extension(Actor(actor)): Extension<Actor>,
    Path(revision): Path<u64>,
) -> Response {
    proxy(
        &state,
        Method::POST,
        format!("/config/promote/{revision}"),
        None,
        actor,
    )
    .await
}

/// `DELETE /peers/{name}` — releases an origin's tunnel address.
async fn release_origin(
    State(state): State<AppState>,
    Extension(Actor(actor)): Extension<Actor>,
    Path(name): Path<String>,
) -> Response {
    release(&state, "peers", &name, actor).await
}

/// `POST /peers/{name}/psk/rotate` — starts a rotation of an origin's tunnel key.
async fn rotate_origin_psk(
    State(state): State<AppState>,
    Extension(Actor(actor)): Extension<Actor>,
    Path(name): Path<String>,
) -> Response {
    let Some(segment) = crate::proxy_util::path_segment(&name) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "invalid registration name".into(),
            }),
        )
            .into_response();
    };
    proxy(
        &state,
        Method::POST,
        format!("/peers/{segment}/psk/rotate"),
        None,
        actor,
    )
    .await
}

/// `DELETE /proxy-peers/{name}` — releases a proxy's tunnel address.
async fn release_proxy(
    State(state): State<AppState>,
    Extension(Actor(actor)): Extension<Actor>,
    Path(name): Path<String>,
) -> Response {
    release(&state, "proxy-peers", &name, actor).await
}

async fn release(state: &AppState, base: &str, name: &str, actor: Option<String>) -> Response {
    let Some(segment) = crate::proxy_util::path_segment(name) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "invalid registration name".into(),
            }),
        )
            .into_response();
    };
    proxy(
        state,
        Method::DELETE,
        format!("/{base}/{segment}"),
        None,
        actor,
    )
    .await
}

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
}

/// Forwards one call to the configured controller, passing its response
/// (status + body) straight through. `503` if no `--controller-url` was
/// given at all; `502` if it was but couldn't be reached — both handled
/// cleanly, matching `aggregator_proxy::proxy`'s shape exactly.
async fn proxy(
    state: &AppState,
    method: Method,
    path_suffix: String,
    body: Option<Bytes>,
    actor: Option<String>,
) -> Response {
    proxy_typed(state, method, path_suffix, body, None, actor).await
}

/// [`proxy`] with a content type for the forwarded body.
pub(crate) async fn proxy_typed(
    state: &AppState,
    method: Method,
    path_suffix: String,
    body: Option<Bytes>,
    content_type: Option<&str>,
    actor: Option<String>,
) -> Response {
    let Some(controller) = &state.controller else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ErrorResponse {
                error: "no controller configured (--controller-url)".into(),
            }),
        )
            .into_response();
    };
    send(
        state,
        controller,
        "controller",
        method,
        path_suffix,
        body,
        content_type,
        actor,
    )
    .await
}

/// One call to `target` (the controller, or a proxy's admin API for the config editor), relayed to
/// the browser; the URL and the transport error stay in the log.
#[allow(clippy::too_many_arguments)] // one call's parts; a struct would only rename them
async fn send(
    state: &AppState,
    target: &crate::api::ControllerTarget,
    who: &str,
    method: Method,
    path_suffix: String,
    body: Option<Bytes>,
    content_type: Option<&str>,
    actor: Option<String>,
) -> Response {
    let url = format!("{}{}", target.base_url, path_suffix);
    let mut req = state.http.request(method, &url);
    if let Some(token) = &target.token {
        req = req.bearer_auth(token);
    }
    if let Some(actor) = actor {
        req = req.header("X-Actor", actor);
    }
    if let Some(ct) = content_type {
        req = req.header(axum::http::header::CONTENT_TYPE, ct);
    }
    if let Some(body) = body {
        req = req.body(body); // raw YAML text by default — no content-type forced
    }

    match req.send().await {
        Ok(resp) => crate::proxy_util::relay(resp, who).await,
        Err(e) => {
            tracing::warn!(%url, error = %wayhouse_http::error_chain(&e), "{who} unreachable");
            (
                StatusCode::BAD_GATEWAY,
                Json(ErrorResponse {
                    error: format!("{who} unreachable"),
                }),
            )
                .into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    fn app_with_controller(url: String, token: Option<String>) -> Router {
        let state = crate::api::AppState::new(None).with_controller(url, token);
        crate::api::router(state)
    }

    #[tokio::test]
    async fn get_config_proxies_to_the_controller() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let mock = Router::new().route(
            "/config",
            get(|| async { ([("x-config-revision", "3")], "pools: []") }),
        );
        tokio::spawn(async move {
            axum::serve(listener, mock).await.unwrap();
        });

        let app = app_with_controller(format!("http://{addr}"), None);
        let resp = app
            .oneshot(Request::get("/api/config").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.headers().get("x-config-revision").unwrap(), "3");
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(bytes, "pools: []".as_bytes());
    }

    #[tokio::test]
    async fn submit_config_forwards_the_raw_body_with_the_token() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let captured = std::sync::Arc::new(std::sync::Mutex::new(None));
        let captured_for_handler = captured.clone();
        let mock = Router::new().route(
            "/config",
            post(move |headers: axum::http::HeaderMap, body: Bytes| {
                let captured = captured_for_handler.clone();
                async move {
                    let auth = headers
                        .get(axum::http::header::AUTHORIZATION)
                        .and_then(|v| v.to_str().ok())
                        .map(str::to_string);
                    *captured.lock().unwrap() = Some((body.to_vec(), auth));
                    (StatusCode::OK, r#"{"revision":1}"#)
                }
            }),
        );
        tokio::spawn(async move {
            axum::serve(listener, mock).await.unwrap();
        });

        let app = app_with_controller(format!("http://{addr}"), Some("ctl-token".into()));
        let resp = app
            .oneshot(
                Request::post("/api/config")
                    .body(Body::from("pools:\n  - name: p\n    targets: []\n"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let (body, auth) = captured.lock().unwrap().clone().unwrap();
        assert_eq!(body, b"pools:\n  - name: p\n    targets: []\n");
        assert_eq!(auth.as_deref(), Some("Bearer ctl-token"));
    }

    /// Serves `router` on a loopback port and returns an app proxying to it.
    async fn app_for(router: Router) -> Router {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        app_with_controller(format!("http://{addr}"), None)
    }

    #[tokio::test]
    async fn validate_forwards_the_raw_body() {
        let app = app_for(Router::new().route(
            "/config/validate",
            post(|body: String| async move {
                (StatusCode::UNPROCESSABLE_ENTITY, format!("got {body}"))
            }),
        ))
        .await;
        let resp = app
            .oneshot(
                Request::post("/api/config/validate")
                    .body(Body::from("pools: 7"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(bytes, "got pools: 7".as_bytes());
    }

    #[tokio::test]
    async fn schema_is_proxied_with_its_version_header() {
        let app = app_for(Router::new().route(
            "/config/schema",
            get(|| async { ([("x-config-schema-version", "3")], "{}") }),
        ))
        .await;
        let resp = app
            .oneshot(
                Request::get("/api/config/schema")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.headers().get("x-config-schema-version").unwrap(), "3");
    }

    #[tokio::test]
    async fn submit_forwards_base_untouched_and_passes_a_409_through() {
        let app = app_for(Router::new().route(
            "/config",
            post(|RawQuery(q): RawQuery| async move {
                assert_eq!(q.as_deref(), Some("base=4&stage=canary&group=eu"));
                (StatusCode::CONFLICT, r#"{"error":"moved","current":5}"#)
            }),
        ))
        .await;
        let resp = app
            .oneshot(
                Request::post("/api/config?base=4&stage=canary&group=eu")
                    .body(Body::from("pools: []"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(bytes, r#"{"error":"moved","current":5}"#.as_bytes());
    }

    #[tokio::test]
    async fn diff_forwards_the_query_string() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let captured = std::sync::Arc::new(std::sync::Mutex::new(None));
        let captured_for_handler = captured.clone();
        let mock = Router::new().route(
            "/config/revisions/{revision}/diff",
            get(
                move |Path(revision): Path<u64>, RawQuery(query): RawQuery| {
                    let captured = captured_for_handler.clone();
                    async move {
                        *captured.lock().unwrap() = Some((revision, query));
                        "diff text"
                    }
                },
            ),
        );
        tokio::spawn(async move {
            axum::serve(listener, mock).await.unwrap();
        });

        let app = app_with_controller(format!("http://{addr}"), None);
        let resp = app
            .oneshot(
                Request::get("/api/config/revisions/1/diff?against=2")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            captured.lock().unwrap().clone().unwrap(),
            (1, Some("against=2".to_string()))
        );
    }

    #[tokio::test]
    async fn revert_names_the_revision_the_operator_saw() {
        let app = app_for(Router::new().route(
            "/config/revert",
            post(|RawQuery(q): RawQuery| async move {
                assert_eq!(q.as_deref(), Some("revision=6"));
                r#"{"revision":7}"#
            }),
        ))
        .await;
        let resp = app
            .oneshot(
                Request::post("/api/config/revert?revision=6&evil=1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn rollback_hits_the_right_path() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let mock = Router::new().route(
            "/config/rollback/{revision}",
            post(|Path(revision): Path<u64>| async move {
                assert_eq!(revision, 5);
                r#"{"revision":6}"#
            }),
        );
        tokio::spawn(async move {
            axum::serve(listener, mock).await.unwrap();
        });

        let app = app_with_controller(format!("http://{addr}"), None);
        let resp = app
            .oneshot(
                Request::post("/api/config/rollback/5")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn tunnel_addresses_proxies_to_the_controller_with_the_token() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let mock = Router::new().route(
            "/tunnel/addresses",
            get(|headers: axum::http::HeaderMap| async move {
                assert_eq!(
                    headers.get(axum::http::header::AUTHORIZATION).unwrap(),
                    "Bearer ctl-token"
                );
                r#"{"network":"10.200.0.0/24","allocated":0,"capacity":254,"entries":[]}"#
            }),
        );
        tokio::spawn(async move {
            axum::serve(listener, mock).await.unwrap();
        });

        let app = app_with_controller(format!("http://{addr}"), Some("ctl-token".into()));
        let resp = app
            .oneshot(
                Request::get("/api/tunnel/addresses")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(String::from_utf8_lossy(&bytes).contains("10.200.0.0/24"));
    }

    #[tokio::test]
    async fn release_proxies_a_registry_delete_with_the_token() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let mock = Router::new()
            .route(
                "/peers/{name}",
                axum::routing::delete(
                    |headers: axum::http::HeaderMap, Path(name): Path<String>| async move {
                        assert_eq!(
                            headers.get(axum::http::header::AUTHORIZATION).unwrap(),
                            "Bearer ctl-token"
                        );
                        format!(r#"{{"revision":4,"released":"{name}"}}"#)
                    },
                ),
            )
            .route(
                "/proxy-peers/{name}",
                axum::routing::delete(|| async { (StatusCode::NOT_FOUND, "no such proxy") }),
            );
        tokio::spawn(async move {
            axum::serve(listener, mock).await.unwrap();
        });
        let app = app_with_controller(format!("http://{addr}"), Some("ctl-token".into()));

        let resp = app
            .clone()
            .oneshot(
                Request::delete("/api/tunnel/origins/game-1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(String::from_utf8_lossy(&bytes).contains("game-1"));

        // The controller's status passes straight through (a 404 stays a 404).
        let resp = app
            .oneshot(
                Request::delete("/api/tunnel/proxies/p1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn rotate_proxies_to_the_origin_rotation_route_and_keeps_odd_names_in_one_segment() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let mock = Router::new().fallback(|method: Method, uri: axum::http::Uri| async move {
            assert_eq!(method, Method::POST);
            assert!(
                matches!(
                    uri.path(),
                    "/peers/game-1/psk/rotate" | "/peers/a%2Fb/psk/rotate"
                ),
                "escaped to {uri}"
            );
            (
                StatusCode::ACCEPTED,
                r#"{"name":"game-1","active":"a","pending":"b"}"#,
            )
        });
        tokio::spawn(async move {
            axum::serve(listener, mock).await.unwrap();
        });
        let app = app_with_controller(format!("http://{addr}"), Some("ctl-token".into()));
        let resp = app
            .clone()
            .oneshot(
                Request::post("/api/tunnel/origins/game-1/psk/rotate")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::ACCEPTED);
        let resp = app
            .clone()
            .oneshot(
                Request::post("/api/tunnel/origins/../psk/rotate")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_ne!(resp.status(), StatusCode::ACCEPTED);
        let resp = app
            .oneshot(
                Request::post("/api/tunnel/origins/a%2Fb/psk/rotate")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::ACCEPTED);
    }

    #[tokio::test]
    async fn release_cannot_be_steered_to_another_controller_path() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        // Anything but the registry route would be a DELETE on /config.
        let mock = Router::new().fallback(|uri: axum::http::Uri| async move {
            assert!(uri.path().starts_with("/peers/"), "escaped to {uri}");
            "{}"
        });
        tokio::spawn(async move {
            axum::serve(listener, mock).await.unwrap();
        });
        let app = app_with_controller(format!("http://{addr}"), None);

        for name in ["..", "%2E%2E", "..%2Fconfig", "a%2Fb", "a%3Fb"] {
            let resp = app
                .clone()
                .oneshot(
                    Request::delete(format!("/api/tunnel/origins/{name}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert!(
                resp.status() == StatusCode::BAD_REQUEST || resp.status() == StatusCode::OK,
                "{name}: {}",
                resp.status()
            );
        }
    }

    #[tokio::test]
    async fn no_controller_configured_is_a_clean_503() {
        let state = crate::api::AppState::new(None);
        let app = crate::api::router(state);
        let resp = app
            .oneshot(Request::get("/api/config").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    type Seen = std::sync::Arc<std::sync::Mutex<Vec<(String, String, Option<String>, Vec<u8>)>>>;

    /// A controller stand-in that records (method, path, content-type, body) of every call
    /// and answers `status` with a small JSON body.
    /// The step-up key the mock controller publishes the salt of.
    fn test_key() -> &'static wayhouse_enroll::step_up::StepUpKey {
        static K: std::sync::OnceLock<wayhouse_enroll::step_up::StepUpKey> =
            std::sync::OnceLock::new();
        K.get_or_init(|| wayhouse_enroll::step_up::StepUpKey {
            salt: rand::random(),
            public: rand::random(),
        })
    }

    async fn recording_controller(status: StatusCode) -> (String, Seen) {
        let seen = Seen::default();
        let log = seen.clone();
        let mock = Router::new().fallback(move |req: axum::http::Request<Body>| {
            let log = log.clone();
            async move {
                let (parts, body) = req.into_parts();
                let body = axum::body::to_bytes(body, usize::MAX).await.unwrap();
                let ct = parts
                    .headers
                    .get(axum::http::header::CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string);
                log.lock().unwrap().push((
                    parts.method.to_string(),
                    parts.uri.path().to_string(),
                    ct,
                    body.to_vec(),
                ));
                let answer = if parts.uri.path() == "/enrollments/step-up" {
                    format!(r#"{{"salt":"{}"}}"#, test_key().salt_b64())
                } else {
                    r#"{"ok":true}"#.to_owned()
                };
                (status, [("content-type", "application/json")], answer)
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, mock).await.unwrap();
        });
        (format!("http://{addr}"), seen)
    }

    #[tokio::test]
    async fn plugin_list_and_actions_hit_the_right_controller_paths() {
        let (url, seen) = recording_controller(StatusCode::OK).await;
        let app = app_with_controller(url, None);
        for (method, path) in [
            ("GET", "/api/plugins"),
            ("POST", "/api/plugins/0123abcd/enable"),
            ("POST", "/api/plugins/0123abcd/disable"),
            ("DELETE", "/api/plugins/0123abcd"),
        ] {
            let resp = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(path)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK, "{method} {path}");
        }
        let paths: Vec<_> = seen
            .lock()
            .unwrap()
            .iter()
            .map(|(m, p, _, _)| format!("{m} {}", p))
            .collect();
        assert_eq!(
            paths,
            [
                "GET /plugins",
                "POST /plugins/0123abcd/enable",
                "POST /plugins/0123abcd/disable",
                "DELETE /plugins/0123abcd"
            ]
        );
    }

    #[tokio::test]
    async fn a_cluster_secret_name_stays_one_path_segment() {
        let (url, seen) = recording_controller(StatusCode::OK).await;
        let app = app_with_controller(url, None);
        let call = |method: &'static str, path: &'static str| {
            let app = app.clone();
            async move {
                app.oneshot(
                    Request::builder()
                        .method(method)
                        .uri(path)
                        .body(Body::from("{}"))
                        .unwrap(),
                )
                .await
                .unwrap()
                .status()
            }
        };
        // A slash in the name is sent encoded, so it cannot reach another controller route.
        assert_eq!(call("PUT", "/api/secrets/a%2Fb").await, StatusCode::OK);
        assert_eq!(call("DELETE", "/api/secrets/a%2Fb").await, StatusCode::OK);
        // `..` is refused outright, whether written plain or encoded.
        for path in ["/api/secrets/%2E%2E", "/api/secrets/.."] {
            for method in ["PUT", "DELETE"] {
                let status = call(method, path).await;
                assert!(
                    status == StatusCode::BAD_REQUEST || status == StatusCode::NOT_FOUND,
                    "{method} {path}: {status}"
                );
            }
        }
        let paths: Vec<_> = seen
            .lock()
            .unwrap()
            .iter()
            .map(|(m, p, _, _)| format!("{m} {p}"))
            .collect();
        assert_eq!(paths, ["PUT /secrets/a%2Fb", "DELETE /secrets/a%2Fb"]);
    }

    #[tokio::test]
    async fn plugin_config_secrets_webhook_and_status_hit_the_right_controller_paths() {
        let (url, seen) = recording_controller(StatusCode::OK).await;
        let app = app_with_controller(url, None);
        for (method, path) in [
            ("GET", "/api/plugins/0123abcd"),
            ("GET", "/api/plugins/0123abcd/status"),
            ("PUT", "/api/plugins/0123abcd/config"),
            ("GET", "/api/plugins/0123abcd/secrets"),
            ("PUT", "/api/plugins/0123abcd/secrets/PANEL_TOKEN"),
            ("DELETE", "/api/plugins/0123abcd/secrets/PANEL_TOKEN"),
            ("POST", "/api/plugins/0123abcd/webhook"),
            ("DELETE", "/api/plugins/0123abcd/webhook"),
            // The plugins-file dry run: its own path, not an install id.
            ("GET", "/api/plugins/file/plan"),
            // The key-rotation helper: the key ids per node and the rewrap, not install ids.
            ("GET", "/api/plugins/secrets/keys"),
            ("POST", "/api/plugins/secrets/rewrap"),
            ("POST", "/api/acme/rewrap"),
            ("POST", "/api/secrets/rewrap"),
        ] {
            let resp = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(path)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK, "{method} {path}");
        }
        let paths: Vec<_> = seen
            .lock()
            .unwrap()
            .iter()
            .map(|(m, p, _, _)| format!("{m} {p}"))
            .collect();
        assert_eq!(
            paths,
            [
                "GET /plugins/0123abcd",
                "GET /plugins/0123abcd/status",
                "PUT /plugins/0123abcd/config",
                "GET /plugins/0123abcd/secrets",
                "PUT /plugins/0123abcd/secrets/PANEL_TOKEN",
                "DELETE /plugins/0123abcd/secrets/PANEL_TOKEN",
                "POST /plugins/0123abcd/webhook",
                "DELETE /plugins/0123abcd/webhook",
                "GET /plugins/file/plan",
                "GET /admin/plugins/secrets/keys",
                "POST /admin/plugins/secrets/rewrap",
                "POST /admin/acme/rewrap",
                "POST /secrets/rewrap",
            ]
        );
    }

    #[tokio::test]
    async fn a_secret_slot_stays_one_path_segment() {
        let (url, seen) = recording_controller(StatusCode::OK).await;
        let app = app_with_controller(url, None);
        let resp = app
            .oneshot(
                Request::put("/api/plugins/0123abcd/secrets/a%2Fb%2F..%2Fc")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"value":"x"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            seen.lock().unwrap()[0].1,
            "/plugins/0123abcd/secrets/a%2Fb%2F..%2Fc"
        );
    }

    #[tokio::test]
    async fn a_secret_body_is_forwarded_as_json_and_capped() {
        let (url, seen) = recording_controller(StatusCode::NO_CONTENT).await;
        let app = app_with_controller(url, None);
        let body = r#"{"value":"hunter2-hunter2"}"#;
        let resp = app
            .clone()
            .oneshot(
                Request::put("/api/plugins/0123abcd/secrets/TOKEN")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        {
            let seen = seen.lock().unwrap();
            assert_eq!(seen[0].2.as_deref(), Some("application/json"));
            assert_eq!(seen[0].3, body.as_bytes());
        }
        let big = format!(r#"{{"value":"{}"}}"#, "x".repeat(17 * 1024));
        let resp = app
            .oneshot(
                Request::put("/api/plugins/0123abcd/secrets/TOKEN")
                    .body(Body::from(big))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(
            seen.lock().unwrap().len(),
            1,
            "the oversized body is not forwarded"
        );
    }

    #[tokio::test]
    async fn a_secret_value_is_not_in_the_answer_when_the_controller_is_down() {
        // Nothing listens on port 1.
        let app = app_with_controller("http://127.0.0.1:1".into(), None);
        let resp = app
            .oneshot(
                Request::put("/api/plugins/0123abcd/secrets/TOKEN")
                    .body(Body::from(r#"{"value":"sentinel-secret-value-9f3a1c77"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(!text.contains("sentinel-secret-value"));
        assert!(
            !text.contains("127.0.0.1"),
            "no upstream URL in the answer: {text}"
        );
    }

    #[tokio::test]
    async fn a_config_body_keeps_its_json_content_type() {
        let (url, seen) = recording_controller(StatusCode::OK).await;
        let app = app_with_controller(url, None);
        let body = r#"{"config":{"panel":"p.example"}}"#;
        let resp = app
            .oneshot(
                Request::put("/api/plugins/0123abcd/config")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let seen = seen.lock().unwrap();
        assert_eq!(seen[0].2.as_deref(), Some("application/json"));
        assert_eq!(seen[0].3, body.as_bytes());
    }

    #[tokio::test]
    async fn the_plugin_route_overlay_is_a_read_of_the_controller_overlay() {
        let (url, seen) = recording_controller(StatusCode::OK).await;
        let app = app_with_controller(url, None);
        let resp = app
            .oneshot(
                Request::get("/api/plugin-routes")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let paths: Vec<_> = seen
            .lock()
            .unwrap()
            .iter()
            .map(|(m, p, _, _)| format!("{m} {p}"))
            .collect();
        assert_eq!(paths, ["GET /plugin-routes"]);
    }

    #[tokio::test]
    async fn acme_status_reads_hit_the_controller_acme_paths() {
        let (url, seen) = recording_controller(StatusCode::OK).await;
        let app = app_with_controller(url, None);
        for path in ["/api/acme/sites", "/api/acme/gateways", "/api/acme/account"] {
            let resp = app
                .clone()
                .oneshot(Request::get(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK, "{path}");
        }
        let paths: Vec<_> = seen
            .lock()
            .unwrap()
            .iter()
            .map(|(m, p, _, _)| format!("{m} {p}"))
            .collect();
        assert_eq!(
            paths,
            ["GET /acme/sites", "GET /acme/gateways", "GET /acme/account"]
        );
    }

    #[tokio::test]
    async fn feature_reads_and_override_writes_reach_the_controller() {
        let (url, seen) = recording_controller(StatusCode::OK).await;
        let app = app_with_controller(url, None);
        let body = r#"{"value":false,"reason":"storm","expires_at":1900000000}"#;
        for (method, path, send) in [
            ("GET", "/api/features", ""),
            ("GET", "/api/features/overrides", ""),
            ("PUT", "/api/features/overrides/acl", body),
            ("DELETE", "/api/features/overrides/admin_write.backends", ""),
        ] {
            let resp = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(path)
                        .body(Body::from(send))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK, "{method} {path}");
        }
        let seen = seen.lock().unwrap();
        let calls: Vec<_> = seen.iter().map(|(m, p, _, _)| format!("{m} {p}")).collect();
        assert_eq!(
            calls,
            [
                "GET /features",
                "GET /features/overrides",
                "PUT /features/overrides/acl",
                "DELETE /features/overrides/admin_write.backends"
            ]
        );
        assert_eq!(seen[2].2.as_deref(), Some("application/json"));
        assert_eq!(seen[2].3, body.as_bytes());
    }

    #[tokio::test]
    async fn cluster_secret_calls_reach_the_controller_and_the_sealed_form_stays_behind() {
        let (url, seen) = recording_controller(StatusCode::OK).await;
        let app = app_with_controller(url, None);
        let body = r#"{"value":"hunter2"}"#;
        for (method, path, send, want) in [
            ("GET", "/api/secrets", "", StatusCode::OK),
            ("PUT", "/api/secrets/gossip-psk", body, StatusCode::OK),
            ("DELETE", "/api/secrets/gossip-psk", "", StatusCode::OK),
            (
                "GET",
                "/api/secrets/gossip-psk/sealed",
                "",
                StatusCode::NOT_FOUND,
            ),
            ("PUT", "/api/secrets/..", body, StatusCode::BAD_REQUEST),
        ] {
            let resp = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(path)
                        .body(Body::from(send))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), want, "{method} {path}");
        }
        let seen = seen.lock().unwrap();
        let calls: Vec<_> = seen.iter().map(|(m, p, _, _)| format!("{m} {p}")).collect();
        assert_eq!(
            calls,
            [
                "GET /secrets",
                "PUT /secrets/gossip-psk",
                "DELETE /secrets/gossip-psk"
            ]
        );
        assert_eq!(seen[1].3, body.as_bytes());
    }

    #[tokio::test]
    async fn a_flag_name_cannot_be_steered_to_another_controller_path() {
        let (url, seen) = recording_controller(StatusCode::OK).await;
        let app = app_with_controller(url, None);
        for (name, want) in [
            ("..", StatusCode::BAD_REQUEST),
            ("%2e%2e", StatusCode::BAD_REQUEST),
            ("..%2Fconfig", StatusCode::OK),
            ("a%2Fb", StatusCode::OK),
        ] {
            let resp = app
                .clone()
                .oneshot(
                    Request::delete(format!("/api/features/overrides/{name}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), want, "{name}");
        }
        // The slash stays inside one segment, so the controller sees a name, not a path.
        let paths: Vec<_> = seen.lock().unwrap().iter().map(|c| c.1.clone()).collect();
        assert_eq!(
            paths,
            [
                "/features/overrides/..%2Fconfig",
                "/features/overrides/a%2Fb"
            ]
        );
    }

    #[tokio::test]
    async fn acme_site_and_gateway_writes_reach_the_controller_with_their_body() {
        let (url, seen) = recording_controller(StatusCode::OK).await;
        let app = app_with_controller(url, None);
        let site = r#"{"names":["panel.example.com"],"challenge":"http-01"}"#;
        let put = app
            .clone()
            .oneshot(
                Request::put("/api/acme/sites/panel")
                    .header("content-type", "application/json")
                    .body(Body::from(site))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(put.status(), StatusCode::OK);
        let del = app
            .clone()
            .oneshot(
                Request::delete("/api/acme/sites/panel")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(del.status(), StatusCode::OK);
        let gw = app
            .clone()
            .oneshot(
                Request::post("/api/acme/gateways")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"edge-1"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(gw.status(), StatusCode::OK);
        let seen = seen.lock().unwrap();
        let calls: Vec<_> = seen
            .iter()
            .map(|(m, p, _, b)| (m.as_str(), p.as_str(), b.clone()))
            .collect();
        assert_eq!(
            calls[0],
            ("PUT", "/acme/sites/panel", site.as_bytes().to_vec())
        );
        assert_eq!(calls[1], ("DELETE", "/acme/sites/panel", Vec::new()));
        assert_eq!(
            calls[2],
            ("POST", "/acme/gateways", br#"{"name":"edge-1"}"#.to_vec())
        );
    }

    #[tokio::test]
    async fn enrollment_routes_reach_the_controller_and_the_mint_answer_is_not_cached() {
        let (url, seen) = recording_controller(StatusCode::OK).await;
        let app = app_with_controller(url, None);
        let body =
            r#"{"role":"proxy","name":"edge-1","step_up":"a long enough step-up passphrase"}"#;
        let mint = app
            .clone()
            .oneshot(
                Request::post("/api/enrollments")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(mint.status(), StatusCode::OK);
        assert_eq!(mint.headers()["cache-control"], "no-store");
        for req in [
            Request::get("/api/enrollments"),
            Request::delete("/api/enrollments/abc123"),
            Request::delete("/api/enrollments/a%2Fb"),
        ] {
            let r = app.clone().oneshot(req.body(Body::empty()).unwrap()).await;
            assert_eq!(r.unwrap().status(), StatusCode::OK);
        }
        let seen = seen.lock().unwrap();
        let calls: Vec<_> = seen
            .iter()
            .map(|(m, p, _, b)| (m.as_str(), p.as_str(), b.clone()))
            .collect();
        // The passphrase never leaves the UI: the controller got its salt request, then a signed assertion.
        let sent = String::from_utf8(calls[1].2.clone()).unwrap();
        assert!(!sent.contains("long enough"), "{sent}");
        let sent: serde_json::Value = serde_json::from_str(&sent).unwrap();
        let assertion: wayhouse_enroll::step_up::Assertion =
            serde_json::from_value(sent["step_up"].clone()).unwrap();
        let signing =
            wayhouse_enroll::step_up::derive("a long enough step-up passphrase", &test_key().salt)
                .unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let req = wayhouse_enroll::step_up::Request {
            role: "proxy",
            name: "edge-1",
            ttl_secs: None,
            uses: None,
            name_prefix: None,
            parent_url: None,
        };
        wayhouse_enroll::step_up::verify(
            &signing.verifying_key().to_bytes(),
            &req,
            &assertion,
            now,
        )
        .unwrap();
        let calls: Vec<_> = calls.iter().map(|(m, p, _)| (*m, *p)).collect();
        assert_eq!(
            calls,
            [
                ("GET", "/enrollments/step-up"),
                ("POST", "/enrollments"),
                ("GET", "/enrollments"),
                ("DELETE", "/enrollments/abc123"),
                ("DELETE", "/enrollments/a%2Fb"),
            ]
        );
    }

    #[tokio::test]
    async fn a_short_passphrase_is_refused_before_any_controller_call() {
        let (url, seen) = recording_controller(StatusCode::OK).await;
        let app = app_with_controller(url, None);
        let r = app
            .oneshot(
                Request::post("/api/enrollments")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"role":"agent","name":"a","step_up":"too short"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);
        assert!(seen.lock().unwrap().len() <= 1, "no mint was forwarded");
        assert!(seen.lock().unwrap().iter().all(|c| c.0 != "POST"));
    }

    #[tokio::test]
    async fn an_oversized_mint_body_and_a_dot_segment_id_forward_nothing() {
        let (url, seen) = recording_controller(StatusCode::OK).await;
        let app = app_with_controller(url, None);
        let big = app
            .clone()
            .oneshot(
                Request::post("/api/enrollments")
                    .body(Body::from("x".repeat(ENROLLMENT_BODY_BYTES + 1)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(big.status(), StatusCode::PAYLOAD_TOO_LARGE);
        let dots = app
            .oneshot(
                Request::delete("/api/enrollments/..")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(dots.status(), StatusCode::BAD_REQUEST);
        assert!(seen.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn acme_site_name_cannot_be_steered_to_another_controller_path() {
        let (url, seen) = recording_controller(StatusCode::OK).await;
        let app = app_with_controller(url, None);
        // Dot segments are refused outright; a separator or query character stays inside one segment.
        for (name, forwarded) in [
            ("..", None),
            ("%2E%2E", None),
            ("a%2Fb", Some("/acme/sites/a%2Fb")),
            ("a%3Fb", Some("/acme/sites/a%3Fb")),
        ] {
            seen.lock().unwrap().clear();
            let resp = app
                .clone()
                .oneshot(
                    Request::delete(format!("/api/acme/sites/{name}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            let paths: Vec<String> = seen.lock().unwrap().iter().map(|r| r.1.clone()).collect();
            match forwarded {
                None => {
                    assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{name}");
                    assert!(paths.is_empty(), "{name}: {paths:?}");
                }
                Some(path) => {
                    assert_eq!(resp.status(), StatusCode::OK, "{name}");
                    assert_eq!(paths, [path], "{name}");
                }
            }
        }
    }

    #[tokio::test]
    async fn acme_writes_refuse_an_oversized_body_and_forward_nothing() {
        let (url, seen) = recording_controller(StatusCode::OK).await;
        let app = app_with_controller(url, None);
        let big = |n: usize| Body::from("x".repeat(n));
        for (req, name) in [
            (
                Request::put("/api/acme/sites/web").body(big(ACME_SITE_BODY_BYTES + 1)),
                "put site",
            ),
            (
                Request::post("/api/acme/gateways").body(big(ACME_GATEWAY_BODY_BYTES + 1)),
                "register gateway",
            ),
        ] {
            let resp = app.clone().oneshot(req.unwrap()).await.unwrap();
            assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE, "{name}");
        }
        assert!(seen.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_feature_override_body_over_the_cap_is_refused_and_forwards_nothing() {
        let (url, seen) = recording_controller(StatusCode::NO_CONTENT).await;
        let app = app_with_controller(url, None);
        let resp = app
            .oneshot(
                Request::put("/api/features/overrides/acl")
                    .body(Body::from("x".repeat(FEATURE_OVERRIDE_BODY_BYTES + 1)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert!(seen.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn revoking_a_gateway_calls_the_controller_with_one_encoded_segment() {
        let (url, seen) = recording_controller(StatusCode::NO_CONTENT).await;
        let app = app_with_controller(url, None);
        let resp = app
            .clone()
            .oneshot(
                Request::delete("/api/acme/gateways/edge-1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        let resp = app
            .oneshot(
                Request::delete("/api/acme/gateways/..")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(
            (seen[0].0.as_str(), seen[0].1.as_str()),
            ("DELETE", "/acme/gateways/edge-1")
        );
    }

    #[tokio::test]
    async fn plugin_install_keeps_its_json_content_type_and_body() {
        let (url, seen) = recording_controller(StatusCode::CREATED).await;
        let app = app_with_controller(url, None);
        let body = r#"{"name":"demo","sha256":"ab"}"#;
        let resp = app
            .oneshot(
                Request::post("/api/plugins")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let seen = seen.lock().unwrap();
        assert_eq!(seen[0].1, "/plugins");
        assert_eq!(seen[0].2.as_deref(), Some("application/json"));
        assert_eq!(seen[0].3, body.as_bytes());
    }

    #[tokio::test]
    async fn plugin_module_upload_forwards_raw_bytes_up_to_the_module_cap() {
        let (url, seen) = recording_controller(StatusCode::CREATED).await;
        let app = app_with_controller(url, None);
        let module = vec![7u8; 3 * 1024 * 1024];
        let resp = app
            .clone()
            .oneshot(
                Request::post("/api/plugins/modules")
                    .body(Body::from(module.clone()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        assert_eq!(seen.lock().unwrap()[0].3, module);

        let too_big = vec![0u8; wayhouse_http::MAX_SNIFFER_MODULE_BYTES + 1];
        let resp = app
            .oneshot(
                Request::post("/api/plugins/modules")
                    .body(Body::from(too_big))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(
            seen.lock().unwrap().len(),
            1,
            "the oversized body must not be forwarded"
        );
    }

    #[tokio::test]
    async fn plugin_501_from_the_controller_passes_through() {
        let (url, _) = recording_controller(StatusCode::NOT_IMPLEMENTED).await;
        let app = app_with_controller(url, None);
        let resp = app
            .oneshot(Request::get("/api/plugins").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_IMPLEMENTED);
    }

    #[tokio::test]
    async fn plugin_id_cannot_steer_the_proxy_to_another_controller_path() {
        let (url, seen) = recording_controller(StatusCode::OK).await;
        let app = app_with_controller(url, None);
        for id in ["..", "%2E%2E", "..%2Fconfig", "a%2Fb", "a%3Fb"] {
            for (method, suffix) in [("DELETE", ""), ("POST", "/enable")] {
                let resp = app
                    .clone()
                    .oneshot(
                        Request::builder()
                            .method(method)
                            .uri(format!("/api/plugins/{id}{suffix}"))
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                assert!(
                    resp.status() == StatusCode::BAD_REQUEST
                        || resp.status() == StatusCode::OK
                        || resp.status() == StatusCode::NOT_FOUND,
                    "{id}: {}",
                    resp.status()
                );
            }
        }
        for (_, path, _, _) in seen.lock().unwrap().iter() {
            assert!(path.starts_with("/plugins/"), "escaped to {path}");
            // The id stays one segment: an encoded slash is not a path separator.
            let segments: Vec<_> = path.trim_start_matches('/').split('/').collect();
            assert!(segments.len() <= 3, "extra segments in {path}");
            assert!(
                segments.iter().all(|seg| *seg != ".."),
                "dot segment in {path}"
            );
        }
    }

    /// A mock proxy admin API recording `(method, path and query, auth, body)` of what it is sent.
    async fn mock_proxy_admin() -> (
        String,
        std::sync::Arc<std::sync::Mutex<Vec<(String, String, Option<String>, String)>>>,
    ) {
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let record = calls.clone();
        let mock = Router::new().fallback(move |req: Request<Body>| {
            let record = record.clone();
            async move {
                let (parts, body) = req.into_parts();
                let body = axum::body::to_bytes(body, usize::MAX).await.unwrap();
                let auth = parts
                    .headers
                    .get(axum::http::header::AUTHORIZATION)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string);
                record.lock().unwrap().push((
                    parts.method.to_string(),
                    parts.uri.to_string(),
                    auth,
                    String::from_utf8_lossy(&body).into_owned(),
                ));
                (
                    [("x-config-revision", "77"), ("etag", "\"abc\"")],
                    r#"{"revision":78}"#,
                )
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, mock).await.unwrap() });
        (format!("http://{addr}"), calls)
    }

    #[tokio::test]
    async fn without_a_controller_the_config_editor_talks_to_the_file_config_proxy() {
        let (url, calls) = mock_proxy_admin().await;
        let state = crate::api::AppState::new(None).with_proxy_admin(url, Some("ptok".into()));
        let app = crate::api::router(state);
        for (method, uri, body) in [
            ("GET", "/api/config", ""),
            ("GET", "/api/config/schema", ""),
            ("POST", "/api/config/validate", "pools: []"),
            // The canary and confirm parameters mean nothing for a file; the revision does.
            (
                "POST",
                "/api/config?base=77&stage=canary&group=g&confirm_within=30",
                "pools: []",
            ),
            ("POST", "/api/config", "pools: []"),
        ] {
            let resp = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(uri)
                        .body(Body::from(body))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK, "{method} {uri}");
        }
        let seen: Vec<_> = calls
            .lock()
            .unwrap()
            .iter()
            .map(|(m, u, a, b)| (m.clone(), u.clone(), a.clone(), b.clone()))
            .collect();
        let ptok = Some("Bearer ptok".to_string());
        assert_eq!(
            seen,
            vec![
                (
                    "GET".into(),
                    "/config/file".into(),
                    ptok.clone(),
                    String::new()
                ),
                (
                    "GET".into(),
                    "/config/schema".into(),
                    ptok.clone(),
                    String::new()
                ),
                (
                    "POST".into(),
                    "/config/validate".into(),
                    ptok.clone(),
                    "pools: []".into()
                ),
                (
                    "PUT".into(),
                    "/config/file?base=77".into(),
                    ptok.clone(),
                    "pools: []".into()
                ),
                (
                    "PUT".into(),
                    "/config/file".into(),
                    ptok,
                    "pools: []".into()
                ),
            ]
        );
    }

    #[tokio::test]
    async fn the_proxys_revision_and_etag_reach_the_browser_and_a_controller_wins_over_the_proxy() {
        let (proxy_url, proxy_calls) = mock_proxy_admin().await;
        let state = crate::api::AppState::new(None).with_proxy_admin(proxy_url.clone(), None);
        let resp = crate::api::router(state)
            .oneshot(Request::get("/api/config").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.headers()["x-config-revision"], "77");
        assert_eq!(resp.headers()["etag"], "\"abc\"");
        // With both set, the controller answers and the proxy is not called again.
        let (controller_url, controller_calls) = mock_proxy_admin().await;
        let state = crate::api::AppState::new(None)
            .with_proxy_admin(proxy_url, None)
            .with_controller(controller_url, None);
        let before = proxy_calls.lock().unwrap().len();
        crate::api::router(state)
            .oneshot(Request::get("/api/config").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(proxy_calls.lock().unwrap().len(), before);
        assert_eq!(controller_calls.lock().unwrap()[0].1, "/config");
    }
}
