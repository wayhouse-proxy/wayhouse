//! Shared helper for `crate::aggregator_proxy` and `crate::controller_proxy`
//! — both are thin pass-through proxies to a machine service, and both need
//! to forward the upstream response's headers faithfully, not just its
//! status and body.

/// The upstream response's headers, minus the hop-by-hop ones that don't
/// make sense to forward verbatim (`axum` recomputes `content-length` for
/// the body we actually send; `connection`/`transfer-encoding` describe
/// *this* hop's framing, not the content). Everything else — notably
/// `wayhouse-controller`'s `X-Config-Revision` — passes through, so a caller
/// reading a response header from `wayhouse-ui` sees exactly what the upstream
/// service sent.
pub fn forwardable_headers(upstream: &axum::http::HeaderMap) -> axum::http::HeaderMap {
    let mut headers = upstream.clone();
    for h in [
        axum::http::header::CONNECTION,
        axum::http::header::TRANSFER_ENCODING,
        axum::http::header::CONTENT_LENGTH,
    ] {
        headers.remove(h);
    }
    headers
}

/// `Path` has already percent-decoded `name`; re-encode it so it stays one
/// path segment. `None` for the dot segments a URL normalizer would resolve
/// away from the route they are meant for.
pub(crate) fn path_segment(name: &str) -> Option<String> {
    if name == "." || name == ".." {
        return None;
    }
    Some(encode_segment(name))
}

