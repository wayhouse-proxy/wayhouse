//! The one bearer-token middleware every fleet HTTP server shares
//! (`wayhouse-controller`, `wayhouse-aggregator`, `wayhouse`'s admin API).
//!
//! A single shared secret, not per-caller identity or RBAC. No token
//! configured means the routes are open (network-boundary-only auth, the
//! long-standing posture); the startup rules that refuse or warn about that
//! live in each binary, not here. Wire it in with
//! `axum::middleware::from_fn_with_state(BearerAuth::new(token), require_bearer)`
//! — the middleware state is independent of the router's own state, which is
//! why one function serves every router.

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::rotating::RotatingSecret;

/// The expected token (if any) plus the body sent with the `401`.
#[derive(Clone)]
pub struct BearerAuth {
    expected: Option<Arc<RotatingSecret>>,
    body: &'static str,
}

impl BearerAuth {
    pub fn new(token: Option<&str>) -> Self {
        Self {
            expected: token.map(|t| Arc::new(RotatingSecret::new(t))),
            body: "unauthorized",
        }
    }

    /// A token that can be rotated while the server runs: every request checks the value
    /// `live` holds at that moment.
    pub fn rotating(live: Arc<RotatingSecret>) -> Self {
        Self {
            expected: Some(live),
            body: "unauthorized",
        }
    }

    /// Override the `401` response body (the wayhouse admin API ends it with `\n`).
    pub fn with_body(mut self, body: &'static str) -> Self {
        self.body = body;
        self
    }
}

pub async fn require_bearer(State(auth): State<BearerAuth>, req: Request, next: Next) -> Response {
    let Some(expected) = auth.expected.as_deref() else {
        return next.run(req).await;
    };
    let presented = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    match presented {
        Some(token) if expected.accepts(token) => next.run(req).await,
        _ => (StatusCode::UNAUTHORIZED, auth.body).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::routing::get;
    use axum::Router;
    use tower::ServiceExt;

    fn app(auth: BearerAuth) -> Router {
        Router::new()
            .route("/x", get(|| async { "ok" }))
            .route_layer(axum::middleware::from_fn_with_state(auth, require_bearer))
    }

    async fn call(auth: BearerAuth, header_value: Option<&str>) -> (StatusCode, String) {
        let mut req = Request::builder().uri("/x");
        if let Some(v) = header_value {
            req = req.header(header::AUTHORIZATION, v);
        }
        let resp = app(auth)
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        (status, String::from_utf8(body.to_vec()).unwrap())
    }

    #[tokio::test]
    async fn no_token_configured_is_open() {
        assert_eq!(call(BearerAuth::new(None), None).await.0, StatusCode::OK);
    }

    #[tokio::test]
    async fn correct_token_passes_and_wrong_or_missing_is_401() {
        let auth = || BearerAuth::new(Some("s3cret"));
        assert_eq!(call(auth(), Some("Bearer s3cret")).await.0, StatusCode::OK);
        for h in [
            None,
            Some("Bearer nope"),
            Some("s3cret"),
            Some("Basic s3cret"),
        ] {
            assert_eq!(
                call(auth(), h).await,
                (StatusCode::UNAUTHORIZED, "unauthorized".into())
            );
        }
    }

    #[tokio::test]
    async fn a_rotated_token_works_at_once_and_the_old_one_until_its_window_ends() {
        let live = Arc::new(crate::rotating::RotatingSecret::new("old-token"));
        let auth = BearerAuth::rotating(live.clone());
        assert_eq!(
            call(auth.clone(), Some("Bearer old-token")).await.0,
            StatusCode::OK
        );
        live.rotate("new-token", std::time::Duration::from_secs(3600));
        // The same `BearerAuth` a running router holds sees the swap.
        assert_eq!(
            call(auth.clone(), Some("Bearer new-token")).await.0,
            StatusCode::OK
        );
        assert_eq!(
            call(auth.clone(), Some("Bearer old-token")).await.0,
            StatusCode::OK
        );
        live.rotate("newer-token", std::time::Duration::ZERO);
        for revoked in ["Bearer old-token", "Bearer new-token"] {
            assert_eq!(
                call(auth.clone(), Some(revoked)).await.0,
                StatusCode::UNAUTHORIZED
            );
        }
        assert_eq!(
            call(auth, Some("Bearer newer-token")).await.0,
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn the_401_body_can_be_overridden() {
        let auth = BearerAuth::new(Some("t")).with_body("unauthorized\n");
        assert_eq!(call(auth, None).await.1, "unauthorized\n");
    }
}
