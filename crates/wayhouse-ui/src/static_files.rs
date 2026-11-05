//! The built frontend (`web/dist`): files as they are, `index.html` for any other page path so a
//! reload or a pasted link on a client route (`/settings/source`) opens the app, and a plain 404
//! for unknown API paths instead of a page.

use std::path::Path;

use axum::body::Body;
use axum::http::{header, HeaderValue, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::Router;
use tower::ServiceExt;
use tower_http::services::{ServeDir, ServeFile};

/// Prefixes the BFF owns: an unknown path below one is an error, never the app.
const API_PREFIXES: [&str; 3] = ["/api", "/ui", "/ws"];

/// Hashed build output: a file's name changes with its content, so it may be cached for good.
const IMMUTABLE: &str = "public, max-age=31536000, immutable";

/// A path that names a file (its last segment has an extension) is a file or a 404; only a page path
/// gets the app. Otherwise a stale `/assets/index-OLDHASH.js` after an upgrade would be served the
/// app as `text/html` with status 200.
fn names_a_file(path: &str) -> bool {
    path.rsplit('/').next().is_some_and(|last| last.contains('.'))
}

pub fn service(dir: &Path) -> Router {
    let files = ServeDir::new(dir);
    let pages = ServeDir::new(dir).fallback(ServeFile::new(dir.join("index.html")));
    let app = any(move |req: Request<Body>| {
        let (files, pages) = (files.clone(), pages.clone());
        async move {
            let path = req.uri().path().to_owned();
            let resp: Response = if names_a_file(&path) {
                files.oneshot(req).await.into_response()
            } else {
                pages.oneshot(req).await.into_response()
            };
            cached(&path, resp)
        }
    });
    API_PREFIXES
        .iter()
        .fold(Router::new(), |r, p| {
            r.route(
                &format!("{p}/{{*rest}}"),
                any(|| async { StatusCode::NOT_FOUND }),
            )
        })
        .fallback(app)
}

/// `index.html` and the page fallback must be revalidated, or an upgrade would keep serving the old app
/// shell; what is under `/assets/` is named by content hash.
fn cached(path: &str, mut resp: Response) -> Response {
    let value = if path.starts_with("/assets/") {
        if !resp.status().is_success() {
            return resp;
        }
        IMMUTABLE
    } else {
        "no-cache"
    };
    resp.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static(value));
    resp
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn get(app: &Router, path: &str) -> (StatusCode, String) {
        let resp = app
            .clone()
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        (
            status,
            String::from_utf8(
                resp.into_body()
                    .collect()
                    .await
                    .unwrap()
                    .to_bytes()
                    .to_vec(),
            )
            .unwrap(),
        )
    }

    async fn cache_control(app: &Router, path: &str) -> Option<String> {
        let resp = app
            .clone()
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        resp.headers()
            .get(header::CACHE_CONTROL)
            .map(|v| v.to_str().unwrap().to_owned())
    }

    #[tokio::test]
    async fn the_app_shell_is_revalidated_and_hashed_assets_are_cached_for_good() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("index.html"), "<app>").unwrap();
        std::fs::create_dir(dir.path().join("assets")).unwrap();
        std::fs::write(dir.path().join("assets/a.js"), "js").unwrap();
        let app = service(dir.path());

        assert_eq!(cache_control(&app, "/").await.as_deref(), Some("no-cache"));
        assert_eq!(cache_control(&app, "/settings/source").await.as_deref(), Some("no-cache"));
        assert_eq!(cache_control(&app, "/assets/a.js").await.as_deref(), Some(IMMUTABLE));
        // A missing asset must not be remembered as immutable.
        assert_eq!(cache_control(&app, "/assets/missing.js").await, None);
    }

    #[tokio::test]
    async fn pages_fall_back_to_the_app_and_api_paths_do_not() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("index.html"), "<app>").unwrap();
        std::fs::create_dir(dir.path().join("assets")).unwrap();
        std::fs::write(dir.path().join("assets/a.js"), "js").unwrap();
        let app = service(dir.path());

        assert_eq!(get(&app, "/").await, (StatusCode::OK, "<app>".into()));
        assert_eq!(
            get(&app, "/assets/a.js").await,
            (StatusCode::OK, "js".into())
        );
        for page in ["/settings", "/settings/source", "/plugins/0123abcd"] {
            assert_eq!(
                get(&app, page).await,
                (StatusCode::OK, "<app>".into()),
                "{page}"
            );
        }
        // A file name that does not exist is a 404, not the app; and nothing else is cached wrongly.
        assert_eq!(get(&app, "/assets/missing.js").await.0, StatusCode::NOT_FOUND);
        assert_eq!(get(&app, "/favicon.ico").await.0, StatusCode::NOT_FOUND);
        for api in ["/api/nope", "/ui/nope", "/ws/nope"] {
            assert_eq!(get(&app, api).await.0, StatusCode::NOT_FOUND, "{api}");
        }
    }
}
