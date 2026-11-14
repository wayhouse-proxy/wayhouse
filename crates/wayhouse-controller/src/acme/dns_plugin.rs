//! The controller's DNS-01 solver: it asks a DNS-provider plugin (trigger `on_acme_dns`) to set
//! and remove the TXT record, and looks the record up itself before the CA is told to
//! validate. The plugin never sees an ACME key.

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use wayhouse_acme::{DnsSolver, SolverError};
use wayhouse_plugin_host::{AcmeDnsOp, AcmeDnsRequest};

use super::runner::SolverSource;
use crate::plugins::runner::{HookError, Runner as PluginRunner};
use crate::plugins::PluginStore;

/// The least one `present` or `cleanup` call may take, queue wait included; a site with a longer
/// `propagation_timeout` allows its provider that long (see [`call_limit`]).
const CALL_FLOOR: Duration = Duration::from_secs(30);
/// Pause between two lookups while waiting for a record to appear.
const POLL: Duration = Duration::from_secs(5);
/// How long to leave the provider before the first lookup. Asked in the instant before the
/// provider serves the record, a caching resolver remembers "does not exist" for the zone's
/// negative TTL (30 minutes at Cloudflare), so the first lookup waits longer than a poll.
const FIRST_WAIT: Duration = Duration::from_secs(20);
/// How long one query to an authoritative nameserver may take.
const NS_TIMEOUT: Duration = Duration::from_secs(3);

fn call_limit(propagation: Duration) -> Duration {
    propagation.max(CALL_FLOOR)
}

/// Reads TXT records the way the CA's resolver will see them.
#[async_trait]
pub trait TxtLookup: Send + Sync {
    /// The TXT strings at `fqdn`; an empty list when there are none or the lookup failed.
    async fn txt(&self, fqdn: &str) -> Vec<String>;
}

/// Looks a record up at the zone's own nameservers (no recursion, so no cache can hold an
/// early "does not exist"), and through the host's resolver (`/etc/resolv.conf`) as well:
/// a network that redirects port-53 traffic to a local resolver, or a CNAME-delegated name,
/// only shows up there. A record is visible when either way shows it; an authoritative answer
/// counts only for what *every* answering server has.
pub struct SystemTxt {
    resolver: hickory_resolver::TokioResolver,
    /// The nameservers' port; 53 outside tests.
    ns_port: u16,
}

impl SystemTxt {
    /// Uses the host resolver config; without one (no `/etc/resolv.conf`) a default public
    /// resolver, so a missing file does not stop the controller (the `dns_srv` source does the same).
    pub fn new() -> Self {
        let mut builder = hickory_resolver::TokioResolver::builder_tokio().unwrap_or_else(|e| {
            tracing::warn!(error = %e, "acme: system resolver config unavailable; using defaults");
            hickory_resolver::TokioResolver::builder_with_config(
                hickory_resolver::config::ResolverConfig::default(),
                hickory_resolver::net::runtime::TokioRuntimeProvider::default(),
            )
        });
        // Only this process's cache: a caching resolver upstream can still hold a negative answer.
        builder.options_mut().cache_size = 0;
        Self {
            resolver: builder
                .build()
                .expect("building a resolver from a config never fails"),
            ns_port: 53,
        }
    }

    /// The addresses of the nameservers of the zone that holds `fqdn`, found by walking up
    /// from the name to the first label set that has `NS` records.
    async fn zone_servers(&self, fqdn: &str) -> Vec<IpAddr> {
        let labels: Vec<&str> = fqdn.trim_end_matches('.').split('.').collect();
        for start in 1..labels.len().saturating_sub(1) {
            let Ok(ns) = self
                .resolver
                .ns_lookup(format!("{}.", labels[start..].join(".")))
                .await
            else {
                continue;
            };
            let mut ips = Vec::new();
            for r in ns.answers() {
                if let hickory_resolver::proto::rr::RData::NS(host) = &r.data {
                    if let Ok(found) = self.resolver.lookup_ip(host.0.clone()).await {
                        ips.extend(found.iter());
                    }
                }
            }
            return ips;
        }
        Vec::new()
    }

