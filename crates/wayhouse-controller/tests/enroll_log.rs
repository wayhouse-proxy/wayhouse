//! No secret reaches a log line during an enrollment, even at trace level. Its own test binary:
//! tracing caches per-callsite interest process-wide, so a scoped subscriber can miss events
//! when parallel tests in one binary hit the same callsites first.

use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::Request;
use serde_json::{json, Value};
use tower::ServiceExt;
use wayhouse_controller::ui_data::{api::UiState, UiStore};

const KEY: &str = "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=";
const ADMIN: &str = "admin-token-admin-token";
const AGENT: &str = "agent-token-agent-token";

#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<u8>>>);
impl std::io::Write for Log {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Log {
    type Writer = Log;
    fn make_writer(&'a self) -> Log {
        self.clone()
    }
}

async fn call(app: &axum::Router, uri: &str, body: Value, token: Option<&str>) -> Value {
    let mut req = Request::builder().method("POST").uri(uri);
    if let Some(t) = token {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    let mut req = req.body(Body::from(body.to_string())).unwrap();
    req.extensions_mut()
        .insert(ConnectInfo(wayhouse_http::tls::PeerAddr("10.0.0.2:4000".parse().unwrap())));
    let resp = app.clone().oneshot(req).await.unwrap();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

#[tokio::test]
async fn no_secret_reaches_the_log_even_at_trace() {
    let log = Log::default();
    tracing::subscriber::set_global_default(
        tracing_subscriber::fmt()
            .with_writer(log.clone())
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .finish(),
    )
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let store = UiStore::open(&wayhouse_controller::kv::Db::open(dir.path()).unwrap()).unwrap();
    let ui = UiState {
        store,
        auth_token: Some(Arc::from(ADMIN)),
        ha: None,
        keyring: wayhouse_secrets::KeyringHandle::new(wayhouse_secrets::Keyring::parse(KEY).unwrap()),
    };
    let app = wayhouse_controller::enroll::router(ui, Some(Arc::from(AGENT)));
    let minted = call(&app, "/enrollments", json!({"origin": "quiet"}), Some(ADMIN)).await;
    let code = minted["code"].as_str().unwrap().to_owned();
    let redeemed = call(&app, "/enroll", json!({"code": code, "role": "agent"}), None).await;
    call(&app, "/enroll", json!({"code": code, "role": "agent"}), None).await;
    let out = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
    for secret in [
        code.split_once('.').unwrap().1,
        redeemed["tunnel_psk"].as_str().unwrap(),
        AGENT,
        ADMIN,
    ] {
        assert!(!out.contains(secret), "a secret is in the log: {out}");
    }
    assert!(
        out.contains("enrollment code minted") && out.contains("enrollment code redeemed"),
        "the audit lines are missing: {out}"
    );
}
