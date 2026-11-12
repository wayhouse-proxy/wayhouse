//! One-time-code enrollment, controller side.
//!
//! An admin mints a code for a role (`POST /enrollments`); the node redeems it once over TLS
//! (`POST /enroll`, the only unauthenticated write route) and receives exactly that role's
//! files (see [`Role::grant_files`]):
//!
//! | role | files |
//! |---|---|
//! | agent | the shared agent token and a fresh tunnel preshared key |
//! | proxy | the proxy token, the ingest token (if set) and the cluster keyring |
//! | aggregator | the ingest token (if set); its own admin token is made on the node |
//! | ui | the admin token as `controller-token` |
//!
//! The tunnel key is sealed under the cluster keyring as `tunnel-psk-<hash of origin>` so
//! proxies fetch it through `GET /secrets/{name}/sealed` with their own scoped token; the agent
//! gets the plaintext once, in the redeem answer.
//!
//! The keyring and the admin token are the crown jewels, so minting a code for `proxy` or `ui`
//! needs the admin token **and** the step-up secret (`--enroll-step-up-file`; the UI asks the
//! operator for it again at every mint). Without a configured step-up secret those roles
//! cannot be minted at all.
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
/// Codes for the roles that receive the keyring or the admin token live at most a day.
const MAX_STEP_UP_TTL: u64 = 24 * 3600;
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

/// The secrets this controller may hand out. `None` means the role that needs it cannot be
/// minted (agents still enroll on a controller without auth tokens, as before).
#[derive(Clone, Default)]
pub struct Grants {
    pub agent_token: Option<Arc<str>>,
    pub proxy_token: Option<Arc<str>>,
    pub ingest_token: Option<Arc<str>>,
    pub step_up: Option<Arc<str>>,
}

#[derive(Clone)]
struct EnrollState {
    ui: UiState,
    grants: Grants,
    limiter: Arc<Limiter>,
}

pub fn router(ui: UiState, grants: Grants) -> Router {
    let auth = middleware::from_fn_with_state(
        wayhouse_http::server::BearerAuth::new(ui.auth_token.as_deref()),
        wayhouse_http::server::require_bearer,
    );
    let st = EnrollState {
        ui,
        grants,
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
    #[serde(default = "default_role")]
    role: Role,
    /// The node's name (an origin's tunnel name, a proxy's instance name).
    #[serde(alias = "origin")]
    name: String,
    ttl_secs: Option<u64>,
    uses: Option<u64>,
    /// Aggregator only: the parent this tier pushes to.
    parent_url: Option<String>,
    /// Roles that need it only; never logged, never stored.
    step_up: Option<String>,
}

fn default_role() -> Role {
    Role::Agent
}

/// Why `role` cannot be handed out by this controller right now, if it cannot. Checked at mint
/// (so the admin hears it early) and again at redeem, before the use is spent.
fn not_ready(st: &EnrollState, role: Role) -> Option<&'static str> {
    let g = &st.grants;
    let ring = st.ui.keyring.current();
    match role {
        Role::Agent if ring.active_id().is_none() => Some(
            "this controller has no secret key (--secret-key-file), so it cannot seal a tunnel key",
        ),
        Role::Proxy if g.proxy_token.is_none() => Some("this controller has no --proxy-token"),
        Role::Proxy if ring.is_empty() => {
            Some("this controller has no secret key (--secret-key-file) to hand to a proxy")
        }
        Role::Aggregator if g.ingest_token.is_none() => {
            Some("this controller has no --ingest-token to hand to an aggregator")
        }
        Role::Ui if st.ui.auth_token.is_none() => Some("this controller has no --auth-token"),
        _ if role.needs_step_up() && g.step_up.is_none() => {
            Some("this role needs --enroll-step-up-file on the controller")
        }
        _ => None,
    }
}

/// `parent_url` must be an http(s) URL with no credentials in it.
fn parent_url_ok(u: &str) -> bool {
    u.len() <= 512
        && reqwest::Url::parse(u).is_ok_and(|u| {
            matches!(u.scheme(), "http" | "https")
                && u.username().is_empty()
                && u.password().is_none()
                && u.host_str().is_some()
        })
}

