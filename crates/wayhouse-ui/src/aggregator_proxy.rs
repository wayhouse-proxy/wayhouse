//! Proxies fleet reads and the slice-9 operational verbs to `wayhouse-aggregator`
//! (`--aggregator-url`/`--aggregator-token`) — the browser's session cookie
//! never becomes a bearer token; `wayhouse-ui` holds the aggregator's token
//! itself and presents it server-side on every call. [`viewer_router`]'s
//! reads and [`operator_router`]'s intent verbs are gated separately, at
//! `Role::Viewer` and `Role::Operator` respectively (applied by
//! `crate::api::router`, not here — matches `wayhouse-aggregator::fanout`'s
//! "route definitions only" shape, since this module composes into a router
//! another module finishes building).
//!
//! Thin and stateless, the same as `wayhouse-aggregator::fanout` it calls
//! through to: `wayhouse-ui` decides nothing and stores no intent, it only
//! relays. [`operator_router`]'s verbs each add an `X-Actor` header (phase
//! 12 slice 8, `docs/10` "RBAC and audit (design)") naming the session's
//! username, if any (`crate::auth::Actor`, stashed by
//! `crate::auth::check_role`) — `wayhouse-aggregator` doesn't persist this today
//! (it's stateless by design), but forwards it the same way to each
//! instance's own admin API, where the eventual effect lands.

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Extension, Path, Query, RawQuery, State};
use axum::http::{Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use serde::Serialize;

use crate::api::AppState;
use crate::auth::Actor;

/// `GET` fleet reads — `Role::Viewer`.
pub fn viewer_router() -> Router<AppState> {
    Router::new()
        .route("/api/fleet/pools", get(get_pools))
        .route("/api/fleet/sessions", get(get_sessions))
        .route("/api/fleet/healthz", get(get_healthz))
        .route("/api/fleet/stats", get(get_stats))
        .route("/api/fleet/nodes", get(get_nodes))
        .route("/api/fleet/nodes/resolve", post(resolve_nodes))
        .route(
            "/api/fleet/instances/{instance}/sniffers",
            get(get_instance_sniffers),
        )
}

/// The phase-5 intent verbs, fanned out through the aggregator —
/// `Role::Operator`.
pub fn operator_router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/fleet/instances/{instance}/drain",
            post(drain_instance),
        )
        .route(
            "/api/fleet/instances/{instance}/undrain",
            post(undrain_instance),
        )
        .route("/api/fleet/pools/{pool}/backends", post(add_backend))
        .route(
            "/api/fleet/pools/{pool}/backends/{addr}",
            patch(patch_backend).delete(delete_backend),
        )
        .route("/api/fleet/route-hint", post(route_hint))
        .route(
            "/api/fleet/sniffers",
            post(upload_sniffer).layer(DefaultBodyLimit::max(
                wayhouse_http::MAX_SNIFFER_MODULE_BYTES,
            )),
        )
        .route(
            "/api/fleet/sniffers/{name}",
            axum::routing::delete(delete_sniffer),
        )
        .route(
            "/api/fleet/sniffers/{name}/rollback",
            post(rollback_sniffer),
        )
}

async fn get_pools(State(state): State<AppState>) -> Response {
    proxy(&state, Method::GET, "/fleet/pools", None, None).await
}

async fn get_sessions(State(state): State<AppState>) -> Response {
    proxy(&state, Method::GET, "/fleet/sessions", None, None).await
}

async fn get_healthz(State(state): State<AppState>) -> Response {
    proxy(&state, Method::GET, "/fleet/healthz", None, None).await
}

/// The `range` query rides along unchanged; the aggregator validates it.
async fn get_stats(State(state): State<AppState>, RawQuery(query): RawQuery) -> Response {
    let path = match query {
        Some(q) => format!("/fleet/stats?{q}"),
        None => "/fleet/stats".to_string(),
    };
    proxy(&state, Method::GET, &path, None, None).await
}

