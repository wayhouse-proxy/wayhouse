//! What an enrolled origin holds: its own bearer token and its tunnel key, and how the key is
//! rotated.
//!
//! **Token.** Redeeming an agent code issues the origin a random token (replacing the shared
//! `--agent-token` in its `agent-token` file). Only its SHA-256 is stored, in the replicated
//! store (kind `OriginToken`), so the token is found by one read and no copy of it exists on the
//! controller. [`crate::scoped_auth`] turns the token into a [`Caller::Origin`]; the guard on
//! `POST /peers` then holds it to its own name, and refuses the shared agent token for any
//! origin that has one. A re-enroll replaces the token, which revokes the old one.
//!
//! **Key generations.** An origin's tunnel key is a sealed cluster secret, one per generation
//! (`wayhouse_enroll::psk_secret_name_gen`); the registration names the generation proxies use,
//! so a flip is one registration write. `POST /peers/{name}/psk/rotate` seals generation n+1
//! beside the active n. The agent fetches it (`GET /peers/{name}/psk`, its own token), applies
//! it and re-registers naming n+1; that registration is the acknowledgement and the flip, and
//! proxies re-read the secret from the changed registration. While the agent has not
//! acknowledged, n stays active and nothing changes for the proxies. The next rotation removes
//! the generations older than the active one.

use axum::body::Body;
use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use serde_json::json;
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::enroll::{apply, no_store, store_psk_as};
use crate::peers::api::PeersState;
use crate::scoped_auth::{Caller, OriginLookup};
use crate::ui_data::api::{actor, err, internal, now, UiState};
use crate::ui_data::{Applied, Kind, UiOp, UiStore};

/// A registration is a few hundred bytes (64 backends at most); anything bigger is not one.
const MAX_REGISTRATION_BODY: usize = 64 * 1024;

