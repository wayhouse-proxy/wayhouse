//! Scoped tokens for the components that talk to the controller but must not administer it.
//!
//! `--auth-token` is the admin token and works everywhere. `--proxy-token` and `--agent-token`
//! each unlock a fixed list of method and path pairs (what that component really calls) and
//! nothing else, so a compromised edge proxy cannot submit config, install plugins or write
//! secrets (#589). [`promote`] runs once, outside every router: a scoped token on an allowed
//! request is exchanged for the admin token, so the per-module bearer layers stay untouched.

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{header, HeaderValue, Method};
use axum::middleware::Next;
use axum::response::Response;

/// What a scoped token may call. `*` stands for exactly one path segment.
type Allowed = &'static [(&'static Method, &'static str)];

const PROXY: Allowed = &[
    (&Method::GET, "/config"),
    (&Method::GET, "/config/subscribe"),
    (&Method::GET, "/secrets/*/sealed"),
    (&Method::GET, "/plugin-routes/subscribe"),
    (&Method::GET, "/intent/subscribe"),
    (&Method::GET, "/peers/subscribe"),
    (&Method::POST, "/proxy-peers"),
];

const AGENT: Allowed = &[
    (&Method::POST, "/peers"),
    (&Method::GET, "/proxy-peers/subscribe"),
];

/// Startup rule: with an admin token, both scoped tokens are required, each at least 16 bytes
/// and all three different, so no component quietly keeps using the admin token.
pub fn check(admin: Option<&str>, proxy: Option<&str>, agent: Option<&str>) -> Result<(), String> {
    wayhouse_http::policy::check_optional_secret("--proxy-token", proxy)?;
    wayhouse_http::policy::check_optional_secret("--agent-token", agent)?;
    let Some(admin) = admin else {
        return match (proxy, agent) {
            (None, None) => Ok(()),
            _ => Err("--proxy-token and --agent-token need --auth-token".into()),
        };
    };
    let (Some(proxy), Some(agent)) = (proxy, agent) else {
        return Err(
            "--auth-token needs --proxy-token and --agent-token too, so proxies and \
                    agents do not hold the admin token (#589)"
                .into(),
        );
    };
    if admin == proxy || admin == agent || proxy == agent {
        return Err("--auth-token, --proxy-token and --agent-token must all differ".into());
    }
    Ok(())
}

#[derive(Clone)]
pub struct ScopedTokens {
    admin: HeaderValue,
    scoped: Vec<(Arc<str>, Allowed)>,
}

impl ScopedTokens {
    /// `None` when there is nothing to promote (no admin token, or no scoped token set).
    pub fn new(admin: Option<&str>, proxy: Option<&str>, agent: Option<&str>) -> Option<Self> {
        let admin = HeaderValue::from_str(&format!("Bearer {}", admin?)).ok()?;
        let scoped: Vec<_> = [(proxy, PROXY), (agent, AGENT)]
            .into_iter()
            .filter_map(|(t, a)| Some((Arc::from(t?), a)))
            .collect();
        (!scoped.is_empty()).then_some(Self { admin, scoped })
    }
}

fn allows(allowed: Allowed, method: &Method, path: &str) -> bool {
    allowed.iter().any(|(m, pattern)| {
        *m == method && {
            let mut got = path.split('/');
            let mut want = pattern.split('/');
            loop {
                match (got.next(), want.next()) {
                    (None, None) => break true,
                    (Some(g), Some("*")) => {
                        if g.is_empty() {
                            break false;
                        }
                    }
                    (Some(g), Some(w)) if g == w => {}
                    _ => break false,
                }
            }
        }
    })
}