/// The paged node list: every filter, sort and page key rides along unchanged.
async fn get_nodes(State(state): State<AppState>, RawQuery(query): RawQuery) -> Response {
    proxy(
        &state,
        Method::GET,
        &with_query("/fleet/nodes", query),
        None,
        None,
    )
    .await
}

/// Which instances a filter selects (a read, so a viewer may ask before an operator acts).
async fn resolve_nodes(State(state): State<AppState>, RawQuery(query): RawQuery) -> Response {
    proxy(
        &state,
        Method::POST,
        &with_query("/fleet/nodes/resolve", query),
        None,
        None,
    )
    .await
}

fn with_query(path: &str, query: Option<String>) -> String {
    match query {
        Some(q) => format!("{path}?{q}"),
        None => path.to_string(),
    }
}

async fn get_instance_sniffers(
    State(state): State<AppState>,
    Extension(Actor(actor)): Extension<Actor>,
    Path(instance): Path<String>,
) -> Response {
    proxy_segments(
        &state,
        Method::GET,
        &["instances", &instance, "sniffers"],
        None,
        actor,
    )
    .await
}

async fn drain_instance(
    State(state): State<AppState>,
    Extension(Actor(actor)): Extension<Actor>,
    Path(instance): Path<String>,
) -> Response {
    proxy_segments(
        &state,
        Method::POST,
        &["instances", &instance, "drain"],
        None,
        actor,
    )
    .await
}

async fn undrain_instance(
    State(state): State<AppState>,
    Extension(Actor(actor)): Extension<Actor>,
    Path(instance): Path<String>,
) -> Response {
    proxy_segments(
        &state,
        Method::POST,
        &["instances", &instance, "undrain"],
        None,
        actor,
    )
    .await
}

async fn add_backend(
    State(state): State<AppState>,
    Extension(Actor(actor)): Extension<Actor>,
    Path(pool): Path<String>,
    body: Bytes,
) -> Response {
    proxy_segments(
        &state,
        Method::POST,
        &["pools", &pool, "backends"],
        Some(body),
        actor,
    )
    .await
}

async fn patch_backend(
    State(state): State<AppState>,
    Extension(Actor(actor)): Extension<Actor>,
    Path((pool, addr)): Path<(String, String)>,
    body: Bytes,
) -> Response {
    proxy_segments(
        &state,
        Method::PATCH,
        &["pools", &pool, "backends", &addr],
        Some(body),
        actor,
    )
    .await
}

async fn delete_backend(
    State(state): State<AppState>,
    Extension(Actor(actor)): Extension<Actor>,
    Path((pool, addr)): Path<(String, String)>,
) -> Response {
    proxy_segments(
        &state,
        Method::DELETE,
        &["pools", &pool, "backends", &addr],
        None,
        actor,
    )
    .await
}

async fn route_hint(
    State(state): State<AppState>,
    Extension(Actor(actor)): Extension<Actor>,
    body: Bytes,
) -> Response {
    proxy(&state, Method::POST, "/fleet/route-hint", Some(body), actor).await
}

#[derive(serde::Deserialize)]
struct SnifferUploadQuery {
    name: String,
}

async fn upload_sniffer(
    State(state): State<AppState>,
    Extension(Actor(actor)): Extension<Actor>,
    Query(q): Query<SnifferUploadQuery>,
    body: Bytes,
) -> Response {
    let Some(name) = crate::proxy_util::path_segment(&q.name) else {
        return bad_segment();
    };
    proxy_raw(
        &state,
        Method::POST,
        &format!("/fleet/sniffers?name={name}"),
        Some(body),
        "application/octet-stream",
        actor,
    )
    .await
}

async fn delete_sniffer(
    State(state): State<AppState>,
    Extension(Actor(actor)): Extension<Actor>,
    Path(name): Path<String>,
) -> Response {
    proxy_segments(&state, Method::DELETE, &["sniffers", &name], None, actor).await
}