    /// The TXT strings every answering server of `servers` shows at `fqdn`; empty when none
    /// answered or they disagree.
    async fn ask(&self, servers: &[IpAddr], fqdn: &str) -> Vec<String> {
        let mut asked = tokio::task::JoinSet::new();
        for ip in servers {
            asked.spawn(ask_one(*ip, self.ns_port, fqdn.to_string()));
        }
        let mut shared: Option<Vec<String>> = None;
        while let Some(done) = asked.join_next().await {
            let Ok(Some(found)) = done else { continue };
            shared = Some(match shared {
                None => found,
                Some(have) => have.into_iter().filter(|t| found.contains(t)).collect(),
            });
        }
        shared.unwrap_or_default()
    }

    async fn host(&self, fqdn: &str) -> Vec<String> {
        self.resolver
            .txt_lookup(format!("{fqdn}."))
            .await
            .map_or_else(|_| Vec::new(), |found| txt_strings(&found))
    }
}

impl Default for SystemTxt {
    fn default() -> Self {
        Self::new()
    }
}

/// One non-recursive TXT query to `ip`. `None` when the server did not answer.
async fn ask_one(ip: IpAddr, port: u16, fqdn: String) -> Option<Vec<String>> {
    use hickory_resolver::config::{ConnectionConfig, NameServerConfig, ResolverConfig};
    use hickory_resolver::net::runtime::TokioRuntimeProvider;
    let mut udp = ConnectionConfig::udp();
    udp.port = port;
    let config =
        ResolverConfig::from_name_servers(vec![NameServerConfig::new(ip, true, vec![udp])]);
    let mut builder = hickory_resolver::TokioResolver::builder_with_config(
        config,
        TokioRuntimeProvider::default(),
    );
    let opts = builder.options_mut();
    opts.recursion_desired = false;
    opts.cache_size = 0;
    opts.attempts = 1;
    opts.timeout = NS_TIMEOUT;
    match builder.build().ok()?.txt_lookup(format!("{fqdn}.")).await {
        Ok(found) => Some(txt_strings(&found)),
        Err(e) if e.is_no_records_found() => Some(Vec::new()),
        Err(_) => None,
    }
}

