//! The issuance and renewal loop. Only the Raft leader (or a standalone controller) runs it.
//!
//! Per enabled site that is due, it runs one DNS-01 order through the site's solver, seals the
//! new private key under the cluster keyring and commits the certificate as one entry tagged
//! with the leader's term, so a deposed leader's late result is dropped. A failure advances a
//! replicated backoff, so a new leader continues it instead of hammering the CA.

use std::sync::Arc;
use std::time::Duration;

use wayhouse_acme::{renew, AccountCreds, AcmeCa, CertBundle, DnsSolver, Redacted};

use super::types::{cert_slot, AcmeOp, Applied, Attempt, Site, StoredAccount, StoredCert};
use super::types::{ACCOUNT_SLOT, SEAL_SCOPE};
use super::AcmeStore;
use crate::ha::{HaHandle, WriteRequest, WriteResponse};
use crate::plugins::secrets::KeyringHandle;

/// Failed orders allowed per site and hour before the loop waits out the window, so a
/// misconfigured DNS plugin cannot burn the CA's failed-validation limit.
pub const MAX_FAILURES_PER_HOUR: u32 = 5;
const WINDOW_SECS: u64 = 3600;

/// Resolves a site's `dns_plugin` to the solver that drives it. `None` means the plugin is
/// not available on this node (not installed, disabled, not approved for the trigger).
pub trait SolverSource: Send + Sync {
    fn solver_for(&self, dns_plugin: &str) -> Option<Arc<dyn DnsSolver>>;
}

/// The solver source of a controller that has no DNS plugin hook yet: every order fails with a
/// recorded, visible reason.
pub struct NoSolvers;

impl SolverSource for NoSolvers {
    fn solver_for(&self, _: &str) -> Option<Arc<dyn DnsSolver>> {
        None
    }
}

type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;

pub fn system_clock() -> Clock {
    Arc::new(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs())
    })
}

pub struct Runner {
    store: AcmeStore,
    ca: AcmeCa,
    directory_url: String,
    solvers: Arc<dyn SolverSource>,
    keyring: KeyringHandle,
    ha: Option<Arc<HaHandle>>,
    clock: Clock,
}

/// Why a site was skipped or how its pass ended, for tests and the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pass {
    /// Not the leader.
    Follower,
    /// No usable keyring: nothing is issued.
    Held,
    /// Ran; the sites that were issued and the ones that failed.
    Ran {
        issued: Vec<String>,
        failed: Vec<String>,
    },
}

impl Runner {
    pub fn new(
        store: AcmeStore,
        ca: AcmeCa,
        directory_url: &str,
        solvers: Arc<dyn SolverSource>,
        keyring: KeyringHandle,
        ha: Option<Arc<HaHandle>>,
        clock: Clock,
    ) -> Arc<Self> {
        Arc::new(Self {
            store,
            ca,
            directory_url: directory_url.to_string(),
            solvers,
            keyring,
            ha,
            clock,
        })
    }

    /// Where this node issues. Standalone: `Some(0)`. HA leader: `Some(term)`. Follower: `None`.
    fn lead(&self) -> Option<u64> {
        let Some(ha) = &self.ha else {
            return Some(0);
        };
        let m = ha.raft.metrics().borrow().clone();
        (m.current_leader == Some(ha.node_id)).then_some(m.current_term)
    }

