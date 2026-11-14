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
//! | controller | every token this controller runs with, the cluster keyring, the Raft token and a join proof |
//!
//! A controller code is the odd one out: redeeming it only hands over the files, the new node
//! then starts with `--ha-join` and asks to be added (`POST /enroll/ha-join`, shown the join
//! proof that came with the files), and the cluster runs the member add itself, so no admin
//! token is needed on the new host. The node id is chosen when the code is minted and is unique
//! across codes (the replicated store refuses a second code with the same id).
//! A controller code with several uses and a `name_prefix` is a pool for a StatefulSet: the node id
//! is allocated when each pod redeems (in the same atomic store step that spends the use and
//! writes a child record), and a name enrolls once.
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

use crate::ha::members::Joiner;
use crate::ha::{WriteRequest, WriteResponse};
use crate::ui_data::api::{actor, err, internal, now, UiState};
use crate::ui_data::{self, Applied, Kind, PoolChild, UiOp};

/// Default and longest code lifetime, in seconds.
const DEFAULT_TTL: u64 = 900;
const MAX_TTL: u64 = 7 * 24 * 3600;
/// Codes for the roles that receive the keyring or the admin token live at most a day.
const MAX_STEP_UP_TTL: u64 = 24 * 3600;
/// `/enroll` takes a code and a role; anything bigger is not one.
const MAX_ENROLL_BODY: usize = 1024;
/// A joining replica retries while the leader works out why it cannot add it, so its limits are
/// looser than the redeem route's (every try costs a request, not a guess at a secret).
const JOIN_PER_IP: u32 = 30;
const JOIN_PER_ID: u32 = 20;

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
    /// This controller's `--tunnel-network`, given to a replica (it must match the cluster's).
    pub tunnel_network: Option<Arc<str>>,
}

#[derive(Clone)]
struct EnrollState {
    ui: UiState,
    grants: Grants,
    limiter: Arc<Limiter>,
    /// Present under HA: what adds a joining controller replica to the Raft group.
    joiner: Option<Joiner>,
}

pub fn router(ui: UiState, grants: Grants) -> Router {
    let auth = middleware::from_fn_with_state(
        wayhouse_http::server::BearerAuth::new(ui.auth_token.as_deref()),
        wayhouse_http::server::require_bearer,
    );
    let st = EnrollState {
        joiner: ui.ha.clone().map(Joiner::new),
        ui,
        grants,
        limiter: Arc::default(),
    };
    let admin = Router::new()
        .route("/enrollments", post(mint).get(list))
        .route("/enrollments/{id}", delete(revoke))
        .route_layer(auth);
    let public = Router::new()
        .route(
            "/enroll",
            post(redeem).layer(DefaultBodyLimit::max(MAX_ENROLL_BODY)),
        )
        .route(
            "/enroll/ha-join",
            post(ha_join).layer(DefaultBodyLimit::max(MAX_ENROLL_BODY)),
        );
    admin.merge(public).with_state(st)
}

