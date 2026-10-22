//! The controller's DNS-01 solver: it asks a DNS-provider plugin (trigger `on_acme_dns`) to set
//! and remove the TXT record, and looks the record up itself before the CA is told to
//! validate. The plugin never sees an ACME key.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use wayhouse_acme::{DnsSolver, SolverError};
use wayhouse_plugin_host::{AcmeDnsOp, AcmeDnsRequest};

use super::runner::SolverSource;
use crate::plugins::runner::{HookError, Runner as PluginRunner};
use crate::plugins::PluginStore;

/// How long one `present` or `cleanup` call may take, queue wait included.
const CALL_LIMIT: Duration = Duration::from_secs(120);
/// Pause between two lookups while waiting for a record to appear.
const POLL: Duration = Duration::from_secs(5);

/// Reads TXT records the way the CA's resolver will see them.
#[async_trait]
pub trait TxtLookup: Send + Sync {
    /// The TXT strings at `fqdn`; an empty list when there are none or the lookup failed.
    async fn txt(&self, fqdn: &str) -> Vec<String>;
}

/// The host's own resolver (`/etc/resolv.conf`). Caching is off here, which only covers this
/// process: a caching resolver upstream (Unbound, systemd-resolved) can still hold a negative
/// answer for the zone's negative TTL, which is why [`PluginDnsSolver::wait_visible`] does
/// not look up before the first poll interval.
pub struct SystemTxt {
    resolver: hickory_resolver::TokioResolver,
}

impl SystemTxt {
    pub fn new() -> Result<Self, String> {
        let mut builder = hickory_resolver::TokioResolver::builder_tokio()
            .map_err(|e| format!("no usable DNS resolver configuration: {e}"))?;
        builder.options_mut().cache_size = 0;
        let resolver = builder
            .build()
            .map_err(|e| format!("could not build the DNS resolver: {e}"))?;
        Ok(Self { resolver })
    }
}

#[async_trait]
impl TxtLookup for SystemTxt {
    async fn txt(&self, fqdn: &str) -> Vec<String> {
        let Ok(found) = self.resolver.txt_lookup(format!("{fqdn}.")).await else {
            return Vec::new();
        };
        found
            .answers()
            .iter()
            .filter_map(|r| match &r.data {
                hickory_resolver::proto::rr::RData::TXT(t) => Some(
                    t.txt_data
                        .iter()
                        .map(|part| String::from_utf8_lossy(part).into_owned())
                        .collect::<String>(),
                ),
                _ => None,
            })
            .collect()
    }
}

/// Hands out a [`PluginDnsSolver`] for a site whose `dns_plugin` is an enabled install that
/// approved `on_acme_dns`.
pub struct PluginSolvers {
    pub store: PluginStore,
    pub runner: Arc<PluginRunner>,
    pub lookup: Arc<dyn TxtLookup>,
}

impl SolverSource for PluginSolvers {
    fn solver_for(&self, dns_plugin: &str) -> Option<Arc<dyn DnsSolver>> {
        let rec = self.store.get(dns_plugin).ok().flatten()?;
        (rec.enabled && rec.approved.triggers.on_acme_dns).then(|| {
            Arc::new(PluginDnsSolver {
                install: rec.id,
                runner: self.runner.clone(),
                lookup: self.lookup.clone(),
                poll: POLL,
                call_limit: CALL_LIMIT,
            }) as Arc<dyn DnsSolver>
        })
    }
}

pub struct PluginDnsSolver {
    install: String,
    runner: Arc<PluginRunner>,
    lookup: Arc<dyn TxtLookup>,
    poll: Duration,
    call_limit: Duration,
}

impl PluginDnsSolver {
    #[cfg(test)]
    fn fast(mut self) -> Self {
        self.poll = Duration::from_millis(20);
        self
    }

