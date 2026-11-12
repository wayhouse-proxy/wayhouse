//! Picks up a rotated tunnel key.
//!
//! Every interval the agent asks the controller for its key (`GET /peers/{name}/psk`, with its
//! own token). The answer names a generation: when it is newer than the one the agent holds,
//! the agent writes the key to its file, switches every peer on the live interface to it in
//! place, records the generation, and registers again naming that generation, which is the
//! acknowledgement. Only then do the proxies switch, so the two ends differ for the few seconds
//! between the agent's switch and the registration reaching them; a handshake in that window
//! retries. Without a key of its own (a manually set one, or an agent that did not enroll) the
//! controller answers 401 or 404 and the agent stops asking.
//!
//! Order matters: key file, then the device, then the generation file. A crash between them
//! leaves a generation older than the key, and the next poll offers the same one again.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use defguard_wireguard_rs::key::Key;
use serde::Deserialize;
use zeroize::Zeroizing;

use crate::live_interface::LiveInterface;
use crate::register::{self, Registration};

/// What the controller offers: the generation and the key itself.
#[derive(Deserialize)]
struct Offer {
    gen: u64,
    key: String,
}

#[derive(Debug)]
pub enum FetchError {
    /// The controller does not serve this agent a key (401, 403, 404): not a rotating agent.
    NotOffered,
    /// 404: nothing stored for the name yet (before its first registration); try later.
    Absent,
    Other(anyhow::Error),
}

pub struct Rotation {
    pub client: reqwest::Client,
    pub controller_url: String,
    pub token: Option<String>,
    pub reg: Registration,
    /// The key file; the generation lives beside it.
    pub psk_file: PathBuf,
    pub live: Arc<LiveInterface>,
    pub interval: Duration,
}

impl Rotation {
    async fn fetch(&self) -> Result<Offer, FetchError> {
        let url = format!(
            "{}/peers/{}/psk",
            self.controller_url.trim_end_matches('/'),
            self.reg.name
        );
        let mut req = self.client.get(&url);
        if let Some(t) = &self.token {
            req = req.bearer_auth(t);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| FetchError::Other(anyhow::Error::new(e)))?;
        let status = resp.status();
        if status.as_u16() == 404 {
            return Err(FetchError::Absent);
        }
        if matches!(status.as_u16(), 401 | 403) {
            return Err(FetchError::NotOffered);
        }
        if !status.is_success() {
            return Err(FetchError::Other(anyhow::anyhow!(
                "the controller answered {status}"
            )));
        }
        resp.json()
            .await
            .map_err(|e| FetchError::Other(anyhow::Error::new(e).context("reading the key offer")))
    }

    /// Applies `offer` if it is newer than what the agent holds. `Ok(true)` when it switched.
    async fn apply(&self, offer: Offer) -> anyhow::Result<bool> {
        let Some(gen) = self.reg.psk_gen.clone() else {
            return Ok(false);
        };
        if offer.gen <= gen.load(Ordering::SeqCst) {
            return Ok(false);
        }
        let key = Key::try_from(offer.key.as_str())
            .map_err(|e| anyhow::anyhow!("the offered tunnel key is not a WireGuard key: {e}"))?;
        let (file, live, new_gen) = (self.psk_file.clone(), self.live.clone(), offer.gen);
        let text = Zeroizing::new(offer.key);
        tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            wayhouse_enroll::files::replace_psk(&file, &text)?;
            live.rekey(&key)?;
            wayhouse_enroll::files::write_psk_gen(&file, new_gen)?;
            Ok(())
        })
        .await
        .map_err(|e| anyhow::anyhow!("key switch task failed: {e}"))??;
        advance(&gen, new_gen);
        Ok(true)
    }

    /// One poll. `Ok(false)` when the controller offers this agent no key at all.
    pub async fn poll(&self) -> anyhow::Result<bool> {
        let offer = match self.fetch().await {
            Ok(o) => o,
            Err(FetchError::NotOffered) => return Ok(false),
            Err(FetchError::Absent) => return Ok(true),
            Err(FetchError::Other(e)) => return Err(e),
        };
        if self.apply(offer).await? {
            let g = self
                .reg
                .psk_gen
                .as_ref()
                .map_or(0, |g| g.load(Ordering::SeqCst));
            tracing::info!(
                generation = g,
                "switched to the rotated tunnel key; acknowledging"
            );
            let acked = register::register_once(
                &self.client,
                &self.controller_url,
                self.token.as_deref(),
                &self.reg,
            )
            .await;
            if let Err(e) = acked {
                // The periodic registration carries the same generation and tries again.
                tracing::warn!(error = %e, "could not acknowledge the rotated tunnel key yet; will retry");
            }
        }
        Ok(true)
    }

    pub async fn run(self) {
        let mut told = false;
        loop {
            match self.poll().await {
                Ok(true) => told = false,
                Ok(false) => {
                    if !told {
                        tracing::info!(
                            "the controller offers this agent no tunnel key to rotate (it needs \
                             its own enrollment token); keeping the key it has"
                        );
                        told = true;
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %format!("{e:#}"), "could not check for a rotated tunnel key")
                }
            }
            tokio::time::sleep(self.interval).await;
        }
    }
}