pub async fn promote(State(t): State<ScopedTokens>, mut req: Request, next: Next) -> Response {
    let presented = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    let scope = presented.and_then(|p| {
        t.scoped
            .iter()
            .find(|(tok, _)| wayhouse_http::token_eq(p, tok))
    });
    if let Some((_, allowed)) = scope {
        if allows(allowed, req.method(), req.uri().path()) {
            req.headers_mut()
                .insert(header::AUTHORIZATION, t.admin.clone());
        }
    }
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::StatusCode;
    use axum::middleware;
    use axum::routing::any;
    use axum::Router;
    use tower::ServiceExt;

    const ADMIN: &str = "admin-token-0123456789";
    const PROXY_T: &str = "proxy-token-0123456789";
    const AGENT_T: &str = "agent-token-0123456789";

    /// A stand-in for the real routers: everything sits behind the admin bearer check, and the
    /// scoped-token layer wraps the whole thing exactly as `main.rs` does.
    fn app() -> Router {
        let tokens = ScopedTokens::new(Some(ADMIN), Some(PROXY_T), Some(AGENT_T)).unwrap();
        Router::new()
            .route("/{*any}", any(|| async { "ok" }))
            .route_layer(middleware::from_fn_with_state(
                wayhouse_http::server::BearerAuth::new(Some(ADMIN)),
                wayhouse_http::server::require_bearer,
            ))
            .layer(middleware::from_fn_with_state(tokens, promote))
    }

    async fn status(method: &str, path: &str, token: &str) -> StatusCode {
        app()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
            .status()
    }

    #[tokio::test]
    async fn the_proxy_token_reaches_what_a_proxy_calls() {
        for (m, p) in [
            ("GET", "/config"),
            ("GET", "/config/subscribe"),
            ("GET", "/secrets/gossip-psk/sealed"),
            ("GET", "/plugin-routes/subscribe"),
            ("GET", "/intent/subscribe"),
            ("GET", "/peers/subscribe"),
            ("POST", "/proxy-peers"),
        ] {
            assert_eq!(status(m, p, PROXY_T).await, StatusCode::OK, "{m} {p}");
        }
    }

    #[tokio::test]
    async fn the_proxy_token_cannot_administer() {
        for (m, p) in [
            ("POST", "/config"),
            ("POST", "/config/rollback/3"),
            ("GET", "/config/revisions"),
            ("GET", "/secrets"),
            ("PUT", "/secrets/x"),
            ("GET", "/secrets/x"),
            ("GET", "/secrets//sealed"),
            ("GET", "/secrets/a/b/sealed"),
            ("POST", "/plugins"),
            ("GET", "/plugins"),
            ("GET", "/acme/bundles"),
            ("POST", "/admin/adopt"),
            ("POST", "/peers"),
            ("GET", "/proxy-peers/subscribe"),
            ("GET", "/config/"),
        ] {
            assert_eq!(
                status(m, p, PROXY_T).await,
                StatusCode::UNAUTHORIZED,
                "{m} {p}"
            );
        }
    }

    #[tokio::test]
    async fn the_agent_token_is_its_own_scope() {
        assert_eq!(status("POST", "/peers", AGENT_T).await, StatusCode::OK);
        assert_eq!(
            status("GET", "/proxy-peers/subscribe", AGENT_T).await,
            StatusCode::OK
        );
        for (m, p) in [
            ("GET", "/config"),
            ("POST", "/proxy-peers"),
            ("GET", "/secrets/x/sealed"),
        ] {
            assert_eq!(
                status(m, p, AGENT_T).await,
                StatusCode::UNAUTHORIZED,
                "{m} {p}"
            );
        }
    }

    #[tokio::test]
    async fn the_admin_token_and_a_wrong_token_behave_as_before() {
        assert_eq!(status("POST", "/config", ADMIN).await, StatusCode::OK);
        assert_eq!(
            status("GET", "/config", "nope").await,
            StatusCode::UNAUTHORIZED
        );
    }

    #[test]
    fn startup_requires_both_scoped_tokens_with_an_admin_token() {
        let (a, p, g) = (ADMIN, PROXY_T, AGENT_T);
        assert!(check(None, None, None).is_ok());
        assert!(check(Some(a), Some(p), Some(g)).is_ok());
        assert!(check(Some(a), None, Some(g)).is_err());
        assert!(check(Some(a), Some(p), None).is_err());
        assert!(check(None, Some(p), None).is_err());
        assert!(check(Some(a), Some(a), Some(g)).is_err());
        assert!(check(Some(a), Some(p), Some(p)).is_err());
        assert!(check(Some(a), Some("short"), Some(g)).is_err());
    }

    #[test]
    fn nothing_to_promote_without_both_an_admin_and_a_scoped_token() {
        assert!(ScopedTokens::new(None, Some(PROXY_T), None).is_none());
        assert!(ScopedTokens::new(Some(ADMIN), None, None).is_none());
    }
}