    async fn call(&self, op: AcmeDnsOp, fqdn: &str, value: &str) -> Result<(), SolverError> {
        let zone_hint = fqdn.strip_prefix("_acme-challenge.").unwrap_or(fqdn);
        let request = AcmeDnsRequest {
            op,
            fqdn: fqdn.to_string(),
            value: value.to_string(),
            zone_hint: zone_hint.to_string(),
        };
        let asked = tokio::time::timeout(
            self.call_limit,
            self.runner.run_acme_dns(&self.install, request),
        )
        .await;
        let what = match op {
            AcmeDnsOp::Present => "set",
            AcmeDnsOp::Cleanup => "remove",
        };
        match asked {
            Err(_) => Err(SolverError(format!(
                "the DNS plugin did not {what} the record within {}s",
                self.call_limit.as_secs()
            ))),
            Ok(Ok(Some(answer))) if answer.ok => Ok(()),
            Ok(Ok(Some(answer))) => Err(SolverError(format!(
                "the DNS plugin could not {what} the record: {}",
                answer.message
            ))),
            Ok(Ok(None)) => Err(SolverError(format!(
                "the DNS plugin returned without saying whether it could {what} the record"
            ))),
            Ok(Err(HookError::NotLeader)) => Err(SolverError(
                "this node stopped leading; the order was dropped".into(),
            )),
            Ok(Err(HookError::NotFound)) => Err(SolverError(
                "the DNS plugin is gone, disabled, or no longer approved for on_acme_dns".into(),
            )),
            Ok(Err(HookError::QueueFull)) => Err(SolverError(
                "the DNS plugin is busy (its queue is full)".into(),
            )),
            Ok(Err(HookError::Unavailable(why))) => {
                Err(SolverError(format!("the DNS plugin is unavailable: {why}")))
            }
            Ok(Err(HookError::Failed)) => Err(SolverError(format!(
                "the DNS plugin trapped or ran past its time limit while asked to {what} the record"
            ))),
        }
    }
}

#[async_trait]
impl DnsSolver for PluginDnsSolver {
    async fn present(&self, fqdn: &str, value: &str) -> Result<(), SolverError> {
        self.call(AcmeDnsOp::Present, fqdn, value).await
    }

    async fn cleanup(&self, fqdn: &str, value: &str) -> Result<(), SolverError> {
        self.call(AcmeDnsOp::Cleanup, fqdn, value).await
    }

