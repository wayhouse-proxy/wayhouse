//! Proxies the controller's `/ui/*` records (dashboards, themes, saved filters, the form layout
//! override and the deployment defaults) to the browser. Viewers read; operators write
//! dashboards and filters; admins write themes, the layout override and the defaults.

use axum::body::Bytes;
use axum::extract::{Extension, Path, State};
use axum::http::{Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

use crate::api::AppState;
use crate::auth::Actor;
use crate::controller_proxy::proxy_typed;

#[derive(Serialize)]
struct ErrorResponse {
    error: &'static str,
}

/// `GET` only: collections, single records and the two single-record resources.
pub fn viewer_router() -> Router<AppState> {
    let mut r = Router::new()
        .route("/api/ui/layout", get(|s| read(s, "layout".into())))
        .route("/api/ui/defaults", get(|s| read(s, "defaults".into())));
    for kind in ["dashboards", "themes", "filters"] {
        r = r
            .route(
                &format!("/api/ui/{kind}"),
                get(move |s| read(s, kind.into())),
            )
            .route(
                &format!("/api/ui/{kind}/{{name}}"),
                get(move |s, Path(name): Path<String>| async move {
                    match crate::proxy_util::path_segment(&name) {
                        Some(seg) => read(s, format!("{kind}/{seg}")).await,
                        None => bad_name(),
                    }
                }),
            );
    }
    r
}

/// Operators save and delete dashboards and saved filters.
pub fn operator_router() -> Router<AppState> {
    named_writes(&["dashboards", "filters"])
}

/// Admins save themes, the form layout override and the deployment defaults.
pub fn admin_router() -> Router<AppState> {
    let mut r = named_writes(&["themes"]);
    for single in ["layout", "defaults"] {
        r = r.route(
            &format!("/api/ui/{single}"),
            axum::routing::put(move |s, a, b| write(s, a, Method::PUT, single.into(), Some(b)))
                .delete(move |s, a| write(s, a, Method::DELETE, single.into(), None)),
        );
    }
    r
}

fn named_writes(kinds: &[&'static str]) -> Router<AppState> {
    let mut r = Router::new();
    for &kind in kinds {
        r = r.route(
            &format!("/api/ui/{kind}/{{name}}"),
            axum::routing::put(move |s, a, Path(name): Path<String>, b: Bytes| async move {
                match crate::proxy_util::path_segment(&name) {
                    Some(seg) => write(s, a, Method::PUT, format!("{kind}/{seg}"), Some(b)).await,
                    None => bad_name(),
                }
            })
            .delete(move |s, a, Path(name): Path<String>| async move {
                match crate::proxy_util::path_segment(&name) {
                    Some(seg) => write(s, a, Method::DELETE, format!("{kind}/{seg}"), None).await,
                    None => bad_name(),
                }
            }),
        );
    }
    r
}

fn bad_name() -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(ErrorResponse {
            error: "invalid record name",
        }),
    )
        .into_response()
}

async fn read(State(state): State<AppState>, path: String) -> Response {
    proxy_typed(&state, Method::GET, format!("/ui/{path}"), None, None, None).await
}

