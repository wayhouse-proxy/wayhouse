//! Confirm-and-revert (A4a): a config submission that could lock the operator out is applied on
//! trial. The controller records the revision to fall back to and a deadline; unless someone
//! confirms the revision before then, the controller re-submits the old bytes as a new revision.
//! A rollback is an ordinary revision, so history is never rewritten (see `crate::api`).
//!
//! The record lives in a `kv` tree next to the revisions and lands in the same transaction as the
//! revision it guards, so no crash leaves a risky revision applied without its timer. A record is
//! live only while its revision is still the latest: any newer revision (a confirm-less edit, the
//! revert itself) retires it without a write.

use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use wayhouse_config::Config;

/// The window applied when the diff is risky and the caller named none.
pub const AUTO_CONFIRM_SECS: u64 = 60;
/// Shortest and longest window a caller may ask for.
pub const MIN_CONFIRM_SECS: u64 = 10;
pub const MAX_CONFIRM_SECS: u64 = 600;

/// Why a diff triggers the automatic window: `(id, what changed)`. The ids are what a response's
/// `reasons` carries, so a client shows the text without keeping its own copy of this list.
pub const RISKS: &[(&str, &str)] = &[
    ("admin_listen", "settings.admin.listen changed"),
    ("admin_auth_token", "settings.admin.auth_token changed"),
    ("admin_tls", "settings.admin.tls changed"),
    ("gossip", "settings.gossip changed"),
    (
        "admin_port_listener",
        "a listener on the admin port was added, changed or removed",
    ),
];

/// The [`RISKS`] ids `new` hits relative to `old` (empty for a safe diff). With no `old` (the
/// first revision) nothing can be locked out.
pub fn lockout_risks(old: Option<&Config>, new: &Config) -> Vec<&'static str> {
    let Some(old) = old else {
        return Vec::new();
    };
    let on_admin_port = |c: &Config, l: &wayhouse_config::ListenerConfig| {
        std::iter::once(&l.bind)
            .chain(&l.extra_binds)
            .any(|b| b.port() == c.admin_listen.port())
    };
    let touched_listener = |from: &Config, to: &Config| {
        from.listeners
            .iter()
            .filter(|l| on_admin_port(from, l))
            .any(|l| !to.listeners.contains(l))
    };
    let hits = [
        old.admin_listen != new.admin_listen,
        old.admin_auth_token != new.admin_auth_token,
        format!("{:?}", old.admin_tls) != format!("{:?}", new.admin_tls),
        format!("{:?}", old.gossip) != format!("{:?}", new.gossip),
        touched_listener(old, new) || touched_listener(new, old),
    ];
    RISKS
        .iter()
        .zip(hits)
        .filter_map(|((id, _), hit)| hit.then_some(*id))
        .collect()
}

/// What a submission records beside its revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Trial {
    /// The revision whose bytes come back if nobody confirms.
    pub revert_to: u64,
    /// Unix milliseconds. The proposer's clock, carried through the Raft log, so every replica
    /// reads the same deadline; controllers need clocks within a second or so of each other.
    pub deadline_ms: u64,
    /// Why the window exists: [`RISKS`] ids, or empty when the caller asked for it.
    pub reasons: Vec<String>,
}

/// A revision on trial.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pending {
    pub revision: u64,
    #[serde(flatten)]
    pub trial: Trial,
}

/// The one key of the `config_pending` tree.
pub const PENDING_KEY: &[u8] = b"pending";

/// How a submission gets its window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Window {
    /// [`AUTO_CONFIRM_SECS`] when the diff is risky, none otherwise.
    Auto,
    /// The caller asked for this many seconds.
    Secs(u64),
    /// Never: the revert itself, or a caller that opted out.
    Off,
}

impl Window {
    /// The `confirm_within` query parameter: absent is [`Window::Auto`], `0` is [`Window::Off`].
    pub fn from_param(param: Option<u64>) -> Self {
        match param {
            None => Window::Auto,
            Some(0) => Window::Off,
            Some(secs) => Window::Secs(secs),
        }
    }
}

