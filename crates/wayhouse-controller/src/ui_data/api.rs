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
}

fn err(status: StatusCode, msg: impl std::fmt::Display) -> Response {
    (status, Json(json!({ "error": msg.to_string() }))).into_response()
}

fn internal(e: impl std::fmt::Display) -> Response {
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
        .route("/features", get(|State(st): State<UiState>| async move { flags(&st) }))
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
                    delete_one(&st, Kind::FeatureOverride, name, uri.path(), &headers).await
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

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// `GET /features`: the controller's flags with the overrides in force.
fn flags(st: &UiState) -> Response {
    match crate::features::active(&st.store, now()) {
        Ok(o) => Json(json!({ "flags": crate::features::effective(&o) })).into_response(),
        Err(e) => internal(e),
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct OverrideBody {
    value: bool,
    reason: String,
    expires_at: Option<u64>,
}

/// `PUT /features/overrides/{name}`: `{value, reason, expires_at?}`; the actor is the
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
    if b.expires_at.is_some_and(|e| e <= set_at) {
        return err(StatusCode::UNPROCESSABLE_ENTITY, "expires_at is already past");
    }
    if let Err(why) = wayhouse_config::features::check_override(&name, b.value, b.expires_at) {
        return err(StatusCode::UNPROCESSABLE_ENTITY, why);
    }
    let mut value = json!({ "value": b.value, "reason": b.reason, "set_at": set_at });
    if let Some(e) = b.expires_at {
        value["expires_at"] = e.into();
    }
    if let Some(a) = headers.get("x-actor").and_then(|v| v.to_str().ok()) {
        value["actor"] = a.into();
    }
    let reply = exported(Kind::FeatureOverride, &name, value.clone());
    let op = UiOp::Put { kind: Kind::FeatureOverride, name, value };
    write(st, op, Method::PUT, path, headers, body, move || Json(reply).into_response()).await
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
        Applied::Replayed => internal("a write was answered as a replay"),
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
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use serde_json::{json, Value};
    use tower::ServiceExt;

    use super::*;

    fn app(token: Option<&str>) -> (Router, UiStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::kv::Db::open(dir.path()).unwrap();
        let store = UiStore::open(&db).unwrap();
        let state = UiState {
            store: store.clone(),
            auth_token: token.map(Arc::from),
            ha: None,
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
        v["flags"].as_array().unwrap().iter().find(|f| f["name"] == name).unwrap()
    }

    #[tokio::test]
    async fn an_override_turns_a_flag_off_until_it_is_deleted() {
        let (app, _s, _d) = app(None);
        let (_, v) = call(&app, "GET", "/features", Value::Null, None).await;
        assert_eq!(flag(&v, "plugin_webhooks")["source"], "default");
        let body = json!({"value": false, "reason": "webhook storm"});
        let (st, got) = call(&app, "PUT", "/features/overrides/plugin_webhooks", body, None).await;
        assert_eq!(st, StatusCode::OK, "{got}");
        assert_eq!(got["reason"], "webhook storm");
        let (_, v) = call(&app, "GET", "/features", Value::Null, None).await;
        assert_eq!(flag(&v, "plugin_webhooks")["value"], false);
        assert_eq!(flag(&v, "plugin_webhooks")["source"], "override");
        let (_, list) = call(&app, "GET", "/features/overrides", Value::Null, None).await;
        assert_eq!(list["items"][0]["name"], "plugin_webhooks");
        let (st, _) = call(&app, "DELETE", "/features/overrides/plugin_webhooks", Value::Null, None).await;
        assert_eq!(st, StatusCode::NO_CONTENT);
        let (_, v) = call(&app, "GET", "/features", Value::Null, None).await;
        assert_eq!(flag(&v, "plugin_webhooks")["value"], true);
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
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "protection needs an expiry");
        let (st, _) = put("rate_limit", json!({"value": false, "reason": "r", "expires_at": 1})).await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "an expiry in the past");
        let soon = now() + 600;
        let (st, _) = put("rate_limit", json!({"value": false, "reason": "r", "expires_at": soon})).await;
        assert_eq!(st, StatusCode::OK);
    }

    #[tokio::test]
    async fn an_expired_override_no_longer_applies_and_features_need_the_token() {
        let (app, store, _d) = app(Some("t"));
        let rec = json!({"value": false, "reason": "r", "set_at": 1, "expires_at": 2});
        store.apply_local(&UiOp::Put { kind: Kind::FeatureOverride, name: "plugin_webhooks".into(), value: rec }).unwrap();
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
}
