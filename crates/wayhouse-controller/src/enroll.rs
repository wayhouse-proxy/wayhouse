//! One-time-code enrollment, controller side (slice 1: the origin agent).
//!
//! An admin mints a code (`POST /enrollments`); the node redeems it once over TLS
//! (`POST /enroll`, the only unauthenticated write route) and receives the shared agent token
//! and a fresh tunnel preshared key. The key is sealed under the cluster keyring as
//! `tunnel-psk-<hash of origin>` so proxies fetch it through `GET /secrets/{name}/sealed`
//! with their own scoped token; the agent gets the plaintext once, in the redeem answer.
//!
//! Codes live in the replicated [`UiStore`] (kind `Enrollment`) as a hash only. A redeem is the
//! `UiOp::Redeem` entry, applied atomically per Raft index, so a single-use code redeems
//! exactly once even when nodes race across controller replicas.
//!
//! Order matters: the use is spent first, then the key is stored. The other order would let a
//! losing racer overwrite the winner's key. A failure between the two burns the code; the
//! admin mints another.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use axum::body::Bytes;
use axum::extract::{ConnectInfo, DefaultBodyLimit, Path, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, post};
use axum::{middleware, Json, Router};
use serde_json::{json, Value};
use wayhouse_enroll::{Code, RedeemRequest, RedeemResponse, Role};
use zeroize::Zeroizing;

use crate::ha::{WriteRequest, WriteResponse};
use crate::ui_data::api::{actor, err, internal, now, UiState};
use crate::ui_data::{Applied, Kind, UiOp};

/// Default and longest code lifetime, in seconds.
const DEFAULT_TTL: u64 = 900;
const MAX_TTL: u64 = 7 * 24 * 3600;
/// `/enroll` takes a code and a role; anything bigger is not one.
const MAX_ENROLL_BODY: usize = 1024;

/// Attempts allowed per window. Every `/enroll` try counts against the caller's address; one
/// that names a real-looking id also counts against that id, so guesses at one code cannot be
/// spread over many addresses. A success is refunded.
const PER_IP: u32 = 10;
const PER_ID: u32 = 5;
const WINDOW: Duration = Duration::from_secs(60);
const MAX_KEYS: usize = 10_000;

#[derive(Default)]
struct Limiter(Mutex<HashMap<String, (Instant, u32)>>);

impl Limiter {
    /// Takes one attempt on `key`; false when its window is full. At the key cap new keys are
    /// refused once expired windows are pruned (fail closed).
    fn take(&self, key: &str, max: u32) -> bool {
        let mut map = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let t = Instant::now();
        if map.len() >= MAX_KEYS && !map.contains_key(key) {
            map.retain(|_, (start, _)| t.duration_since(*start) < WINDOW);
            if map.len() >= MAX_KEYS {
                return false;
            }
        }
        let e = map.entry(key.to_owned()).or_insert((t, 0));
        if t.duration_since(e.0) >= WINDOW {
            *e = (t, 0);
        }
        if e.1 >= max {
            return false;
        }
        e.1 += 1;
        true
    }

    fn refund(&self, key: &str) {
        let mut g = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(e) = g.get_mut(key) {
            e.1 = e.1.saturating_sub(1);
        }
    }
}

/// IPv6 callers share a bucket per /64, as the UI login limit does.
fn ip_key(ip: IpAddr) -> String {
    match ip {
        IpAddr::V4(v) => format!("ip:{v}"),
        IpAddr::V6(v) => {
            let o = v.octets();
            format!(
                "ip:{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
                o[0], o[1], o[2], o[3], o[4], o[5], o[6], o[7]
            )
        }
    }
}

#[derive(Clone)]
struct EnrollState {
    ui: UiState,
    /// The shared agent token handed to a redeemed agent; `None` on a controller without
    /// auth tokens.
    agent_token: Option<Arc<str>>,
    limiter: Arc<Limiter>,
}

