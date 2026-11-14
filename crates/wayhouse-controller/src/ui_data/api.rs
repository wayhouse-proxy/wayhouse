//! `/ui/*`: the controller side of the web UI's stored data. Viewers read and operators write
//! through the UI backend, which holds the controller token; the controller itself only checks
//! the bearer token.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{OriginalUri, Path, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{middleware, Json, Router};
use serde_json::{json, Value};

use super::{Applied, Kind, UiOp, UiStore};
use crate::ha::client::{propose_write_as, ForwardHeaders};
use crate::ha::{HaHandle, WriteRequest, WriteResponse};

#[derive(Clone)]
pub struct UiState {
    pub store: UiStore,
    pub auth_token: Option<Arc<str>>,
    /// `Some` under HA: writes go through Raft and `store` is the state machine's.
    pub ha: Option<Arc<HaHandle>>,
    /// Seals cluster secrets (the same key file as plugin secrets); empty without a key.
    pub keyring: wayhouse_secrets::KeyringHandle,
}

pub(crate) fn err(status: StatusCode, msg: impl std::fmt::Display) -> Response {
    (status, Json(json!({ "error": msg.to_string() }))).into_response()
}

pub(crate) fn internal(e: impl std::fmt::Display) -> Response {
    tracing::error!(error = %e, "ui store failure");
    err(StatusCode::INTERNAL_SERVER_ERROR, "ui store failure")
}

/// The record as exported: a named kind carries its name in the body.
fn exported(kind: Kind, name: &str, mut v: Value) -> Value {
    if !kind_is_single(kind) {
        if let Some(o) = v.as_object_mut() {
            o.insert("name".into(), name.into());
        }
    }
    v
}

fn kind_is_single(kind: Kind) -> bool {
    matches!(kind, Kind::FormLayout | Kind::Defaults)
}

pub fn router(state: UiState) -> Router {
    let mut r = Router::new();
    for (base, kind) in [
        ("/ui/dashboards", Kind::Dashboard),
        ("/ui/themes", Kind::Theme),
        ("/ui/filters", Kind::SavedFilter),
        ("/ui/node-tags", Kind::NodeTags),
    ] {
        r = r
            .route(
                base,
                get(move |State(st): State<UiState>| async move { list(&st, kind) }),
            )
            .route(
                &format!("{base}/{{name}}"),
                get(
                    move |State(st): State<UiState>, Path(name): Path<String>| async move {
                        read_one(&st, kind, &name)
                    },
                )
                .put(
                    move |State(st): State<UiState>,
                          OriginalUri(uri): OriginalUri,
                          headers: HeaderMap,
                          Path(name): Path<String>,
                          body: Bytes| async move {
                        put_one(&st, kind, name, uri.path(), &headers, body).await
                    },
                )
                .delete(
                    move |State(st): State<UiState>,
                          OriginalUri(uri): OriginalUri,
                          headers: HeaderMap,
                          Path(name): Path<String>| async move {
                        delete_one(&st, kind, name, uri.path(), &headers).await
                    },
                ),
            );
    }
    for (path, kind) in [
        ("/ui/layout", Kind::FormLayout),
        ("/ui/defaults", Kind::Defaults),
    ] {
        r = r.route(
            path,
            get(move |State(st): State<UiState>| async move { read_one(&st, kind, "") })
                .put(
                    move |State(st): State<UiState>, headers: HeaderMap, body: Bytes| async move {
                        put_one(&st, kind, String::new(), path, &headers, body).await
                    },
                )
                .delete(
                    move |State(st): State<UiState>, headers: HeaderMap| async move {
                        delete_one(&st, kind, String::new(), path, &headers).await
                    },
                ),
        );
    }
    r = r
        .route(
            "/secrets",
            get(|State(st): State<UiState>| async move { list_secrets(&st) }),
        )
        .route(
            "/secrets/rewrap",
            axum::routing::post(
                |State(st): State<UiState>| async move { rewrap_secrets(&st).await },
            ),
        )
        .route(
            "/secrets/{name}",
            axum::routing::put(
                |State(st): State<UiState>,
                 headers: HeaderMap,
                 Path(name): Path<String>,
                 body: Bytes| async move { put_secret(&st, name, &headers, body).await },
            )
            .delete(
                |State(st): State<UiState>,
                 OriginalUri(uri): OriginalUri,
                 headers: HeaderMap,
                 Path(name): Path<String>| async move {
                    delete_secret(&st, name, uri.path(), &headers).await
                },
            ),
        )
        .route(
            "/secrets/{name}/sealed",
            get(
                |State(st): State<UiState>, Path(name): Path<String>| async move {
                    read_sealed(&st, &name)
                },
            ),
        )
        .route(
            "/features",
            get(|State(st): State<UiState>| async move { flags(&st) }),
        )
        .route(
            "/features/overrides",
            get(|State(st): State<UiState>| async move { list(&st, Kind::FeatureOverride) }),
        )
        .route(
            "/features/overrides/{name}",
            axum::routing::put(
                |State(st): State<UiState>,
                 OriginalUri(uri): OriginalUri,
                 headers: HeaderMap,
                 Path(name): Path<String>,
                 body: Bytes| async move {
                    put_override(&st, name, uri.path(), &headers, body).await
                },
            )
            .delete(
                |State(st): State<UiState>,
                 OriginalUri(uri): OriginalUri,
                 headers: HeaderMap,
                 Path(name): Path<String>| async move {
                    delete_override(&st, name, uri.path(), &headers).await
                },
            ),
        );
    r.route_layer(middleware::from_fn_with_state(
        wayhouse_http::server::BearerAuth::new(state.auth_token.as_deref()),
        wayhouse_http::server::require_bearer,
    ))
    .with_state(state)
}