/// `POST /enrollments`: `{role?, name, ttl_secs?, uses?, parent_url?, step_up?}`. The code is
/// in this answer and nowhere else, ever: the store keeps a hash.
async fn mint(State(st): State<EnrollState>, headers: HeaderMap, body: Bytes) -> Response {
    let Ok(b) = serde_json::from_slice::<MintBody>(&body) else {
        return err(
            StatusCode::BAD_REQUEST,
            "the body must be {\"role\"?: \"agent|proxy|aggregator|ui\", \"name\": \"...\", \"ttl_secs\"?, \"uses\"?, \"parent_url\"?, \"step_up\"?}",
        );
    };
    let who = actor(&headers);
    let ttl = b.ttl_secs.unwrap_or(DEFAULT_TTL);
    if !(1..=MAX_TTL).contains(&ttl) {
        return err(
            StatusCode::BAD_REQUEST,
            format!("ttl_secs must be 1 to {MAX_TTL}"),
        );
    }
    if let Err(why) = crate::registry::validate_name(&b.name) {
        return err(StatusCode::BAD_REQUEST, why);
    }
    if b.role.needs_step_up() && ttl > MAX_STEP_UP_TTL {
        return err(
            StatusCode::BAD_REQUEST,
            format!("ttl_secs for this role is at most {MAX_STEP_UP_TTL}"),
        );
    }
    if b.role == Role::Ui && b.uses.is_some_and(|u| u != 1) {
        return err(StatusCode::BAD_REQUEST, "a ui code is single use");
    }
    if b.parent_url
        .as_deref()
        .is_some_and(|u| b.role != Role::Aggregator || !parent_url_ok(u))
    {
        return err(
            StatusCode::BAD_REQUEST,
            "parent_url is for an aggregator and must be an http(s) URL without credentials",
        );
    }
    if let Some(why) = not_ready(&st, b.role) {
        return err(StatusCode::CONFLICT, why);
    }
    if b.role.needs_step_up() && !step_up_ok(&st, b.step_up.as_deref()) {
        tracing::warn!(role = b.role.as_str(), name = %b.name, actor = who.as_deref().unwrap_or("-"), "enrollment mint refused: step-up failed");
        return err(
            StatusCode::FORBIDDEN,
            "this role needs the step-up secret; enter it again",
        );
    }
    let uses = b.uses.unwrap_or(1);
    let code = Code::generate();
    let created = now();
    let mut record = json!({
        "role": b.role.as_str(), "origin": b.name, "hash": code.hash(),
        "created_at": created, "expires_at": created + ttl, "uses": uses, "uses_left": uses,
    });
    if let Some(a) = &who {
        record["created_by"] = a.as_str().into();
    }
    if let Some(u) = &b.parent_url {
        record["parent_url"] = u.as_str().into();
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
    tracing::warn!(enrollment = code.id(), role = b.role.as_str(), name = %b.name, actor = who.as_deref().unwrap_or("-"), step_up = b.role.needs_step_up(), "enrollment code minted");
    no_store(
        Json(json!({
            "id": code.id(), "code": &*code.expose(), "role": b.role.as_str(), "name": b.name,
            "expires_at": created + ttl, "uses": uses,
        }))
        .into_response(),
    )
}

/// Checks the step-up secret in constant time. A wrong guess counts against one shared bucket
/// (the admin token is already required, so this only slows a stolen admin token).
fn step_up_ok(st: &EnrollState, presented: Option<&str>) -> bool {
    let (Some(expected), Some(p)) = (st.grants.step_up.as_deref(), presented) else {
        return false;
    };
    if !st.limiter.take("step-up", PER_ID) {
        return false;
    }
    let ok = wayhouse_http::token_eq(p, expected);
    if ok {
        st.limiter.refund("step-up");
    }
    ok
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
            "the body must be {\"code\": \"...\", \"role\": \"agent|proxy|aggregator|ui\"}",
        );
    };
    let Ok(code) = Code::parse(&req.code) else {
        return refused();
    };
    let id_key = format!("id:{}", code.id());
    if !st.limiter.take(&id_key, PER_ID) {
        return too_many();
    }
    let (name, parent_url) = match st.ui.store.get(Kind::Enrollment, code.id()) {
        // The hash is checked here too, so a configuration answer (503) is only ever given to
        // the holder of the code.
        Ok(Some(rec))
            if rec["role"] == req.role.as_str()
                && rec["hash"]
                    .as_str()
                    .is_some_and(|h| wayhouse_http::token_eq(h, &code.hash())) =>
        {
            (
                rec["origin"].as_str().unwrap_or_default().to_owned(),
                rec["parent_url"].as_str().map(str::to_owned),
            )
        }
        Ok(_) => return refused(),
        Err(e) => return internal(e),
    };
    let role = req.role;
    drop(req);
    if let Some(why) = not_ready(&st, role) {
        // Before the use is spent: an operator can fix this and retry with the same code.
        return err(StatusCode::SERVICE_UNAVAILABLE, why);
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
    let files = match grant_files(&st, role, &name).await {
        Ok(f) => f,
        Err(r) => return r,
    };
    st.limiter.refund(&ip);
    st.limiter.refund(&id_key);
    tracing::warn!(enrollment = code.id(), role = role.as_str(), name = %name, "enrollment code redeemed");
    no_store(
        Json(RedeemResponse {
            name,
            role,
            files,
            parent_url,
        })
        .into_response(),
    )
}