/// Moves `gen` forward to `to`, never back.
fn advance(gen: &AtomicU64, to: u64) {
    gen.fetch_max(to, Ordering::SeqCst);
}

#[cfg(test)]
mod tests {
    use axum::routing::{get, post};
    use axum::{Json, Router};
    use serde_json::json;

    use super::*;
    use crate::live_interface::testing::{live_with_peers, Log};

    /// A controller that offers `(gen, key)` and records registrations.
    async fn controller(
        offer: (u64, String),
    ) -> (String, Arc<std::sync::Mutex<Vec<serde_json::Value>>>) {
        let regs = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = regs.clone();
        let app = Router::new()
            .route(
                "/peers/{name}/psk",
                get(move || {
                    let (g, k) = offer.clone();
                    async move { Json(json!({"gen": g, "key": k})) }
                }),
            )
            .route(
                "/peers",
                post(move |Json(v): Json<serde_json::Value>| {
                    let seen = seen.clone();
                    async move {
                        seen.lock().unwrap().push(v);
                        Json(json!({"revision": 1, "tunnel_address": "10.60.0.2"}))
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{addr}"), regs)
    }

    fn rotation(url: String, dir: &std::path::Path, gen: u64) -> (Rotation, Arc<AtomicU64>, Log) {
        let log = Log::default();
        let gen = Arc::new(AtomicU64::new(gen));
        let old = wayhouse_enroll::generate_psk();
        wayhouse_enroll::files::replace_psk(&dir.join("tunnel-psk"), &old).unwrap();
        let live = live_with_peers(&[], &[], &[], &log)
            .with_psk(Some(Key::try_from(old.as_str()).unwrap()));
        let rot = Rotation {
            client: register::http_client(),
            controller_url: url,
            token: Some("token".into()),
            reg: Registration {
                name: "home".into(),
                pubkey: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=".into(),
                endpoint: None,
                backends: vec![],
                address: None,
                psk_gen: Some(gen.clone()),
            },
            psk_file: dir.join("tunnel-psk"),
            live: Arc::new(live),
            interval: Duration::from_secs(1),
        };
        (rot, gen, log)
    }

    #[tokio::test]
    async fn a_newer_generation_is_stored_applied_and_acknowledged_by_registering_it() {
        let dir = tempfile::tempdir().unwrap();
        let new = wayhouse_enroll::generate_psk();
        let (url, regs) = controller((1, new.to_string())).await;
        let (rot, gen, _) = rotation(url, dir.path(), 0);
        assert!(rot.poll().await.unwrap());
        assert_eq!(gen.load(Ordering::SeqCst), 1);
        let file = dir.path().join("tunnel-psk");
        assert_eq!(*wayhouse_enroll::files::read_psk(&file).unwrap(), *new);
        assert_eq!(wayhouse_enroll::files::read_psk_gen(&file), 1);
        assert_eq!(
            rot.live.current_psk(),
            Some(Key::try_from(new.as_str()).unwrap())
        );
        let regs = regs.lock().unwrap();
        assert_eq!(
            regs.last().unwrap()["psk_secret"],
            wayhouse_enroll::psk_secret_name_gen("home", 1),
            "the registration names the new generation"
        );
    }

    #[tokio::test]
    async fn the_generation_the_agent_already_holds_changes_nothing_and_registers_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let same = wayhouse_enroll::generate_psk();
        let (url, regs) = controller((2, same.to_string())).await;
        let (rot, gen, _) = rotation(url, dir.path(), 2);
        let before = rot.live.current_psk();
        assert!(rot.poll().await.unwrap());
        assert_eq!(gen.load(Ordering::SeqCst), 2);
        assert_eq!(rot.live.current_psk(), before);
        assert!(regs.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_key_that_is_not_a_key_is_refused_before_anything_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let (url, regs) = controller((1, "not a key".into())).await;
        let (rot, gen, _) = rotation(url, dir.path(), 0);
        let file = dir.path().join("tunnel-psk");
        let before = wayhouse_enroll::files::read_psk(&file).unwrap();
        assert!(rot.poll().await.is_err());
        assert_eq!(gen.load(Ordering::SeqCst), 0);
        assert_eq!(*wayhouse_enroll::files::read_psk(&file).unwrap(), *before);
        assert!(regs.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_agent_the_controller_does_not_serve_a_key_stops_trying_quietly() {
        let app = Router::new();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let dir = tempfile::tempdir().unwrap();
        let (rot, gen, _) = rotation(url, dir.path(), 0);
        assert!(
            rot.poll().await.unwrap(),
            "404 is nothing yet, not a refusal"
        );
        assert_eq!(gen.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn the_generation_only_moves_forward() {
        let g = AtomicU64::new(3);
        advance(&g, 2);
        assert_eq!(g.load(Ordering::SeqCst), 3);
        advance(&g, 4);
        assert_eq!(g.load(Ordering::SeqCst), 4);
    }
}