pub fn router(ui: UiState, agent_token: Option<Arc<str>>) -> Router {
    let auth = middleware::from_fn_with_state(
        wayhouse_http::server::BearerAuth::new(ui.auth_token.as_deref()),
        wayhouse_http::server::require_bearer,
    );
    let st = EnrollState {
        ui,
        agent_token,
        limiter: Arc::default(),
    };
    let admin = Router::new()
        .route("/enrollments", post(mint).get(list))
        .route("/enrollments/{id}", delete(revoke))
        .route_layer(auth);
    let public = Router::new().route(
        "/enroll",
        post(redeem).layer(DefaultBodyLimit::max(MAX_ENROLL_BODY)),
    );
    admin.merge(public).with_state(st)
}

/// Applies `op`: locally on a standalone controller, through Raft under HA. Only entries that
/// carry no plaintext secret go through here.
#[allow(clippy::result_large_err)] // axum `Response` is this module's error type
async fn apply(ui: &UiState, op: UiOp) -> Result<Applied, Response> {
    let Some(ha) = &ui.ha else {
        return ui.store.apply_local(&op).map_err(internal);
    };
    crate::ui_data::check_support(ha)
        .await
        .map_err(|why| err(StatusCode::CONFLICT, why))?;
    match crate::plugins::peer::propose_ciphertext(ha, WriteRequest::Ui(op)).await {
        Ok(WriteResponse::UiResult(a)) => Ok(a),
        Ok(other) => Err(internal(format!("unexpected answer {other:?}"))),
        Err(why) => Err(err(StatusCode::SERVICE_UNAVAILABLE, why)),
    }
}