    /// Runs forever: a pass at start, then on every store change and at least every minute.
    pub fn spawn(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut changed = self.store.watch();
            loop {
                let _ = self.run_once().await;
                tokio::select! {
                    _ = changed.changed() => {}
                    () = tokio::time::sleep(Duration::from_secs(60)) => {}
                }
            }
        })
    }

    /// One pass over every site.
    pub async fn run_once(&self) -> Pass {
        let Some(term) = self.lead() else {
            return Pass::Follower;
        };
        let ring = self.keyring.current();
        let held = ring.active_id().is_none();
        metrics::gauge!("wayhouse_acme_held").set(if held { 1.0 } else { 0.0 });
        let now = (self.clock)();
        let sites = match self.store.sites() {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(error = %e, "acme: could not read the sites");
                return Pass::Ran {
                    issued: vec![],
                    failed: vec![],
                };
            }
        };
        for site in &sites {
            if let Ok(Some(c)) = self.store.cert(&site.name) {
                #[allow(clippy::cast_precision_loss)]
                metrics::gauge!("wayhouse_acme_cert_expiry_seconds", "site" => site.name.clone())
                    .set(c.not_after as f64 - now as f64);
            }
        }
        if held {
            if !sites.is_empty() {
                tracing::warn!(
                    "acme: holding issuance, this node has no secret key (--secret-key-file)"
                );
            }
            return Pass::Held;
        }
        let (mut issued, mut failed) = (vec![], vec![]);
        for site in sites.iter().filter(|s| s.enabled) {
            match self.due(site, now) {
                Ok(false) => continue,
                Ok(true) => {}
                Err(e) => {
                    tracing::error!(site = %site.name, error = %e, "acme: could not read site state");
                    continue;
                }
            }
            match self.issue_site(site, term, now).await {
                Ok(()) => {
                    metrics::counter!("wayhouse_acme_issue_total", "result" => "ok").increment(1);
                    issued.push(site.name.clone());
                }
                Err(why) => {
                    metrics::counter!("wayhouse_acme_issue_total", "result" => "error")
                        .increment(1);
                    tracing::warn!(site = %site.name, error = %why, "acme: issuance failed");
                    self.record_failure(site, &why, now).await;
                    failed.push(site.name.clone());
                }
            }
        }
        Pass::Ran { issued, failed }
    }

    fn due(&self, site: &Site, now: u64) -> Result<bool, super::AcmeStoreError> {
        if let Some(a) = self.store.attempt(&site.name)? {
            if a.next_attempt_at > now {
                return Ok(false);
            }
        }
        Ok(match self.store.cert(&site.name)? {
            None => true,
            Some(c) => {
                c.names != site.names
                    || renew::due(c.issued_at, c.not_after, now, jitter(&site.name))
            }
        })
    }

    async fn issue_site(&self, site: &Site, term: u64, now: u64) -> Result<(), String> {
        let solver = self.solvers.solver_for(&site.dns_plugin).ok_or_else(|| {
            format!(
                "the DNS plugin {:?} is not available on this node",
                site.dns_plugin
            )
        })?;
        let creds = self.account().await?;
        let ca = &self.ca;
        let (bundle, _) = ca
            .issue(Some(&creds), &site.names, solver.as_ref(), now)
            .await
            .map_err(|e| e.to_string())?;
        let stored = self.seal_cert(site, &bundle)?;
        let op = AcmeOp::CertIssued {
            site: site.name.clone(),
            tagged_term: term,
            cert: stored,
        };
        match self.propose(op).await? {
            Applied::Done => Ok(()),
            Applied::WrongTerm => {
                Err("this node stopped leading during the order; the result was dropped".into())
            }
            Applied::Stale => {
                Err("the site changed during the order; the result was dropped".into())
            }
            other => Err(format!("the certificate was refused: {other:?}")),
        }
    }

    /// The stored account's credentials; creates the account on first use. Two leaders that
    /// race converge on the first stored one, the other's account is orphaned at the CA.
    async fn account(&self) -> Result<AccountCreds, String> {
        if self.store.account().map_err(|e| e.to_string())?.is_none() {
            let fresh = self
                .ca
                .ensure_account(None)
                .await
                .map_err(|e| e.to_string())?;
            let sealed = self
                .keyring
                .current()
                .seal(
                    SEAL_SCOPE,
                    ACCOUNT_SLOT,
                    fresh.credentials.expose().as_bytes(),
                )
                .map_err(|e| e.to_string())?;
            let stored = StoredAccount {
                directory_url: self.directory_url.clone(),
                account_url: fresh.account_url,
                sealed,
            };
            self.propose(AcmeOp::AccountSet(stored)).await?;
        }
        let stored = self
            .store
            .account()
            .map_err(|e| e.to_string())?
            .ok_or("the account was not stored")?;
        if stored.directory_url != self.directory_url {
            return Err(format!(
                "the stored account belongs to {} but --acme-ca is {}; a CA change needs a new account",
                stored.directory_url, self.directory_url
            ));
        }
        let plain = self
            .keyring
            .current()
            .open(SEAL_SCOPE, ACCOUNT_SLOT, &stored.sealed)
            .map_err(|e| format!("the account key cannot be opened: {e}"))?;
        let credentials =
            std::str::from_utf8(plain.expose()).map_err(|_| "the account key is malformed")?;
        Ok(AccountCreds {
            account_url: stored.account_url,
            credentials: Redacted::new(credentials),
        })
    }

    fn seal_cert(&self, site: &Site, bundle: &CertBundle) -> Result<StoredCert, String> {
        let version = self
            .store
            .cert(&site.name)
            .map_err(|e| e.to_string())?
            .map_or(1, |c| c.version + 1);
        let sealed_key = self
            .keyring
            .current()
            .seal(
                SEAL_SCOPE,
                &cert_slot(&site.name, version),
                bundle.key_pem.expose().as_bytes(),
            )
            .map_err(|e| e.to_string())?;
        Ok(StoredCert {
            version,
            chain_pem: bundle.chain_pem.clone(),
            sealed_key,
            not_after: bundle.not_after,
            issued_at: bundle.issued_at,
            serial: bundle.serial.clone(),
            names: bundle.names.clone(),
        })
    }

    async fn record_failure(&self, site: &Site, why: &str, now: u64) {
        let prev = self
            .store
            .attempt(&site.name)
            .ok()
            .flatten()
            .unwrap_or_default();
        let (window_start, window_failures) = if now < prev.window_start + WINDOW_SECS {
            (prev.window_start, prev.window_failures + 1)
        } else {
            (now, 1)
        };
        let mut wait = renew::backoff(prev.failures).as_secs();
        if window_failures >= MAX_FAILURES_PER_HOUR {
            wait = wait.max(window_start + WINDOW_SECS - now);
        }
        let attempt = Attempt {
            failures: prev.failures + 1,
            next_attempt_at: now + wait,
            last_error: Some(why.chars().take(500).collect()),
            window_start,
            window_failures,
        };
        if let Err(e) = self
            .propose(AcmeOp::Attempt {
                site: site.name.clone(),
                attempt,
            })
            .await
        {
            tracing::warn!(site = %site.name, error = %e, "acme: could not record the failure");
        }
    }

    /// Commits `op`: locally, or under HA as one Raft entry.
    async fn propose(&self, op: AcmeOp) -> Result<Applied, String> {
        let Some(ha) = &self.ha else {
            return self.store.apply_local(&op).map_err(|e| e.to_string());
        };
        match ha.raft.client_write(WriteRequest::Acme(op)).await {
            Ok(done) => match done.data {
                WriteResponse::AcmeResult(a) => Ok(a),
                other => Err(format!("unexpected answer {other:?}")),
            },
            Err(e) => Err(format!("could not commit through raft: {e}")),
        }
    }
}