fn list(st: &UiState, kind: Kind) -> Response {
    match st.store.list(kind) {
        Ok(items) => {
            let items: Vec<Value> = items
                .into_iter()
                .map(|(n, v)| exported(kind, &n, v))
                .collect();
            Json(json!({ "items": items })).into_response()
        }
        Err(e) => internal(e),
    }
}

pub(crate) fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// `GET /features`: the controller's flags with the overrides in force, and `proxy_flags`,
/// the proxy flags as the fleet sees them (default and override; the config value is the
/// UI's to overlay, each node's build decides `available`).
fn flags(st: &UiState) -> Response {
    match crate::features::active(&st.store, now()) {
        Ok(o) => Json(json!({
            "flags": crate::features::effective(&o),
            "proxy_flags": crate::features::fleet_view(&o),
        }))
        .into_response(),
        Err(e) => internal(e),
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct OverrideBody {
    value: bool,
    reason: String,
    expires_at: Option<u64>,
    /// Seconds from now; the controller stamps `expires_at` itself (the browser clock is not
    /// trusted). Exclusive with `expires_at`.
    ttl_secs: Option<u64>,
}

/// `PUT /features/overrides/{name}`: `{value, reason, expires_at? | ttl_secs?}`; the actor is the
/// `X-Actor` header the UI backend sets, the time is the controller's.
async fn put_override(
    st: &UiState,
    name: String,
    path: &str,
    headers: &HeaderMap,
    body: Bytes,
) -> Response {
    let b: OverrideBody = match serde_json::from_slice(&body) {
        Ok(b) => b,
        Err(e) => return err(StatusCode::UNPROCESSABLE_ENTITY, e),
    };
    let set_at = now();
    let expires_at = match (b.expires_at, b.ttl_secs) {
        (Some(_), Some(_)) => {
            return err(
                StatusCode::UNPROCESSABLE_ENTITY,
                "send expires_at or ttl_secs, not both",
            )
        }
        (_, Some(0)) => {
            return err(
                StatusCode::UNPROCESSABLE_ENTITY,
                "ttl_secs must be positive",
            )
        }
        (_, Some(ttl)) => Some(set_at.saturating_add(ttl)),
        (e, None) => e,
    };
    if expires_at.is_some_and(|e| e <= set_at) {
        return err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "expires_at is already past",
        );
    }
    if let Err(why) = wayhouse_config::features::check_override(&name, b.value, set_at, expires_at)
    {
        return err(StatusCode::UNPROCESSABLE_ENTITY, why);
    }
    let mut value = json!({ "value": b.value, "reason": b.reason, "set_at": set_at });
    if let Some(e) = expires_at {
        value["expires_at"] = e.into();
    }
    let actor = actor(headers);
    if let Some(a) = &actor {
        value["actor"] = a.as_str().into();
    }
    let reply = exported(Kind::FeatureOverride, &name, value.clone());
    let audit = (name.clone(), b.value, b.reason, expires_at);
    let op = UiOp::Put {
        kind: Kind::FeatureOverride,
        name,
        value,
    };
    write(st, op, Method::PUT, path, headers, body, move || {
        let (flag, on, reason, expires_at) = audit;
        tracing::warn!(%flag, value = on, ?reason, ?expires_at, actor = actor.as_deref().unwrap_or("-"), "feature override set");
        Json(reply).into_response()
    })
    .await
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SecretBody {
    value: String,
}

/// `GET /secrets`: what is stored, by name, with when and under which key. Never a value and
/// never the ciphertext.
fn list_secrets(st: &UiState) -> Response {
    match st.store.list(Kind::ClusterSecret) {
        Ok(items) => {
            let items: Vec<Value> = items
                .into_iter()
                .map(|(name, v)| {
                    let mut out =
                        json!({ "name": name, "key_id": v["key_id"], "set_at": v["set_at"] });
                    if let Some(a) = v.get("actor") {
                        out["actor"] = a.clone();
                    }
                    out
                })
                .collect();
            Json(json!({ "items": items })).into_response()
        }
        Err(e) => internal(e),
    }
}

/// `GET /secrets/{name}/sealed`: the ciphertext a proxy opens with its own key file. The
/// controller token gates it, like every route here.
fn read_sealed(st: &UiState, name: &str) -> Response {
    match st.store.get(Kind::ClusterSecret, name) {
        Ok(Some(v)) => Json(json!({
            "key_id": v["key_id"], "nonce": v["nonce"], "ciphertext": v["ciphertext"]
        }))
        .into_response(),
        Ok(None) => err(StatusCode::NOT_FOUND, "no such secret"),
        Err(e) => internal(e),
    }
}

/// `PUT /secrets/{name}` with `{"value": "..."}`: seals the value here and replicates only the
/// ciphertext. Under HA the entry reaches the leader over the peer channel as ciphertext, so the
/// request that carried the value is never forwarded.
async fn put_secret(st: &UiState, name: String, headers: &HeaderMap, body: Bytes) -> Response {
    use wayhouse_secrets::SealError;
    // Never the parser's message: it can quote the submitted text.
    let Ok(parsed) = serde_json::from_slice::<SecretBody>(&body) else {
        return err(
            StatusCode::BAD_REQUEST,
            "the body must be {\"value\": \"...\"}",
        );
    };
    let value = zeroize::Zeroizing::new(parsed.value.into_bytes());
    let sealed = match st.keyring.current().seal_cluster(&name, &value) {
        Ok(s) => s,
        Err(SealError::NoKey) => {
            return err(
                StatusCode::CONFLICT,
                "no secret key configured on this controller (--secret-key-file)",
            )
        }
        Err(SealError::Size) => {
            return err(
                StatusCode::BAD_REQUEST,
                format!(
                    "a secret must be {} to {} bytes",
                    wayhouse_secrets::MIN_SECRET_BYTES,
                    wayhouse_secrets::MAX_SECRET_BYTES
                ),
            )
        }
        Err(e @ SealError::Name) => return err(StatusCode::BAD_REQUEST, e.to_string()),
    };
    drop(value);
    let (set_at, who) = (now(), actor(headers));
    let mut record = json!({
        "key_id": sealed.key_id, "nonce": sealed.nonce, "ciphertext": sealed.ciphertext, "set_at": set_at
    });
    if let Some(a) = &who {
        record["actor"] = a.as_str().into();
    }
    let key_id = sealed.key_id;
    let active_id = key_id.clone();
    let op = UiOp::Put {
        kind: Kind::ClusterSecret,
        name: name.clone(),
        value: record,
    };
    let done = move || {
        tracing::warn!(secret = %name, %key_id, actor = who.as_deref().unwrap_or("-"), "cluster secret set");
        Json(json!({ "name": name, "set": true, "key_id": key_id, "set_at": set_at }))
            .into_response()
    };
    let Some(ha) = &st.ha else {
        return match st.store.apply_local(&op) {
            Ok(applied) => answer(&applied, done),
            Err(e) => internal(e),
        };
    };
    if let Err(why) = super::check_support(ha).await {
        return err(StatusCode::CONFLICT, why);
    }
    // As for a plugin secret: sealed under a key the other nodes and proxies may not hold yet.
    if let Err(why) = crate::plugins::peer::check_active_key(ha, &active_id).await {
        return err(
            StatusCode::CONFLICT,
            format!("every node needs the active key {active_id} first: {why}"),
        );
    }
    match crate::plugins::peer::propose_ciphertext(ha, WriteRequest::Ui(op)).await {
        Ok(WriteResponse::UiResult(applied)) => answer(&applied, done),
        Ok(other) => internal(format!("unexpected answer {other:?}")),
        Err(why) => err(StatusCode::SERVICE_UNAVAILABLE, why),
    }
}

/// `POST /secrets/rewrap`: re-encrypts every cluster secret under the active key (step 2 of a
/// rotation, next to `/admin/plugins/secrets/rewrap`). Refuses while a member lacks the active
/// key. A secret replaced meanwhile is skipped and reported.
async fn rewrap_secrets(st: &UiState) -> Response {
    use crate::ui_data::Applied;
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
        // The entry is a `UiOp` a level-4 member cannot decode.
        if let Err(why) = super::check_support(ha).await {
            return err(StatusCode::CONFLICT, why);
        }
    }
    let stored = match st.store.list(Kind::ClusterSecret) {
        Ok(a) => a,
        Err(e) => return internal(e),
    };
    let (mut total, mut skipped, mut unreadable) = (0, Vec::new(), Vec::new());
    for (name, mut record) in stored {
        if record["key_id"].as_str() == Some(active.as_str()) {
            continue;
        }
        let Ok(old) = serde_json::from_value::<wayhouse_secrets::Sealed>(record.clone()) else {
            unreadable.push(name);
            continue;
        };
        let Ok(value) = ring.open_cluster(&name, &old) else {
            unreadable.push(name);
            continue;
        };
        let Ok(new) = ring.seal_cluster(&name, value.expose()) else {
            unreadable.push(name);
            continue;
        };
        total += 1;
        record["key_id"] = new.key_id.into();
        record["nonce"] = new.nonce.into();
        record["ciphertext"] = new.ciphertext.into();
        let op = UiOp::RewrapSecret {
            name: name.clone(),
            from_nonce: old.nonce,
            value: record,
        };
        let applied = match &st.ha {
            None => st.store.apply_local(&op).map_err(|e| e.to_string()),
            Some(ha) => {
                match crate::plugins::peer::propose_ciphertext(ha, WriteRequest::Ui(op)).await {
                    Ok(WriteResponse::UiResult(a)) => Ok(a),
                    Ok(other) => return internal(format!("unexpected answer {other:?}")),
                    Err(why) => return err(StatusCode::SERVICE_UNAVAILABLE, why),
                }
            }
        };
        match applied {
            Ok(Applied::Done) => {
                tracing::warn!(secret = %name, key_id = %active, "cluster secret rewrapped");
            }
            Ok(Applied::Stale | Applied::NotFound) => skipped.push(name),
            Ok(other) => return internal(format!("unexpected answer {other:?}")),
            Err(e) => return internal(e),
        }
    }
    if let (Some(ha), true) = (&st.ha, total > skipped.len()) {
        crate::plugins::peer::request_purge(ha);
    }
    Json(json!({
        "rewrapped": total - skipped.len(),
        "skipped": skipped,
        "unreadable": unreadable,
        "active_key_id": active,
    }))
    .into_response()
}

/// `DELETE /secrets/{name}`, with the audit line and, under HA, a log purge so the superseded
/// ciphertext does not linger.
async fn delete_secret(st: &UiState, name: String, path: &str, headers: &HeaderMap) -> Response {
    let who = actor(headers);
    let secret = name.clone();
    let ha = st.ha.clone();
    let op = UiOp::Delete {
        kind: Kind::ClusterSecret,
        name,
    };
    write(st, op, Method::DELETE, path, headers, Bytes::new(), move || {
        tracing::warn!(%secret, actor = who.as_deref().unwrap_or("-"), "cluster secret deleted");
        if let Some(ha) = &ha {
            crate::plugins::peer::request_purge(ha);
        }
        StatusCode::NO_CONTENT.into_response()
    })
    .await
}

pub(crate) fn actor(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-actor")
        .and_then(|v| v.to_str().ok())
        // Bounded like the plugin API's: a caller cannot park a huge string in the audit trail.
        .map(|s| s.chars().take(128).collect())
}

/// `DELETE /features/overrides/{name}`, with the audit line.
async fn delete_override(st: &UiState, name: String, path: &str, headers: &HeaderMap) -> Response {
    let who = actor(headers);
    let flag = name.clone();
    let op = UiOp::Delete {
        kind: Kind::FeatureOverride,
        name,
    };
    write(st, op, Method::DELETE, path, headers, Bytes::new(), move || {
        tracing::warn!(%flag, actor = who.as_deref().unwrap_or("-"), "feature override deleted");
        StatusCode::NO_CONTENT.into_response()
    })
    .await
}

fn read_one(st: &UiState, kind: Kind, name: &str) -> Response {
    match st.store.get(kind, name) {
        Ok(Some(v)) => Json(exported(kind, name, v)).into_response(),
        // No deployment defaults set reads as an empty object; everything else is absent.
        Ok(None) if kind == Kind::Defaults => Json(json!({})).into_response(),
        Ok(None) => err(StatusCode::NOT_FOUND, "no such record"),
        Err(e) => internal(e),
    }
}

async fn put_one(
    st: &UiState,
    kind: Kind,
    name: String,
    path: &str,
    headers: &HeaderMap,
    body: Bytes,
) -> Response {
    let Ok(mut value) = serde_json::from_slice::<Value>(&body) else {
        return err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "the body is not valid JSON",
        );
    };
    // The path names the record, whatever the body says.
    if !kind_is_single(kind) {
        if let Some(o) = value.as_object_mut() {
            o.insert("name".into(), name.clone().into());
        }
    }
    // The colour maths runs here, on the node that takes the request, and not when replicas
    // apply the entry (see `contrast`). A shape error is left to the usual validation.
    if kind == Kind::Theme && super::validate(kind, &name, &value).is_ok() {
        if let Err(failures) = super::contrast::check_record(&value) {
            let first = failures
                .first()
                .map(|f| f.message.clone())
                .unwrap_or_default();
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(json!({ "error": first, "failures": failures })),
            )
                .into_response();
        }
    }
    let reply = exported(kind, &name, value.clone());
    let op = UiOp::Put { kind, name, value };
    write(st, op, Method::PUT, path, headers, body, move || {
        Json(reply).into_response()
    })
    .await
}

