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
use axum::routing::{delete, get, post};
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
        .route("/api/acme/sites", get(acme_sites))
        .route("/api/acme/gateways", get(acme_gateways))
        .route("/api/acme/account", get(acme_account))
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
        .route("/api/tunnel/origins/{name}", delete(release_origin))
        .route("/api/tunnel/proxies/{name}", delete(release_proxy))
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
}

/// The ACME status page is read-only: sites with their certificate and last error, the
/// gateways that may fetch them, and the CA account. Writes stay in the controller's API.
async fn acme_sites(State(state): State<AppState>) -> Response {
    proxy(&state, Method::GET, "/acme/sites".to_string(), None, None).await
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

async fn plugin_action(
    state: &AppState,
    method: Method,
    id: &str,
    suffix: &str,
    actor: Option<String>,
) -> Response {
    let Some(segment) = crate::proxy_util::path_segment(id) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "invalid plugin id".into(),
            }),
        )
            .into_response();
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

async fn get_config(State(state): State<AppState>) -> Response {
    proxy(&state, Method::GET, "/config".to_string(), None, None).await
}

async fn config_schema(State(state): State<AppState>) -> Response {
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

async fn revert_config(
    State(state): State<AppState>,
    Extension(Actor(actor)): Extension<Actor>,
) -> Response {
    proxy(
        &state,
        Method::POST,
        "/config/revert".to_string(),
        None,
        actor,
    )
    .await
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

    let url = format!("{}{}", controller.base_url, path_suffix);
    let mut req = state.http.request(method, &url);
    if let Some(token) = &controller.token {
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
        Ok(resp) => crate::proxy_util::relay(resp, "controller").await,
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(ErrorResponse {
                error: format!("controller ({url}): {}", wayhouse_http::error_chain(&e)),
            }),
        )
            .into_response(),
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
                (
                    status,
                    [("content-type", "application/json")],
                    r#"{"ok":true}"#,
                )
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
}