fn no_store(mut r: Response) -> Response {
    r.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    r
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct MintBody {
    origin: String,
    ttl_secs: Option<u64>,
    uses: Option<u64>,
}

/// `POST /enrollments`: `{origin, ttl_secs?, uses?}`. The code is in this answer and nowhere
/// else, ever: the store keeps a hash.
async fn mint(State(st): State<EnrollState>, headers: HeaderMap, body: Bytes) -> Response {
    let Ok(b) = serde_json::from_slice::<MintBody>(&body) else {
        return err(
            StatusCode::BAD_REQUEST,
            "the body must be {\"origin\": \"...\", \"ttl_secs\"?, \"uses\"?}",
        );
    };
    let ttl = b.ttl_secs.unwrap_or(DEFAULT_TTL);
    if !(1..=MAX_TTL).contains(&ttl) {
        return err(
            StatusCode::BAD_REQUEST,
            format!("ttl_secs must be 1 to {MAX_TTL}"),
        );
    }
    let uses = b.uses.unwrap_or(1);
    let code = Code::generate();
    let (created, who) = (now(), actor(&headers));
    let mut record = json!({
        "role": "agent", "origin": b.origin, "hash": code.hash(),
        "created_at": created, "expires_at": created + ttl, "uses": uses, "uses_left": uses,
    });
    if let Some(a) = &who {
        record["created_by"] = a.as_str().into();
    }
    let op = UiOp::Put {
        kind: Kind::Enrollment,
        name: code.id().to_owned(),
        value: record,
    };
    match apply(&st.ui, op).await {
        Ok(Applied::Done) => {}
        Ok(Applied::Invalid(why)) => return err(StatusCode::UNPROCESSABLE_ENTITY, why),
        Ok(Applied::Full(max)) => {
            return err(
                StatusCode::CONFLICT,
                format!("already {max} codes; revoke one first"),
            )
        }
        Ok(_) => return internal("unexpected answer to a mint"),
        Err(r) => return r,
    }
    tracing::warn!(enrollment = code.id(), origin = %b.origin, actor = who.as_deref().unwrap_or("-"), "enrollment code minted");
    no_store(
        Json(json!({
            "id": code.id(), "code": &*code.expose(), "origin": b.origin,
            "expires_at": created + ttl, "uses": uses,
        }))
        .into_response(),
    )
}

/// `GET /enrollments`: every code with its state; never the code or its hash.
async fn list(State(st): State<EnrollState>) -> Response {
    let items = match st.ui.store.list(Kind::Enrollment) {
        Ok(i) => i,
        Err(e) => return internal(e),
    };
    let t = now();
    let items: Vec<Value> = items
        .into_iter()
        .map(|(id, mut v)| {
            let status = if v["uses_left"] == 0 {
                "used"
            } else if v["expires_at"].as_u64().is_some_and(|e| t >= e) {
                "expired"
            } else {
                "pending"
            };
            if let Some(o) = v.as_object_mut() {
                o.remove("hash");
                o.insert("id".into(), id.into());
                o.insert("status".into(), status.into());
            }
            v
        })
        .collect();
    Json(json!({ "items": items })).into_response()
}

/// `DELETE /enrollments/{id}`: revokes a code, spent or not.
async fn revoke(
    State(st): State<EnrollState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let op = UiOp::Delete {
        kind: Kind::Enrollment,
        name: id.clone(),
    };
    match apply(&st.ui, op).await {
        Ok(Applied::Done) => {
            tracing::warn!(enrollment = %id, actor = actor(&headers).as_deref().unwrap_or("-"), "enrollment code revoked");
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(Applied::NotFound) => err(StatusCode::NOT_FOUND, "no such code"),
        Ok(_) => internal("unexpected answer to a revoke"),
        Err(r) => r,
    }
}

fn refused() -> Response {
    err(StatusCode::UNAUTHORIZED, "invalid or expired code")
}

fn too_many() -> Response {
    let mut r = err(
        StatusCode::TOO_MANY_REQUESTS,
        "too many attempts; wait a minute",
    );
    r.headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from_static("60"));
    r
}

/// `POST /enroll`: `{code, role}`. Unauthenticated by design, so it is the most guarded route:
/// 1 KiB body, a per-address and a per-code-id attempt limit, one answer for every kind of
/// refusal, nothing secret in a log line.
async fn redeem(
    State(st): State<EnrollState>,
    ConnectInfo(peer): ConnectInfo<wayhouse_http::tls::PeerAddr>,
    body: Bytes,
) -> Response {
    let ip = ip_key(peer.0.ip());
    if !st.limiter.take(&ip, PER_IP) {
        return too_many();
    }
    let Ok(req) = serde_json::from_slice::<RedeemRequest>(&body) else {
        return err(
            StatusCode::BAD_REQUEST,
            "the body must be {\"code\": \"...\", \"role\": \"agent\"}",
        );
    };
    let Ok(code) = Code::parse(&req.code) else {
        return refused();
    };
    drop(req);
    let id_key = format!("id:{}", code.id());
    if !st.limiter.take(&id_key, PER_ID) {
        return too_many();
    }
    let origin = match st.ui.store.get(Kind::Enrollment, code.id()) {
        Ok(Some(rec)) if rec["role"] == "agent" => {
            rec["origin"].as_str().unwrap_or_default().to_owned()
        }
        Ok(_) => return refused(),
        Err(e) => return internal(e),
    };
    let ring = st.ui.keyring.current();
    if ring.active_id().is_none() {
        // Before the use is spent: an operator can fix this and retry with the same code.
        return err(
            StatusCode::SERVICE_UNAVAILABLE,
            "this controller has no secret key (--secret-key-file), so it cannot seal a tunnel key",
        );
    }
    let op = UiOp::Redeem {
        id: code.id().to_owned(),
        hash: code.hash(),
        now: now(),
    };
    match apply(&st.ui, op).await {
        Ok(Applied::Done) => {}
        Ok(Applied::Refused) => return refused(),
        Ok(_) => return internal("unexpected answer to a redeem"),
        Err(r) => return r,
    }
    let psk = wayhouse_enroll::generate_psk();
    if let Err(r) = store_psk(&st.ui, &origin, &psk).await {
        return r;
    }
    st.limiter.refund(&ip);
    st.limiter.refund(&id_key);
    tracing::warn!(enrollment = code.id(), origin = %origin, "enrollment code redeemed");
    no_store(
        Json(RedeemResponse {
            name: origin,
            role: Role::Agent,
            agent_token: st.agent_token.as_deref().map(str::to_owned),
            tunnel_psk: (*psk).clone(),
        })
        .into_response(),
    )
}

/// Seals `psk` as the origin's tunnel secret and replicates the ciphertext.
#[allow(clippy::result_large_err)]
async fn store_psk(ui: &UiState, origin: &str, psk: &Zeroizing<String>) -> Result<(), Response> {
    let name = wayhouse_enroll::psk_secret_name(origin);
    let sealed = ui
        .keyring
        .current()
        .seal_cluster(&name, psk.as_bytes())
        .map_err(internal)?;
    if let Some(ha) = &ui.ha {
        // As for any cluster secret: sealed under a key every node must already hold.
        crate::plugins::peer::check_active_key(ha, &sealed.key_id)
            .await
            .map_err(|why| {
                err(
                    StatusCode::CONFLICT,
                    format!("every node needs the active key first: {why}"),
                )
            })?;
    }
    let record = json!({
        "key_id": sealed.key_id, "nonce": sealed.nonce, "ciphertext": sealed.ciphertext, "set_at": now(),
        "actor": "enrollment",
    });
    let op = UiOp::Put {
        kind: Kind::ClusterSecret,
        name,
        value: record,
    };
    match apply(ui, op).await {
        Ok(Applied::Done) => Ok(()),
        Ok(other) => Err(internal(format!(
            "storing the tunnel key answered {other:?}"
        ))),
        Err(r) => Err(r),
    }
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::extract::ConnectInfo;
    use axum::http::{Request, StatusCode};
    use serde_json::{json, Value};
    use tower::ServiceExt;

    use super::*;
    use crate::ui_data::UiStore;

    const KEY: &str = "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=";
    const ADMIN: &str = "admin-token-admin-token";
    const AGENT: &str = "agent-token-agent-token";

    struct T {
        app: Router,
        store: UiStore,
        keyring: wayhouse_secrets::KeyringHandle,
        _d: tempfile::TempDir,
    }

    fn setup(key: Option<&str>) -> T {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::kv::Db::open(dir.path()).unwrap();
        let store = UiStore::open(&db).unwrap();
        let keyring = wayhouse_secrets::KeyringHandle::new(
            key.map_or_else(wayhouse_secrets::Keyring::default, |k| {
                wayhouse_secrets::Keyring::parse(k).unwrap()
            }),
        );
        let ui = UiState {
            store: store.clone(),
            auth_token: Some(Arc::from(ADMIN)),
            ha: None,
            keyring: keyring.clone(),
        };
        T {
            app: router(ui, Some(Arc::from(AGENT))),
            store,
            keyring,
            _d: dir,
        }
    }

    async fn call(
        app: &Router,
        method: &str,
        uri: &str,
        body: Value,
        token: Option<&str>,
        ip: &str,
    ) -> (StatusCode, Value) {
        let mut req = Request::builder().method(method).uri(uri);
        if let Some(t) = token {
            req = req.header("authorization", format!("Bearer {t}"));
        }
        let mut req = req.body(Body::from(body.to_string())).unwrap();
        req.extensions_mut()
            .insert(ConnectInfo(wayhouse_http::tls::PeerAddr(
                format!("{ip}:4000").parse().unwrap(),
            )));
        let resp = app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    async fn mint(t: &T, origin: &str) -> String {
        let (st, v) = call(
            &t.app,
            "POST",
            "/enrollments",
            json!({"origin": origin}),
            Some(ADMIN),
            "10.0.0.1",
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{v}");
        v["code"].as_str().unwrap().to_owned()
    }

    async fn redeem(t: &T, code: &str, ip: &str) -> (StatusCode, Value) {
        call(
            &t.app,
            "POST",
            "/enroll",
            json!({"code": code, "role": "agent"}),
            None,
            ip,
        )
        .await
    }

    #[tokio::test]
    async fn the_admin_routes_need_the_admin_token_and_enroll_needs_none() {
        let t = setup(Some(KEY));
        for (m, u) in [
            ("POST", "/enrollments"),
            ("GET", "/enrollments"),
            ("DELETE", "/enrollments/abcd1234"),
        ] {
            let (st, _) = call(&t.app, m, u, json!({"origin": "home"}), None, "10.0.0.1").await;
            assert_eq!(st, StatusCode::UNAUTHORIZED, "{m} {u}");
            let (st, _) = call(
                &t.app,
                m,
                u,
                json!({"origin": "home"}),
                Some(AGENT),
                "10.0.0.1",
            )
            .await;
            assert_eq!(
                st,
                StatusCode::UNAUTHORIZED,
                "agent token must not mint: {m} {u}"
            );
        }
        let (st, _) = redeem(
            &t,
            "abcd1234.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "10.0.0.2",
        )
        .await;
        assert_eq!(
            st,
            StatusCode::UNAUTHORIZED,
            "reaches the handler, not the bearer layer"
        );
    }

    #[tokio::test]
    async fn a_code_redeems_once_into_a_token_and_a_sealed_psk() {
        let t = setup(Some(KEY));
        let code = mint(&t, "home-nas").await;
        let (st, v) = redeem(&t, &code, "10.0.0.2").await;
        assert_eq!(st, StatusCode::OK, "{v}");
        assert_eq!(
            (v["name"].as_str(), v["agent_token"].as_str()),
            (Some("home-nas"), Some(AGENT))
        );
        let psk = v["tunnel_psk"].as_str().unwrap();
        // The proxy opens the same value from the sealed secret.
        let name = wayhouse_enroll::psk_secret_name("home-nas");
        let rec = t.store.get(Kind::ClusterSecret, &name).unwrap().unwrap();
        let sealed: wayhouse_secrets::Sealed = serde_json::from_value(json!({
            "key_id": rec["key_id"], "nonce": rec["nonce"], "ciphertext": rec["ciphertext"]
        }))
        .unwrap();
        let opened = t.keyring.current().open_cluster(&name, &sealed).unwrap();
        assert_eq!(opened.expose(), psk.as_bytes());
        let (st, _) = redeem(&t, &code, "10.0.0.2").await;
        assert_eq!(st, StatusCode::UNAUTHORIZED, "single use");
    }

    #[tokio::test]
    async fn wrong_expired_revoked_and_unknown_codes_are_refused_alike() {
        let t = setup(Some(KEY));
        let code = mint(&t, "a").await;
        let revoked = mint(&t, "b").await;
        let id = revoked.split('.').next().unwrap();
        let (st, _) = call(
            &t.app,
            "DELETE",
            &format!("/enrollments/{id}"),
            Value::Null,
            Some(ADMIN),
            "10.0.0.1",
        )
        .await;
        assert_eq!(st, StatusCode::NO_CONTENT);
        let expired_id = code.split('.').next().unwrap();
        let mut rec = t.store.get(Kind::Enrollment, expired_id).unwrap().unwrap();
        rec["expires_at"] = 1.into();
        t.store
            .apply_local(&UiOp::Put {
                kind: Kind::Enrollment,
                name: expired_id.into(),
                value: rec,
            })
            .unwrap();
        let wrong_secret = format!("{}.{}", &revoked[..8], "B".repeat(43));
        let unknown = format!("ffffffff.{}", "A".repeat(43));
        let mut answers = Vec::new();
        for c in [
            &code,
            &revoked,
            &wrong_secret,
            &unknown,
            &"garbage".to_string(),
        ] {
            let (st, v) = redeem(&t, c, "10.0.0.3").await;
            answers.push((st, v["error"].as_str().unwrap_or("").to_owned()));
        }
        assert!(
            answers.iter().all(|a| a.0 == StatusCode::UNAUTHORIZED),
            "{answers:?}"
        );
        assert!(
            answers.windows(2).all(|w| w[0] == w[1]),
            "the answers differ: {answers:?}"
        );
    }

    #[tokio::test]
    async fn a_burst_of_bad_codes_from_one_address_is_rate_limited() {
        let t = setup(Some(KEY));
        let bad = format!("ffffffff.{}", "A".repeat(43));
        let mut limited = false;
        for _ in 0..40 {
            if redeem(&t, &bad, "10.9.9.9").await.0 == StatusCode::TOO_MANY_REQUESTS {
                limited = true;
                break;
            }
        }
        assert!(limited);
        // Another address is unaffected.
        let other = format!("eeeeeeee.{}", "A".repeat(43));
        assert_eq!(
            redeem(&t, &other, "10.8.8.8").await.0,
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn guesses_at_one_code_id_are_limited_across_addresses() {
        let t = setup(Some(KEY));
        let code = mint(&t, "a").await;
        let id = code.split('.').next().unwrap().to_owned();
        let guess = format!("{id}.{}", "B".repeat(43));
        let mut limited = false;
        for i in 0..40 {
            if redeem(&t, &guess, &format!("10.7.{}.1", i % 250)).await.0
                == StatusCode::TOO_MANY_REQUESTS
            {
                limited = true;
                break;
            }
        }
        assert!(limited);
    }

    #[tokio::test]
    async fn racing_redeems_of_one_code_give_exactly_one_success() {
        let t = setup(Some(KEY));
        let code = mint(&t, "race").await;
        let tasks: Vec<_> = (0..8)
            .map(|i| {
                let (app, code) = (t.app.clone(), code.clone());
                tokio::spawn(async move {
                    call(
                        &app,
                        "POST",
                        "/enroll",
                        json!({"code": code, "role": "agent"}),
                        None,
                        &format!("10.6.0.{i}"),
                    )
                    .await
                    .0
                })
            })
            .collect();
        let mut results = Vec::new();
        for task in tasks {
            results.push(task.await.unwrap());
        }
        assert_eq!(
            results.iter().filter(|s| **s == StatusCode::OK).count(),
            1,
            "{results:?}"
        );
    }

    #[tokio::test]
    async fn without_a_secret_key_the_code_is_not_spent() {
        let t = setup(None);
        let code = mint(&t, "nokey").await;
        let (st, _) = redeem(&t, &code, "10.0.0.2").await;
        assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE);
        let id = code.split('.').next().unwrap();
        assert_eq!(
            t.store.get(Kind::Enrollment, id).unwrap().unwrap()["uses_left"],
            1
        );
    }

    #[tokio::test]
    async fn the_store_and_the_list_hold_no_code_and_a_big_body_is_refused() {
        let t = setup(Some(KEY));
        let code = mint(&t, "home").await;
        let secret = code.split_once('.').unwrap().1;
        let (_, list) = call(
            &t.app,
            "GET",
            "/enrollments",
            Value::Null,
            Some(ADMIN),
            "10.0.0.1",
        )
        .await;
        let stored = serde_json::to_string(&t.store.list(Kind::Enrollment).unwrap()).unwrap();
        for text in [list.to_string(), stored] {
            assert!(!text.contains(secret));
        }
        assert_eq!(list["items"][0]["status"], "pending");
        assert!(list["items"][0].get("hash").is_none());
        let big = json!({"code": "x".repeat(5000), "role": "agent"});
        assert_eq!(
            call(&t.app, "POST", "/enroll", big, None, "10.0.0.4")
                .await
                .0,
            StatusCode::PAYLOAD_TOO_LARGE
        );
    }
}