/// The trial a submission of `new` starts. `Err` is the message for a `400`.
///
/// `fallback` is the live revision (number and parsed document) the new one replaces; `None`
/// before the first revision, or when the live bytes no longer parse.
pub fn plan(
    window: Window,
    new: &Config,
    fallback: Option<(u64, Option<&Config>)>,
    now_ms: u64,
) -> Result<Option<Trial>, String> {
    let reasons = lockout_risks(fallback.and_then(|(_, c)| c), new);
    let secs = match window {
        Window::Off => return Ok(None),
        Window::Secs(secs) if !(MIN_CONFIRM_SECS..=MAX_CONFIRM_SECS).contains(&secs) => {
            return Err(format!(
                "confirm_within must be {MIN_CONFIRM_SECS} to {MAX_CONFIRM_SECS} seconds"
            ))
        }
        Window::Secs(secs) => secs,
        Window::Auto if reasons.is_empty() => return Ok(None),
        Window::Auto => AUTO_CONFIRM_SECS,
    };
    let Some((revert_to, _)) = fallback else {
        return match window {
            Window::Secs(_) => {
                Err("confirm_within needs a previous revision to fall back to".into())
            }
            _ => Ok(None),
        };
    };
    Ok(Some(Trial {
        revert_to,
        deadline_ms: now_ms.saturating_add(secs * 1000),
        reasons: reasons.into_iter().map(String::from).collect(),
    }))
}

/// The rolling-upgrade gate for trials under HA: every member must run a build that decodes the
/// entries, else an old replica could not apply them (or, as the leader, would never revert).
/// A member that does not answer blocks too, like a plugin install.
pub async fn check_support(ha: &crate::ha::HaHandle) -> Result<(), String> {
    check_support_answers(&crate::plugins::peer::whoami_all(ha).await)
}

/// `Ok` when every answer reports a build that decodes trial entries.
pub fn check_support_answers(
    answers: &[(
        crate::ha::NodeId,
        Result<crate::ha::routes::Whoami, crate::ha::members::WhoamiError>,
    )],
) -> Result<(), String> {
    for (id, answer) in answers {
        match answer {
            Err(why) => {
                return Err(format!(
                    "node {id} did not answer ({why}); a confirm window waits until every member reports its build"
                ))
            }
            Ok(who) if who.confirm_support < crate::ha::routes::CONFIRM_SUPPORT => {
                return Err(format!(
                    "node {id} runs a build without confirm-and-revert; upgrade every replica first"
                ))
            }
            Ok(_) => {}
        }
    }
    Ok(())
}

/// The gate for ending a trial (confirm, revert): only a member that *answered* with a build
/// that cannot decode the entries blocks it. A member that does not answer must not stop a
/// revert, which is what gets the operator back in.
pub async fn check_decoders(ha: &crate::ha::HaHandle) -> Result<(), String> {
    check_decoders_answers(&crate::plugins::peer::whoami_all(ha).await)
}

