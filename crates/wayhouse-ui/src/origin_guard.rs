//! Browser cross-site defences for everything `wayhouse-ui` serves besides the
//! static frontend: a CSRF check on state-changing requests and WebSocket
//! upgrades, and (in open mode) a `Host` check against DNS rebinding.
//!
//! Open mode (no `--ui-password` / `--users-file`) is loopback-only by policy
//! (`wayhouse_http::policy::check_exposure`), which is exactly the setting where
//! the operator's own browser is the attacker's vehicle: any page they visit can
//! `fetch("http://127.0.0.1:…/api/config/rollback/1", { method: "POST" })`, and
//! a text/plain "simple request" needs no preflight. Authenticated mode is
//! protected by the `SameSite=Lax` session cookie, but the same checks cost
//! nothing there and also cover the WebSocket upgrade.
//!
//! The rules, for an unsafe method (anything but `GET`/`HEAD`/`OPTIONS`) or a
//! WebSocket upgrade:
//!
//! * `Sec-Fetch-Site`, when the browser sends it, must be `same-origin` or
//!   `none` (a user-typed URL); `cross-site` and `same-site` are refused.
//! * `Origin`, when present and `Sec-Fetch-Site` is absent, must name the host the
//!   request was addressed to. `null` is refused. When `Sec-Fetch-Site` says
//!   `same-origin`/`none` the browser has already decided, so a `Host` rewritten by a
//!   reverse proxy cannot turn a legitimate request into a 403.
//!
//! A request with neither header (curl, scripts, the e2e tests) is not a
//! browser-driven cross-site request and passes. With [`crate::api::AppState::loopback_host_only`]
//! set, *every* request's `Host` must additionally be a loopback name or
//! address, which defeats DNS rebinding of an attacker's own name to 127.0.0.1.

use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::api::AppState;

pub async fn origin_guard(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let host = request_host(&req);
    if state.loopback_host_only && !host.as_deref().is_some_and(is_loopback_host) {
        return refuse(&unexpected_host(host.as_deref()));
    }
    if is_state_changing(&req) {
        if let Some(why) = cross_site(req.headers(), host.as_deref()) {
            return refuse(why);
        }
    }
    next.run(req).await
}

fn refuse(why: &str) -> Response {
    (StatusCode::FORBIDDEN, why.to_string()).into_response()
}

/// The 403 text for a non-loopback `Host` on a loopback-only open-mode UI: says
/// which header was seen and how to fix it. The host is client-supplied, so it is
/// cut to printable ASCII of bounded length before it is echoed.
fn unexpected_host(host: Option<&str>) -> String {
    let seen: String = host
        .unwrap_or("")
        .chars()
        .filter(char::is_ascii_graphic)
        .take(100)
        .collect();
    format!(
        "unexpected Host header \"{seen}\": this UI has no login configured, so it only \
         answers loopback names (localhost, 127.0.0.1, [::1]). Set --ui-password or \
         --users-file to serve other hosts, or have the reverse proxy send `Host: localhost`."
    )
}

fn is_state_changing(req: &Request) -> bool {
    let safe = matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS);
    let upgrade = req
        .headers()
        .get(header::UPGRADE)
        .is_some_and(|v| v.as_bytes().eq_ignore_ascii_case(b"websocket"));
    !safe || upgrade
}

/// The authority the client addressed: the `Host` header, or the URI's authority
/// (HTTP/2 carries `:authority` there instead).
fn request_host(req: &Request) -> Option<String> {
    req.headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
        .or_else(|| req.uri().authority().map(|a| a.as_str().to_string()))
}

/// Why `headers` mark a cross-site request, if they do.
fn cross_site(headers: &HeaderMap, host: Option<&str>) -> Option<&'static str> {
    if let Some(site) = headers.get("sec-fetch-site") {
        if !matches!(site.as_bytes(), b"same-origin" | b"none") {
            return Some("cross-site request refused");
        }
        // The browser vouches for the request; a Host a reverse proxy rewrote must
        // not turn it into a 403, so the Origin/Host comparison is the fallback only.
        return None;
    }
    if let Some(origin) = headers.get(header::ORIGIN) {
        let Ok(origin) = origin.to_str() else {
            return Some("cross-origin request refused");
        };
        let authority = origin
            .split_once("://")
            .map(|(_, rest)| rest.trim_end_matches('/'));
        match (authority, host) {
            (Some(a), Some(h)) if a.eq_ignore_ascii_case(h) => {}
            _ => return Some("cross-origin request refused"),
        }
    }
    None
}