/// A stable value in `[0, 1]` per site, so sites issued together do not renew together.
fn jitter(site: &str) -> f64 {
    let h = site
        .bytes()
        .fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(u32::from(b)));
    f64::from(h % 1000) / 999.0
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use wayhouse_acme::testing::{FailKind, FakeCa, MemDns};

    use super::*;
    use crate::plugins::secrets::Keyring;

    const DAY: u64 = 86_400;
    const KEY: &str = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=";

    struct OneSolver(MemDns);
    impl SolverSource for OneSolver {
        fn solver_for(&self, plugin: &str) -> Option<Arc<dyn DnsSolver>> {
            (plugin == "dns").then(|| Arc::new(self.0.clone()) as Arc<dyn DnsSolver>)
        }
    }

    struct Fixture {
        runner: Arc<Runner>,
        store: AcmeStore,
        fake: FakeCa,
        dns: MemDns,
        now: Arc<AtomicU64>,
        keyring: KeyringHandle,
        db: sled::Db,
        _dir: tempfile::TempDir,
    }

    async fn fixture_with(keyring: KeyringHandle) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let db = sled::open(dir.path()).unwrap();
        let store = AcmeStore::open(&db).unwrap();
        let dns = MemDns::default();
        let now = Arc::new(AtomicU64::new(1_800_000_000));
        let clock: Clock = {
            let now = now.clone();
            Arc::new(move || now.load(Ordering::SeqCst))
        };
        let fake = FakeCa::start_with(dns.clone(), clock.clone(), 90 * DAY).await;
        let ca = AcmeCa::new(&fake.directory_url(), true)
            .with_http(FakeCa::http_factory())
            .fast_retry();
        let runner = Runner::new(
            store.clone(),
            ca,
            &fake.directory_url(),
            Arc::new(OneSolver(dns.clone())),
            keyring.clone(),
            None,
            clock,
        );
        Fixture {
            runner,
            store,
            fake,
            dns,
            now,
            keyring,
            db,
            _dir: dir,
        }
    }

    async fn fixture() -> Fixture {
        fixture_with(KeyringHandle::new(Keyring::parse(KEY).unwrap())).await
    }

    fn site(name: &str, host: &str) -> Site {
        Site {
            name: name.into(),
            names: vec![host.into()],
            gateways: vec!["gw1".into()],
            dns_plugin: "dns".into(),
            propagation_timeout_secs: 120,
            enabled: true,
        }
    }

    fn put(f: &Fixture, s: Site) {
        assert_eq!(
            f.store.apply_local(&AcmeOp::SiteSet(s)).unwrap(),
            Applied::Done
        );
    }

    fn ran(issued: &[&str], failed: &[&str]) -> Pass {
        Pass::Ran {
            issued: issued.iter().map(ToString::to_string).collect(),
            failed: failed.iter().map(ToString::to_string).collect(),
        }
    }

    #[tokio::test]
    async fn issues_a_missing_certificate_and_seals_the_key() {
        let f = fixture().await;
        put(&f, site("web", "panel.example.com"));
        assert_eq!(f.runner.run_once().await, ran(&["web"], &[]));
        let c = f.store.cert("web").unwrap().unwrap();
        assert_eq!(c.version, 1);
        assert!(c.chain_pem.contains("BEGIN CERTIFICATE"));
        let key = f
            .keyring
            .current()
            .open("acme", "cert:web@1", &c.sealed_key)
            .unwrap();
        let pem = std::str::from_utf8(key.expose()).unwrap();
        assert!(pem.contains("PRIVATE KEY"));
        // The plaintext key is in no tree of the database, in any form.
        let body: String = pem.lines().filter(|l| !l.starts_with("-----")).collect();
        for name in f.db.tree_names() {
            let tree = f.db.open_tree(name).unwrap();
            for kv in tree.iter() {
                let (k, v) = kv.unwrap();
                let text = String::from_utf8_lossy(&[k.as_ref(), v.as_ref()].concat()).into_owned();
                assert!(!text.contains(&body[..40]), "key material found in a tree");
            }
        }
        assert!(f.dns.is_empty());
        let account = f.store.account().unwrap().unwrap();
        assert!(!account.account_url.is_empty());
    }

    #[tokio::test]
    async fn one_account_serves_every_site() {
        let f = fixture().await;
        put(&f, site("a", "a.example.com"));
        put(&f, site("b", "b.example.com"));
        assert_eq!(f.runner.run_once().await, ran(&["a", "b"], &[]));
        assert_eq!(f.fake.accounts(), 1);
    }

    #[tokio::test]
    async fn renews_at_a_third_of_the_lifetime_left_not_before() {
        let f = fixture().await;
        put(&f, site("web", "panel.example.com"));
        f.runner.run_once().await;
        f.now.fetch_add(45 * DAY, Ordering::SeqCst);
        assert_eq!(
            f.runner.run_once().await,
            ran(&[], &[]),
            "half the life left"
        );
        f.now.fetch_add(20 * DAY, Ordering::SeqCst); // 25 days left of 90
        assert_eq!(f.runner.run_once().await, ran(&["web"], &[]));
        assert_eq!(f.store.cert("web").unwrap().unwrap().version, 2);
        assert_eq!(f.fake.issued(), 2);
    }

    #[tokio::test]
    async fn a_changed_name_list_is_reissued_at_once() {
        let f = fixture().await;
        put(&f, site("web", "a.example.com"));
        f.runner.run_once().await;
        put(&f, site("web", "b.example.com"));
        assert_eq!(f.runner.run_once().await, ran(&["web"], &[]));
        assert_eq!(
            f.store.cert("web").unwrap().unwrap().names,
            vec!["b.example.com"]
        );
    }

    #[tokio::test]
    async fn a_failure_backs_off_and_the_next_pass_waits_then_succeeds() {
        let f = fixture().await;
        put(&f, site("web", "a.example.com"));
        f.fake.fail_next(FailKind::Dns);
        assert_eq!(f.runner.run_once().await, ran(&[], &["web"]));
        let a = f.store.attempt("web").unwrap().unwrap();
        assert_eq!(a.failures, 1);
        assert_eq!(a.next_attempt_at, f.now.load(Ordering::SeqCst) + 300);
        assert!(a.last_error.is_some());
        assert!(f.dns.is_empty(), "cleaned up after the failure");
        assert_eq!(
            f.runner.run_once().await,
            ran(&[], &[]),
            "inside the backoff"
        );
        f.now.fetch_add(301, Ordering::SeqCst);
        assert_eq!(f.runner.run_once().await, ran(&["web"], &[]));
        assert!(
            f.store.attempt("web").unwrap().is_none(),
            "success clears the state"
        );
    }

    #[tokio::test]
    async fn the_hourly_failure_cap_waits_out_the_window() {
        let f = fixture().await;
        put(&f, site("web", "a.example.com"));
        let start = f.now.load(Ordering::SeqCst);
        for _ in 0..MAX_FAILURES_PER_HOUR {
            f.fake.fail_next(FailKind::RateLimited);
            assert_eq!(f.runner.run_once().await, ran(&[], &["web"]));
            let a = f.store.attempt("web").unwrap().unwrap();
            f.now.store(a.next_attempt_at, Ordering::SeqCst);
        }
        let a = f.store.attempt("web").unwrap().unwrap();
        assert_eq!(a.failures, MAX_FAILURES_PER_HOUR);
        assert!(
            a.next_attempt_at >= start + WINDOW_SECS,
            "waits out the hour"
        );
    }

    #[tokio::test]
    async fn without_a_keyring_issuance_is_held() {
        let f = fixture_with(KeyringHandle::default()).await;
        put(&f, site("web", "a.example.com"));
        assert_eq!(f.runner.run_once().await, Pass::Held);
        assert!(f.store.cert("web").unwrap().is_none());
        assert_eq!(f.fake.accounts(), 0, "no account is created either");
    }

    #[tokio::test]
    async fn an_unavailable_dns_plugin_is_a_recorded_failure() {
        let f = fixture().await;
        let mut s = site("web", "a.example.com");
        s.dns_plugin = "missing".into();
        put(&f, s);
        assert_eq!(f.runner.run_once().await, ran(&[], &["web"]));
        let err = f.store.attempt("web").unwrap().unwrap().last_error.unwrap();
        assert!(err.contains("not available"), "{err}");
    }

    #[tokio::test]
    async fn disabled_sites_are_left_alone() {
        let f = fixture().await;
        let mut s = site("web", "a.example.com");
        s.enabled = false;
        put(&f, s);
        assert_eq!(f.runner.run_once().await, ran(&[], &[]));
    }

    #[tokio::test]
    async fn a_restart_with_an_expired_certificate_renews_immediately() {
        let f = fixture().await;
        put(&f, site("web", "a.example.com"));
        f.runner.run_once().await;
        f.now.fetch_add(100 * DAY, Ordering::SeqCst); // long expired, controller was down
        let ca = AcmeCa::new(&f.fake.directory_url(), true)
            .with_http(FakeCa::http_factory())
            .fast_retry();
        let now = f.now.clone();
        let again = Runner::new(
            f.store.clone(),
            ca,
            &f.fake.directory_url(),
            Arc::new(OneSolver(f.dns.clone())),
            f.keyring.clone(),
            None,
            Arc::new(move || now.load(Ordering::SeqCst)),
        );
        assert_eq!(again.run_once().await, ran(&["web"], &[]));
        assert_eq!(f.store.cert("web").unwrap().unwrap().version, 2);
    }

    #[tokio::test]
    async fn a_ca_change_needs_a_new_account() {
        let f = fixture().await;
        put(&f, site("web", "a.example.com"));
        f.runner.run_once().await;
        let other = FakeCa::start(MemDns::default()).await;
        let ca = AcmeCa::new(&other.directory_url(), true)
            .with_http(FakeCa::http_factory())
            .fast_retry();
        let now = f.now.clone();
        let moved = Runner::new(
            f.store.clone(),
            ca,
            &other.directory_url(),
            Arc::new(OneSolver(f.dns.clone())),
            f.keyring.clone(),
            None,
            Arc::new(move || now.load(Ordering::SeqCst)),
        );
        f.now.fetch_add(80 * DAY, Ordering::SeqCst);
        assert_eq!(moved.run_once().await, ran(&[], &["web"]));
        let err = f.store.attempt("web").unwrap().unwrap().last_error.unwrap();
        assert!(err.contains("needs a new account"), "{err}");
    }

    #[tokio::test]
    async fn under_ha_the_leader_issues_through_raft_and_tags_the_term() {
        let (ha, _cluster, _plugins, store, _dir) =
            crate::ha::test_support::single_node_full(1, "127.0.0.1:1").await;
        let dns = MemDns::default();
        let clock = system_clock();
        let fake = FakeCa::start_with(dns.clone(), clock.clone(), 90 * DAY).await;
        let ca = AcmeCa::new(&fake.directory_url(), true)
            .with_http(FakeCa::http_factory())
            .fast_retry();
        let runner = Runner::new(
            store.clone(),
            ca,
            &fake.directory_url(),
            Arc::new(OneSolver(dns)),
            KeyringHandle::new(Keyring::parse(KEY).unwrap()),
            Some(ha.clone()),
            clock,
        );
        let applied = ha
            .raft
            .client_write(WriteRequest::Acme(AcmeOp::SiteSet(site(
                "web",
                "a.example.com",
            ))))
            .await
            .unwrap();
        assert_eq!(applied.data, WriteResponse::AcmeResult(Applied::Done));
        assert_eq!(runner.run_once().await, ran(&["web"], &[]));
        assert_eq!(store.cert("web").unwrap().unwrap().version, 1);
        assert!(store.account().unwrap().is_some());
        // A late result from a deposed leader (another term) is dropped by the state machine.
        let stale = ha
            .raft
            .client_write(WriteRequest::Acme(AcmeOp::CertIssued {
                site: "web".into(),
                tagged_term: 999,
                cert: store
                    .cert("web")
                    .unwrap()
                    .map(|mut c| {
                        c.version = 2;
                        c
                    })
                    .unwrap(),
            }))
            .await
            .unwrap();
        assert_eq!(stale.data, WriteResponse::AcmeResult(Applied::WrongTerm));
    }
}
