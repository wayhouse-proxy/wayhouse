//! The joining side of a controller enrollment: a node started with `--ha-join` plus the
//! three flags below asks a running replica to add it (`POST /enroll/ha-join`, see
//! `crate::enroll`), then keeps asking until it is a voter.
//!
//! `--ha-advertise` is the address the other replicas dial, `--ha-join-via` any replica's URL
//! (the one the code was redeemed at), and the join proof is the credential `ha-join-proof`
//! that came with the code (`--ha-join-proof-file`, `$CREDENTIALS_DIRECTORY/ha-join-proof` or
//! `$WAYHOUSE_HA_JOIN_PROOF`). Without them the node waits for an admin's
//! `POST /admin/ha/members`, as before.

use std::sync::Arc;
use std::time::Duration;

use zeroize::Zeroizing;

use super::{HaHandle, NodeId};

/// The leader's add waits for the new node to catch up (up to `MEMBER_ADD_TIMEOUT`), so the
/// request that carries it is allowed a little longer.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(330);
const FIRST_DELAY: Duration = Duration::from_secs(1);
const MAX_DELAY: Duration = Duration::from_secs(60);
/// How long after start a refusal is treated as "not replicated yet" rather than final.
const REFUSAL_GRACE: Duration = Duration::from_secs(120);

pub struct JoinConfig {
    pub via: String,
    pub advertise: String,
    pub proof: Zeroizing<String>,
}

/// The three join flags go together and need `--ha-join`. Checked at startup, before anything
/// is sent.
pub fn check_flags(
    ha_join: bool,
    advertise: Option<&str>,
    via: Option<&str>,
    has_proof: bool,
    allow_insecure: bool,
) -> Result<Option<(String, String)>, String> {
    let set = [advertise.is_some(), via.is_some(), has_proof];
    if !set.contains(&true) {
        return Ok(None);
    }
    if !ha_join {
        return Err("--ha-advertise, --ha-join-via and the join proof need --ha-join".into());
    }
    let (Some(advertise), Some(via), true) = (advertise, via, has_proof) else {
        return Err(
            "--ha-advertise, --ha-join-via and the join proof (--ha-join-proof-file) go together"
                .into(),
        );
    };
    super::peers::validate_addr(advertise).map_err(|e| format!("--ha-advertise: {e}"))?;
    super::peers::check_peer_transport(advertise).map_err(|e| format!("--ha-advertise: {e}"))?;
    super::peers::validate_addr(via).map_err(|e| format!("--ha-join-via: {e}"))?;
    // The proof is a credential: it only goes over https, or to loopback.
    wayhouse_http::bearer::check_bearer_transport(
        "--ha-join-via",
        &super::peers::peer_url(via, ""),
        true,
        allow_insecure,
    )?;
    Ok(Some((advertise.to_owned(), via.to_owned())))
}

/// What one try answered.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Joined,
    /// The proof is spent, late or wrong: asking again cannot help.
    Refused,
    /// Worth another try; the text says why (the leader's own words when it gave any).
    Retry(String),
}

fn classify(status: u16, body: &str) -> Outcome {
    match status {
        200..=299 => Outcome::Joined,
        401 => Outcome::Refused,
        _ => {
            let reason = serde_json::from_str::<serde_json::Value>(body)
                .ok()
                .and_then(|v| v["error"].as_str().map(str::to_owned))
                .unwrap_or_else(|| format!("answered {status}"));
            Outcome::Retry(reason.chars().take(300).collect())
        }
    }
}

async fn try_once(client: &reqwest::Client, cfg: &JoinConfig) -> Outcome {
    let body = serde_json::json!({ "proof": &*cfg.proof, "addr": cfg.advertise });
    let resp = client
        .post(super::peers::peer_url(&cfg.via, "/enroll/ha-join"))
        .json(&body)
        .send()
        .await;
    match resp {
        Ok(r) => {
            let status = r.status().as_u16();
            classify(status, &r.text().await.unwrap_or_default())
        }
        Err(e) => Outcome::Retry(format!(
            "could not reach the cluster: {}",
            wayhouse_http::error_chain(&e)
        )),
    }
}

/// Asks until this node is a voter, the proof is refused, or `done` says it already is.
pub async fn announce_until(
    cfg: &JoinConfig,
    node_id: NodeId,
    done: impl Fn() -> bool,
    first_delay: Duration,
    refusal_grace: Duration,
) {
    let client = super::client::forward_client(REQUEST_TIMEOUT);
    let started = std::time::Instant::now();
    let mut delay = first_delay;
    loop {
        tokio::time::sleep(delay).await;
        if done() {
            tracing::info!(node_id, "this node is a member of the HA group");
            return;
        }
        match try_once(&client, cfg).await {
            Outcome::Joined => {
                tracing::info!(node_id, "the cluster added this node");
                return;
            }
            // A replica that has not applied the redeem yet answers like a spent proof; give the
            // cluster a couple of minutes before taking a 401 as final.
            Outcome::Refused if started.elapsed() < refusal_grace => {
                tracing::warn!(
                    node_id,
                    "the cluster does not know this node's join proof yet; retrying"
                );
            }
            Outcome::Refused if done() => {
                // The add went through and only its answer was lost.
                tracing::info!(node_id, "this node is a member of the HA group");
                return;
            }
            Outcome::Refused => {
                tracing::error!(
                    node_id,
                    "the cluster refused this node's join proof (used, expired or revoked); mint a new controller code and enroll again"
                );
                return;
            }
            Outcome::Retry(reason) => {
                tracing::warn!(node_id, reason = %reason, "joining the HA group did not work yet; retrying");
            }
        }
        delay = (delay * 2).clamp(Duration::from_secs(5), MAX_DELAY);
    }
}