async fn write(
    State(state): State<AppState>,
    Extension(Actor(actor)): Extension<Actor>,
    method: Method,
    path: String,
    body: Option<Bytes>,
) -> Response {
    proxy_typed(
        &state,
        method,
        format!("/ui/{path}"),
        body,
        Some("application/json"),
        actor,
    )
    .await
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axum::body::{Body, Bytes};
    use axum::http::{header, Method, Request, StatusCode};
    use axum::Router;
    use tower::ServiceExt;

    use crate::api::AppState;
    use crate::role::Role;

    type Seen = Arc<Mutex<Vec<(String, String, Vec<u8>)>>>;

    /// A controller stand-in that records `(method, path, body)` of every call.
    async fn mock_controller() -> (String, Seen) {
        let seen: Seen = Arc::default();
        let log = seen.clone();
        let app =
            Router::new().fallback(move |method: Method, uri: axum::http::Uri, body: Bytes| {
                let log = log.clone();
                async move {
                    log.lock().unwrap().push((
                        method.to_string(),
                        uri.path().to_string(),
                        body.to_vec(),
                    ));
                    (StatusCode::OK, "{}")
                }
            });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (url, seen)
    }

    async fn app_and_cookies(url: String) -> (Router, [String; 3]) {
        let users = ["viv", "olly", "ada"]
            .map(|name| {
                let record = crate::users::UserRecord {
                    username: name.into(),
                    // Sessions are minted below; nobody logs in.
                    password_hash: String::new(),
                    role: Role::Viewer,
                };
                (name.to_string(), record)
            })
            .into();
        let state = AppState::new(None)
            .with_users(users)
            .with_controller(url, None);
        let cookies = [
            ("viv", Role::Viewer),
            ("olly", Role::Operator),
            ("ada", Role::Admin),
        ]
        .map(|(name, role)| {
            let id = state.sessions.create(crate::session::Session {
                role,
                username: Some(name.into()),
            });
            format!("{}={id}", crate::api::SESSION_COOKIE)
        });
        (crate::api::router(state), cookies)
    }

    async fn call(app: &Router, cookie: &str, method: &str, uri: &str, body: &str) -> StatusCode {
        app.clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .header(header::COOKIE, cookie)
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap()
            .status()
    }

    #[tokio::test]
    async fn viewers_read_operators_write_dashboards_admins_write_the_rest() {
        let (url, seen) = mock_controller().await;
        let (app, [viewer, operator, admin]) = app_and_cookies(url).await;
        let ok = StatusCode::OK;
        let no = StatusCode::FORBIDDEN;
        let dash = r#"{"widgets":[]}"#;

        for p in [
            "dashboards",
            "dashboards/ops",
            "themes",
            "themes/x",
            "filters",
            "filters/f",
            "layout",
            "defaults",
        ] {
            assert_eq!(
                call(&app, &viewer, "GET", &format!("/api/ui/{p}"), "").await,
                ok,
                "viewer GET {p}"
            );
        }
        assert_eq!(
            call(&app, &viewer, "PUT", "/api/ui/dashboards/ops", dash).await,
            no
        );
        assert_eq!(
            call(&app, &operator, "PUT", "/api/ui/dashboards/ops", dash).await,
            ok
        );
        assert_eq!(
            call(&app, &operator, "DELETE", "/api/ui/filters/f", "").await,
            ok
        );
        for p in ["themes/x", "layout", "defaults"] {
            let uri = format!("/api/ui/{p}");
            assert_eq!(
                call(&app, &operator, "PUT", &uri, "{}").await,
                no,
                "operator PUT {p}"
            );
            assert_eq!(
                call(&app, &operator, "DELETE", &uri, "").await,
                no,
                "operator DELETE {p}"
            );
            assert_eq!(
                call(&app, &admin, "PUT", &uri, "{}").await,
                ok,
                "admin PUT {p}"
            );
            assert_eq!(
                call(&app, &admin, "DELETE", &uri, "").await,
                ok,
                "admin DELETE {p}"
            );
        }
        let seen = seen.lock().unwrap();
        assert!(seen.contains(&(
            "PUT".into(),
            "/ui/dashboards/ops".into(),
            dash.as_bytes().to_vec()
        )));
    }

    #[tokio::test]
    async fn a_record_name_stays_one_path_segment() {
        let (url, seen) = mock_controller().await;
        let (app, [_, operator, _]) = app_and_cookies(url).await;
        assert_eq!(
            call(
                &app,
                &operator,
                "PUT",
                "/api/ui/dashboards/Night%20shift",
                "{}"
            )
            .await,
            StatusCode::OK
        );
        assert_eq!(
            call(
                &app,
                &operator,
                "PUT",
                "/api/ui/dashboards/..%2Fconfig",
                "{}"
            )
            .await,
            StatusCode::OK
        );
        assert_eq!(
            call(&app, &operator, "PUT", "/api/ui/dashboards/..", "{}").await,
            StatusCode::BAD_REQUEST
        );
        let paths: Vec<String> = seen.lock().unwrap().iter().map(|c| c.1.clone()).collect();
        assert_eq!(
            paths,
            vec!["/ui/dashboards/Night%20shift", "/ui/dashboards/..%2Fconfig"]
        );
    }
}