async fn rollback_sniffer(
    State(state): State<AppState>,
    Extension(Actor(actor)): Extension<Actor>,
    Path(name): Path<String>,
) -> Response {
    proxy_segments(
        &state,
        Method::POST,
        &["sniffers", &name, "rollback"],
        None,
        actor,
    )
    .await
}

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
}

/// [`proxy`] for `/fleet/{segments…}`, each segment percent-encoded so a decoded
/// path parameter (`..%2F..%2Fmetrics%3F`) stays one segment of the route it was
/// meant for instead of steering the call to another aggregator route.
async fn proxy_segments(
    state: &AppState,
    method: Method,
    segments: &[&str],
    body: Option<Bytes>,
    actor: Option<String>,
) -> Response {
    let mut path = String::from("/fleet");
    for segment in segments {
        let Some(segment) = crate::proxy_util::path_segment(segment) else {
            return bad_segment();
        };
        path.push('/');
        path.push_str(&segment);
    }
    proxy(state, method, &path, body, actor).await
}

fn bad_segment() -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(ErrorResponse {
            error: "invalid path parameter".into(),
        }),
    )
        .into_response()
}

/// Forwards one call to the configured aggregator, passing its response
/// (status + body) straight through. `503` if no `--aggregator-url` was
/// given at all — a configuration gap, not a runtime failure; `502` if the
/// aggregator was configured but couldn't be reached. `actor` (the session's
/// username, if any) rides along as `X-Actor` — phase 12 slice 8's audit
/// trail, see the module doc.
async fn proxy(
    state: &AppState,
    method: Method,
    path_suffix: &str,
    body: Option<Bytes>,
    actor: Option<String>,
) -> Response {
    proxy_raw(state, method, path_suffix, body, "application/json", actor).await
}

