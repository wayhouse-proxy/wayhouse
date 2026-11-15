//! `GET /api/web/sync`: when `wayhouse-web-sync` last left Caddy on the intended configuration.
//!
//! The sync is a sidecar with its own `GET /metrics`; nothing else in the fleet knows its state.
//! With `--web-sync-url` the UI reads that endpoint and shows the few numbers an operator wants
//! on the Web sites page. Read-only, viewer role, never more than the numbers below.

use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{json, Value};

use crate::api::AppState;

/// Where the sync serves its metrics, and the bearer token it asks for (if any).
#[derive(Clone)]
pub struct WebSyncTarget {
    pub base_url: String,
    pub token: Option<String>,
}

pub fn viewer_router() -> Router<AppState> {
    Router::new().route("/api/web/sync", get(status))
}

/// The value of one sample line `name{labels} value`, for the first line whose labels contain `label`.
fn sample(text: &str, name: &str, label: Option<&str>) -> Option<f64> {
    text.lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| {
            let rest = l.strip_prefix(name)?;
            let (labels, value) = match rest.strip_prefix('{') {
                Some(r) => r.split_once("} ")?,
                None => ("", rest.strip_prefix(' ')?),
            };
            label
                .is_none_or(|want| labels.contains(want))
                .then(|| value.trim().parse().ok())?
        })
        .next()
}

/// What the page shows, from the sync's metrics text at unix time `now`.
fn summary(text: &str, now: u64) -> Value {
    let get = |n, l| sample(text, n, l);
    let last_ok = get("wayhouse_web_sync_last_ok", None).filter(|t| *t > 0.0);
    json!({
        "configured": true,
        "reachable": true,
        "last_ok_age_secs": last_ok.map(|t| now.saturating_sub(t as u64)),
        "loads": get("wayhouse_web_sync_loads_total", None).unwrap_or(0.0) as u64,
        "rejects": get("wayhouse_web_sync_rejects_total", None).unwrap_or(0.0) as u64,
        "errors": {
            "read": get("wayhouse_web_sync_errors_total", Some("stage=\"read\"")).unwrap_or(0.0) as u64,
            "render": get("wayhouse_web_sync_errors_total", Some("stage=\"render\"")).unwrap_or(0.0) as u64,
            "send": get("wayhouse_web_sync_errors_total", Some("stage=\"send\"")).unwrap_or(0.0) as u64,
        },
    })
}

async fn status(State(state): State<AppState>) -> Response {
    let Some(target) = &state.web_sync else {
        return Json(json!({ "configured": false })).into_response();
    };
    let mut req = state
        .http
        .get(format!("{}/metrics", target.base_url.trim_end_matches('/')));
    if let Some(t) = &target.token {
        req = req.bearer_auth(t);
    }
    let unreachable = |why: String| {
        Json(json!({ "configured": true, "reachable": false, "error": why })).into_response()
    };
    let resp = match req.timeout(std::time::Duration::from_secs(3)).send().await {
        Ok(r) => r,
        Err(_) => return unreachable("the web sync did not answer".into()),
    };
    if !resp.status().is_success() {
        return unreachable(format!("the web sync answered {}", resp.status().as_u16()));
    }
    let Ok(body) = wayhouse_http::read_capped(resp, 1 << 20).await else {
        return unreachable("the web sync's metrics could not be read".into());
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    Json(summary(&String::from_utf8_lossy(&body), now)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    const METRICS: &str = "# HELP wayhouse_web_sync_last_ok x\n# TYPE wayhouse_web_sync_last_ok gauge\nwayhouse_web_sync_last_ok 1000\nwayhouse_web_sync_loads_total 7\nwayhouse_web_sync_rejects_total 1\nwayhouse_web_sync_errors_total{stage=\"read\"} 2\nwayhouse_web_sync_errors_total{stage=\"send\"} 3\nwayhouse_web_sync_last_ok_other 5\n";

    #[test]
    fn the_summary_reads_age_counters_and_the_error_stages() {
        let s = summary(METRICS, 1090);
        assert_eq!(s["last_ok_age_secs"], 90);
        assert_eq!(
            (s["loads"].clone(), s["rejects"].clone()),
            (json!(7), json!(1))
        );
        assert_eq!(s["errors"], json!({"read": 2, "render": 0, "send": 3}));
    }

    #[test]
    fn a_sync_that_never_succeeded_has_no_age() {
        let s = summary("wayhouse_web_sync_last_ok 0\n", 5000);
        assert!(s["last_ok_age_secs"].is_null());
    }

    #[test]
    fn a_metric_whose_name_only_starts_the_same_is_not_mistaken() {
        assert_eq!(
            sample(
                "wayhouse_web_sync_last_ok_other 5\n",
                "wayhouse_web_sync_last_ok",
                None
            ),
            None
        );
    }

    async fn get_status(state: AppState) -> (StatusCode, Value) {
        let resp = crate::api::router(state)
            .oneshot(Request::get("/api/web/sync").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let st = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap();
        (st, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    async fn serve_metrics(token: Option<&'static str>) -> String {
        let app = Router::new().route(
            "/metrics",
            get(move |h: axum::http::HeaderMap| async move {
                match token {
                    Some(t)
                        if h.get("authorization").and_then(|v| v.to_str().ok())
                            != Some(&format!("Bearer {t}")) =>
                    {
                        (StatusCode::UNAUTHORIZED, String::new())
                    }
                    _ => (StatusCode::OK, METRICS.to_string()),
                }
            }),
        );
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn without_a_web_sync_url_it_says_not_configured() {
        let (st, body) = get_status(AppState::new(None)).await;
        assert_eq!((st, body), (StatusCode::OK, json!({"configured": false})));
    }

    #[tokio::test]
    async fn it_reads_the_sync_with_its_token() {
        let url = serve_metrics(Some("t0ken-t0ken")).await;
        let (_, body) =
            get_status(AppState::new(None).with_web_sync(url, Some("t0ken-t0ken".into()))).await;
        assert_eq!(
            (body["reachable"].clone(), body["loads"].clone()),
            (json!(true), json!(7))
        );
    }

    #[tokio::test]
    async fn a_refusing_or_absent_sync_is_reported_not_hidden() {
        let url = serve_metrics(Some("right-token")).await;
        let (_, body) =
            get_status(AppState::new(None).with_web_sync(url, Some("wrong".into()))).await;
        assert_eq!(body["reachable"], false);
        assert!(
            body["error"].as_str().unwrap().contains("answered 401"),
            "{body}"
        );
        let (_, body) =
            get_status(AppState::new(None).with_web_sync("http://127.0.0.1:1".into(), None)).await;
        assert_eq!(body["reachable"], false);
    }
}
