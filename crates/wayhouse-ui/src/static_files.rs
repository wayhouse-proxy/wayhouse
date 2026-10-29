//! The built frontend (`web/dist`): files as they are, `index.html` for any other page path so a
//! reload or a pasted link on a client route (`/settings/source`) opens the app, and a plain 404
//! for unknown API paths instead of a page.

use std::path::Path;

use axum::http::StatusCode;
use axum::routing::any;
use axum::Router;
use tower_http::services::{ServeDir, ServeFile};

/// Prefixes the BFF owns: an unknown path below one is an error, never the app.
const API_PREFIXES: [&str; 3] = ["/api", "/ui", "/ws"];

pub fn service(dir: &Path) -> Router {
    let app = ServeDir::new(dir).fallback(ServeFile::new(dir.join("index.html")));
    API_PREFIXES
        .iter()
        .fold(Router::new(), |r, p| {
            r.route(
                &format!("{p}/{{*rest}}"),
                any(|| async { StatusCode::NOT_FOUND }),
            )
        })
        .fallback_service(app)
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
        for api in ["/api/nope", "/ui/nope", "/ws/nope"] {
            assert_eq!(get(&app, api).await.0, StatusCode::NOT_FOUND, "{api}");
        }
    }
}