/// Like [`proxy`], but lets the caller pick the forwarded body's
/// `content-type` — needed for `/api/fleet/sniffers` uploads, which carry
/// raw `.wasm` bytes rather than JSON.
pub(crate) async fn proxy_raw(
    state: &AppState,
    method: Method,
    path_suffix: &str,
    body: Option<Bytes>,
    content_type: &str,
    actor: Option<String>,
) -> Response {
    let Some(aggregator) = &state.aggregator else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ErrorResponse {
                error: "no aggregator configured (--aggregator-url)".into(),
            }),
        )
            .into_response();
    };

    let url = format!("{}{}", aggregator.base_url, path_suffix);
    let mut req = state.http.request(method, &url);
    if let Some(token) = &aggregator.token {
        req = req.bearer_auth(token);
    }
    if let Some(actor) = actor {
        req = req.header("X-Actor", actor);
    }
    if let Some(body) = body {
        req = req.header("content-type", content_type).body(body);
    }

    match req.send().await {
        Ok(resp) => crate::proxy_util::relay(resp, "aggregator").await,
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(ErrorResponse {
                error: format!("aggregator ({url}): {}", wayhouse_http::error_chain(&e)),
            }),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::header;
    use axum::http::Request;
    use std::sync::{Arc, Mutex};
    use tower::ServiceExt;

    /// `(request body, Authorization header)` from the last request a mock
    /// aggregator received.
    type Captured = Arc<Mutex<Option<(Vec<u8>, Option<String>)>>>;

    /// A minimal stand-in for `wayhouse-aggregator`: one route, a fixed response,
    /// and the last request body + auth header it received.
    async fn spawn_mock_aggregator(
        path: &'static str,
        method: Method,
        status: StatusCode,
        response_body: &'static str,
    ) -> (String, Captured) {
        let captured: Captured = Arc::new(Mutex::new(None));
        let captured_for_handler = captured.clone();
        let handler = move |headers: axum::http::HeaderMap, body: Bytes| {
            let captured = captured_for_handler.clone();
            async move {
                let auth = headers
                    .get(header::AUTHORIZATION)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string);
                *captured.lock().unwrap() = Some((body.to_vec(), auth));
                (status, response_body)
            }
        };
        let method_router = match method {
            Method::GET => axum::routing::get(handler),
            Method::POST => axum::routing::post(handler),
            Method::PATCH => axum::routing::patch(handler),
            Method::DELETE => axum::routing::delete(handler),
            _ => unreachable!("test helper only used with GET/POST/PATCH/DELETE"),
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        // The stand-in must not impose axum's 2 MiB default on top of what is under test.
        let app = Router::new()
            .route(path, method_router)
            .layer(axum::extract::DefaultBodyLimit::disable());
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{addr}"), captured)
    }

    fn logged_in_app(aggregator_url: String, token: Option<String>) -> (Router, String) {
        let state =
            crate::api::AppState::new(Some("secret".into())).with_aggregator(aggregator_url, token);
        let sessions = state.sessions.clone();
        let session_id = sessions.create(crate::session::Session {
            role: crate::role::Role::Admin,
            username: None,
        });
        (crate::api::router(state), session_id)
    }

    fn cookie(session_id: &str) -> String {
        format!("{}={session_id}", crate::api::SESSION_COOKIE)
    }

    #[tokio::test]
    async fn path_parameters_cannot_steer_a_call_to_another_aggregator_route() {
        let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let seen2 = seen.clone();
        let upstream = Router::new().fallback(move |uri: axum::http::Uri| {
            let seen = seen2.clone();
            async move {
                seen.lock().unwrap().push(uri.to_string());
                "{}"
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
        let (app, session) = logged_in_app(format!("http://{addr}"), Some("tok".into()));

        let hostile = [
            "..%2F..%2F..%2Fmetrics%3F",
            "..",
            "%2E%2E",
            "a%2Fb",
            "a%3Fb%23c",
        ];
        for name in hostile {
            for (method, path) in [
                ("GET", format!("/api/fleet/instances/{name}/sniffers")),
                ("POST", format!("/api/fleet/instances/{name}/drain")),
                ("DELETE", format!("/api/fleet/sniffers/{name}")),
                ("POST", format!("/api/fleet/sniffers/{name}/rollback")),
                ("DELETE", format!("/api/fleet/pools/{name}/backends/{name}")),
            ] {
                let resp = app
                    .clone()
                    .oneshot(
                        Request::builder()
                            .method(method)
                            .uri(&path)
                            .header(header::COOKIE, cookie(&session))
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                assert!(
                    resp.status() == StatusCode::BAD_REQUEST || resp.status() == StatusCode::OK,
                    "{method} {path}: {}",
                    resp.status()
                );
            }
        }
        for uri in seen.lock().unwrap().iter() {
            assert!(
                uri.starts_with("/fleet/instances/")
                    || uri.starts_with("/fleet/sniffers/")
                    || uri.starts_with("/fleet/pools/"),
                "escaped to {uri}"
            );
            assert!(!uri.contains("/../") && !uri.contains('?'), "{uri}");
        }
    }

    #[tokio::test]
    async fn sniffer_upload_name_cannot_inject_query_parameters() {
        let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let seen2 = seen.clone();
        let upstream = Router::new().fallback(move |uri: axum::http::Uri| {
            let seen = seen2.clone();
            async move {
                seen.lock().unwrap().push(uri.to_string());
                "{}"
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
        let (app, session) = logged_in_app(format!("http://{addr}"), None);
        let resp = app
            .oneshot(
                Request::post("/api/fleet/sniffers?name=evil%26force%3Dtrue%23x")
                    .header(header::COOKIE, cookie(&session))
                    .body(Body::from("wasm"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert!(!seen[0].contains("&force"), "{}", seen[0]);
        assert!(
            seen[0].contains("name=evil%26force%3Dtrue%23x"),
            "{}",
            seen[0]
        );
    }

    #[tokio::test]
    async fn ui_proxies_rollback_with_the_actor_header_and_mutating_role() {
        let seen: Arc<Mutex<Vec<Option<String>>>> = Arc::new(Mutex::new(Vec::new()));
        let seen2 = seen.clone();
        let upstream = Router::new().route(
            "/fleet/sniffers/demo/rollback",
            axum::routing::post(move |headers: axum::http::HeaderMap| {
                let seen = seen2.clone();
                async move {
                    seen.lock().unwrap().push(
                        headers
                            .get("X-Actor")
                            .and_then(|v| v.to_str().ok())
                            .map(str::to_string),
                    );
                    (StatusCode::OK, r#"{"results":[]}"#)
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
        let state = crate::api::AppState::new(Some("secret".into()))
            .with_aggregator(format!("http://{addr}"), None);
        let sessions = state.sessions.clone();
        let operator = sessions.create(crate::session::Session {
            role: crate::role::Role::Operator,
            username: Some("ann".into()),
        });
        let viewer = sessions.create(crate::session::Session {
            role: crate::role::Role::Viewer,
            username: Some("vic".into()),
        });
        let app = crate::api::router(state);

        let denied = app
            .clone()
            .oneshot(
                Request::post("/api/fleet/sniffers/demo/rollback")
                    .header(header::COOKIE, cookie(&viewer))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(denied.status(), StatusCode::FORBIDDEN);
        assert!(seen.lock().unwrap().is_empty(), "a viewer reaches nothing");

        let ok = app
            .oneshot(
                Request::post("/api/fleet/sniffers/demo/rollback")
                    .header(header::COOKIE, cookie(&operator))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(ok.status(), StatusCode::OK);
        assert_eq!(*seen.lock().unwrap(), vec![Some("ann".to_string())]);
    }

    #[tokio::test]
    async fn upload_sniffer_forwards_a_module_the_proxy_would_accept() {
        // Between axum's 2 MiB default and the proxy's 8 MiB cap (#197).
        let (url, captured) = spawn_mock_aggregator(
            "/fleet/sniffers",
            Method::POST,
            StatusCode::OK,
            r#"{"results":[]}"#,
        )
        .await;
        let (app, session_id) = logged_in_app(url, None);
        let module = vec![7u8; 3 * 1024 * 1024];

        let resp = app
            .oneshot(
                Request::post("/api/fleet/sniffers?name=big")
                    .header(header::COOKIE, cookie(&session_id))
                    .body(Body::from(module.clone()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(captured.lock().unwrap().clone().unwrap().0, module);
    }

    #[tokio::test]
    async fn upload_sniffer_rejects_more_than_the_proxy_cap() {
        let (url, captured) = spawn_mock_aggregator(
            "/fleet/sniffers",
            Method::POST,
            StatusCode::OK,
            r#"{"results":[]}"#,
        )
        .await;
        let (app, session_id) = logged_in_app(url, None);
        let too_big = vec![0u8; wayhouse_http::MAX_SNIFFER_MODULE_BYTES + 1];

        let resp = app
            .oneshot(
                Request::post("/api/fleet/sniffers?name=big")
                    .header(header::COOKIE, cookie(&session_id))
                    .body(Body::from(too_big))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert!(
            captured.lock().unwrap().is_none(),
            "nothing may be forwarded"
        );
    }

    #[tokio::test]
    async fn get_pools_is_gated_by_session() {
        let (url, _captured) =
            spawn_mock_aggregator("/fleet/pools", Method::GET, StatusCode::OK, "[]").await;
        let (app, _session_id) = logged_in_app(url, None);

        let resp = app
            .oneshot(
                Request::get("/api/fleet/pools")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn stats_forwards_the_range_to_the_aggregator() {
        let upstream = Router::new().route(
            "/fleet/stats",
            axum::routing::get(
                |axum::extract::RawQuery(q): axum::extract::RawQuery| async move {
                    q.unwrap_or_default()
                },
            ),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
        let (app, session_id) = logged_in_app(url, None);

        let resp = app
            .oneshot(
                Request::get("/api/fleet/stats?range=1h")
                    .header(header::COOKIE, cookie(&session_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(bytes, "range=1h".as_bytes());
    }

    #[tokio::test]
    async fn the_node_list_and_resolve_forward_the_whole_query() {
        let echo = |path: &'static str, post: bool| {
            let handler = |axum::extract::RawQuery(q): axum::extract::RawQuery| async move {
                q.unwrap_or_default()
            };
            Router::new().route(
                path,
                if post {
                    axum::routing::post(handler)
                } else {
                    axum::routing::get(handler)
                },
            )
        };
        for (method, path, uri) in [
            (
                "GET",
                "/fleet/nodes",
                "/api/fleet/nodes?page=2&tag=env%3Dprod&tag=gpu",
            ),
            (
                "POST",
                "/fleet/nodes/resolve",
                "/api/fleet/nodes/resolve?q=eu&state=degraded",
            ),
        ] {
            let upstream = echo(path, method == "POST");
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
            let (app, session_id) = logged_in_app(url, None);
            let resp = app
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(uri)
                        .header(header::COOKIE, cookie(&session_id))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK, "{uri}");
            let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap();
            assert_eq!(bytes, uri.split_once('?').unwrap().1.as_bytes(), "{uri}");
        }
    }

    #[tokio::test]
    async fn get_pools_proxies_to_the_aggregator_with_its_token() {
        let (url, captured) = spawn_mock_aggregator(
            "/fleet/pools",
            Method::GET,
            StatusCode::OK,
            r#"[{"instance":"a"}]"#,
        )
        .await;
        let (app, session_id) = logged_in_app(url, Some("agg-token".into()));

        let resp = app
            .oneshot(
                Request::get("/api/fleet/pools")
                    .header(header::COOKIE, cookie(&session_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(bytes, r#"[{"instance":"a"}]"#.as_bytes());

        let (_, auth) = captured.lock().unwrap().clone().unwrap();
        assert_eq!(auth.as_deref(), Some("Bearer agg-token"));
    }

    #[tokio::test]
    async fn add_backend_forwards_the_body_and_the_aggregators_status() {
        let (url, captured) = spawn_mock_aggregator(
            "/fleet/pools/local/backends",
            Method::POST,
            StatusCode::OK,
            r#"{"results":[]}"#,
        )
        .await;
        let (app, session_id) = logged_in_app(url, None);

        let resp = app
            .oneshot(
                Request::post("/api/fleet/pools/local/backends")
                    .header(header::COOKIE, cookie(&session_id))
                    .body(Body::from(r#"{"addr":"127.0.0.1:9001"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let (body, _) = captured.lock().unwrap().clone().unwrap();
        assert_eq!(body, r#"{"addr":"127.0.0.1:9001"}"#.as_bytes());
    }

    #[tokio::test]
    async fn drain_instance_hits_the_targeted_path() {
        let (url, _captured) = spawn_mock_aggregator(
            "/fleet/instances/proxy-1/drain",
            Method::POST,
            StatusCode::OK,
            "draining\n",
        )
        .await;
        let (app, session_id) = logged_in_app(url, None);

        let resp = app
            .oneshot(
                Request::post("/api/fleet/instances/proxy-1/drain")
                    .header(header::COOKIE, cookie(&session_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn no_aggregator_configured_is_a_clean_503_not_a_panic() {
        let state = crate::api::AppState::new(None);
        let app = crate::api::router(state);
        let resp = app
            .oneshot(
                Request::get("/api/fleet/pools")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn an_unreachable_aggregator_is_a_clean_502_not_a_panic() {
        let state =
            crate::api::AppState::new(None).with_aggregator("http://127.0.0.1:1".to_string(), None);
        let sessions = state.sessions.clone();
        let session_id = sessions.create(crate::session::Session {
            role: crate::role::Role::Admin,
            username: None,
        });
        let app = crate::api::router(state);

        let resp = app
            .oneshot(
                Request::get("/api/fleet/pools")
                    .header(header::COOKIE, cookie(&session_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    }
}