/// The hex SHA-256 a token is stored under.
fn token_hash(token: &str) -> String {
    Sha256::digest(token.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The origin `token` was issued to. Reads the local replica of the store.
pub fn lookup(store: UiStore) -> OriginLookup {
    std::sync::Arc::new(move |token| {
        let rec = store
            .get(Kind::OriginToken, &token_hash(token))
            .ok()
            .flatten()?;
        rec["origin"].as_str().map(str::to_owned)
    })
}

#[allow(clippy::result_large_err)]
fn has_token(store: &UiStore, origin: &str) -> Result<bool, Response> {
    Ok(store
        .list(Kind::OriginToken)
        .map_err(internal)?
        .iter()
        .any(|(_, v)| v["origin"] == origin))
}

/// Issues `origin` a fresh token and revokes any earlier one. The old token goes first: if the
/// new one cannot be stored the origin has none, and an admin mints another code, which is
/// safer than leaving a token that may be the reason for the re-enroll.
#[allow(clippy::result_large_err)]
pub(crate) async fn issue_token(ui: &UiState, origin: &str) -> Result<Zeroizing<String>, Response> {
    let old: Vec<String> = ui
        .store
        .list(Kind::OriginToken)
        .map_err(internal)?
        .into_iter()
        .filter(|(_, v)| v["origin"] == origin)
        .map(|(name, _)| name)
        .collect();
    for name in old {
        done(
            apply(
                ui,
                UiOp::Delete {
                    kind: Kind::OriginToken,
                    name,
                },
            )
            .await?,
        )?;
    }
    let token = Zeroizing::new(wayhouse_enroll::generate_token());
    done(
        apply(
            ui,
            UiOp::Put {
                kind: Kind::OriginToken,
                name: token_hash(&token),
                value: json!({ "origin": origin, "created_at": now() }),
            },
        )
        .await?,
    )?;
    Ok(token)
}

#[allow(clippy::result_large_err)]
fn done(a: Applied) -> Result<(), Response> {
    match a {
        Applied::Done | Applied::Replayed | Applied::NotFound => Ok(()),
        Applied::Full(n) => Err(err(
            StatusCode::INSUFFICIENT_STORAGE,
            format!("the store already holds {n} records of this kind"),
        )),
        other => Err(internal(format!("unexpected answer {other:?}"))),
    }
}

/// Removes every tunnel key generation of `origin` that is not generation `keep`, so a
/// re-enroll leaves no earlier key behind for a later rotation to mistake for its own.
#[allow(clippy::result_large_err)]
pub(crate) async fn forget_generations_except(
    ui: &UiState,
    origin: &str,
    keep: u64,
) -> Result<(), Response> {
    let stale: Vec<String> = ui
        .store
        .list(Kind::ClusterSecret)
        .map_err(internal)?
        .into_iter()
        .map(|(name, _)| name)
        .filter(|name| wayhouse_enroll::psk_secret_gen(origin, name).is_some_and(|g| g != keep))
        .collect();
    for name in stale {
        done(
            apply(
                ui,
                UiOp::Delete {
                    kind: Kind::ClusterSecret,
                    name,
                },
            )
            .await?,
        )?;
    }
    Ok(())
}

#[derive(serde::Deserialize)]
struct Head {
    name: String,
    #[serde(default)]
    psk_secret: Option<String>,
}

/// The layer on the peers router. Everything but `POST /peers` passes untouched. For that one:
/// an origin's own token may register only its own name, the shared agent token may not touch
/// an origin that holds its own token, and a registration may only name a tunnel key that
/// exists, so a typo or a stale generation is refused here instead of silently leaving every
/// proxy without the peer.
pub async fn guard(State(store): State<UiStore>, req: Request, next: Next) -> Response {
    if req.method() != Method::POST || req.uri().path() != "/peers" {
        return next.run(req).await;
    }
    let (parts, body) = req.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, MAX_REGISTRATION_BODY).await else {
        return err(StatusCode::PAYLOAD_TOO_LARGE, "the registration is too big");
    };
    let restricted = matches!(
        parts.extensions.get::<Caller>(),
        Some(Caller::Origin(_) | Caller::SharedAgent)
    );
    match serde_json::from_slice::<Head>(&bytes) {
        Ok(head) => {
            match parts.extensions.get::<Caller>() {
                Some(Caller::Origin(own)) if *own != head.name => {
                    return err(
                        StatusCode::FORBIDDEN,
                        "this token belongs to another origin",
                    );
                }
                Some(Caller::SharedAgent) => match has_token(&store, &head.name) {
                    Ok(false) => {}
                    Ok(true) => {
                        return err(
                            StatusCode::FORBIDDEN,
                            "this origin enrolled and holds its own token; the shared agent token cannot register it",
                        )
                    }
                    Err(r) => return r,
                },
                _ => {}
            }
            if let Some(secret) = &head.psk_secret {
                let exists = wayhouse_enroll::psk_secret_gen(&head.name, secret).is_none()
                    || matches!(store.get(Kind::ClusterSecret, secret), Ok(Some(_)));
                // A name that is not this origin's own is `validate`'s to refuse (422).
                if !exists {
                    return err(
                        StatusCode::CONFLICT,
                        "the tunnel key this registration names does not exist; enroll again or fetch the current one",
                    );
                }
            }
        }
        // The handler refuses a malformed body with its own words; a restricted caller does not
        // get to slip one past the name check.
        Err(_) if restricted => {
            return err(
                StatusCode::UNPROCESSABLE_ENTITY,
                "a registration needs a string name",
            )
        }
        Err(_) => {}
    }
    next.run(Request::from_parts(parts, Body::from(bytes)))
        .await
}

#[derive(Clone)]
struct PskState {
    ui: UiState,
    peers: PeersState,
}

/// `GET /peers/{name}/psk` (the origin's own token or the admin token) and
/// `POST /peers/{name}/psk/rotate` (admin only).
pub fn router(ui: UiState, peers: PeersState) -> Router {
    Router::new()
        .route("/peers/{name}/psk", get(read_current))
        .route("/peers/{name}/psk/rotate", post(rotate))
        // Scoped tokens arrive here already exchanged for the admin token by `promote`.
        .route_layer(axum::middleware::from_fn_with_state(
            wayhouse_http::server::BearerAuth::new(ui.auth_token.as_deref()),
            wayhouse_http::server::require_bearer,
        ))
        .with_state(PskState { ui, peers })
}

/// The generation the registration names (0 when it names none, or the origin never
/// registered).
#[allow(clippy::result_large_err)]
fn active_gen(st: &PskState, name: &str) -> Result<(u64, bool), Response> {
    let reg = st.peers.current_for(name).map_err(internal)?;
    Ok(match reg {
        None => (0, false),
        Some(r) => match r.psk_secret.as_deref() {
            None => (0, false),
            Some(s) => (wayhouse_enroll::psk_secret_gen(name, s).unwrap_or(0), true),
        },
    })
}

#[allow(clippy::result_large_err)]
fn exists(st: &PskState, name: &str, gen: u64) -> Result<bool, Response> {
    Ok(st
        .ui
        .store
        .get(
            Kind::ClusterSecret,
            &wayhouse_enroll::psk_secret_name_gen(name, gen),
        )
        .map_err(internal)?
        .is_some())
}

/// What an origin should hold now: the generation waiting for its acknowledgement, else the
/// active one.
async fn read_current(
    State(st): State<PskState>,
    caller: Option<Extension<Caller>>,
    Path(name): Path<String>,
) -> Response {
    if let Some(Extension(Caller::Origin(own))) = &caller {
        if *own != name {
            return err(
                StatusCode::FORBIDDEN,
                "this token belongs to another origin",
            );
        }
    } else if caller.is_some() {
        return err(StatusCode::FORBIDDEN, "not for this token");
    }
    if crate::registry::validate_name(&name).is_err() {
        return err(StatusCode::NOT_FOUND, "no tunnel key for that name");
    }
    let (active, _) = match active_gen(&st, &name) {
        Ok(a) => a,
        Err(r) => return r,
    };
    let gen = match exists(&st, &name, active + 1) {
        Ok(true) => active + 1,
        Ok(false) => active,
        Err(r) => return r,
    };
    let secret = wayhouse_enroll::psk_secret_name_gen(&name, gen);
    let sealed = match st.ui.store.get(Kind::ClusterSecret, &secret) {
        Ok(Some(rec)) => match serde_json::from_value::<wayhouse_secrets::Sealed>(rec) {
            Ok(s) => s,
            Err(e) => return internal(e),
        },
        Ok(None) => return err(StatusCode::NOT_FOUND, "no tunnel key for that name"),
        Err(e) => return internal(e),
    };
    let Ok(key) = st.ui.keyring.current().open_cluster(&secret, &sealed) else {
        return err(
            StatusCode::SERVICE_UNAVAILABLE,
            "this controller cannot open the tunnel key",
        );
    };
    let Ok(key) = std::str::from_utf8(key.expose()) else {
        return internal("the tunnel key is not text");
    };
    no_store(Json(json!({ "gen": gen, "key": key })).into_response())
}

/// Starts a rotation: seals generation n+1 next to the active n and answers `202`. The
/// proxies keep using n until the agent has fetched and acknowledged n+1.
async fn rotate(
    State(st): State<PskState>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Response {
    if let Err(e) = crate::registry::validate_name(&name) {
        return err(StatusCode::UNPROCESSABLE_ENTITY, e);
    }
    let (active, keyed) = match active_gen(&st, &name) {
        Ok(a) => a,
        Err(r) => return r,
    };
    match st.peers.current_for(&name) {
        Ok(None) => return err(StatusCode::NOT_FOUND, "that origin is not registered"),
        Ok(Some(_)) if !keyed => {
            return err(
                StatusCode::CONFLICT,
                "that origin registered without a tunnel key; enroll it again to give it one",
            )
        }
        Ok(Some(_)) => {}
        Err(e) => return internal(e),
    }
    // The active generation's secret must be there: a re-enroll in flight (or damage) would
    // otherwise have the cleanup below delete what the agent is about to register.
    match exists(&st, &name, active) {
        Ok(true) => {}
        Ok(false) => return err(
            StatusCode::CONFLICT,
            "the active tunnel key is missing (a re-enroll may be in progress); try again shortly",
        ),
        Err(r) => return r,
    }
    match exists(&st, &name, active + 1) {
        Ok(true) => {
            return err(
                StatusCode::CONFLICT,
                format!(
                "generation {} is waiting for the agent to pick it up; the active one is {active}",
                active + 1
            ),
            )
        }
        Ok(false) => {}
        Err(r) => return r,
    }
    if let Err(r) = forget_generations_except(&st.ui, &name, active).await {
        return r;
    }
    let next = active + 1;
    let key = wayhouse_enroll::generate_psk();
    if let Err(r) = store_psk_as(
        &st.ui,
        wayhouse_enroll::psk_secret_name_gen(&name, next),
        &key,
    )
    .await
    {
        return r;
    }
    tracing::warn!(origin = %name, gen = next, actor = ?actor(&headers), "tunnel key rotation started");
    (
        StatusCode::ACCEPTED,
        Json(json!({ "name": name, "active": active, "pending": next })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::body::Body;
    use axum::http::Request;
    use serde_json::Value;
    use tower::ServiceExt;

    use super::*;
    use crate::addresses::{AddressBook, Network};
    use crate::peers::PeerRegistration;
    use crate::store::Store;

    const KEY: &str = "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=";
    const ADMIN: &str = "admin-token-admin-token";

    struct T {
        ui: UiState,
        peers: PeersState,
        _d: tempfile::TempDir,
    }

    fn setup() -> T {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::kv::Db::open(dir.path().join("ui")).unwrap();
        let ui = UiState {
            store: UiStore::open(&db).unwrap(),
            auth_token: Some(Arc::from(ADMIN)),
            ha: None,
            keyring: wayhouse_secrets::KeyringHandle::new(
                wayhouse_secrets::Keyring::parse(KEY).unwrap(),
            ),
        };
        let store = Arc::new(Store::open(&dir.path().join("peers")).unwrap());
        let book = Arc::new(
            AddressBook::open(
                &dir.path().join("addresses"),
                Some(Network::parse("10.60.0.0/16").unwrap()),
            )
            .unwrap(),
        );
        T {
            ui,
            peers: PeersState::new(store, None, book),
            _d: dir,
        }
    }

    impl T {
        /// The psk routes, optionally as a given caller (what `scoped_auth` would have set).
        async fn call(
            &self,
            method: &str,
            uri: &str,
            caller: Option<Caller>,
        ) -> (StatusCode, Value) {
            let mut req = Request::builder()
                .method(method)
                .uri(uri)
                .header("authorization", format!("Bearer {ADMIN}"))
                .body(Body::empty())
                .unwrap();
            if let Some(c) = caller {
                req.extensions_mut().insert(c);
            }
            let resp = router(self.ui.clone(), self.peers.clone())
                .oneshot(req)
                .await
                .unwrap();
            let status = resp.status();
            let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
                .await
                .unwrap();
            (
                status,
                serde_json::from_slice(&bytes).unwrap_or(Value::Null),
            )
        }

        /// What enrollment leaves: generation 0 sealed, and the origin registered naming it.
        async fn enrolled(&self, name: &str) -> String {
            let psk = wayhouse_enroll::generate_psk();
            store_psk_as(&self.ui, wayhouse_enroll::psk_secret_name(name), &psk)
                .await
                .unwrap();
            self.register(name, 0);
            psk.to_string()
        }

        fn register(&self, name: &str, gen: u64) {
            let mut reg = PeerRegistration {
                name: name.into(),
                pubkey: crate::registry::test_pubkey(name),
                endpoint: None,
                backends: vec![],
                tunnel_address: Some("10.60.0.2".into()),
                psk_secret: Some(wayhouse_enroll::psk_secret_name_gen(name, gen)),
            };
            reg.backends.clear();
            self.peers.register_applied(&reg, None).unwrap();
        }
    }

    fn origin(n: &str) -> Option<Caller> {
        Some(Caller::Origin(n.into()))
    }

    #[tokio::test]
    async fn the_key_routes_need_a_token_and_a_wrong_one_is_refused() {
        let t = setup();
        t.enrolled("home").await;
        for (m, u) in [
            ("GET", "/peers/home/psk"),
            ("POST", "/peers/home/psk/rotate"),
        ] {
            for auth in [None, Some("Bearer wrong-wrong-wrong-wrong")] {
                let mut req = Request::builder().method(m).uri(u);
                if let Some(a) = auth {
                    req = req.header("authorization", a);
                }
                let resp = router(t.ui.clone(), t.peers.clone())
                    .oneshot(req.body(Body::empty()).unwrap())
                    .await
                    .unwrap();
                assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "{m} {u} {auth:?}");
            }
        }
        assert_eq!(
            t.ui.store
                .get(
                    Kind::ClusterSecret,
                    &wayhouse_enroll::psk_secret_name_gen("home", 1)
                )
                .unwrap(),
            None,
            "nothing was rotated"
        );
    }

    #[tokio::test]
    async fn rotation_waits_while_the_active_key_is_missing() {
        let t = setup();
        t.enrolled("home").await;
        t.register("home", 3);
        let (st, _) = t.call("POST", "/peers/home/psk/rotate", None).await;
        assert_eq!(st, StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn an_origin_reads_its_own_key_and_nobody_elses() {
        let t = setup();
        let key = t.enrolled("home").await;
        t.enrolled("other").await;
        let (st, v) = t.call("GET", "/peers/home/psk", origin("home")).await;
        assert_eq!(st, StatusCode::OK, "{v}");
        assert_eq!(
            (v["gen"].as_u64(), v["key"].as_str()),
            (Some(0), Some(key.as_str()))
        );
        for caller in [
            origin("other"),
            Some(Caller::SharedAgent),
            Some(Caller::Proxy),
        ] {
            let (st, v) = t.call("GET", "/peers/home/psk", caller.clone()).await;
            assert_eq!(st, StatusCode::FORBIDDEN, "{caller:?}");
            assert!(!v.to_string().contains(&key));
        }
        // The admin token (no caller) reads any origin's.
        assert_eq!(
            t.call("GET", "/peers/home/psk", None).await.0,
            StatusCode::OK
        );
        assert_eq!(
            t.call("GET", "/peers/nobody/psk", None).await.0,
            StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn a_rotation_is_pending_until_the_agent_acknowledges_it() {
        let t = setup();
        let old = t.enrolled("home").await;
        let (st, v) = t.call("POST", "/peers/home/psk/rotate", None).await;
        assert_eq!(st, StatusCode::ACCEPTED, "{v}");
        assert_eq!(
            (v["active"].as_u64(), v["pending"].as_u64()),
            (Some(0), Some(1))
        );
        // The agent is offered generation 1; the registration (what proxies use) still names 0.
        let (_, v) = t.call("GET", "/peers/home/psk", origin("home")).await;
        assert_eq!(v["gen"], 1);
        assert_ne!(v["key"].as_str(), Some(old.as_str()));
        let reg = t.peers.current_for("home").unwrap().unwrap();
        assert_eq!(
            reg.psk_secret,
            Some(wayhouse_enroll::psk_secret_name("home"))
        );
        // A second rotation waits for the first.
        let (st, v) = t.call("POST", "/peers/home/psk/rotate", None).await;
        assert_eq!(st, StatusCode::CONFLICT, "{v}");
        // The acknowledgement is the registration naming generation 1.
        t.register("home", 1);
        let (_, v) = t.call("GET", "/peers/home/psk", origin("home")).await;
        assert_eq!(v["gen"], 1, "generation 1 is now the active one");
        let (st, v) = t.call("POST", "/peers/home/psk/rotate", None).await;
        assert_eq!(st, StatusCode::ACCEPTED, "{v}");
        assert_eq!(
            (v["active"].as_u64(), v["pending"].as_u64()),
            (Some(1), Some(2))
        );
        // The generation before the active one is gone; the active one stays for the proxies.
        let has = |g| {
            t.ui.store
                .get(
                    Kind::ClusterSecret,
                    &wayhouse_enroll::psk_secret_name_gen("home", g),
                )
                .unwrap()
                .is_some()
        };
        assert!(!has(0) && has(1) && has(2));
    }

    #[tokio::test]
    async fn rotation_needs_a_registered_origin_that_holds_a_key() {
        let t = setup();
        assert_eq!(
            t.call("POST", "/peers/ghost/psk/rotate", None).await.0,
            StatusCode::NOT_FOUND
        );
        let mut reg = PeerRegistration {
            name: "plain".into(),
            pubkey: crate::registry::test_pubkey("plain"),
            endpoint: None,
            backends: vec![],
            tunnel_address: Some("10.60.0.3".into()),
            psk_secret: None,
        };
        t.peers.register_applied(&reg, None).unwrap();
        assert_eq!(
            t.call("POST", "/peers/plain/psk/rotate", None).await.0,
            StatusCode::CONFLICT
        );
        reg.name = "bad name".into();
        assert_eq!(
            t.call("POST", "/peers/bad%20name/psk/rotate", None).await.0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }

    #[tokio::test]
    async fn a_token_is_stored_as_a_hash_found_by_one_read_and_replaced_by_a_new_one() {
        let t = setup();
        let first = issue_token(&t.ui, "home").await.unwrap();
        let other = issue_token(&t.ui, "other").await.unwrap();
        let find = lookup(t.ui.store.clone());
        assert_eq!(find(&first).as_deref(), Some("home"));
        assert_eq!(find(&other).as_deref(), Some("other"));
        assert_eq!(find("not-a-token"), None);
        let stored = serde_json::to_string(&t.ui.store.list(Kind::OriginToken).unwrap()).unwrap();
        assert!(!stored.contains(&*first), "only the hash is stored");
        // A re-enroll revokes the old token for good.
        let second = issue_token(&t.ui, "home").await.unwrap();
        assert_ne!(*first, *second);
        assert_eq!(find(&first), None);
        assert_eq!(find(&second).as_deref(), Some("home"));
        assert_eq!(
            find(&other).as_deref(),
            Some("other"),
            "other origins are untouched"
        );
        assert_eq!(t.ui.store.list(Kind::OriginToken).unwrap().len(), 2);
    }

    #[tokio::test]
    async fn re_enrolling_forgets_every_other_key_generation_of_that_origin_only() {
        let t = setup();
        t.enrolled("home").await;
        t.enrolled("other").await;
        t.call("POST", "/peers/home/psk/rotate", None).await;
        t.call("POST", "/peers/other/psk/rotate", None).await;
        forget_generations_except(&t.ui, "home", 0).await.unwrap();
        let has = |o, g| {
            t.ui.store
                .get(
                    Kind::ClusterSecret,
                    &wayhouse_enroll::psk_secret_name_gen(o, g),
                )
                .unwrap()
                .is_some()
        };
        assert!(has("home", 0) && !has("home", 1));
        assert!(
            has("other", 0) && has("other", 1),
            "another origin's generations stay"
        );
    }

    // The guard, with the real peers router behind it.
    async fn register_as(t: &T, caller: Option<Caller>, body: Value) -> (StatusCode, Value) {
        let app = crate::peers::api::router(t.peers.clone()).route_layer(
            axum::middleware::from_fn_with_state(t.ui.store.clone(), guard),
        );
        let mut req = Request::post("/peers")
            .header(wayhouse_http::protocol::HEADER, "1.0")
            .body(Body::from(body.to_string()))
            .unwrap();
        if let Some(c) = caller {
            req.extensions_mut().insert(c);
        }
        let resp = app.oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    fn body(name: &str, gen: Option<u64>) -> Value {
        let mut v =
            json!({"name": name, "pubkey": crate::registry::test_pubkey(name), "backends": []});
        if let Some(g) = gen {
            v["psk_secret"] = json!(wayhouse_enroll::psk_secret_name_gen(name, g));
        }
        v
    }

    #[tokio::test]
    async fn the_guard_holds_an_origin_token_to_its_own_name() {
        let t = setup();
        t.enrolled("home").await;
        let (st, v) = register_as(&t, origin("home"), body("home", Some(0))).await;
        assert_eq!(st, StatusCode::OK, "{v}");
        let (st, _) = register_as(&t, origin("home"), body("other", None)).await;
        assert_eq!(st, StatusCode::FORBIDDEN);
        // Neither a malformed body nor one without a name gets past a restricted caller.
        let (st, _) = register_as(&t, origin("home"), json!({"pubkey": "x"})).await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(t.peers.current_for("other").unwrap().is_none());
    }

    #[tokio::test]
    async fn the_shared_agent_token_cannot_register_an_origin_that_holds_its_own() {
        let t = setup();
        t.enrolled("home").await;
        issue_token(&t.ui, "home").await.unwrap();
        let (st, v) = register_as(&t, Some(Caller::SharedAgent), body("home", Some(0))).await;
        assert_eq!(st, StatusCode::FORBIDDEN, "{v}");
        // Origins without a token of their own keep working with the shared one.
        let (st, v) = register_as(&t, Some(Caller::SharedAgent), body("legacy", None)).await;
        assert_eq!(st, StatusCode::OK, "{v}");
        // And the admin token still registers anything.
        let (st, _) = register_as(&t, None, body("home", Some(0))).await;
        assert_eq!(st, StatusCode::OK);
    }

    #[tokio::test]
    async fn a_registration_may_only_name_a_tunnel_key_that_exists() {
        let t = setup();
        t.enrolled("home").await;
        let (st, _) = register_as(&t, origin("home"), body("home", Some(7))).await;
        assert_eq!(st, StatusCode::CONFLICT, "generation 7 was never made");
        t.call("POST", "/peers/home/psk/rotate", None).await;
        let (st, v) = register_as(&t, origin("home"), body("home", Some(1))).await;
        assert_eq!(st, StatusCode::OK, "{v}");
        let reg = t.peers.current_for("home").unwrap().unwrap();
        assert_eq!(
            reg.psk_secret,
            Some(wayhouse_enroll::psk_secret_name_gen("home", 1))
        );
        // Another origin's key, or a plain secret, is the registry's to refuse.
        let mut b = body("home", None);
        b["psk_secret"] = json!("gossip-psk");
        assert_eq!(
            register_as(&t, origin("home"), b).await.0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }

    #[tokio::test]
    async fn the_guard_leaves_other_routes_and_oversized_bodies_alone() {
        let t = setup();
        let big = json!({"name": "home", "pad": "x".repeat(MAX_REGISTRATION_BODY)});
        let (st, _) = register_as(&t, origin("home"), big).await;
        assert_eq!(st, StatusCode::PAYLOAD_TOO_LARGE);
    }
}
