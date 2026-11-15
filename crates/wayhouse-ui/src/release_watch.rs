//! "A newer wayhouse release exists": the UI backend reads the signed release feed once a day
//! and tells the console (`GET /api/release`). It only reports; nothing is downloaded or
//! installed. See `wayhouse_registry::release_feed` for the format and `RELEASING.md` for
//! who publishes it.
//!
//! The fetch goes through [`RegistryClient`], so it has the same guards as every registry fetch
//! (https only, public addresses only, a byte cap, short timeouts). The feed is trusted only
//! after its signature checks against the release key; anything else is logged and leaves the
//! last good answer in place.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use semver::Version;
use serde::Serialize;
use wayhouse_registry::release_feed::MAX_FEED_BYTES;
use wayhouse_registry::{newer_than, verify_release_feed, ReleaseEntry, RELEASE_FEED_PUBKEY};

use crate::registry_client::RegistryClient;

/// Where the feed is published (`release.yml` attaches it to every stable release).
pub const DEFAULT_FEED_URL: &str =
    "https://github.com/wayhouse-proxy/wayhouse/releases/latest/download/release-feed.json";

/// One check a day, plus up to an hour of spread so every install does not call at once.
const REFRESH: Duration = Duration::from_secs(24 * 60 * 60);
const SPREAD_SECS: u64 = 60 * 60;
/// First check shortly after start, not during it.
const FIRST_DELAY: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Latest {
    pub version: String,
    pub date: String,
    pub notes_url: String,
}

/// What `GET /api/release` answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReleaseStatus {
    /// The version this UI backend runs.
    pub current: String,
    /// `false`: the feed is turned off (`--release-feed-url ""`).
    pub enabled: bool,
    /// The newest stable release above `current`, if the feed says there is one.
    pub latest: Option<Latest>,
    /// Unix seconds of the last good check; `None` before the first one.
    pub checked_at: Option<u64>,
}

pub struct ReleaseWatch {
    current: Version,
    url: Option<String>,
    state: Mutex<(Option<Latest>, Option<u64>)>,
}

impl ReleaseWatch {
    /// `url` of `None` (or empty) turns the check off.
    ///
    /// # Panics
    /// If `current` is not a semantic version (it is this crate's own package version).
    #[must_use]
    pub fn new(current: &str, url: Option<String>) -> Arc<Self> {
        Arc::new(Self {
            current: Version::parse(current).expect("the crate version is semver"),
            url: url.filter(|u| !u.is_empty()),
            state: Mutex::new((None, None)),
        })
    }

    #[must_use]
    pub fn status(&self) -> ReleaseStatus {
        let (latest, checked_at) = self
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        ReleaseStatus {
            current: self.current.to_string(),
            enabled: self.url.is_some(),
            latest,
            checked_at,
        }
    }

    /// Verify and apply one fetched feed. A feed that does not verify changes nothing.
    ///
    /// # Errors
    /// The signature or the feed is invalid.
    pub fn apply(&self, feed: &[u8], sig: &[u8], key_b64: &str, now: u64) -> Result<(), String> {
        let parsed = verify_release_feed(feed, sig, key_b64).map_err(|e| e.to_string())?;
        let latest = newer_than(&parsed, &self.current).map(|r: &ReleaseEntry| Latest {
            version: r.version.to_string(),
            date: r.date.clone(),
            notes_url: r.notes_url.clone(),
        });
        *self.state.lock().unwrap_or_else(PoisonError::into_inner) = (latest, Some(now));
        Ok(())
    }

    async fn refresh(&self, client: &RegistryClient) -> Result<(), String> {
        let Some(url) = &self.url else {
            return Ok(());
        };
        let feed = client
            .fetch_artifact(url, MAX_FEED_BYTES as u64)
            .await
            .map_err(|e| e.to_string())?;
        let sig = client
            .fetch_signature(&format!("{url}.minisig"))
            .await
            .map_err(|e| e.to_string())?;
        self.apply(&feed, &sig, RELEASE_FEED_PUBKEY, unix_now())
    }