async fn delete_one(
    st: &UiState,
    kind: Kind,
    name: String,
    path: &str,
    headers: &HeaderMap,
) -> Response {
    let op = UiOp::Delete { kind, name };
    write(st, op, Method::DELETE, path, headers, Bytes::new(), || {
        StatusCode::NO_CONTENT.into_response()
    })
    .await
}

fn answer(applied: &Applied, ok: impl FnOnce() -> Response) -> Response {
    match applied {
        Applied::Done => ok(),
        Applied::NotFound => err(StatusCode::NOT_FOUND, "no such record"),
        Applied::Invalid(why) => err(StatusCode::UNPROCESSABLE_ENTITY, why),
        Applied::Full(max) => err(
            StatusCode::CONFLICT,
            format!("already {max} records of this kind; delete one first"),
        ),
        Applied::Stale => err(
            StatusCode::CONFLICT,
            "the record changed while the request was applied; try again",
        ),
        Applied::Refused => err(StatusCode::UNAUTHORIZED, "invalid or expired code"),
        Applied::Taken => err(StatusCode::CONFLICT, "that node id is already taken"),
        Applied::Replayed | Applied::Allocated(_) => {
            internal("a write was answered as a replay or a redeem")
        }
    }
}

async fn write(
    st: &UiState,
    op: UiOp,
    method: Method,
    path: &str,
    headers: &HeaderMap,
    body: Bytes,
    ok: impl FnOnce() -> Response + Send,
) -> Response {
    let Some(ha) = &st.ha else {
        return match st.store.apply_local(&op) {
            Ok(applied) => answer(&applied, ok),
            Err(e) => internal(e),
        };
    };
    if let Err(why) = super::check_support(ha).await {
        return err(StatusCode::CONFLICT, why);
    }
    // A write that is invalid on every replica need not cost a log entry.
    if let UiOp::Put { kind, name, value } = &op {
        if let Err(why) = super::validate(*kind, name, value) {
            return err(StatusCode::UNPROCESSABLE_ENTITY, why);
        }
    }
    let forward = ForwardHeaders::from_headers(headers);
    let text = String::from_utf8_lossy(&body).into_owned();
    propose_write_as(
        ha,
        WriteRequest::Ui(op),
        method,
        path,
        text,
        &forward,
        |resp| match resp {
            WriteResponse::UiResult(applied) => answer(&applied, ok),
            other => internal(format!("unexpected answer {other:?}")),
        },
    )
    .await
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_actor_header_is_cut_to_128_characters() {
        let mut h = HeaderMap::new();
        h.insert("x-actor", "a".repeat(500).parse().unwrap());
        assert_eq!(super::actor(&h).unwrap().len(), 128);
    }

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use serde_json::{json, Value};
    use tower::ServiceExt;

    use super::*;

    /// A key for the tests: 32 bytes, base64.
    const KEY: &str = "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=";

    fn app(token: Option<&str>) -> (Router, UiStore, tempfile::TempDir) {
        app_with(token, wayhouse_secrets::Keyring::parse(KEY).unwrap())
    }

    fn app_with(
        token: Option<&str>,
        keyring: wayhouse_secrets::Keyring,
    ) -> (Router, UiStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::kv::Db::open(dir.path()).unwrap();
        let store = UiStore::open(&db).unwrap();
        let state = UiState {
            store: store.clone(),
            auth_token: token.map(Arc::from),
            ha: None,
            keyring: wayhouse_secrets::KeyringHandle::new(keyring),
        };
        (router(state), store, dir)
    }

    async fn call(
        app: &Router,
        method: &str,
        uri: &str,
        body: Value,
        token: Option<&str>,
    ) -> (StatusCode, Value) {
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
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    fn dash() -> Value {
        json!({"widgets": [{"id": "nodes", "width": 6}, {"id": "from-the-future", "width": 3}]})
    }

    fn flag<'a>(v: &'a Value, name: &str) -> &'a Value {
        v["flags"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["name"] == name)
            .unwrap()
    }

    #[tokio::test]
    async fn an_override_turns_a_flag_off_until_it_is_deleted() {
        let (app, _s, _d) = app(None);
        let (_, v) = call(&app, "GET", "/features", Value::Null, None).await;
        assert_eq!(flag(&v, "plugin_webhooks")["source"], "default");
        let body = json!({"value": false, "reason": "webhook storm"});
        let (st, got) = call(
            &app,
            "PUT",
            "/features/overrides/plugin_webhooks",
            body,
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{got}");
        assert_eq!(got["reason"], "webhook storm");
        let (_, v) = call(&app, "GET", "/features", Value::Null, None).await;
        assert_eq!(flag(&v, "plugin_webhooks")["value"], false);
        assert_eq!(flag(&v, "plugin_webhooks")["source"], "override");
        let (_, list) = call(&app, "GET", "/features/overrides", Value::Null, None).await;
        assert_eq!(list["items"][0]["name"], "plugin_webhooks");
        let (st, _) = call(
            &app,
            "DELETE",
            "/features/overrides/plugin_webhooks",
            Value::Null,
            None,
        )
        .await;
        assert_eq!(st, StatusCode::NO_CONTENT);
        let (_, v) = call(&app, "GET", "/features", Value::Null, None).await;
        assert_eq!(flag(&v, "plugin_webhooks")["value"], true);
    }

    #[tokio::test]
    async fn the_proxy_flags_are_listed_with_the_overrides_that_apply_to_them() {
        let (app, _s, _d) = app(None);
        let (_, v) = call(&app, "GET", "/features", Value::Null, None).await;
        let proxy = |v: &Value, n: &str| {
            v["proxy_flags"]
                .as_array()
                .unwrap()
                .iter()
                .find(|f| f["name"] == n)
                .unwrap()
                .clone()
        };
        assert_eq!(proxy(&v, "geo")["scope"], "proxy");
        assert_eq!(proxy(&v, "geo")["source"], "default");
        let body = json!({"value": false, "reason": "geo db wrong", "expires_at": now() + 600});
        let (st, got) = call(&app, "PUT", "/features/overrides/geo", body, None).await;
        assert_eq!(st, StatusCode::OK, "{got}");
        let (_, v) = call(&app, "GET", "/features", Value::Null, None).await;
        assert_eq!(proxy(&v, "geo")["value"], false);
        assert_eq!(proxy(&v, "geo")["source"], "override");
    }

    #[tokio::test]
    async fn operator_tags_are_stored_per_node_and_listed() {
        let (app, _s, _d) = app(None);
        let tags = json!({"tags": {"env": "prod"}});
        let (st, got) = call(&app, "PUT", "/ui/node-tags/fra-1", tags.clone(), None).await;
        assert_eq!(
            (st, &got["name"]),
            (StatusCode::OK, &json!("fra-1")),
            "{got}"
        );
        let (st, _) = call(
            &app,
            "PUT",
            "/ui/node-tags/x",
            json!({"tags": {"Env": "p"}}),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
        let (_, list) = call(&app, "GET", "/ui/node-tags", Value::Null, None).await;
        assert_eq!(
            list["items"],
            json!([{"name": "fra-1", "tags": {"env": "prod"}}])
        );
        let (st, _) = call(&app, "DELETE", "/ui/node-tags/fra-1", Value::Null, None).await;
        assert_eq!(st, StatusCode::NO_CONTENT);
        let (st, _) = call(&app, "DELETE", "/ui/node-tags/fra-1", Value::Null, None).await;
        assert_eq!(st, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn an_override_the_registry_forbids_is_refused() {
        let (app, _s, _d) = app(None);
        let put = |n: &str, b: Value| {
            let app = app.clone();
            let uri = format!("/features/overrides/{n}");
            async move { call(&app, "PUT", &uri, b, None).await }
        };
        let (st, _) = put("plugin_webhooks", json!({"value": true, "reason": "r"})).await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "off_only refuses on");
        let (st, _) = put("nope", json!({"value": false, "reason": "r"})).await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "unknown flag");
        let (st, _) = put("plugin_webhooks", json!({"value": false})).await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "a reason is required");
        let (st, _) = put("rate_limit", json!({"value": false, "reason": "r"})).await;
        assert_eq!(
            st,
            StatusCode::UNPROCESSABLE_ENTITY,
            "protection needs an expiry"
        );
        let (st, _) = put(
            "rate_limit",
            json!({"value": false, "reason": "r", "expires_at": 1}),
        )
        .await;
        assert_eq!(
            st,
            StatusCode::UNPROCESSABLE_ENTITY,
            "an expiry in the past"
        );
        let soon = now() + 600;
        let (st, _) = put(
            "rate_limit",
            json!({"value": false, "reason": "r", "expires_at": soon}),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
    }

    #[tokio::test]
    async fn a_ttl_is_turned_into_an_expiry_by_the_controller_and_a_protection_flag_is_capped() {
        let (app, _s, _d) = app(None);
        let put = |b: Value| {
            let app = app.clone();
            async move { call(&app, "PUT", "/features/overrides/rate_limit", b, None).await }
        };
        let before = now();
        let (st, got) = put(json!({"value": false, "reason": "r", "ttl_secs": 600})).await;
        assert_eq!(st, StatusCode::OK, "{got}");
        let exp = got["expires_at"].as_u64().unwrap();
        assert!((before + 600..=now() + 600).contains(&exp), "{exp}");
        let both =
            json!({"value": false, "reason": "r", "ttl_secs": 60, "expires_at": before + 60});
        assert_eq!(put(both).await.0, StatusCode::UNPROCESSABLE_ENTITY);
        let zero = json!({"value": false, "reason": "r", "ttl_secs": 0});
        assert_eq!(put(zero).await.0, StatusCode::UNPROCESSABLE_ENTITY);
        let long = json!({"value": false, "reason": "r", "ttl_secs": 8 * 24 * 3600});
        let (st, got) = put(long).await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "{got}");
        let forever = json!({"value": false, "reason": "r", "expires_at": u64::MAX});
        assert_eq!(put(forever).await.0, StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[derive(Clone, Default)]
    struct Log(Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for Log {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Log {
        type Writer = Log;
        fn make_writer(&'a self) -> Log {
            self.clone()
        }
    }

    #[tokio::test]
    async fn setting_and_deleting_an_override_leave_an_audit_line() {
        let log = Log::default();
        let _g = tracing::subscriber::set_default(
            tracing_subscriber::fmt()
                .with_writer(log.clone())
                .with_ansi(false)
                .finish(),
        );
        let (app, _s, _d) = app(None);
        let uri = "/features/overrides/plugin_webhooks";
        call(
            &app,
            "PUT",
            uri,
            json!({"value": false, "reason": "storm"}),
            None,
        )
        .await;
        call(&app, "DELETE", uri, Value::Null, None).await;
        let out = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
        assert!(
            out.contains("feature override set") && out.contains("storm"),
            "{out}"
        );
        assert!(out.contains("feature override deleted"), "{out}");
    }

    #[tokio::test]
    async fn an_expired_override_no_longer_applies_and_features_need_the_token() {
        let (app, store, _d) = app(Some("t"));
        let rec = json!({"value": false, "reason": "r", "set_at": 1, "expires_at": 2});
        store
            .apply_local(&UiOp::Put {
                kind: Kind::FeatureOverride,
                name: "plugin_webhooks".into(),
                value: rec,
            })
            .unwrap();
        let (st, _) = call(&app, "GET", "/features", Value::Null, None).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);
        let (_, v) = call(&app, "GET", "/features", Value::Null, Some("t")).await;
        assert_eq!(flag(&v, "plugin_webhooks")["value"], true);
    }

    #[tokio::test]
    async fn a_dashboard_is_saved_listed_exported_and_deleted() {
        let (app, _s, _d) = app(None);
        let (st, _) = call(&app, "PUT", "/ui/dashboards/ops", dash(), None).await;
        assert_eq!(st, StatusCode::OK);
        let (st, got) = call(&app, "GET", "/ui/dashboards/ops", Value::Null, None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(got["name"], "ops", "the export shape carries the name");
        assert_eq!(got["widgets"], dash()["widgets"]);
        let (_, list) = call(&app, "GET", "/ui/dashboards", Value::Null, None).await;
        assert_eq!(list["items"].as_array().unwrap().len(), 1);
        assert_eq!(list["items"][0]["name"], "ops");
        let (st, _) = call(&app, "DELETE", "/ui/dashboards/ops", Value::Null, None).await;
        assert_eq!(st, StatusCode::NO_CONTENT);
        let (st, _) = call(&app, "GET", "/ui/dashboards/ops", Value::Null, None).await;
        assert_eq!(st, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn the_path_name_wins_over_a_name_in_the_body() {
        let (app, store, _d) = app(None);
        let mut body = dash();
        body["name"] = json!("something else");
        call(&app, "PUT", "/ui/dashboards/ops", body, None).await;
        let stored = store
            .get(super::super::Kind::Dashboard, "ops")
            .unwrap()
            .unwrap();
        assert_eq!(stored["name"], "ops");
    }

    #[tokio::test]
    async fn a_malformed_or_oversize_import_is_a_422_naming_the_problem() {
        let (app, _s, _d) = app(None);
        let bad = json!({"widgets": [{"id": "nodes", "width": 99}]});
        let (st, body) = call(&app, "PUT", "/ui/dashboards/ops", bad, None).await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            body["error"].as_str().unwrap().contains("widgets[0].width"),
            "{body}"
        );
        let (st, _) = call(&app, "PUT", "/ui/dashboards/ops", json!([1, 2]), None).await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn a_theme_that_fails_the_contrast_checks_is_a_422_naming_the_pair() {
        let (app, store, _d) = app(None);
        let pale = json!({"base": "light", "tokens": {"ink": "#bbbbbb"}});
        let (st, body) = call(&app, "PUT", "/ui/themes/pale", pale, None).await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert!(
            body["error"].as_str().unwrap().contains("ink on bg is"),
            "{body}"
        );
        let failed: Vec<_> = body["failures"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["check"].as_u64().unwrap())
            .collect();
        assert_eq!(failed, [1, 1, 1, 4]);
        assert_eq!(body["failures"][0]["pair"], json!(["ink", "bg"]));
        assert!(store
            .get(super::super::Kind::Theme, "pale")
            .unwrap()
            .is_none());
        // A theme that passes is saved.
        let night = json!({"base": "dark", "tokens": {"bg": "#101216", "select": "#7fb0ff"}});
        let (st, body) = call(&app, "PUT", "/ui/themes/night", night, None).await;
        assert_eq!(st, StatusCode::OK, "{body}");
    }

    #[tokio::test]
    async fn a_full_collection_is_a_409() {
        let (app, _s, _d) = app(None);
        for i in 0..50 {
            let t = json!({"base": "dark", "tokens": {}});
            let (st, _) = call(&app, "PUT", &format!("/ui/themes/t{i}"), t, None).await;
            assert_eq!(st, StatusCode::OK);
        }
        let (st, _) = call(
            &app,
            "PUT",
            "/ui/themes/extra",
            json!({"base": "dark", "tokens": {}}),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn the_layout_override_and_the_defaults_are_single_records() {
        let (app, _s, _d) = app(None);
        let (st, _) = call(&app, "GET", "/ui/layout", Value::Null, None).await;
        assert_eq!(
            st,
            StatusCode::NOT_FOUND,
            "no record means use the shipped default"
        );
        let layout = json!({"version": 1, "fields": {"/settings/group": {"order": 1}}});
        assert_eq!(
            call(&app, "PUT", "/ui/layout", layout.clone(), None)
                .await
                .0,
            StatusCode::OK
        );
        assert_eq!(
            call(&app, "GET", "/ui/layout", Value::Null, None).await.1,
            layout
        );
        assert_eq!(
            call(&app, "DELETE", "/ui/layout", Value::Null, None)
                .await
                .0,
            StatusCode::NO_CONTENT
        );

        let (_, d) = call(&app, "GET", "/ui/defaults", Value::Null, None).await;
        assert_eq!(d, json!({}), "no defaults set reads as an empty object");
        let (st, body) = call(
            &app,
            "PUT",
            "/ui/defaults",
            json!({"dashboard": "ghost"}),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    }

    #[tokio::test]
    async fn every_route_needs_the_bearer_token_when_one_is_set() {
        let (app, _s, _d) = app(Some("secret"));
        assert_eq!(
            call(&app, "GET", "/ui/dashboards", Value::Null, None)
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            call(&app, "PUT", "/ui/layout", json!({}), Some("wrong"))
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            call(&app, "GET", "/ui/dashboards", Value::Null, Some("secret"))
                .await
                .0,
            StatusCode::OK
        );
    }

    const VALUE: &str = "correct-horse-battery-staple";

    #[tokio::test]
    async fn a_cluster_secret_is_stored_as_ciphertext_and_listed_by_name_only() {
        let (app, store, _d) = app(None);
        let (st, got) = call(
            &app,
            "PUT",
            "/secrets/gossip-psk",
            json!({"value": VALUE}),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{got}");
        assert_eq!(got["name"], "gossip-psk");
        assert!(!got.to_string().contains(VALUE));
        let (_, list) = call(&app, "GET", "/secrets", Value::Null, None).await;
        assert_eq!(list["items"][0]["name"], "gossip-psk");
        assert!(
            list["items"][0].get("ciphertext").is_none() && list["items"][0].get("nonce").is_none()
        );
        assert!(!list.to_string().contains(VALUE));
        // Nothing the store keeps holds the plaintext.
        let stored = store
            .get(Kind::ClusterSecret, "gossip-psk")
            .unwrap()
            .unwrap()
            .to_string();
        assert!(!stored.contains(VALUE), "{stored}");
    }

    #[tokio::test]
    async fn rotation_rewraps_cluster_secrets_so_the_old_key_can_be_retired() {
        const KEY2: &str = "AgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgI=";
        let (app, store, _d) = app(None);
        let put = json!({"value": VALUE});
        call(&app, "PUT", "/secrets/gossip-psk", put, None).await;
        // Step 1: the new key is appended to the file.
        let both = wayhouse_secrets::Keyring::parse(&format!("{KEY}\n{KEY2}")).unwrap();
        let rotated = router(UiState {
            store: store.clone(),
            auth_token: None,
            ha: None,
            keyring: wayhouse_secrets::KeyringHandle::new(both),
        });
        // Step 2: rewrap.
        let (st, r) = call(&rotated, "POST", "/secrets/rewrap", Value::Null, None).await;
        assert_eq!(st, StatusCode::OK, "{r}");
        assert_eq!(r["rewrapped"], 1, "{r}");
        let (_, list) = call(&rotated, "GET", "/secrets", Value::Null, None).await;
        assert_eq!(list["items"][0]["key_id"], r["active_key_id"]);
        assert!(list["items"][0]["set_at"].as_u64().is_some());
        let (_, again) = call(&rotated, "POST", "/secrets/rewrap", Value::Null, None).await;
        assert_eq!(again["rewrapped"], 0, "{again}");
        // Step 3: a proxy holding only the new key still opens it.
        let (_, sealed) = call(
            &rotated,
            "GET",
            "/secrets/gossip-psk/sealed",
            Value::Null,
            None,
        )
        .await;
        let sealed: wayhouse_secrets::Sealed = serde_json::from_value(sealed).unwrap();
        let new_only = wayhouse_secrets::Keyring::parse(KEY2).unwrap();
        assert_eq!(
            new_only
                .open_cluster("gossip-psk", &sealed)
                .unwrap()
                .expose(),
            VALUE.as_bytes()
        );
    }

    #[tokio::test]
    async fn rewrap_needs_a_key_and_the_controller_token() {
        let (app, _s, _d) = app_with(Some("t"), wayhouse_secrets::Keyring::default());
        let (st, _) = call(&app, "POST", "/secrets/rewrap", Value::Null, None).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);
        let (st, _) = call(&app, "POST", "/secrets/rewrap", Value::Null, Some("t")).await;
        assert_eq!(st, StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn the_sealed_form_opens_with_the_key_and_only_under_its_own_name() {
        let (app, _s, _d) = app(None);
        call(
            &app,
            "PUT",
            "/secrets/gossip-psk",
            json!({"value": VALUE}),
            None,
        )
        .await;
        let (st, sealed) = call(&app, "GET", "/secrets/gossip-psk/sealed", Value::Null, None).await;
        assert_eq!(st, StatusCode::OK, "{sealed}");
        let sealed: wayhouse_secrets::Sealed = serde_json::from_value(sealed).unwrap();
        let ring = wayhouse_secrets::Keyring::parse(KEY).unwrap();
        assert_eq!(
            ring.open_cluster("gossip-psk", &sealed).unwrap().expose(),
            VALUE.as_bytes()
        );
        assert!(ring.open_cluster("admin-token", &sealed).is_err());
        let (st, _) = call(&app, "GET", "/secrets/nope/sealed", Value::Null, None).await;
        assert_eq!(st, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn a_secret_can_be_replaced_and_deleted() {
        let (app, _s, _d) = app(None);
        call(
            &app,
            "PUT",
            "/secrets/admin-token",
            json!({"value": VALUE}),
            None,
        )
        .await;
        let (st, _) = call(
            &app,
            "PUT",
            "/secrets/admin-token",
            json!({"value": "another-long-value"}),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        let (_, list) = call(&app, "GET", "/secrets", Value::Null, None).await;
        assert_eq!(list["items"].as_array().unwrap().len(), 1);
        let (st, _) = call(&app, "DELETE", "/secrets/admin-token", Value::Null, None).await;
        assert_eq!(st, StatusCode::NO_CONTENT);
        let (_, list) = call(&app, "GET", "/secrets", Value::Null, None).await;
        assert!(list["items"].as_array().unwrap().is_empty());
        let (st, _) = call(&app, "DELETE", "/secrets/admin-token", Value::Null, None).await;
        assert_eq!(st, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn bad_requests_are_refused_without_echoing_the_value() {
        let (app, _s, _d) = app(None);
        let (st, _) = call(
            &app,
            "PUT",
            "/secrets/Bad_Name",
            json!({"value": VALUE}),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        let (st, got) = call(
            &app,
            "PUT",
            "/secrets/short",
            json!({"value": "tiny"}),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        assert!(got.to_string().contains("to 4096 bytes"));
        let (st, got) = call(
            &app,
            "PUT",
            "/secrets/x1",
            json!({"value": VALUE, "extra": 1}),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        assert!(!got.to_string().contains(VALUE), "{got}");
    }

    #[tokio::test]
    async fn without_a_key_a_secret_cannot_be_stored() {
        let (app, store, _d) = app_with(None, wayhouse_secrets::Keyring::default());
        let (st, got) = call(
            &app,
            "PUT",
            "/secrets/gossip-psk",
            json!({"value": VALUE}),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::CONFLICT, "{got}");
        assert!(store.list(Kind::ClusterSecret).unwrap().is_empty());
    }

    #[tokio::test]
    async fn the_secret_routes_need_the_controller_token() {
        let (app, _s, _d) = app(Some("tok-tok-tok-tok"));
        for (m, uri) in [
            ("GET", "/secrets"),
            ("PUT", "/secrets/a1"),
            ("GET", "/secrets/a1/sealed"),
            ("DELETE", "/secrets/a1"),
        ] {
            let (st, _) = call(&app, m, uri, json!({"value": VALUE}), None).await;
            assert_eq!(st, StatusCode::UNAUTHORIZED, "{m} {uri}");
        }
    }

    #[test]
    fn the_store_refuses_a_cluster_secret_record_of_the_wrong_shape() {
        let bad =
            json!({"key_id": "k", "nonce": "n", "ciphertext": "c", "set_at": 1, "plaintext": "x"});
        assert!(crate::ui_data::validate(Kind::ClusterSecret, "ok-name", &bad).is_err());
        let good = json!({"key_id": "k", "nonce": "n", "ciphertext": "c", "set_at": 1});
        assert!(crate::ui_data::validate(Kind::ClusterSecret, "ok-name", &good).is_ok());
        assert!(crate::ui_data::validate(Kind::ClusterSecret, "Not Ok", &good).is_err());
    }
}