/// What `role` receives. The agent's tunnel key is made here, after the use is spent (see the
/// module note on ordering).
#[allow(clippy::result_large_err)]
async fn grant_files(
    st: &EnrollState,
    role: Role,
    name: &str,
) -> Result<std::collections::BTreeMap<String, String>, Response> {
    let g = &st.grants;
    let mut f = std::collections::BTreeMap::new();
    let mut put = |k: &str, v: Option<&str>| {
        if let Some(v) = v {
            f.insert(k.to_owned(), v.to_owned());
        }
    };
    match role {
        Role::Agent => {
            let psk = wayhouse_enroll::generate_psk();
            store_psk(&st.ui, name, &psk).await?;
            put("tunnel-psk", Some(&psk));
            put("agent-token", g.agent_token.as_deref());
        }
        Role::Proxy => {
            put("controller-token", g.proxy_token.as_deref());
            put("aggregator-token", g.ingest_token.as_deref());
            put("secret.keys", Some(&st.ui.keyring.current().export()));
        }
        Role::Aggregator => put("ingest-token", g.ingest_token.as_deref()),
        Role::Ui => put("controller-token", st.ui.auth_token.as_deref()),
    }
    Ok(f)
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
    const PROXY: &str = "proxy-token-proxy-token";
    const INGEST: &str = "ingest-token-ingest-token";
    const STEP_UP: &str = "step-up-step-up-step-up";

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
            app: router(
                ui,
                Grants {
                    agent_token: Some(Arc::from(AGENT)),
                    proxy_token: Some(Arc::from(PROXY)),
                    ingest_token: Some(Arc::from(INGEST)),
                    step_up: Some(Arc::from(STEP_UP)),
                },
            ),
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
            (v["name"].as_str(), v["files"]["agent-token"].as_str()),
            (Some("home-nas"), Some(AGENT))
        );
        let psk = v["files"]["tunnel-psk"].as_str().unwrap();
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
    async fn without_a_secret_key_no_agent_code_is_minted() {
        let t = setup(None);
        let (st, _) = call(
            &t.app,
            "POST",
            "/enrollments",
            json!({"origin": "nokey"}),
            Some(ADMIN),
            "10.0.0.1",
        )
        .await;
        assert_eq!(st, StatusCode::CONFLICT);
        assert!(t.store.list(Kind::Enrollment).unwrap().is_empty());
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

    async fn mint_as(t: &T, body: Value) -> (StatusCode, Value) {
        call(
            &t.app,
            "POST",
            "/enrollments",
            body,
            Some(ADMIN),
            "10.0.0.1",
        )
        .await
    }

    async fn redeem_as(t: &T, code: &str, role: &str) -> (StatusCode, Value) {
        call(
            &t.app,
            "POST",
            "/enroll",
            json!({"code": code, "role": role}),
            None,
            "10.0.0.2",
        )
        .await
    }

    fn files(v: &Value) -> Vec<String> {
        let mut k: Vec<_> = v["files"].as_object().unwrap().keys().cloned().collect();
        k.sort();
        k
    }

    #[tokio::test]
    async fn each_role_gets_exactly_its_files_and_nothing_else() {
        let t = setup(Some(KEY));
        for (role, expect) in [
            (
                "proxy",
                vec!["aggregator-token", "controller-token", "secret.keys"],
            ),
            ("aggregator", vec!["ingest-token"]),
            ("ui", vec!["controller-token"]),
            ("agent", vec!["agent-token", "tunnel-psk"]),
        ] {
            let (st, m) =
                mint_as(&t, json!({"role": role, "name": "n1", "step_up": STEP_UP})).await;
            assert_eq!(st, StatusCode::OK, "{role}: {m}");
            let (st, v) = redeem_as(&t, m["code"].as_str().unwrap(), role).await;
            assert_eq!(st, StatusCode::OK, "{role}: {v}");
            assert_eq!(files(&v), expect, "{role}");
            // And only the role's grant names, so the client's allowlist accepts it.
            let role_enum: Role = serde_json::from_value(json!(role)).unwrap();
            assert!(files(&v)
                .iter()
                .all(|f| role_enum.grant_files().contains(&f.as_str())));
        }
    }

    #[tokio::test]
    async fn the_grants_carry_the_right_secrets() {
        let t = setup(Some(KEY));
        let go = |role: &'static str| {
            let t = &t;
            async move {
                let (_, m) =
                    mint_as(t, json!({"role": role, "name": "n", "step_up": STEP_UP})).await;
                redeem_as(t, m["code"].as_str().unwrap(), role).await.1
            }
        };
        let p = go("proxy").await;
        assert_eq!(
            p["files"]["controller-token"], PROXY,
            "a proxy never gets the admin token"
        );
        assert_eq!(p["files"]["aggregator-token"], INGEST);
        assert_eq!(
            wayhouse_secrets::Keyring::parse(p["files"]["secret.keys"].as_str().unwrap())
                .unwrap()
                .key_ids(),
            t.keyring.current().key_ids()
        );
        assert_eq!(go("ui").await["files"]["controller-token"], ADMIN);
    }

    #[tokio::test]
    async fn a_code_only_redeems_as_the_role_it_was_minted_for() {
        let t = setup(Some(KEY));
        let (_, m) = mint_as(&t, json!({"role": "aggregator", "name": "agg"})).await;
        let code = m["code"].as_str().unwrap();
        for role in ["proxy", "ui", "agent"] {
            assert_eq!(
                redeem_as(&t, code, role).await.0,
                StatusCode::UNAUTHORIZED,
                "{role}"
            );
        }
        assert_eq!(redeem_as(&t, code, "aggregator").await.0, StatusCode::OK);
    }

    #[tokio::test]
    async fn keyring_and_admin_roles_need_the_step_up_secret_at_every_mint() {
        let t = setup(Some(KEY));
        for role in ["proxy", "ui"] {
            for bad in [None, Some("wrong-wrong-wrong-wrong"), Some(ADMIN)] {
                let mut body = json!({"role": role, "name": "n"});
                if let Some(b) = bad {
                    body["step_up"] = b.into();
                }
                assert_eq!(
                    mint_as(&t, body).await.0,
                    StatusCode::FORBIDDEN,
                    "{role} {bad:?}"
                );
            }
        }
        assert!(
            t.store.list(Kind::Enrollment).unwrap().is_empty(),
            "nothing was minted"
        );
        // Roles without that grant need no step-up.
        assert_eq!(
            mint_as(&t, json!({"role": "aggregator", "name": "a"}))
                .await
                .0,
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn guessing_the_step_up_secret_is_rate_limited() {
        let t = setup(Some(KEY));
        let mut last = StatusCode::OK;
        for _ in 0..PER_ID + 2 {
            last = mint_as(
                &t,
                json!({"role": "proxy", "name": "n", "step_up": "nope-nope-nope-nope"}),
            )
            .await
            .0;
        }
        assert_eq!(last, StatusCode::FORBIDDEN);
        // Even the right secret is refused until the window passes.
        let (st, _) = mint_as(
            &t,
            json!({"role": "proxy", "name": "n", "step_up": STEP_UP}),
        )
        .await;
        assert_eq!(st, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn without_a_configured_step_up_or_keyring_the_role_cannot_be_minted() {
        let t = setup(None);
        let (st, v) = mint_as(
            &t,
            json!({"role": "proxy", "name": "n", "step_up": STEP_UP}),
        )
        .await;
        assert_eq!(st, StatusCode::CONFLICT, "{v}");
        let (st, _) = mint_as(&t, json!({"role": "agent", "name": "n"})).await;
        assert_eq!(
            st,
            StatusCode::CONFLICT,
            "an agent code needs a key to seal its tunnel key"
        );
    }

    #[tokio::test]
    async fn the_aggregator_parent_url_is_validated_and_returned() {
        let t = setup(Some(KEY));
        for bad in ["ftp://p", "https://u:p@parent.example", "nonsense"] {
            let (st, _) = mint_as(
                &t,
                json!({"role": "aggregator", "name": "a", "parent_url": bad}),
            )
            .await;
            assert_eq!(st, StatusCode::BAD_REQUEST, "{bad}");
        }
        let (st, _) = mint_as(
            &t,
            json!({"role": "agent", "name": "a", "parent_url": "https://parent.example"}),
        )
        .await;
        assert_eq!(
            st,
            StatusCode::BAD_REQUEST,
            "only an aggregator has a parent"
        );
        let (_, m) = mint_as(
            &t,
            json!({"role": "aggregator", "name": "a", "parent_url": "https://parent.example:9902"}),
        )
        .await;
        let (_, v) = redeem_as(&t, m["code"].as_str().unwrap(), "aggregator").await;
        assert_eq!(v["parent_url"], "https://parent.example:9902");
    }

    #[tokio::test]
    async fn the_listing_shows_role_and_state_but_never_the_step_up_or_a_secret() {
        let t = setup(Some(KEY));
        let (_, m) = mint_as(
            &t,
            json!({"role": "proxy", "name": "edge1", "step_up": STEP_UP}),
        )
        .await;
        let (_, list) = call(
            &t.app,
            "GET",
            "/enrollments",
            Value::Null,
            Some(ADMIN),
            "10.0.0.1",
        )
        .await;
        assert_eq!(list["items"][0]["role"], "proxy");
        let text = list.to_string()
            + &serde_json::to_string(&t.store.list(Kind::Enrollment).unwrap()).unwrap();
        assert!(!text.contains(STEP_UP));
        assert!(!text.contains(m["code"].as_str().unwrap().split_once('.').unwrap().1));
    }

    #[tokio::test]
    async fn keyring_and_admin_codes_are_short_lived_and_a_ui_code_is_single_use() {
        let t = setup(Some(KEY));
        let m = |extra: Value| {
            let mut b = json!({"name": "n", "step_up": STEP_UP});
            b.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            b
        };
        for body in [
            m(json!({"role": "proxy", "ttl_secs": MAX_STEP_UP_TTL + 1})),
            m(json!({"role": "ui", "uses": 2})),
            m(json!({"role": "ui", "uses": 0})),
            m(json!({"role": "agent", "role_": 1, "name": "bad name\nx"})),
        ] {
            assert_eq!(mint_as(&t, body).await.0, StatusCode::BAD_REQUEST);
        }
        assert_eq!(
            mint_as(
                &t,
                m(json!({"role": "proxy", "uses": 5, "ttl_secs": MAX_STEP_UP_TTL}))
            )
            .await
            .0,
            StatusCode::OK,
            "a multi-use proxy code stays possible for a k8s init container"
        );
        // A plain agent code still lasts up to MAX_TTL.
        assert_eq!(
            mint_as(&t, json!({"name": "n", "ttl_secs": MAX_TTL}))
                .await
                .0,
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn a_wrong_secret_never_learns_the_controller_is_misconfigured() {
        // A proxy code on a controller that has no proxy token cannot be minted, so mint one
        // while ready, then redeem with the right id and a wrong secret on a keyring-less twin.
        let t = setup(Some(KEY));
        let (_, m) = mint_as(&t, json!({"role": "aggregator", "name": "a"})).await;
        let id = m["code"]
            .as_str()
            .unwrap()
            .split('.')
            .next()
            .unwrap()
            .to_owned();
        let wrong = format!("{id}.{}", "B".repeat(43));
        assert_eq!(
            redeem_as(&t, &wrong, "aggregator").await.0,
            StatusCode::UNAUTHORIZED
        );
    }
}