pub fn check_decoders_answers(
    answers: &[(
        crate::ha::NodeId,
        Result<crate::ha::routes::Whoami, crate::ha::members::WhoamiError>,
    )],
) -> Result<(), String> {
    for (id, answer) in answers {
        if answer
            .as_ref()
            .is_ok_and(|who| who.confirm_support < crate::ha::routes::CONFIRM_SUPPORT)
        {
            return Err(format!(
                "node {id} runs a build without confirm-and-revert; upgrade every replica first"
            ));
        }
    }
    Ok(())
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "pools:\n  - name: mc\n    targets: [\"127.0.0.1:25566\"]\nlisteners:\n  - name: main\n    bind: \"0.0.0.0:25565\"\n    protocol: tcp\n    pool: mc\n";

    fn cfg(extra: &str) -> Config {
        wayhouse_config::parse_str(&format!("{BASE}{extra}")).unwrap()
    }

    fn risks(old: &str, new: &str) -> Vec<&'static str> {
        lockout_risks(Some(&cfg(old)), &cfg(new))
    }

    #[test]
    fn the_first_revision_and_an_unrelated_edit_are_safe() {
        assert!(lockout_risks(None, &cfg("settings:\n  workers: 2\n")).is_empty());
        assert!(risks("", "settings:\n  workers: 2\n").is_empty());
    }

    #[test]
    fn moving_the_admin_listener_or_changing_its_token_is_risky() {
        assert_eq!(
            risks("", "settings:\n  admin:\n    listen: \"0.0.0.0:9999\"\n"),
            ["admin_listen"]
        );
        assert_eq!(
            risks(
                "",
                "settings:\n  admin:\n    auth_token: \"a-long-enough-token-0123456789\"\n"
            ),
            ["admin_auth_token"]
        );
    }

    #[test]
    fn a_gossip_change_is_risky() {
        let gossip = "settings:\n  failure_domain: a\n  gossip:\n    bind: \"10.0.1.4:7946\"\n    psk: \"a-long-random-string\"\n";
        assert_eq!(risks("", gossip), ["gossip"]);
        assert!(risks(gossip, gossip).is_empty());
    }

    #[test]
    fn a_listener_on_the_admin_port_is_risky_only_when_it_changes() {
        // 9900 is the default admin port.
        let listener =
            "  - name: api\n    bind: \"0.0.0.0:9900\"\n    protocol: tcp\n    pool: mc\n";
        let old = cfg("");
        let new = cfg(listener);
        assert_eq!(lockout_risks(Some(&old), &new), ["admin_port_listener"]);
        assert_eq!(lockout_risks(Some(&new), &old), ["admin_port_listener"]);
        assert!(lockout_risks(Some(&new), &new).is_empty());
    }

    #[test]
    fn windows_wait_until_every_member_reports_confirm_support() {
        use crate::ha::members::WhoamiError;
        let who = |confirm_support| crate::ha::routes::Whoami {
            node_id: 2,
            log_empty: false,
            pre_ha: crate::ha::import::PreHaSummary::default(),
            plugin_support: 0,
            secret_key_ids: Vec::new(),
            acme_support: 0,
            confirm_support,
            ui_support: 0,
        };
        let current = crate::ha::routes::CONFIRM_SUPPORT;
        assert!(
            check_support_answers(&[]).is_ok(),
            "a single node has no peers"
        );
        assert!(check_support_answers(&[(2, Ok(who(current)))]).is_ok());
        let old = check_support_answers(&[(2, Ok(who(current))), (3, Ok(who(0)))]).unwrap_err();
        assert!(old.contains("node 3") && old.contains("upgrade"), "{old}");
        let silent = check_support_answers(&[(2, Err(WhoamiError::Unreachable("down".into())))])
            .unwrap_err();
        assert!(silent.contains("node 2"), "{silent}");
    }

    // --- the API, standalone ---

    use crate::api::{router, AppState};
    use crate::role::{Role, RoleHandle};
    use crate::store::Store;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use std::sync::Arc;
    use tower::ServiceExt;

    const MOVED_ADMIN: &str = "settings:\n  admin:\n    listen: \"0.0.0.0:9999\"\n";

    fn state(dir: &std::path::Path) -> AppState {
        let store = Arc::new(Store::open(dir).unwrap());
        AppState::new(store, None, RoleHandle::new(Role::Standalone))
    }

    async fn call(
        app: &axum::Router,
        method: &str,
        uri: &str,
        body: String,
    ) -> (StatusCode, serde_json::Value) {
        let req = Request::builder()
            .method(method)
            .uri(uri)
            .body(Body::from(body))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
        )
    }

    async fn post(app: &axum::Router, uri: &str, doc: &str) -> (StatusCode, serde_json::Value) {
        call(app, "POST", uri, format!("{BASE}{doc}")).await
    }

    async fn pending(app: &axum::Router) -> serde_json::Value {
        call(app, "GET", "/config/pending", String::new()).await.1["pending"].clone()
    }

    async fn current(app: &axum::Router) -> String {
        let resp = app
            .clone()
            .oneshot(Request::get("/config").body(Body::empty()).unwrap())
            .await
            .unwrap();
        String::from_utf8(
            axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn a_risky_diff_starts_the_automatic_window_and_a_safe_one_does_not() {
        let dir = tempfile::tempdir().unwrap();
        let app = router(state(dir.path()));
        assert_eq!(post(&app, "/config", "").await.0, StatusCode::OK);
        assert!(
            pending(&app).await.is_null(),
            "the first revision has nothing to fall back to"
        );
        post(&app, "/config", "settings:\n  workers: 2\n").await;
        assert!(pending(&app).await.is_null());

        post(&app, "/config", MOVED_ADMIN).await;
        let p = pending(&app).await;
        assert_eq!(
            (p["revision"].as_u64(), p["revert_to"].as_u64()),
            (Some(3), Some(2))
        );
        assert_eq!(p["reasons"], serde_json::json!(["admin_listen"]));
        let remaining = p["remaining_ms"].as_u64().unwrap();
        assert!((55_000..=60_000).contains(&remaining), "{remaining}");
    }

    #[tokio::test]
    async fn the_caller_can_ask_for_a_window_within_bounds() {
        let dir = tempfile::tempdir().unwrap();
        let app = router(state(dir.path()));
        post(&app, "/config", "").await;
        let (status, _) = post(
            &app,
            "/config?confirm_within=30",
            "settings:\n  workers: 2\n",
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let p = pending(&app).await;
        assert_eq!(p["reasons"], serde_json::json!([]));
        assert!(p["remaining_ms"].as_u64().unwrap() <= 30_000);

        for bad in [
            "confirm_within=1",
            "confirm_within=100000",
            "confirm_within=30&stage=canary&group=eu",
        ] {
            let (status, body) = post(&app, &format!("/config?{bad}"), "").await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}: {body}");
        }
    }

    #[tokio::test]
    async fn confirm_within_zero_opts_out_even_for_a_risky_diff() {
        let dir = tempfile::tempdir().unwrap();
        let app = router(state(dir.path()));
        post(&app, "/config", "").await;
        let (status, _) = post(&app, "/config?confirm_within=0", MOVED_ADMIN).await;
        assert_eq!(status, StatusCode::OK);
        assert!(pending(&app).await.is_null());
    }

    #[tokio::test]
    async fn a_window_needs_a_revision_to_fall_back_to() {
        let dir = tempfile::tempdir().unwrap();
        let app = router(state(dir.path()));
        let (status, _) = post(&app, "/config?confirm_within=30", "").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn an_unconfirmed_trial_reverts_to_the_old_bytes_as_a_new_revision() {
        let dir = tempfile::tempdir().unwrap();
        let st = state(dir.path());
        let app = router(st.clone());
        post(&app, "/config", "").await;
        post(&app, "/config", MOVED_ADMIN).await;

        assert!(
            !crate::api::revert_expired(&st, now_ms()).await,
            "not yet due"
        );
        assert!(crate::api::revert_expired(&st, now_ms() + 61_000).await);
        assert_eq!(current(&app).await, BASE, "the old bytes are live again");
        assert_eq!(
            st.store.current_revision().unwrap(),
            Some(3),
            "a new revision, history kept"
        );
        assert!(pending(&app).await.is_null());
        assert!(
            !crate::api::revert_expired(&st, now_ms() + 120_000).await,
            "nothing left to revert"
        );
    }

    #[tokio::test]
    async fn confirming_keeps_the_revision() {
        let dir = tempfile::tempdir().unwrap();
        let st = state(dir.path());
        let app = router(st.clone());
        post(&app, "/config", "").await;
        post(&app, "/config", MOVED_ADMIN).await;

        assert_eq!(
            call(&app, "POST", "/config/confirm/1", String::new())
                .await
                .0,
            StatusCode::CONFLICT,
            "revision 1 is not on trial"
        );
        assert_eq!(
            call(&app, "POST", "/config/confirm/2", String::new())
                .await
                .0,
            StatusCode::OK
        );
        assert!(pending(&app).await.is_null());
        assert!(!crate::api::revert_expired(&st, now_ms() + 120_000).await);
        assert_eq!(st.store.current_revision().unwrap(), Some(2));
        assert_eq!(
            call(&app, "POST", "/config/confirm/2", String::new())
                .await
                .0,
            StatusCode::CONFLICT,
            "confirming twice"
        );
    }

    #[tokio::test]
    async fn a_newer_revision_retires_the_trial() {
        let dir = tempfile::tempdir().unwrap();
        let st = state(dir.path());
        let app = router(st.clone());
        post(&app, "/config", "").await;
        post(&app, "/config", MOVED_ADMIN).await;
        let still_moved = format!("{MOVED_ADMIN}  workers: 2\n");
        post(&app, "/config", &still_moved).await;
        assert!(pending(&app).await.is_null());
        assert!(!crate::api::revert_expired(&st, now_ms() + 120_000).await);
        assert_eq!(st.store.current_revision().unwrap(), Some(3));
    }

    #[tokio::test]
    async fn a_revert_decided_before_a_confirm_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let st = state(dir.path());
        let app = router(st.clone());
        post(&app, "/config", "").await;
        post(&app, "/config", MOVED_ADMIN).await;
        let stale = st.pending().unwrap().unwrap();

        // The confirm lands between the timer reading the trial and acting on it.
        assert!(st.confirm(2).unwrap());
        assert_eq!(
            st.revert_trial(stale.revision, Some("auto-revert"))
                .unwrap(),
            None
        );
        assert_eq!(st.store.current_revision().unwrap(), Some(2));
        assert_eq!(current(&app).await, format!("{BASE}{MOVED_ADMIN}"));
    }

    #[tokio::test]
    async fn a_revert_decided_before_a_newer_edit_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let st = state(dir.path());
        let app = router(st.clone());
        post(&app, "/config", "").await;
        post(&app, "/config", MOVED_ADMIN).await;
        let stale = st.pending().unwrap().unwrap();

        let newer = format!("{MOVED_ADMIN}  workers: 2\n");
        post(&app, "/config", &newer).await;
        assert_eq!(st.revert_trial(stale.revision, None).unwrap(), None);
        assert_eq!(st.store.current_revision().unwrap(), Some(3));
        assert_eq!(current(&app).await, format!("{BASE}{newer}"));
    }

    #[test]
    fn only_a_build_that_answered_without_support_blocks_a_confirm_or_revert() {
        use crate::ha::members::WhoamiError;
        let who = |confirm_support| crate::ha::routes::Whoami {
            node_id: 2,
            log_empty: false,
            pre_ha: crate::ha::import::PreHaSummary::default(),
            plugin_support: 0,
            secret_key_ids: Vec::new(),
            acme_support: 0,
            confirm_support,
            ui_support: 0,
        };
        let current = crate::ha::routes::CONFIRM_SUPPORT;
        // A member that is down may be old, but a revert must not wait for it.
        let down = (3, Err(WhoamiError::Unreachable("down".into())));
        assert!(check_decoders_answers(&[(2, Ok(who(current))), down]).is_ok());
        let old = check_decoders_answers(&[(2, Ok(who(0)))]).unwrap_err();
        assert!(old.contains("node 2") && old.contains("upgrade"), "{old}");
    }

    #[tokio::test]
    async fn revert_now_does_not_wait_for_the_deadline() {
        let dir = tempfile::tempdir().unwrap();
        let app = router(state(dir.path()));
        post(&app, "/config", "").await;
        assert_eq!(
            call(&app, "POST", "/config/revert", String::new()).await.0,
            StatusCode::CONFLICT,
            "nothing on trial"
        );
        post(&app, "/config", MOVED_ADMIN).await;
        assert_eq!(
            call(&app, "POST", "/config/revert", String::new()).await.0,
            StatusCode::OK
        );
        assert_eq!(current(&app).await, BASE);
        assert!(pending(&app).await.is_null());
    }

    #[tokio::test]
    async fn a_restart_during_the_window_keeps_the_deadline() {
        let dir = tempfile::tempdir().unwrap();
        {
            let app = router(state(dir.path()));
            post(&app, "/config", "").await;
            post(&app, "/config", MOVED_ADMIN).await;
        }
        let st = state(dir.path());
        let app = router(st.clone());
        assert_eq!(pending(&app).await["revision"], 2);
        assert!(
            crate::api::revert_expired(&st, now_ms() + 61_000).await,
            "a deadline that passed while down reverts at startup"
        );
        assert_eq!(current(&app).await, BASE);
    }
}