/// Starts [`announce_until`] in the background for a node that is not a voter yet.
pub fn spawn(ha: Arc<HaHandle>, cfg: JoinConfig) {
    let id = ha.node_id;
    tokio::spawn(async move {
        let me = ha.clone();
        announce_until(
            &cfg,
            id,
            move || {
                me.raft
                    .metrics()
                    .borrow()
                    .membership_config
                    .membership()
                    .voter_ids()
                    .any(|v| v == id)
            },
            FIRST_DELAY,
            REFUSAL_GRACE,
        )
        .await;
    });
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use axum::http::StatusCode;
    use axum::routing::post;
    use axum::{Json, Router};

    use super::*;

    async fn stub(statuses: Vec<u16>) -> (String, Arc<AtomicU32>) {
        let calls = Arc::new(AtomicU32::new(0));
        let c = calls.clone();
        let app = Router::new().route(
            "/enroll/ha-join",
            post(move |Json(b): Json<serde_json::Value>| {
                let n = c.fetch_add(1, Ordering::SeqCst) as usize;
                let status = statuses[n.min(statuses.len() - 1)];
                async move {
                    assert!(b["proof"].is_string() && b["addr"].is_string());
                    (
                        StatusCode::from_u16(status).unwrap(),
                        Json(serde_json::json!({"error": "no route to the node"})),
                    )
                }
            }),
        );
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = format!("http://{}", l.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        (addr, calls)
    }

    fn cfg(via: String) -> JoinConfig {
        JoinConfig {
            via,
            advertise: "http://127.0.0.1:9".into(),
            proof: Zeroizing::new("abcd1234.x".into()),
        }
    }

    #[test]
    fn the_answers_are_sorted_into_done_give_up_and_try_again() {
        assert_eq!(classify(200, ""), Outcome::Joined);
        assert_eq!(classify(401, "{}"), Outcome::Refused);
        assert_eq!(
            classify(503, r#"{"error":"node at x did not answer"}"#),
            Outcome::Retry("node at x did not answer".into())
        );
        assert_eq!(
            classify(502, "<html>"),
            Outcome::Retry("answered 502".into())
        );
    }

    #[tokio::test]
    async fn the_node_keeps_asking_through_failures_and_stops_at_the_first_success() {
        let (via, calls) = stub(vec![503, 422, 200]).await;
        announce_until(
            &cfg(via),
            4,
            || false,
            Duration::from_millis(1),
            Duration::ZERO,
        )
        .await;
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn a_refused_proof_ends_the_asking() {
        let (via, calls) = stub(vec![401, 200]).await;
        announce_until(
            &cfg(via),
            4,
            || false,
            Duration::from_millis(1),
            Duration::ZERO,
        )
        .await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_refusal_right_after_start_is_retried_as_not_replicated_yet() {
        let (via, calls) = stub(vec![401, 200]).await;
        announce_until(
            &cfg(via),
            4,
            || false,
            Duration::from_millis(1),
            Duration::from_secs(60),
        )
        .await;
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn a_refusal_from_a_node_that_is_a_voter_is_not_an_error() {
        // Asked once (not yet a member), refused, then seen as a voter: stops without a third ask.
        let (via, calls) = stub(vec![401]).await;
        let asked = std::sync::atomic::AtomicU32::new(0);
        announce_until(
            &cfg(via),
            4,
            || asked.fetch_add(1, Ordering::SeqCst) >= 1,
            Duration::from_millis(1),
            Duration::ZERO,
        )
        .await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_node_that_is_already_a_member_asks_nothing() {
        let (via, calls) = stub(vec![200]).await;
        announce_until(
            &cfg(via),
            4,
            || true,
            Duration::from_millis(1),
            Duration::ZERO,
        )
        .await;
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn the_join_flags_go_together_and_need_ha_join() {
        let ok = check_flags(
            true,
            Some("https://c2:9901"),
            Some("https://c1:9901"),
            true,
            false,
        );
        assert_eq!(
            ok,
            Ok(Some(("https://c2:9901".into(), "https://c1:9901".into())))
        );
        assert_eq!(check_flags(true, None, None, false, false), Ok(None));
        assert!(check_flags(
            false,
            Some("https://c2:9901"),
            Some("https://c1:9901"),
            true,
            false
        )
        .is_err());
        assert!(check_flags(true, Some("https://c2:9901"), None, true, false).is_err());
        assert!(check_flags(
            true,
            Some("https://c2:9901"),
            Some("https://c1:9901"),
            false,
            false
        )
        .is_err());
    }

    #[test]
    fn the_proof_never_goes_over_plain_http_to_another_host() {
        let plain = check_flags(
            true,
            Some("https://c2:9901"),
            Some("http://c1.example:9901"),
            true,
            false,
        );
        assert!(plain.is_err(), "{plain:?}");
        let local = check_flags(
            true,
            Some("https://c2:9901"),
            Some("http://127.0.0.1:9901"),
            true,
            false,
        );
        assert!(local.is_ok(), "{local:?}");
        assert!(check_flags(
            true,
            Some("http://c2.example:9901"),
            Some("https://c1:9901"),
            true,
            false
        )
        .is_err());
    }
}
