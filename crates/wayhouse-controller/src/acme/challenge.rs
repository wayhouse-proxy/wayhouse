//! The HTTP-01 answers: `GET /.well-known/acme-challenge/{token}`.
//!
//! The CA asks the site's own name on port 80, which a gateway terminates; the gateway
//! forwards that path to any controller node, and this route answers from the replicated store
//! while the order that published it is running. There is no authentication on purpose: the
//! key authorization is what the CA is meant to read, a token is 128 bits of the CA's choosing
//! that nobody can guess before the order starts, and an unknown or expired token is a plain
//! 404.

use axum::extract::{Path, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;

use super::{AcmeStore, HttpChallenge};

#[derive(Clone)]
pub struct ChallengeState {
    pub store: AcmeStore,
    pub clock: std::sync::Arc<dyn Fn() -> u64 + Send + Sync>,
}

pub fn router(state: ChallengeState) -> Router {
    Router::new()
        .route("/.well-known/acme-challenge/{token}", get(answer))
        .with_state(state)
}

async fn answer(State(st): State<ChallengeState>, Path(token): Path<String>) -> Response {
    // Nothing that cannot be a stored token reaches the database.
    if !HttpChallenge::valid_token(&token) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let now = (st.clock)();
    match st.store.challenge(&token, now) {
        Ok(Some(c)) => (
            [
                (
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("text/plain; charset=utf-8"),
                ),
                (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
                (
                    header::X_CONTENT_TYPE_OPTIONS,
                    HeaderValue::from_static("nosniff"),
                ),
            ],
            c.key_authorization,
        )
            .into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!(error = %e, "acme: could not read an HTTP-01 answer");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    use super::*;
    use crate::acme::{AcmeOp, Applied, HttpChallenge};

    fn app() -> (Router, AcmeStore, Arc<AtomicU64>) {
        let db = sled::Config::new().temporary(true).open().unwrap();
        let store = AcmeStore::open(&db).unwrap();
        let now = Arc::new(AtomicU64::new(1_000));
        let clock = now.clone();
        let router = router(ChallengeState {
            store: store.clone(),
            clock: Arc::new(move || clock.load(Ordering::SeqCst)),
        });
        (router, store, now)
    }

    async fn get(app: &Router, token: &str) -> (StatusCode, String, Option<String>) {
        let rsp = app
            .clone()
            .oneshot(
                Request::get(format!("/.well-known/acme-challenge/{token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let cache = rsp
            .headers()
            .get(header::CACHE_CONTROL)
            .map(|v| v.to_str().unwrap().to_string());
        let status = rsp.status();
        let body = axum::body::to_bytes(rsp.into_body(), 4096).await.unwrap();
        (status, String::from_utf8(body.to_vec()).unwrap(), cache)
    }

    fn publish(store: &AcmeStore, token: &str, expires_at: u64) {
        let op = AcmeOp::ChallengeSet(HttpChallenge {
            token: token.into(),
            key_authorization: format!("{token}.thumb"),
            expires_at,
        });
        assert_eq!(store.apply_local(&op).unwrap(), Applied::Done);
    }

    #[tokio::test]
    async fn a_published_token_answers_its_key_authorization_until_it_expires() {
        let (app, store, now) = app();
        publish(&store, "tok-1_A", 2_000);
        let (status, body, cache) = get(&app, "tok-1_A").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "tok-1_A.thumb");
        assert_eq!(cache.as_deref(), Some("no-store"));
        assert_eq!(get(&app, "bad.token").await.0, StatusCode::NOT_FOUND);
        assert_eq!(get(&app, &"a".repeat(129)).await.0, StatusCode::NOT_FOUND);
        now.store(2_000, Ordering::SeqCst);
        assert_eq!(get(&app, "tok-1_A").await.0, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn an_unknown_or_withdrawn_token_is_a_404_with_no_body() {
        let (app, store, _) = app();
        assert_eq!(get(&app, "nope").await.0, StatusCode::NOT_FOUND);
        publish(&store, "gone", 5_000);
        store
            .apply_local(&AcmeOp::ChallengeDelete {
                token: "gone".into(),
            })
            .unwrap();
        let (status, body, _) = get(&app, "gone").await;
        assert_eq!((status, body.as_str()), (StatusCode::NOT_FOUND, ""));
    }

    #[test]
    fn only_a_url_safe_token_with_a_matching_key_authorization_is_stored() {
        let (_, store, _) = app();
        let bad = |token: &str, key: &str| {
            store
                .apply_local(&AcmeOp::ChallengeSet(HttpChallenge {
                    token: token.into(),
                    key_authorization: key.into(),
                    expires_at: 9,
                }))
                .unwrap()
        };
        assert!(matches!(bad("a/b", "a/b.x"), Applied::Invalid(_)));
        assert!(matches!(bad("", ".x"), Applied::Invalid(_)));
        assert!(matches!(bad("abc", "other.x"), Applied::Invalid(_)));
        assert!(matches!(bad("abc", "abc"), Applied::Invalid(_)));
        assert!(matches!(bad(&"a".repeat(129), "x"), Applied::Invalid(_)));
        assert_eq!(bad("abc", "abc.x"), Applied::Done);
    }

    #[test]
    fn a_prune_drops_exactly_the_expired_answers_and_a_snapshot_carries_the_rest() {
        let (_, store, _) = app();
        publish(&store, "old", 100);
        publish(&store, "new", 900);
        assert_eq!(
            store
                .apply_local(&AcmeOp::ChallengesPrune { now: 500 })
                .unwrap(),
            Applied::Done
        );
        let tokens: Vec<_> = store
            .challenges()
            .unwrap()
            .into_iter()
            .map(|c| c.token)
            .collect();
        assert_eq!(tokens, ["new"]);
        let snap = store.snapshot().unwrap();
        let db = sled::Config::new().temporary(true).open().unwrap();
        let other = AcmeStore::open(&db).unwrap();
        other.replace(&snap).unwrap();
        assert_eq!(other.challenges().unwrap(), store.challenges().unwrap());
        // A snapshot taken before this level still decodes.
        let mut json = serde_json::to_value(&snap).unwrap();
        json.as_object_mut().unwrap().remove("challenges");
        let old: crate::acme::AcmeSnapshot = serde_json::from_value(json).unwrap();
        assert!(old.challenges.is_empty());
    }
}