/// `localhost`, `*.localhost` and loopback IP literals, with or without a port.
fn is_loopback_host(authority: &str) -> bool {
    let host = if let Some(rest) = authority.strip_prefix('[') {
        rest.split(']').next().unwrap_or_default()
    } else {
        authority.split(':').next().unwrap_or_default()
    };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host == "localhost" || host.ends_with(".localhost") {
        return true;
    }
    host.parse::<std::net::IpAddr>()
        .is_ok_and(|ip| ip.to_canonical().is_loopback())
}

/// Adds the response headers that keep the UI from being framed, sniffed or
/// leaking its URLs: a same-origin-only CSP (`frame-ancestors 'none'` is the
/// anti-clickjacking part), `nosniff`, and no `Referer`. Applied to the whole
/// app, static frontend included.
pub fn with_security_headers(router: axum::Router) -> axum::Router {
    use axum::http::HeaderValue;
    use tower_http::set_header::SetResponseHeaderLayer;
    let set = |name: &'static str, value: &'static str| {
        SetResponseHeaderLayer::overriding(
            header::HeaderName::from_static(name),
            HeaderValue::from_static(value),
        )
    };
    router
        .layer(set(
            "content-security-policy",
            "default-src 'self'; frame-ancestors 'none'; base-uri 'self'; form-action 'self'",
        ))
        .layer(set("x-content-type-options", "nosniff"))
        .layer(set("referrer-policy", "no-referrer"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request as HttpRequest;
    use tower::ServiceExt;

    async fn status(state: AppState, req: HttpRequest<Body>) -> StatusCode {
        crate::api::router(state)
            .oneshot(req)
            .await
            .unwrap()
            .status()
    }

    fn post(host: &str) -> axum::http::request::Builder {
        HttpRequest::post("/api/config/rollback/1").header(header::HOST, host)
    }

    fn open() -> AppState {
        AppState::new(None)
    }

    #[tokio::test]
    async fn responses_carry_the_security_headers() {
        let app = with_security_headers(crate::api::router(open()));
        let resp = app
            .oneshot(HttpRequest::get("/ui/session").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let h = |n: &str| {
            resp.headers()
                .get(n)
                .map(|v| v.to_str().unwrap().to_string())
        };
        assert!(h("content-security-policy")
            .unwrap()
            .contains("frame-ancestors 'none'"));
        assert_eq!(h("x-content-type-options").as_deref(), Some("nosniff"));
        assert_eq!(h("referrer-policy").as_deref(), Some("no-referrer"));
    }

    #[tokio::test]
    async fn open_mode_refuses_a_cross_origin_post() {
        let req = post("127.0.0.1:7000")
            .header(header::ORIGIN, "https://evil.example")
            .body(Body::empty())
            .unwrap();
        assert_eq!(status(open(), req).await, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn fetch_metadata_cross_site_is_refused_and_same_origin_passes() {
        for (site, refused) in [
            ("cross-site", true),
            ("same-site", true),
            ("same-origin", false),
            ("none", false),
        ] {
            let req = post("localhost:7000")
                .header("sec-fetch-site", site)
                .body(Body::empty())
                .unwrap();
            let got = status(open(), req).await;
            assert_eq!(got == StatusCode::FORBIDDEN, refused, "{site}: {got}");
        }
    }

    #[tokio::test]
    async fn a_same_origin_post_and_a_headerless_client_pass() {
        let same = post("localhost:7000")
            .header(header::ORIGIN, "http://localhost:7000")
            .body(Body::empty())
            .unwrap();
        assert_ne!(status(open(), same).await, StatusCode::FORBIDDEN);
        let curl = post("localhost:7000").body(Body::empty()).unwrap();
        assert_ne!(status(open(), curl).await, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn same_origin_fetch_metadata_wins_over_a_rewritten_host() {
        // Behind nginx with a rewritten Host the browser's Origin no longer
        // matches Host; Sec-Fetch-Site: same-origin is the trustworthy answer.
        let req = post("backend.internal:7000")
            .header("sec-fetch-site", "same-origin")
            .header(header::ORIGIN, "https://wayhouse.example.com")
            .body(Body::empty())
            .unwrap();
        assert_ne!(status(open(), req).await, StatusCode::FORBIDDEN);
        // Without the header the Origin/Host comparison still applies.
        let req = post("backend.internal:7000")
            .header(header::ORIGIN, "https://wayhouse.example.com")
            .body(Body::empty())
            .unwrap();
        assert_eq!(status(open(), req).await, StatusCode::FORBIDDEN);
        // A cross-site marker still wins over a matching Origin.
        let req = post("wayhouse.example.com")
            .header("sec-fetch-site", "cross-site")
            .header(header::ORIGIN, "https://wayhouse.example.com")
            .body(Body::empty())
            .unwrap();
        assert_eq!(status(open(), req).await, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn origin_null_is_refused() {
        let req = post("localhost:7000")
            .header(header::ORIGIN, "null")
            .body(Body::empty())
            .unwrap();
        assert_eq!(status(open(), req).await, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn a_cross_origin_websocket_upgrade_is_refused_with_and_without_auth() {
        for state in [open(), AppState::new(Some("secret".into()))] {
            let req = HttpRequest::get("/ws/fleet")
                .header(header::HOST, "localhost:7000")
                .header(header::ORIGIN, "https://evil.example")
                .header(header::UPGRADE, "websocket")
                .header(header::CONNECTION, "upgrade")
                .header("sec-websocket-version", "13")
                .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
                .body(Body::empty())
                .unwrap();
            assert_eq!(status(state, req).await, StatusCode::FORBIDDEN);
        }
    }

    #[tokio::test]
    async fn a_plain_cross_origin_get_is_not_blocked() {
        let req = HttpRequest::get("/ui/session")
            .header(header::HOST, "localhost:7000")
            .header(header::ORIGIN, "https://evil.example")
            .body(Body::empty())
            .unwrap();
        assert_eq!(status(open(), req).await, StatusCode::OK);
    }

    #[tokio::test]
    async fn authenticated_mode_also_refuses_a_cross_site_post() {
        let state = AppState::new(Some("secret".into()));
        let id = state.sessions.create(crate::session::Session {
            role: crate::role::Role::Admin,
            username: None,
        });
        let req = post("localhost:7000")
            .header(
                header::COOKIE,
                format!("{}={id}", crate::api::SESSION_COOKIE),
            )
            .header("sec-fetch-site", "cross-site")
            .body(Body::empty())
            .unwrap();
        assert_eq!(status(state, req).await, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn the_host_refusal_names_the_host_and_the_fix() {
        let req = HttpRequest::get("/ui/session")
            .header(header::HOST, "ui.example.com")
            .body(Body::empty())
            .unwrap();
        let resp = crate::api::router(open().with_loopback_host_only(true))
            .oneshot(req)
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(body.contains("Host header \"ui.example.com\""), "{body}");
        assert!(body.contains("--ui-password"), "{body}");
        assert!(body.contains("--users-file"), "{body}");
        assert!(body.contains("Host: localhost"), "{body}");
    }

    #[tokio::test]
    async fn loopback_only_mode_refuses_a_foreign_host_header() {
        let get = |host: &str| {
            HttpRequest::get("/ui/session")
                .header(header::HOST, host)
                .body(Body::empty())
                .unwrap()
        };
        let state = || open().with_loopback_host_only(true);
        for ok in [
            "localhost:7000",
            "127.0.0.1:7000",
            "[::1]:7000",
            "127.0.0.1",
        ] {
            assert_eq!(status(state(), get(ok)).await, StatusCode::OK, "{ok}");
        }
        for bad in [
            "evil.example:7000",
            "evil.example",
            "10.0.0.5:7000",
            "localhost.evil.example",
        ] {
            assert_eq!(
                status(state(), get(bad)).await,
                StatusCode::FORBIDDEN,
                "{bad}"
            );
        }
        // The check is off unless asked for (non-loopback binds, authenticated mode).
        assert_eq!(status(open(), get("evil.example")).await, StatusCode::OK);
    }
}