/// `value` with everything but the URL-unreserved bytes percent-encoded: one path segment or one
/// query value, whatever it holds. Building the text byte by byte (not by `format!`-ing the input
/// into the URL) also keeps request-derived text out of the URL as itself.
pub(crate) fn encode_segment(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for b in value.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// A revision number as a URL segment (`42`): digits only, so encoding changes nothing.
pub(crate) fn number_segment(n: u64) -> String {
    encode_segment(&n.to_string())
}

/// Most bytes of an upstream reply the UI buffers before answering the browser.
pub(crate) const MAX_UPSTREAM_BODY: usize = 16 << 20;

/// The upstream reply as the browser's reply: status, forwardable headers and a
/// body capped at [`MAX_UPSTREAM_BODY`] (a `502` instead when it is larger or the
/// transfer fails).
pub(crate) async fn relay(resp: reqwest::Response, who: &str) -> axum::response::Response {
    use axum::response::IntoResponse;
    let status = resp.status();
    let headers = forwardable_headers(resp.headers());
    match wayhouse_http::read_capped(resp, MAX_UPSTREAM_BODY).await {
        Ok(body) => (status, headers, body).into_response(),
        Err(e) => (
            axum::http::StatusCode::BAD_GATEWAY,
            axum::Json(serde_json::json!({ "error": format!("{who}: {e}") })),
        )
            .into_response(),
    }
}

/// Wire-shape contract for *every* route `wayhouse-ui` proxies: whatever the
/// upstream service answers — status, custom headers — must reach the
/// browser unchanged, and the hop-by-hop framing headers must not. This bug
/// class (a pass-through proxy silently dropping response headers) has hit
/// this codebase more than once, and status + body assertions don't catch
/// it, so the check is table-driven over the whole proxied surface.
///
/// **When you add a proxied route, add it to `ROUTES`.** (axum can't
/// enumerate a router's routes, so the table is maintained by hand.)
#[cfg(test)]
mod header_contract {
    use axum::body::Body;
    use axum::http::{header, Method, Request, StatusCode};
    use axum::routing::any;
    use axum::Router;
    use tower::ServiceExt;

    /// `(method, browser-facing path)` — one entry per proxied route.
    const ROUTES: &[(&str, &str)] = &[
        // aggregator_proxy: viewer reads
        ("GET", "/api/fleet/pools"),
        ("GET", "/api/fleet/sessions"),
        ("GET", "/api/fleet/healthz"),
        ("GET", "/api/fleet/features"),
        ("GET", "/api/fleet/nodes?page=2&tag=env%3Dprod"),
        ("POST", "/api/fleet/nodes/resolve?region=eu"),
        ("GET", "/api/fleet/instances/fra-1/sniffers"),
        // aggregator_proxy: operator verbs
        ("POST", "/api/fleet/instances/fra-1/drain"),
        ("POST", "/api/fleet/instances/fra-1/undrain"),
        ("POST", "/api/fleet/pools/lobby/backends"),
        ("PATCH", "/api/fleet/pools/lobby/backends/10.0.0.5:7777"),
        ("DELETE", "/api/fleet/pools/lobby/backends/10.0.0.5:7777"),
        ("PUT", "/api/fleet/nodes/fra-1/tags"),
        ("DELETE", "/api/fleet/nodes/fra-1/tags"),
        ("POST", "/api/fleet/route-hint"),
        ("POST", "/api/fleet/sniffers?name=a2s.wasm"),
        ("DELETE", "/api/fleet/sniffers/a2s.wasm"),
        ("POST", "/api/fleet/sniffers/a2s.wasm/rollback"),
        // controller_proxy: viewer reads
        ("GET", "/api/config"),
        ("GET", "/api/config/schema"),
        ("GET", "/api/config/revisions"),
        ("GET", "/api/config/revisions/1"),
        ("GET", "/api/config/revisions/1/diff?against=2"),
        ("GET", "/api/config/pending"),
        ("GET", "/api/tunnel/addresses"),
        ("GET", "/api/acme/sites"),
        ("GET", "/api/acme/gateways"),
        ("GET", "/api/acme/account"),
        ("GET", "/api/features"),
        ("GET", "/api/features/overrides"),
        ("GET", "/api/plugins/0123abcd/status"),
        // controller_proxy: admin writes
        ("POST", "/api/config"),
        ("POST", "/api/config/validate"),
        ("POST", "/api/config/rollback/1"),
        ("POST", "/api/config/promote/1"),
        ("POST", "/api/config/confirm/1"),
        ("POST", "/api/config/revert"),
        ("DELETE", "/api/tunnel/origins/fra-1"),
        ("DELETE", "/api/tunnel/proxies/edge-1"),
        ("POST", "/api/tunnel/origins/fra-1/psk/rotate"),
        ("PUT", "/api/acme/sites/panel"),
        ("DELETE", "/api/acme/sites/panel"),
        ("POST", "/api/acme/gateways"),
        ("DELETE", "/api/acme/gateways/edge-1"),
        ("POST", "/api/enrollments"),
        ("GET", "/api/enrollments"),
        ("DELETE", "/api/enrollments/abc123"),
        ("PUT", "/api/plugins/0123abcd/config"),
        ("GET", "/api/plugins/0123abcd/secrets"),
        ("PUT", "/api/plugins/0123abcd/secrets/TOKEN"),
        ("DELETE", "/api/plugins/0123abcd/secrets/TOKEN"),
        ("POST", "/api/plugins/0123abcd/webhook"),
        ("DELETE", "/api/plugins/0123abcd/webhook"),
        ("PUT", "/api/features/overrides/acl"),
        ("DELETE", "/api/features/overrides/acl"),
    ];

    /// An upstream that answers *every* method and path the same way: a
    /// non-200 success status, two end-to-end headers, and a body.
    async fn spawn_upstream() -> String {
        // The one read the UI backend makes of its own accord (operator tags) must parse.
        let app = Router::new()
            .route(
                "/ui/node-tags",
                axum::routing::get(|| async { r#"{"items":[]}"# }),
            )
            .fallback(any(|| async {
                (
                    StatusCode::ACCEPTED,
                    [
                        ("x-config-revision", "42"),
                        ("x-contract-probe", "survives"),
                    ],
                    "upstream-body",
                )
            }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn every_proxied_route_forwards_status_and_response_headers() {
        let upstream = spawn_upstream().await;
        let state = crate::api::AppState::new(Some("secret".into()))
            .with_aggregator(upstream.clone(), None)
            .with_controller(upstream, None);
        let session = state.sessions.create(crate::session::Session {
            role: crate::role::Role::Admin,
            username: Some("alice".into()),
        });
        let app = crate::api::router(state);

        for (method, path) in ROUTES {
            let req = Request::builder()
                .method(Method::from_bytes(method.as_bytes()).unwrap())
                .uri(*path)
                .header(
                    header::COOKIE,
                    format!("{}={session}", crate::api::SESSION_COOKIE),
                )
                .body(Body::from("{}"))
                .unwrap();
            let resp = app.clone().oneshot(req).await.unwrap();

            assert_eq!(
                resp.status(),
                StatusCode::ACCEPTED,
                "{method} {path}: status"
            );
            for (name, want) in [
                ("x-config-revision", "42"),
                ("x-contract-probe", "survives"),
            ] {
                assert_eq!(
                    resp.headers().get(name).and_then(|v| v.to_str().ok()),
                    Some(want),
                    "{method} {path}: response header `{name}` was dropped or changed"
                );
            }
            let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap();
            assert_eq!(body, "upstream-body".as_bytes(), "{method} {path}: body");
        }
    }

    #[test]
    fn hop_by_hop_headers_are_stripped_and_the_rest_kept() {
        let mut upstream = axum::http::HeaderMap::new();
        upstream.insert(header::CONNECTION, "close".parse().unwrap());
        upstream.insert(header::TRANSFER_ENCODING, "chunked".parse().unwrap());
        upstream.insert(header::CONTENT_LENGTH, "9".parse().unwrap());
        upstream.insert("x-config-revision", "7".parse().unwrap());
        let out = super::forwardable_headers(&upstream);
        assert!(out.get(header::CONNECTION).is_none());
        assert!(out.get(header::TRANSFER_ENCODING).is_none());
        assert!(out.get(header::CONTENT_LENGTH).is_none());
        assert_eq!(out.get("x-config-revision").unwrap(), "7");
    }
}

#[cfg(test)]
mod segment_tests {
    use super::*;

    #[test]
    fn a_number_is_its_digits_and_text_cannot_leave_its_segment() {
        assert_eq!(number_segment(0), "0");
        assert_eq!(number_segment(u64::MAX), "18446744073709551615");
        assert_eq!(encode_segment("a/b?c=d#e%f g"), "a%2Fb%3Fc%3Dd%23e%25f%20g");
        assert_eq!(path_segment(".."), None);
        assert_eq!(path_segment("ok-1.x").as_deref(), Some("ok-1.x"));
    }
}