    async fn wait_visible(
        &self,
        fqdn: &str,
        value: &str,
        timeout: Duration,
    ) -> Result<(), SolverError> {
        // The first lookup waits one interval: asked in the instant before the provider serves
        // the record, a caching resolver would remember "does not exist" for the zone's
        // negative TTL (30 minutes at Cloudflare) and every later poll and retry would get it.
        let poll = async {
            loop {
                tokio::time::sleep(self.poll).await;
                if self.lookup.txt(fqdn).await.iter().any(|t| t == value) {
                    return;
                }
            }
        };
        tokio::time::timeout(timeout, poll).await.map_err(|_| {
            SolverError(format!(
                "the TXT record at {fqdn} was not visible to the resolver within {}s",
                timeout.as_secs()
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use wayhouse_plugin_host::{inspect, CompilePool, Limits, PluginHost};

    use super::*;
    use crate::plugins::InstallRecord;

    const CAPS: &str =
        r#"{"triggers":{"on_timer":true,"on_acme_dns":true},"tick_interval_secs":30,"log":true}"#;

    /// Answers by the first letter of the op: `present` is refused with "nope!!", every other
    /// op is accepted.
    const REFUSE_PRESENT: &str = r#"
        (func (export "on_acme_dns") (param $p i32) (param $n i32)
          (if (i32.eq (i32.load8_u (i32.add (local.get $p) (i32.const 7))) (i32.const 112))
            (then (drop (call $result (i32.const 0) (i32.const 0) (i32.const 6))))
            (else (drop (call $result (i32.const 1) (i32.const 0) (i32.const 0))))))"#;
    const ACCEPT: &str = r#"
        (func (export "on_acme_dns") (param $p i32) (param $n i32)
          (drop (call $result (i32.const 1) (i32.const 0) (i32.const 0))))"#;
    const SILENT: &str = r#"(func (export "on_acme_dns") (param i32 i32))"#;
    const TRAPS: &str = r#"(func (export "on_acme_dns") (param i32 i32) unreachable)"#;

    fn module(caps: &str, on_acme_dns: &str) -> Vec<u8> {
        let caps = caps.replace('"', "\\\"");
        wat::parse_str(format!(
            r#"(module
  (@custom "wayhouse.plugin-abi" "\00\00\01\00")
  (@custom "wayhouse.plugin-caps" "{caps}")
  (import "wayhouse" "acme_dns_result" (func $result (param i32 i32 i32) (result i32)))
  (memory (export "memory") 1)
  (data (i32.const 0) "nope!!")
  (global $bump (mut i32) (i32.const 1024))
  (func (export "alloc") (param i32) (result i32) (local $p i32)
    (local.set $p (global.get $bump))
    (global.set $bump (i32.add (global.get $bump) (local.get 0)))
    (local.get $p))
  (func (export "init") (param i32 i32))
  (func (export "on_timer"))
  {on_acme_dns})"#
        ))
        .unwrap()
    }

    struct Fixture {
        solvers: PluginSolvers,
        store: PluginStore,
        lookup: Arc<Fake>,
        _dir: tempfile::TempDir,
    }

    #[derive(Default)]
    struct Fake {
        /// The value appears after this many lookups; `usize::MAX` means never.
        after: AtomicUsize,
        asked: AtomicUsize,
    }

    #[async_trait]
    impl TxtLookup for Fake {
        async fn txt(&self, _: &str) -> Vec<String> {
            let n = self.asked.fetch_add(1, Ordering::SeqCst) + 1;
            if n > self.after.load(Ordering::SeqCst) {
                vec!["other".into(), "the-value".into()]
            } else {
                Vec::new()
            }
        }
    }

    fn fixture(module: &[u8], enabled: bool) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let db = sled::open(dir.path()).unwrap();
        let store = PluginStore::open(&db).unwrap();
        let host = Arc::new(PluginHost::new(Limits::default()).unwrap());
        let pool = Arc::new(CompilePool::new(host, 1, 4, Duration::from_secs(10)).unwrap());
        let runner = PluginRunner::new(store.clone(), pool);
        let sha = "a".repeat(64);
        store.put_blob(&sha, module).unwrap();
        store
            .create(&InstallRecord {
                id: "dns".into(),
                name: "dns".into(),
                sha256: sha,
                size: module.len(),
                approved: inspect(module).unwrap().caps,
                config: serde_json::json!({}),
                enabled,
                created_at: 1,
                created_by: None,
                webhook: None,
            })
            .unwrap();
        let lookup = Arc::new(Fake::default());
        Fixture {
            solvers: PluginSolvers {
                store: store.clone(),
                runner,
                lookup: lookup.clone(),
            },
            store,
            lookup,
            _dir: dir,
        }
    }

    fn solver(f: &Fixture) -> PluginDnsSolver {
        // The trait object hides the concrete type; rebuild it for the fast poll.
        let rec = f.store.get("dns").unwrap().unwrap();
        assert!(f.solvers.solver_for(&rec.id).is_some());
        PluginDnsSolver {
            install: rec.id,
            runner: f.solvers.runner.clone(),
            lookup: f.solvers.lookup.clone(),
            poll: POLL,
            call_limit: CALL_LIMIT,
        }
        .fast()
    }

    const FQDN: &str = "_acme-challenge.panel.example.com";

    #[tokio::test]
    async fn present_and_cleanup_reach_the_plugin_with_their_op() {
        let f = fixture(&module(CAPS, REFUSE_PRESENT), true);
        let s = solver(&f);
        // The guest refuses `present` and accepts anything else, so the op is what decides.
        let err = s.present(FQDN, "the-value").await.unwrap_err();
        assert!(err.0.contains("nope!!") && err.0.contains("set"), "{err}");
        s.cleanup(FQDN, "the-value").await.unwrap();
    }

    #[tokio::test]
    async fn an_accepting_plugin_succeeds() {
        let f = fixture(&module(CAPS, ACCEPT), true);
        solver(&f).present(FQDN, "the-value").await.unwrap();
    }

    #[tokio::test]
    async fn a_silent_plugin_fails_the_call() {
        let f = fixture(&module(CAPS, SILENT), true);
        let err = solver(&f).present(FQDN, "v").await.unwrap_err();
        assert!(err.0.contains("without saying"), "{err}");
    }

    #[tokio::test]
    async fn a_trapping_plugin_fails_the_call_and_cleanup_still_reaches_it() {
        let f = fixture(&module(CAPS, TRAPS), true);
        let s = solver(&f);
        let err = s.present(FQDN, "v").await.unwrap_err();
        assert!(err.0.contains("trapped"), "{err}");
        // `cleanup` is a separate call that is made (and fails the same way) rather than
        // being skipped.
        let err = s.cleanup(FQDN, "v").await.unwrap_err();
        assert!(
            err.0.contains("trapped") && err.0.contains("remove"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn a_call_that_outlasts_the_limit_fails_instead_of_queuing_forever() {
        let f = fixture(&module(CAPS, ACCEPT), true);
        let mut s = solver(&f);
        s.call_limit = Duration::ZERO;
        let err = s.present(FQDN, "v").await.unwrap_err();
        assert!(err.0.contains("within"), "{err}");
    }

    #[tokio::test]
    async fn only_an_enabled_install_that_approved_the_trigger_gets_a_solver() {
        let f = fixture(&module(CAPS, ACCEPT), false);
        assert!(f.solvers.solver_for("dns").is_none());
        assert!(f.solvers.solver_for("missing").is_none());
        let g = fixture(
            &module(
                r#"{"triggers":{"on_timer":true},"tick_interval_secs":30}"#,
                "",
            ),
            true,
        );
        assert!(g.solvers.solver_for("dns").is_none());
    }

    #[tokio::test]
    async fn wait_visible_polls_until_the_value_shows_up() {
        let f = fixture(&module(CAPS, ACCEPT), true);
        f.lookup.after.store(3, Ordering::SeqCst);
        solver(&f)
            .wait_visible(FQDN, "the-value", Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(f.lookup.asked.load(Ordering::SeqCst), 4);
    }

    #[tokio::test]
    async fn wait_visible_leaves_the_provider_a_moment_before_the_first_lookup() {
        // A lookup in the instant between the plugin's API call and the provider serving the
        // record makes a caching resolver remember "does not exist" for the zone's negative TTL.
        let f = fixture(&module(CAPS, ACCEPT), true);
        f.lookup.after.store(0, Ordering::SeqCst);
        let s = PluginDnsSolver {
            poll: Duration::from_millis(150),
            ..solver(&f)
        };
        let start = std::time::Instant::now();
        s.wait_visible(FQDN, "the-value", Duration::from_secs(5))
            .await
            .unwrap();
        assert!(
            start.elapsed() >= Duration::from_millis(150),
            "{:?}",
            start.elapsed()
        );
        assert_eq!(f.lookup.asked.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn wait_visible_gives_up_at_the_timeout() {
        let f = fixture(&module(CAPS, ACCEPT), true);
        f.lookup.after.store(usize::MAX, Ordering::SeqCst);
        let err = solver(&f)
            .wait_visible(FQDN, "the-value", Duration::from_millis(100))
            .await
            .unwrap_err();
        assert!(err.0.contains("not visible"), "{err}");
    }
}