/// Applies `op`: locally on a standalone controller, through Raft under HA. Only entries that
/// carry no plaintext secret go through here.
#[allow(clippy::result_large_err)] // axum `Response` is this module's error type
pub(crate) async fn apply(ui: &UiState, op: UiOp) -> Result<Applied, Response> {
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

pub(crate) fn no_store(mut r: Response) -> Response {
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
    /// Agent and proxy only: the code then redeems under any name that starts with this prefix
    /// (and ends the redeem's `name`), one identity per pod. It must end in `-`.
    name_prefix: Option<String>,
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
        Role::Controller if st.ui.ha.is_none() => Some(
            "this controller does not run HA (--ha-peers or --ha-join), so it has no cluster to add a replica to",
        ),
        Role::Controller if st.ui.auth_token.is_none() => Some(
            "this controller has no --auth-token, which every replica has to share",
        ),
        Role::Controller if ring.is_empty() => {
            Some("this controller has no secret key (--secret-key-file) to hand to a replica")
        }
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

/// `POST /enrollments`: `{role?, name, ttl_secs?, uses?, parent_url?, name_prefix?, step_up?}`. The code is
/// in this answer and nowhere else, ever: the store keeps a hash.
async fn mint(State(st): State<EnrollState>, headers: HeaderMap, body: Bytes) -> Response {
    let Ok(b) = serde_json::from_slice::<MintBody>(&body) else {
        return err(
            StatusCode::BAD_REQUEST,
            "the body must be {\"role\"?: \"agent|proxy|aggregator|ui|controller\", \"name\": \"...\", \"ttl_secs\"?, \"uses\"?, \"parent_url\"?, \"name_prefix\"?, \"step_up\"?}",
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
    // A controller code with more than one use is a pool: a StatefulSet's pods each redeem it
    // under their own name and get their own node id. A single-use one has its id fixed at mint.
    let pool = b.role == Role::Controller && b.uses.is_some_and(|u| u > 1);
    if b.role == Role::Controller
        && (b.uses.is_some_and(|u| u > ui_data::POOL_MAX_USES) || pool != b.name_prefix.is_some())
    {
        return err(
            StatusCode::BAD_REQUEST,
            format!(
                "a controller code is single use, or a pool of 2 to {} uses with a name_prefix",
                ui_data::POOL_MAX_USES
            ),
        );
    }
    if let Some(p) = &b.name_prefix {
        let ok = (matches!(b.role, Role::Agent | Role::Proxy) || pool)
            && p.len() > 1
            && p.ends_with('-')
            && crate::registry::validate_name(p).is_ok();
        if !ok {
            return err(
                StatusCode::BAD_REQUEST,
                "name_prefix is for an agent, proxy or pool controller code and must be a name ending in '-'",
            );
        }
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
    if let Some(p) = &b.name_prefix {
        record["name_prefix"] = p.as_str().into();
    }
    // A controller code carries the node id the replica will have. The id is picked above every
    // member and every other code; the store refuses a duplicate atomically, so a clash with a
    // code minted at the same moment elsewhere moves on to the next id.
    let first_id = (b.role == Role::Controller && !pool).then(|| next_node_id(&st));
    let mut node_id = first_id;
    let mut attempts = 0;
    loop {
        if let Some(id) = node_id {
            record["node_id"] = id.into();
        }
        let op = UiOp::Put {
            kind: Kind::Enrollment,
            name: code.id().to_owned(),
            value: record.clone(),
        };
        match apply(&st.ui, op).await {
            Ok(Applied::Done) => break,
            Ok(Applied::Taken) if attempts < 8 => {
                attempts += 1;
                node_id = node_id.map(|i| i + 1);
            }
            Ok(Applied::Taken) => {
                return err(StatusCode::CONFLICT, "could not find a free node id; retry")
            }
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
    }
    tracing::warn!(enrollment = code.id(), role = b.role.as_str(), name = %b.name, actor = who.as_deref().unwrap_or("-"), step_up = b.role.needs_step_up(), "enrollment code minted");
    let mut out = json!({
        "id": code.id(), "code": &*code.expose(), "role": b.role.as_str(), "name": b.name,
        "expires_at": created + ttl, "uses": uses,
    });
    if let Some(id) = node_id {
        out["node_id"] = id.into();
    }
    no_store(Json(out).into_response())
}

/// One above every node in the Raft membership and every controller code (spent or not, so an
/// id is never handed to two nodes).
fn next_node_id(st: &EnrollState) -> u64 {
    let members = st
        .joiner
        .as_ref()
        .map(Joiner::member_ids)
        .unwrap_or_default();
    let codes = st
        .ui
        .store
        .list(Kind::Enrollment)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(_, rec)| rec["node_id"].as_u64());
    members.into_iter().chain(codes).max().unwrap_or(0) + 1
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
            let status = if v["joined"] == true {
                "joined"
            } else if v["uses_left"] == 0 && v["role"] == "controller" && v["uses"] == 1 {
                // Redeemed; the node has not been added to the Raft group yet.
                "joining"
            } else if v["uses_left"] == 0 {
                "used"
            } else if v["expires_at"].as_u64().is_some_and(|e| t >= e) {
                "expired"
            } else {
                "pending"
            };
            if let Some(o) = v.as_object_mut() {
                o.remove("hash");
                o.remove("join_hash");
                o.insert("id".into(), id.into());
                o.insert("status".into(), status.into());
            }
            v
        })
        .collect();
    Json(json!({ "items": items })).into_response()
}

/// `DELETE /enrollments/{id}`: revokes a code, spent or not. A spent single-name agent code
/// also revokes what it issued (the origin's token and tunnel keys), so revoking it cuts the
/// host off. A name-prefix code does not record which names it issued; delete those origins.
async fn revoke(
    State(st): State<EnrollState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let issued = match st.ui.store.get(Kind::Enrollment, &id) {
        Ok(Some(rec))
            if rec["role"] == "agent"
                && rec.get("name_prefix").is_none()
                && rec["uses_left"].as_u64() < rec["uses"].as_u64() =>
        {
            rec["origin"].as_str().map(str::to_owned)
        }
        Ok(_) => None,
        Err(e) => return internal(e),
    };
    let op = UiOp::Delete {
        kind: Kind::Enrollment,
        name: id.clone(),
    };
    match apply(&st.ui, op).await {
        Ok(Applied::Done) => {
            tracing::warn!(enrollment = %id, actor = actor(&headers).as_deref().unwrap_or("-"), "enrollment code revoked");
            if let Some(origin) = issued {
                if let Err(r) = crate::origin::revoke(&st.ui, &origin).await {
                    return r;
                }
            }
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

/// `POST /enroll`: `{code, role, name?}`. Unauthenticated by design, so it is the most guarded route:
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
            "the body must be {\"code\": \"...\", \"role\": \"agent|proxy|aggregator|ui|controller\", \"name\"?: \"...\"}",
        );
    };
    let Ok(code) = Code::parse(&req.code) else {
        return refused();
    };
    let id_key = format!("id:{}", code.id());
    if !st.limiter.take(&id_key, PER_ID) {
        return too_many();
    }
    let (name, parent_url, mut node_id, prefix, pool) =
        match st.ui.store.get(Kind::Enrollment, code.id()) {
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
                    rec["node_id"].as_u64(),
                    rec["name_prefix"].as_str().map(str::to_owned),
                    rec["role"] == "controller" && rec["uses"].as_u64() > Some(1),
                )
            }
            Ok(_) => return refused(),
            Err(e) => return internal(e),
        };
    let role = req.role;
    // A prefix code is for many pods: each names itself, inside the prefix. Checked before the
    // use is spent, and only for the holder of a valid code.
    let name = match (prefix, req.name) {
        (None, None) => name,
        (Some(p), Some(n))
            if n.len() > p.len()
                && n.starts_with(&p)
                && crate::registry::validate_name(&n).is_ok() =>
        {
            n
        }
        (Some(_), Some(n)) if n.len() > crate::registry::NAME_MAX => {
            return err(
                StatusCode::BAD_REQUEST,
                format!(
                    "name too long: at most {} characters",
                    crate::registry::NAME_MAX
                ),
            )
        }
        (Some(p), _) => {
            return err(
                StatusCode::BAD_REQUEST,
                format!("this code needs a name that starts with {p:?}"),
            )
        }
        (None, Some(_)) => {
            return err(
                StatusCode::BAD_REQUEST,
                "this code has a fixed name; do not pass one",
            )
        }
    };
    if let Some(why) = not_ready(&st, role) {
        // Before the use is spent: an operator can fix this and retry with the same code.
        return err(StatusCode::SERVICE_UNAVAILABLE, why);
    }
    // A controller replica also gets a proof it can show later to be added to the Raft group;
    // only its hash is recorded, in the same step that spends the code.
    // A pool code's replica gets a record of its own, so its proof is a fresh code, not one tied
    // to the pool's id.
    let join = (role == Role::Controller).then(|| {
        if pool {
            let c = Code::generate();
            (c.expose(), c.hash(), c.id().to_owned())
        } else {
            let secret = wayhouse_enroll::generate_token();
            let hash = Code::parse(&format!("{}.{secret}", code.id()))
                .expect("a generated token has the secret shape")
                .hash();
            (
                Zeroizing::new(format!("{}.{secret}", code.id())),
                hash,
                code.id().to_owned(),
            )
        }
    });
    let child = join
        .as_ref()
        .filter(|_| pool)
        .map(|(_, hash, id)| PoolChild {
            id: id.clone(),
            name: name.clone(),
            join_hash: hash.clone(),
            min_node_id: next_node_id(&st),
        });
    let op = UiOp::Redeem {
        id: code.id().to_owned(),
        hash: code.hash(),
        now: now(),
        join_hash: join.as_ref().filter(|_| !pool).map(|(_, h, _)| h.clone()),
        pool: child,
    };
    match apply(&st.ui, op).await {
        Ok(Applied::Done) => {}
        Ok(Applied::Allocated(id)) => node_id = Some(id),
        Ok(Applied::Refused) => return refused(),
        Ok(Applied::Full(max)) => {
            return err(
                StatusCode::CONFLICT,
                format!("already {max} enrollment records; revoke some and ask again"),
            )
        }
        Ok(_) => return internal("unexpected answer to a redeem"),
        Err(r) => return r,
    }
    // An agent that enrolls under a name that already has a token takes that identity over.
    let replaced = role == Role::Agent
        && st
            .ui
            .store
            .list(Kind::OriginToken)
            .is_ok_and(|l| l.iter().any(|(_, v)| v["origin"] == name.as_str()));
    let mut files = match grant_files(&st, role, &name).await {
        Ok(f) => f,
        Err(r) => return r,
    };
    if let Some((proof, _, _)) = &join {
        files.insert(
            wayhouse_enroll::files::JOIN_PROOF_FILE.into(),
            proof.to_string(),
        );
    }
    st.limiter.refund(&ip);
    st.limiter.refund(&id_key);
    tracing::warn!(enrollment = code.id(), role = role.as_str(), name = %name, replaced, "enrollment code redeemed");
    no_store(
        Json(RedeemResponse {
            name,
            role,
            files,
            parent_url,
            node_id,
            tunnel_network: (role == Role::Controller)
                .then(|| st.grants.tunnel_network.as_deref().map(str::to_owned))
                .flatten(),
        })
        .into_response(),
    )
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct JoinBody {
    /// The join proof that came with the controller grant (`<id>.<secret>`).
    proof: String,
    /// How the other replicas reach the joining node, as `--ha-peers` takes it.
    addr: String,
}

/// `POST /enroll/ha-join`: `{proof, addr}`. A node that redeemed a controller code asks the
/// cluster to add it. Unauthenticated like `/enroll`, and guarded the same way: 1 KiB body, a
/// per-address and a per-code limit, one answer (401) for every wrong, spent or late proof.
/// Past the proof check the answers name the reason (unreachable address, a node that answers as
/// another id, a log that is not empty) so the operator can fix the address and let the node
/// ask again; the proof stays good for [`crate::ui_data::JOIN_WINDOW_SECS`] after the redeem.
///
/// A follower relays the request to the leader, which re-checks the proof, runs the add and
/// marks the code joined. Once the code is joined the proof is dead (401); a repeat that arrives
/// before the mark (the first answer was lost) answers 200 again. Accepted risk: a proof holder can
/// make the leader send `/raft/whoami` with the Raft bearer to any https host (or loopback over
/// http) and read the reason back, a port probe by someone who already holds the cluster's secrets.
async fn ha_join(
    State(st): State<EnrollState>,
    ConnectInfo(peer): ConnectInfo<wayhouse_http::tls::PeerAddr>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let ip = format!("join-{}", ip_key(peer.0.ip()));
    if !st.limiter.take(&ip, JOIN_PER_IP) {
        return too_many();
    }
    let Ok(req) = serde_json::from_slice::<JoinBody>(&body) else {
        return err(
            StatusCode::BAD_REQUEST,
            "the body must be {\"proof\": \"...\", \"addr\": \"...\"}",
        );
    };
    let Some(joiner) = &st.joiner else {
        return refused();
    };
    let Ok(proof) = Code::parse(&req.proof) else {
        return refused();
    };
    let id_key = format!("join:{}", proof.id());
    if !st.limiter.take(&id_key, JOIN_PER_ID) {
        return too_many();
    }
    let rec = match st.ui.store.get(Kind::Enrollment, proof.id()) {
        Ok(Some(rec))
            if rec["role"] == "controller"
                && rec["joined"] != true
                && rec["join_until"].as_u64().is_some_and(|u| now() < u)
                && rec["join_hash"]
                    .as_str()
                    .is_some_and(|h| wayhouse_http::token_eq(h, &proof.hash())) =>
        {
            rec
        }
        Ok(_) => return refused(),
        Err(e) => return internal(e),
    };
    let Some(node_id) = rec["node_id"].as_u64() else {
        return refused();
    };
    if let Err(relayed) = joiner
        .on_leader(
            "/enroll/ha-join",
            &headers,
            String::from_utf8_lossy(&body).into_owned(),
        )
        .await
    {
        return relayed;
    }
    let who = format!("enrollment {}", proof.id());
    let added = match joiner.add(node_id, &req.addr, &who).await {
        Ok(added) => added,
        Err(why) => return why,
    };
    let mut done = rec;
    done["joined"] = true.into();
    if let Some(o) = done.as_object_mut() {
        o.remove("join_hash");
        o.remove("join_until");
    }
    let op = UiOp::Put {
        kind: Kind::Enrollment,
        name: proof.id().to_owned(),
        value: done,
    };
    match apply(&st.ui, op).await {
        Ok(Applied::Done) => {}
        Ok(other) => return internal(format!("marking the code joined answered {other:?}")),
        Err(r) => return r,
    }
    st.limiter.refund(&ip);
    st.limiter.refund(&id_key);
    tracing::warn!(enrollment = proof.id(), node_id, addr = %req.addr, added, "controller replica joined the HA group by enrollment");
    no_store(Json(json!({ "node_id": node_id, "joined": true })).into_response())
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
            // A re-enroll starts over at generation 0: no key of an earlier life may linger.
            crate::origin::forget_generations_except(&st.ui, name, 0).await?;
            put("tunnel-psk", Some(&psk));
            // With auth on, the origin gets a token of its own (and the old one stops working);
            // the shared agent token is not handed out any more.
            let own = match st.ui.auth_token {
                Some(_) => Some(crate::origin::issue_token(&st.ui, name).await?),
                None => None,
            };
            put(
                "agent-token",
                own.as_deref()
                    .map(String::as_str)
                    .or(g.agent_token.as_deref()),
            );
        }
        Role::Proxy => {
            put("controller-token", g.proxy_token.as_deref());
            put("aggregator-token", g.ingest_token.as_deref());
            put("secret.keys", Some(&st.ui.keyring.current().export()));
        }
        Role::Aggregator => put("ingest-token", g.ingest_token.as_deref()),
        Role::Ui => put("controller-token", st.ui.auth_token.as_deref()),
        Role::Controller => {
            put(
                "ha-token",
                st.ui.ha.as_ref().and_then(|h| h.ha_token.as_deref()),
            );
            put("secret.keys", Some(&st.ui.keyring.current().export()));
            put("auth-token", st.ui.auth_token.as_deref());
            put("proxy-token", g.proxy_token.as_deref());
            put("agent-token", g.agent_token.as_deref());
            put("ingest-token", g.ingest_token.as_deref());
            put("enroll-step-up", g.step_up.as_deref());
        }
    }
    Ok(f)
}