    /// Starts the daily check. Nothing runs when the feed is off.
    pub fn spawn(self: &Arc<Self>, client: RegistryClient) -> Option<tokio::task::JoinHandle<()>> {
        self.url.as_ref()?;
        let me = self.clone();
        Some(tokio::spawn(async move {
            tokio::time::sleep(FIRST_DELAY).await;
            loop {
                if let Err(e) = me.refresh(&client).await {
                    // Debug-level on purpose: an air-gapped install fails every day.
                    tracing::debug!(error = %e, "release feed not updated");
                }
                tokio::time::sleep(REFRESH + Duration::from_secs(unix_now() % SPREAD_SECS)).await;
            }
        }))
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use wayhouse_registry::release_feed::{to_json, ReleaseFeed, FEED_CLAIM, FEED_SCHEMA};

    fn feed(versions: &[&str]) -> Vec<u8> {
        to_json(&ReleaseFeed {
            schema: FEED_SCHEMA,
            releases: versions
                .iter()
                .map(|v| ReleaseEntry {
                    version: Version::parse(v).unwrap(),
                    date: "2026-12-01".into(),
                    notes_url: format!("https://github.com/o/r/releases/tag/v{v}"),
                })
                .collect(),
        })
        .unwrap()
    }

    fn sign(data: &[u8]) -> (String, Vec<u8>) {
        let kp = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let sig = minisign::sign(
            Some(&kp.pk),
            &kp.sk,
            Cursor::new(data),
            Some(FEED_CLAIM),
            None,
        )
        .unwrap();
        let key = kp.pk.to_box().unwrap().to_string();
        (
            key.lines().nth(1).unwrap().into(),
            sig.to_string().into_bytes(),
        )
    }

    #[test]
    fn a_newer_stable_release_is_reported_and_the_same_version_is_not() {
        let bytes = feed(&["0.4.1", "0.4.0"]);
        let (key, sig) = sign(&bytes);

        let behind = ReleaseWatch::new("0.4.0", Some("https://x".into()));
        assert_eq!(behind.status().latest, None);
        assert_eq!(behind.status().checked_at, None);
        behind.apply(&bytes, &sig, &key, 77).unwrap();
        let s = behind.status();
        assert_eq!(s.latest.unwrap().version, "0.4.1");
        assert_eq!(s.checked_at, Some(77));

        let current = ReleaseWatch::new("0.4.1", Some("https://x".into()));
        current.apply(&bytes, &sig, &key, 1).unwrap();
        assert_eq!(current.status().latest, None);
        assert!(current.status().checked_at.is_some());
    }

    #[test]
    fn a_feed_that_does_not_verify_changes_nothing() {
        let bytes = feed(&["0.4.1"]);
        let (key, sig) = sign(&bytes);
        let w = ReleaseWatch::new("0.4.0", Some("https://x".into()));
        w.apply(&bytes, &sig, &key, 1).unwrap();
        // Altered bytes, and a feed signed by another key: both refused, last answer kept.
        let altered = feed(&["9.9.9"]);
        assert!(w.apply(&altered, &sig, &key, 2).is_err());
        let (other_key, _) = sign(&bytes);
        assert!(w.apply(&bytes, &sig, &other_key, 3).is_err());
        let s = w.status();
        assert_eq!(s.latest.unwrap().version, "0.4.1");
        assert_eq!(s.checked_at, Some(1));
    }

    #[test]
    fn an_empty_url_turns_the_check_off() {
        let w = ReleaseWatch::new("0.4.0", Some(String::new()));
        assert!(!w.status().enabled);
        assert!(w.spawn(RegistryClient::new()).is_none());
        assert!(
            ReleaseWatch::new("0.4.0", Some(DEFAULT_FEED_URL.into()))
                .status()
                .enabled
        );
    }
}