fn txt_strings(found: &hickory_resolver::lookup::Lookup) -> Vec<String> {
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

#[async_trait]
impl TxtLookup for SystemTxt {
    async fn txt(&self, fqdn: &str) -> Vec<String> {
        let servers = self.zone_servers(fqdn).await;
        let mut found = self.ask(&servers, fqdn).await;
        found.extend(self.host(fqdn).await);
        found
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
    fn solver_for(
        &self,
        dns_plugin: &str,
        propagation_timeout: Duration,
    ) -> Option<Arc<dyn DnsSolver>> {
        let rec = self.store.get(dns_plugin).ok().flatten()?;
        (rec.enabled && rec.approved.triggers.on_acme_dns).then(|| {
            Arc::new(PluginDnsSolver {
                install: rec.id,
                runner: self.runner.clone(),
                lookup: self.lookup.clone(),
                poll: POLL,
                first_wait: FIRST_WAIT,
                call_limit: call_limit(propagation_timeout),
            }) as Arc<dyn DnsSolver>
        })
    }
}

pub struct PluginDnsSolver {
    install: String,
    runner: Arc<PluginRunner>,
    lookup: Arc<dyn TxtLookup>,
    poll: Duration,
    first_wait: Duration,
    call_limit: Duration,
}

impl PluginDnsSolver {
    #[cfg(test)]
    fn fast(mut self) -> Self {
        self.poll = Duration::from_millis(20);
        self.first_wait = Duration::from_millis(20);
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
        // The first lookup waits (see FIRST_WAIT), but never more than a quarter of the time
        // allowed, so a short `propagation_timeout` still gets a few lookups.
        let poll = async {
            tokio::time::sleep(self.first_wait.min(timeout / 4)).await;
            loop {
                if self.lookup.txt(fqdn).await.iter().any(|t| t == value) {
                    return;
                }
                tokio::time::sleep(self.poll).await;
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
  (@custom "wayhouse.plugin-abi" "\00\00\02\00")
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
        let db = crate::kv::Db::open(dir.path()).unwrap();
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
                suspended: std::collections::BTreeSet::default(),
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
        assert!(f.solvers.solver_for(&rec.id, CALL_FLOOR).is_some());
        PluginDnsSolver {
            install: rec.id,
            runner: f.solvers.runner.clone(),
            lookup: f.solvers.lookup.clone(),
            poll: POLL,
            first_wait: FIRST_WAIT,
            call_limit: CALL_FLOOR,
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
        assert!(f.solvers.solver_for("dns", CALL_FLOOR).is_none());
        assert!(f.solvers.solver_for("missing", CALL_FLOOR).is_none());
        let g = fixture(
            &module(
                r#"{"triggers":{"on_timer":true},"tick_interval_secs":30}"#,
                "",
            ),
            true,
        );
        assert!(g.solvers.solver_for("dns", CALL_FLOOR).is_none());
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
            first_wait: Duration::from_millis(150),
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

    #[tokio::test]
    async fn a_short_propagation_timeout_shortens_the_first_wait_so_it_can_still_succeed() {
        let f = fixture(&module(CAPS, ACCEPT), true);
        f.lookup.after.store(0, Ordering::SeqCst);
        let s = PluginDnsSolver {
            first_wait: Duration::from_secs(60),
            ..solver(&f)
        };
        s.wait_visible(FQDN, "the-value", Duration::from_millis(800))
            .await
            .unwrap();
    }

    #[test]
    fn a_plugin_call_may_take_the_sites_propagation_timeout_but_never_less_than_the_floor() {
        assert_eq!(call_limit(Duration::from_secs(10)), CALL_FLOOR);
        assert_eq!(
            call_limit(Duration::from_secs(900)),
            Duration::from_secs(900)
        );
    }

    /// A nameserver on loopback that answers every TXT query with `values` (none: NXDOMAIN).
    async fn fake_ns(ip: &str, port: u16, values: &[&str]) -> std::net::SocketAddr {
        use hickory_resolver::proto::op::{Message, MessageType, OpCode, ResponseCode};
        use hickory_resolver::proto::rr::{rdata::TXT, RData, Record};
        let sock = tokio::net::UdpSocket::bind((ip, port)).await.unwrap();
        let addr = sock.local_addr().unwrap();
        let values: Vec<String> = values.iter().map(ToString::to_string).collect();
        tokio::spawn(async move {
            let mut buf = [0u8; 512];
            loop {
                let Ok((n, from)) = sock.recv_from(&mut buf).await else {
                    return;
                };
                let Ok(q) = Message::from_vec(&buf[..n]) else {
                    continue;
                };
                let mut r = Message::new(q.metadata.id, MessageType::Response, OpCode::Query);
                r.metadata.authoritative = true;
                r.add_queries(q.queries.clone());
                if values.is_empty() {
                    r.metadata.response_code = ResponseCode::NXDomain;
                }
                for v in &values {
                    let name = q.queries[0].name().clone();
                    r.add_answer(Record::from_rdata(
                        name,
                        60,
                        RData::TXT(TXT::new(vec![v.clone()])),
                    ));
                }
                let _ = sock.send_to(&r.to_vec().unwrap(), from).await;
            }
        });
        addr
    }

    fn txt_at(port: u16) -> SystemTxt {
        SystemTxt {
            ns_port: port,
            ..SystemTxt::new()
        }
    }

    #[tokio::test]
    async fn the_authoritative_servers_show_a_record_a_caching_resolver_does_not() {
        let ns = fake_ns("127.0.0.1", 0, &["other", "the-value"]).await;
        let found = txt_at(ns.port()).ask(&[ns.ip()], FQDN).await;
        assert!(found.iter().any(|t| t == "the-value"), "{found:?}");
    }

    #[tokio::test]
    async fn a_value_counts_only_when_every_answering_server_has_it() {
        // Two loopback addresses, one port: the same lookup asks both.
        let has = fake_ns("127.0.0.1", 0, &["the-value"]).await;
        let lacks = fake_ns("127.0.0.2", has.port(), &[]).await;
        let t = txt_at(has.port());
        assert!(t
            .ask(&[has.ip()], FQDN)
            .await
            .contains(&"the-value".to_string()));
        assert!(t.ask(&[has.ip(), lacks.ip()], FQDN).await.is_empty());
    }

    #[tokio::test]
    async fn a_server_that_does_not_answer_is_ignored() {
        let dead = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let t = txt_at(dead.local_addr().unwrap().port());
        let found = tokio::time::timeout(
            Duration::from_secs(15),
            t.ask(&[dead.local_addr().unwrap().ip()], FQDN),
        )
        .await
        .unwrap();
        assert!(found.is_empty());
    }
}