/// Seals `psk` as the origin's tunnel secret and replicates the ciphertext.
#[allow(clippy::result_large_err)]
async fn store_psk(ui: &UiState, origin: &str, psk: &Zeroizing<String>) -> Result<(), Response> {
    store_psk_as(ui, wayhouse_enroll::psk_secret_name(origin), psk).await
}

/// Seals `psk` as the cluster secret `name` (one generation of an origin's tunnel key).
#[allow(clippy::result_large_err)]
pub(crate) async fn store_psk_as(
    ui: &UiState,
    name: String,
    psk: &Zeroizing<String>,
) -> Result<(), Response> {
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
        ha: Option<Arc<crate::ha::HaHandle>>,
        _d: tempfile::TempDir,
    }

    fn setup(key: Option<&str>) -> T {
        setup_with(key, Some(ADMIN))
    }

    fn setup_with(key: Option<&str>, admin: Option<&str>) -> T {
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
            auth_token: admin.map(Arc::from),
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
                    tunnel_network: Some(Arc::from("10.60.0.0/24")),
                },
            ),
            store,
            keyring,
            ha: None,
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
        // The origin gets a token of its own, never the shared one.
        let own = v["files"]["agent-token"].as_str().unwrap();
        assert_ne!(own, AGENT);
        let find = crate::origin::lookup(t.store.clone());
        assert_eq!(find(own).as_deref(), Some("home-nas"));
        assert_eq!(v["name"].as_str(), Some("home-nas"));
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
        // Enrolling the same name again replaces the token: the old one stops working.
        let again = mint(&t, "home-nas").await;
        let (_, w) = redeem(&t, &again, "10.0.0.3").await;
        let newer = w["files"]["agent-token"].as_str().unwrap();
        assert_ne!(newer, own);
        assert_eq!(find(own), None);
        assert_eq!(find(newer).as_deref(), Some("home-nas"));
    }

    #[tokio::test]
    async fn revoking_a_spent_agent_code_revokes_its_token_and_tunnel_key() {
        let t = setup(Some(KEY));
        let code = mint(&t, "home-nas").await;
        let (_, v) = redeem(&t, &code, "10.0.0.2").await;
        let token = v["files"]["agent-token"].as_str().unwrap().to_owned();
        let find = crate::origin::lookup(t.store.clone());
        assert_eq!(find(&token).as_deref(), Some("home-nas"));
        let id = code.split('.').next().unwrap();
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
        assert_eq!(find(&token), None);
        assert!(t.store.list(Kind::ClusterSecret).unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_controller_without_auth_hands_out_no_token_and_keeps_enrolling_agents() {
        let t = setup_with(Some(KEY), None);
        let code = mint(&t, "home-nas").await;
        let (st, v) = redeem(&t, &code, "10.0.0.2").await;
        assert_eq!(st, StatusCode::OK, "{v}");
        assert!(v["files"]["agent-token"].is_null() || v["files"]["agent-token"] == AGENT);
        assert!(v["files"]["tunnel-psk"].is_string());
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

    async fn redeem_named(t: &T, code: &str, role: &str, name: Value) -> (StatusCode, Value) {
        call(
            &t.app,
            "POST",
            "/enroll",
            json!({"code": code, "role": role, "name": name}),
            None,
            "10.0.0.2",
        )
        .await
    }

    #[tokio::test]
    async fn a_prefix_code_gives_each_pod_its_own_name_and_token_within_the_prefix() {
        let t = setup(Some(KEY));
        let (st, m) = mint_as(
            &t,
            json!({"role": "agent", "name": "pods", "name_prefix": "k8s-", "uses": 3}),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{m}");
        let code = m["code"].as_str().unwrap();
        let mut tokens = Vec::new();
        for n in ["k8s-node-a", "k8s-node-b"] {
            let (st, v) = redeem_named(&t, code, "agent", n.into()).await;
            assert_eq!(st, StatusCode::OK, "{v}");
            assert_eq!(v["name"], n);
            tokens.push(v["files"]["agent-token"].as_str().unwrap().to_owned());
        }
        assert_ne!(tokens[0], tokens[1], "one token per pod");
        // Outside the prefix, the prefix alone, a bad name and no name are refused without
        // spending a use.
        for bad in [
            json!("other-node"),
            json!("k8s-"),
            json!("k8s-bad name"),
            Value::Null,
        ] {
            let (st, _) = redeem_named(&t, code, "agent", bad).await;
            assert_eq!(st, StatusCode::BAD_REQUEST);
        }
        assert_eq!(
            redeem_named(&t, code, "agent", "k8s-node-c".into()).await.0,
            StatusCode::OK,
            "the refused names left the last use"
        );
        assert_eq!(
            redeem_named(&t, code, "agent", "k8s-node-d".into()).await.0,
            StatusCode::UNAUTHORIZED,
            "out of uses"
        );
    }

    #[tokio::test]
    async fn a_name_over_the_limit_says_so_and_a_re_enroll_replaces_the_token() {
        let t = setup(Some(KEY));
        let (_, m) = mint_as(
            &t,
            json!({"role": "agent", "name": "pods", "name_prefix": "k8s-", "uses": 4}),
        )
        .await;
        let code = m["code"].as_str().unwrap();
        let long = format!("k8s-{}", "x".repeat(70));
        let (st, v) = redeem_named(&t, code, "agent", long.into()).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        assert!(v["error"].as_str().unwrap().contains("too long"), "{v}");
        let (_, first) = redeem_named(&t, code, "agent", "k8s-a".into()).await;
        let (_, again) = redeem_named(&t, code, "agent", "k8s-a".into()).await;
        assert_ne!(first["files"]["agent-token"], again["files"]["agent-token"]);
    }

    #[tokio::test]
    async fn a_name_is_only_for_a_prefix_code_and_a_prefix_only_for_agents_and_proxies() {
        let t = setup(Some(KEY));
        let code = mint(&t, "plain").await;
        let (st, _) = redeem_named(&t, &code, "agent", "plain".into()).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "a plain code has a fixed name");
        for body in [
            json!({"role": "aggregator", "name": "a", "name_prefix": "k8s-"}),
            json!({"role": "ui", "name": "a", "name_prefix": "k8s-"}),
            json!({"role": "controller", "name": "a", "name_prefix": "k8s-", "step_up": STEP_UP}),
            json!({"role": "agent", "name": "a", "name_prefix": "k8s"}),
            json!({"role": "agent", "name": "a", "name_prefix": "bad prefix-"}),
            json!({"role": "agent", "name": "a", "name_prefix": ""}),
        ] {
            assert_eq!(
                mint_as(&t, body.clone()).await.0,
                StatusCode::BAD_REQUEST,
                "{body}"
            );
        }
        let (st, m) = mint_as(
            &t,
            json!({"role": "proxy", "name": "p", "name_prefix": "edge-", "uses": 50, "step_up": STEP_UP}),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{m}");
        let (st, v) =
            redeem_named(&t, m["code"].as_str().unwrap(), "proxy", "edge-n1".into()).await;
        assert_eq!(st, StatusCode::OK, "{v}");
        assert_eq!(v["name"], "edge-n1");
    }

    #[tokio::test]
    async fn a_multi_use_prefix_code_stops_working_after_its_ttl() {
        let t = setup(Some(KEY));
        let (_, m) = mint_as(
            &t,
            json!({"role": "agent", "name": "pods", "name_prefix": "k8s-", "uses": 5}),
        )
        .await;
        let code = m["code"].as_str().unwrap();
        let id = code.split('.').next().unwrap();
        assert_eq!(
            redeem_named(&t, code, "agent", "k8s-a".into()).await.0,
            StatusCode::OK
        );
        let mut rec = t.store.get(Kind::Enrollment, id).unwrap().unwrap();
        rec["expires_at"] = 1.into();
        t.store
            .apply_local(&UiOp::Put {
                kind: Kind::Enrollment,
                name: id.into(),
                value: rec,
            })
            .unwrap();
        let (st, _) = redeem_named(&t, code, "agent", "k8s-b".into()).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED, "uses left but expired");
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

    // ---- controller replicas (HA join) ----

    /// A one-voter HA group (node 1, the leader) behind the enrollment routes.
    async fn setup_ha() -> T {
        let (ha, store, dir) =
            crate::ha::test_support::single_node_ui(1, "127.0.0.1:1", "ha-token-ha-token-ha").await;
        let keyring =
            wayhouse_secrets::KeyringHandle::new(wayhouse_secrets::Keyring::parse(KEY).unwrap());
        let ui = UiState {
            store: store.clone(),
            auth_token: Some(Arc::from(ADMIN)),
            ha: Some(ha.clone()),
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
                    tunnel_network: Some(Arc::from("10.60.0.0/24")),
                },
            ),
            store,
            keyring,
            ha: Some(ha),
            _d: dir,
        }
    }

    /// Rewrites a record through the Raft log (the store's own cursor must not be touched).
    async fn put_through_raft(t: &T, id: &str, rec: Value) {
        let op = UiOp::Put {
            kind: Kind::Enrollment,
            name: id.into(),
            value: rec,
        };
        let ha = t.ha.as_ref().unwrap();
        let r = crate::plugins::peer::propose_ciphertext(ha, WriteRequest::Ui(op))
            .await
            .unwrap();
        assert!(matches!(r, WriteResponse::UiResult(Applied::Done)), "{r:?}");
    }

    async fn mint_controller(t: &T, name: &str) -> (StatusCode, Value) {
        mint_as(
            t,
            json!({"role": "controller", "name": name, "step_up": STEP_UP}),
        )
        .await
    }

    async fn join(t: &T, proof: &str, addr: &str, ip: &str) -> (StatusCode, Value) {
        call(
            &t.app,
            "POST",
            "/enroll/ha-join",
            json!({"proof": proof, "addr": addr}),
            None,
            ip,
        )
        .await
    }

    /// A stub that answers `/raft/whoami` as node `id`.
    async fn stub_node(id: u64) -> String {
        let app = Router::new().route(
            "/raft/whoami",
            axum::routing::get(move || async move {
                Json(crate::ha::routes::Whoami {
                    node_id: id,
                    log_empty: true,
                    pre_ha: crate::ha::import::PreHaSummary::default(),
                    plugin_support: 0,
                    secret_key_ids: Vec::new(),
                    acme_support: 0,
                    confirm_support: 0,
                    ui_support: 0,
                })
            }),
        );
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = format!("http://{}", l.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        addr
    }

    #[tokio::test]
    async fn a_controller_code_needs_ha_the_step_up_secret_and_is_single_use() {
        // Standalone: there is no cluster to join.
        let t = setup(Some(KEY));
        assert_eq!(mint_controller(&t, "c2").await.0, StatusCode::CONFLICT);
        let t = setup_ha().await;
        let (st, _) = mint_as(&t, json!({"role": "controller", "name": "c2"})).await;
        assert_eq!(st, StatusCode::FORBIDDEN);
        let (st, _) = mint_as(
            &t,
            json!({"role": "controller", "name": "c2", "step_up": STEP_UP, "uses": 2}),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        assert!(t.store.list(Kind::Enrollment).unwrap().is_empty());
    }

    #[tokio::test]
    async fn node_ids_start_above_the_members_and_never_repeat_even_when_minted_at_once() {
        let t = setup_ha().await;
        let (st, m) = mint_controller(&t, "c2").await;
        assert_eq!(st, StatusCode::OK, "{m}");
        assert_eq!(m["node_id"], 2, "node 1 is a member");
        let tasks: Vec<_> = (0..6)
            .map(|i| {
                let app = t.app.clone();
                tokio::spawn(async move {
                    call(
                        &app,
                        "POST",
                        "/enrollments",
                        json!({"role": "controller", "name": format!("c{i}"), "step_up": STEP_UP}),
                        Some(ADMIN),
                        "10.0.0.1",
                    )
                    .await
                })
            })
            .collect();
        let mut ids = vec![2u64];
        for task in tasks {
            let (st, v) = task.await.unwrap();
            assert_eq!(st, StatusCode::OK, "{v}");
            ids.push(v["node_id"].as_u64().unwrap());
        }
        let mut unique = ids.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), ids.len(), "{ids:?}");
        assert!(ids.iter().all(|i| *i >= 2));
    }

    #[tokio::test]
    async fn the_store_refuses_a_second_controller_code_with_the_same_node_id() {
        let t = setup_ha().await;
        let (_, m) = mint_controller(&t, "c2").await;
        let id = m["id"].as_str().unwrap();
        let mut dup = t.store.get(Kind::Enrollment, id).unwrap().unwrap();
        dup["origin"] = "other".into();
        let applied = t
            .store
            .apply_local(&UiOp::Put {
                kind: Kind::Enrollment,
                name: "ffffffff".into(),
                value: dup,
            })
            .unwrap();
        assert_eq!(applied, Applied::Taken);
    }

    #[tokio::test]
    async fn a_controller_code_redeems_into_every_token_the_keyring_the_node_id_and_a_join_proof() {
        let t = setup_ha().await;
        let (_, m) = mint_controller(&t, "c2").await;
        let (st, v) = redeem_as(&t, m["code"].as_str().unwrap(), "controller").await;
        assert_eq!(st, StatusCode::OK, "{v}");
        assert_eq!(v["node_id"], 2);
        assert_eq!(v["tunnel_network"], "10.60.0.0/24");
        assert_eq!(
            files(&v),
            [
                "agent-token",
                "auth-token",
                "enroll-step-up",
                "ha-join-proof",
                "ha-token",
                "ingest-token",
                "proxy-token",
                "secret.keys"
            ]
        );
        assert_eq!(v["files"]["auth-token"], ADMIN);
        assert_eq!(v["files"]["ha-token"], "ha-token-ha-token-ha");
        // Only a hash of the proof is recorded.
        let proof = v["files"]["ha-join-proof"].as_str().unwrap();
        let secret = proof.split_once('.').unwrap().1;
        let stored = serde_json::to_string(&t.store.list(Kind::Enrollment).unwrap()).unwrap();
        assert!(stored.contains("join_hash") && !stored.contains(secret));
        let (_, list) = call(
            &t.app,
            "GET",
            "/enrollments",
            Value::Null,
            Some(ADMIN),
            "10.0.0.1",
        )
        .await;
        assert_eq!(list["items"][0]["status"], "joining");
        assert!(!list.to_string().contains("join_hash"));
        // The same code does not redeem twice, and the controller role is not redeemable as another.
        assert_eq!(
            redeem_as(&t, m["code"].as_str().unwrap(), "controller")
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
    }

    async fn redeemed(t: &T) -> (String, String) {
        let (_, m) = mint_controller(t, "c2").await;
        let (_, v) = redeem_as(t, m["code"].as_str().unwrap(), "controller").await;
        (
            v["files"]["ha-join-proof"].as_str().unwrap().to_owned(),
            m["id"].as_str().unwrap().to_owned(),
        )
    }

    #[tokio::test]
    async fn a_join_with_a_wrong_unknown_or_unredeemed_proof_is_refused_alike() {
        let t = setup_ha().await;
        let (proof, id) = redeemed(&t).await;
        // Answers as another node, so a valid proof stops at the identity check.
        let addr = stub_node(9).await;
        let (_, unredeemed) = mint_controller(&t, "c3").await;
        let wrong = format!("{id}.{}", "B".repeat(43));
        let unknown = format!("ffffffff.{}", "A".repeat(43));
        let mut answers = Vec::new();
        for p in [
            wrong.as_str(),
            unknown.as_str(),
            "garbage",
            // The code itself is not a proof.
            unredeemed["code"].as_str().unwrap(),
        ] {
            let (st, v) = join(&t, p, &addr, "10.0.0.5").await;
            answers.push((st, v["error"].as_str().unwrap_or("").to_owned()));
        }
        assert!(
            answers.iter().all(|a| a.0 == StatusCode::UNAUTHORIZED),
            "{answers:?}"
        );
        assert!(answers.windows(2).all(|w| w[0] == w[1]), "{answers:?}");
        // The right proof is not burnt by those tries.
        assert_ne!(
            join(&t, &proof, &addr, "10.0.0.5").await.0,
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn a_join_to_the_wrong_address_names_the_reason_and_can_be_tried_again() {
        let t = setup_ha().await;
        let (proof, _) = redeemed(&t).await;
        // Nothing listens there.
        let (st, v) = join(&t, &proof, "http://127.0.0.1:1", "10.0.0.5").await;
        assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE, "{v}");
        assert!(
            v["error"].as_str().unwrap().contains("did not answer"),
            "{v}"
        );
        // Something else answers as another node.
        let other = stub_node(9).await;
        let (st, v) = join(&t, &proof, &other, "10.0.0.5").await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
        assert!(v["error"].as_str().unwrap().contains("node 9"), "{v}");
        // A plain-http address to another host is refused before the Raft token is sent there.
        let (st, _) = join(&t, &proof, "http://203.0.113.9:9901", "10.0.0.5").await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
        // None of this spent the proof.
        let (_, list) = call(
            &t.app,
            "GET",
            "/enrollments",
            Value::Null,
            Some(ADMIN),
            "10.0.0.1",
        )
        .await;
        assert_eq!(list["items"][0]["status"], "joining");
    }

    async fn mint_pool(t: &T, uses: u64) -> Value {
        let (st, m) = mint_as(
            t,
            json!({"role": "controller", "name": "ctl-pool", "name_prefix": "ctl-",
                   "uses": uses, "step_up": STEP_UP}),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{m}");
        m
    }

    #[tokio::test]
    async fn a_pool_code_enrolls_each_named_pod_with_its_own_node_id_and_proof() {
        let t = setup_ha().await;
        let m = mint_pool(&t, 3).await;
        assert!(m.get("node_id").is_none(), "{m}");
        let code = m["code"].as_str().unwrap();
        let (st, a) = redeem_named(&t, code, "controller", json!("ctl-1")).await;
        assert_eq!(st, StatusCode::OK, "{a}");
        let (st, b) = redeem_named(&t, code, "controller", json!("ctl-2")).await;
        assert_eq!(st, StatusCode::OK, "{b}");
        assert_eq!((&a["node_id"], &b["node_id"]), (&json!(2), &json!(3)));
        assert_eq!((&a["name"], &b["name"]), (&json!("ctl-1"), &json!("ctl-2")));
        let (pa, pb) = (
            a["files"]["ha-join-proof"].as_str().unwrap(),
            b["files"]["ha-join-proof"].as_str().unwrap(),
        );
        assert_ne!(pa.split_once('.').unwrap().0, pb.split_once('.').unwrap().0);
        assert_ne!(pa.split_once('.').unwrap().0, m["id"].as_str().unwrap());
        assert!(a["files"]["secret.keys"].is_string());
        // The proof passes the 401 gate (it reaches the identity check) and names its own node.
        let other = stub_node(9).await;
        let (st, v) = join(&t, pa, &other, "10.0.0.5").await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
    }

    #[tokio::test]
    async fn a_pool_name_enrolls_once_and_a_bad_or_missing_name_spends_nothing() {
        let t = setup_ha().await;
        let m = mint_pool(&t, 3).await;
        let code = m["code"].as_str().unwrap();
        assert_eq!(
            redeem_named(&t, code, "controller", json!("ctl-1")).await.0,
            StatusCode::OK
        );
        assert_eq!(
            redeem_named(&t, code, "controller", json!("ctl-1")).await.0,
            StatusCode::UNAUTHORIZED,
            "a name that already has a replica"
        );
        for name in [json!("other-1"), json!(null)] {
            assert_eq!(
                redeem_named(&t, code, "controller", name).await.0,
                StatusCode::BAD_REQUEST
            );
        }
        let pool = t
            .store
            .get(Kind::Enrollment, m["id"].as_str().unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(pool["uses_left"], 2, "only the one good redeem spent a use");
    }

    #[tokio::test]
    async fn a_pool_code_needs_a_prefix_and_at_most_sixteen_uses_and_a_single_use_code_takes_no_prefix(
    ) {
        let t = setup_ha().await;
        for body in [
            json!({"role": "controller", "name": "p", "uses": 3, "step_up": STEP_UP}),
            json!({"role": "controller", "name": "p", "name_prefix": "ctl-", "uses": 17, "step_up": STEP_UP}),
            json!({"role": "controller", "name": "p", "name_prefix": "ctl-", "step_up": STEP_UP}),
        ] {
            assert_eq!(mint_as(&t, body).await.0, StatusCode::BAD_REQUEST);
        }
        assert!(t.store.list(Kind::Enrollment).unwrap().is_empty());
        let (st, _) = mint_as(
            &t,
            json!({"role": "controller", "name": "ctl-pool", "name_prefix": "ctl-", "uses": 2}),
        )
        .await;
        assert_eq!(
            st,
            StatusCode::FORBIDDEN,
            "a pool needs the step-up secret too"
        );
    }

    #[tokio::test]
    async fn the_listing_shows_a_pool_and_each_replica_it_made() {
        let t = setup_ha().await;
        let m = mint_pool(&t, 2).await;
        for n in ["ctl-1", "ctl-2"] {
            redeem_named(&t, m["code"].as_str().unwrap(), "controller", json!(n)).await;
        }
        let (_, list) = call(
            &t.app,
            "GET",
            "/enrollments",
            Value::Null,
            Some(ADMIN),
            "10.0.0.1",
        )
        .await;
        let items = list["items"].as_array().unwrap();
        assert_eq!(items.len(), 3, "{list}");
        let mut seen: Vec<_> = items
            .iter()
            .map(|i| (i["origin"].as_str().unwrap(), i["status"].as_str().unwrap()))
            .collect();
        seen.sort_unstable();
        assert_eq!(
            seen,
            [
                ("ctl-1", "joining"),
                ("ctl-2", "joining"),
                ("ctl-pool", "used")
            ]
        );
        assert!(!list.to_string().contains("join_hash"));
    }

    #[tokio::test]
    async fn a_join_after_the_window_is_refused() {
        let t = setup_ha().await;
        let (proof, id) = redeemed(&t).await;
        let mut rec = t.store.get(Kind::Enrollment, &id).unwrap().unwrap();
        rec["join_until"] = 1.into();
        put_through_raft(&t, &id, rec).await;
        let addr = stub_node(2).await;
        assert_eq!(
            join(&t, &proof, &addr, "10.0.0.5").await.0,
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn a_node_that_is_a_voter_already_joins_again_without_effect_and_is_marked_joined() {
        let t = setup_ha().await;
        let (_, m) = mint_controller(&t, "c2").await;
        // Pretend the cluster handed this code node id 1, which is the running voter.
        let id = m["id"].as_str().unwrap().to_owned();
        let mut rec = t.store.get(Kind::Enrollment, &id).unwrap().unwrap();
        rec["node_id"] = 1.into();
        put_through_raft(&t, &id, rec).await;
        let (_, v) = redeem_as(&t, m["code"].as_str().unwrap(), "controller").await;
        let proof = v["files"]["ha-join-proof"].as_str().unwrap();
        // Another address for that id is another node.
        let other = stub_node(1).await;
        let (st, v) = join(&t, proof, &other, "10.0.0.5").await;
        assert_eq!(st, StatusCode::CONFLICT, "{v}");
        let addr = "127.0.0.1:1".to_owned();
        let (st, v) = join(&t, proof, &addr, "10.0.0.5").await;
        assert_eq!(st, StatusCode::OK, "{v}");
        let (_, list) = call(
            &t.app,
            "GET",
            "/enrollments",
            Value::Null,
            Some(ADMIN),
            "10.0.0.1",
        )
        .await;
        assert_eq!(list["items"][0]["status"], "joined");
        // The proof is dead once the node is in.
        assert_eq!(
            join(&t, proof, &addr, "10.0.0.5").await.0,
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn guesses_at_one_join_proof_are_limited_across_addresses() {
        let t = setup_ha().await;
        let (_, id) = redeemed(&t).await;
        let guess = format!("{id}.{}", "B".repeat(43));
        let mut limited = false;
        for i in 0..80 {
            let (st, _) = join(
                &t,
                &guess,
                "http://127.0.0.1:1",
                &format!("10.7.{}.1", i % 250),
            )
            .await;
            if st == StatusCode::TOO_MANY_REQUESTS {
                limited = true;
                break;
            }
        }
        assert!(limited);
    }
}
