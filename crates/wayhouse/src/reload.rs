//! Hot configuration reload.
//!
//! A `SIGHUP` or a change to the config file rebuilds the snapshot and swaps it
//! in atomically. Pool membership, balancer, health-check and capacity changes
//! take effect live, and so do listeners (reconciled by name: added, removed or
//! re-spawned), resolvers and backend sources. Only the startup-only settings
//! (`workers`, `limits`, `geo_db`, `admin.listen` / `admin.tls`, sniffer engine limits) need a
//! restart. A rotated admin token or gossip key does not: references are read again on every
//! reload and the old value keeps working for [`crate::secrets::OVERLAP`].

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::Notify;

use wayhouse_core::metrics_defs as m;
use wayhouse_core::sniff::Sniffers;
use wayhouse_core::{Resolvers, RuntimeHandle, Snapshot};

use crate::secrets::Live;
use crate::sniffer_loader::SnifferLoader;

/// Quiet period the trigger must stay silent for before a reload runs.
const DEBOUNCE: Duration = Duration::from_millis(200);
/// Upper bound on how long a continuous stream of triggers can postpone a reload.
const MAX_DEBOUNCE: Duration = Duration::from_secs(2);

/// Trailing-edge debounce: waits until `trigger` has been quiet for `quiet`, so
/// a burst of file events spaced wider than one window still coalesces into a
/// single reload. `max` caps the total wait so a steady stream can't starve it.
async fn settle(trigger: &Notify, quiet: Duration, max: Duration) {
    let deadline = tokio::time::Instant::now() + max;
    loop {
        let wait = quiet.min(deadline.saturating_duration_since(tokio::time::Instant::now()));
        if tokio::time::timeout(wait, trigger.notified())
            .await
            .is_err()
        {
            return;
        }
    }
}

pub async fn run(
    path: PathBuf,
    handle: RuntimeHandle,
    resolvers: Arc<Resolvers>,
    sniffer_loader: Option<Arc<SnifferLoader>>,
    sniffers: Arc<Sniffers>,
    live: Arc<Live>,
) {
    let trigger = Arc::new(Notify::new());
    // Kept alive for the lifetime of this task; dropping it stops the watch.
    let _watcher = spawn_watcher(&path, trigger.clone())
        .inspect_err(
            |e| tracing::warn!(error = %e, "config file watch unavailable; SIGHUP-only reload"),
        )
        .ok();

    #[cfg(unix)]
    {
        let mut sighup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())
            .expect("install SIGHUP handler");
        let admin = handle.reload_requested().clone();
        loop {
            tokio::select! {
                _ = sighup.recv() => {}
                _ = trigger.notified() => {}
                _ = admin.notified() => {}
            }
            settle(&trigger, DEBOUNCE, MAX_DEBOUNCE).await;
            apply(
                &path,
                &handle,
                &resolvers,
                sniffer_loader.as_deref(),
                &sniffers,
                &live,
            )
            .await;
        }
    }

    #[cfg(not(unix))]
    {
        let admin = handle.reload_requested().clone();
        loop {
            tokio::select! {
                _ = trigger.notified() => {}
                _ = admin.notified() => {}
            }
            settle(&trigger, DEBOUNCE, MAX_DEBOUNCE).await;
            apply(
                &path,
                &handle,
                &resolvers,
                sniffer_loader.as_deref(),
                &sniffers,
                &live,
            )
            .await;
        }
    }
}

async fn apply(
    path: &Path,
    handle: &RuntimeHandle,
    resolvers: &Resolvers,
    sniffer_loader: Option<&SnifferLoader>,
    sniffers: &Sniffers,
    live: &Live,
) {
    match crate::plugin_overlay::load(path) {
        Ok(cfg) => {
            apply_config(
                cfg,
                handle,
                resolvers,
                sniffer_loader,
                sniffers,
                live,
                &format!("file {}", path.display()),
            )
            .await;
        }
        Err(e) => {
            metrics::counter!(m::CONFIG_RELOAD, "result" => "failed").increment(1);
            tracing::error!(error = %e, "config reload failed; keeping current configuration");
        }
    }
}

/// What [`apply_config`] did with a config.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Applied {
    Yes,
    /// The config itself was refused; sending it again changes nothing.
    Rejected,
    /// Its secrets could not be read or are unusable (say a transient fetch failure): the same
    /// config may apply later, so a caller that can ask again should.
    SecretsUnavailable,
}

/// Rebuilds the snapshot from an already-parsed-and-validated [`wayhouse_config::Config`]
/// and reconciles everything that reads it — the shared tail of a file
/// reload (`apply`, above) and a controller-pushed revision
/// (`controller_client::run`, phase 10+11 slice 3). Splitting this out means
/// both triggers get the exact same rebuild/reconcile behavior; only how the
/// `Config` was obtained (and validated — the caller ran `wayhouse_config::load` /
/// `parse_str` before this) differs.
pub(crate) async fn apply_config(
    cfg: wayhouse_config::Config,
    handle: &RuntimeHandle,
    resolvers: &Resolvers,
    sniffer_loader: Option<&SnifferLoader>,
    sniffers: &Sniffers,
    live: &Live,
    source_desc: &str,
) -> Applied {
    for w in cfg.warnings() {
        tracing::warn!(source = source_desc, "{w}");
    }
    // Secret references are read again on every reload, so a rotated admin token or gossip key
    // applies without a restart. A secret that cannot be read, or a value that is too weak,
    // rejects the reload and the old secrets keep working.
    let cfg = match live.prepare(cfg).await {
        Ok(cfg) => cfg,
        Err(e) => {
            metrics::counter!(m::CONFIG_RELOAD, "result" => "failed").increment(1);
            tracing::error!(
                source = source_desc,
                error = %e,
                "secrets could not be applied; reload rejected, keeping the current configuration and secrets"
            );
            return Applied::SecretsUnavailable;
        }
    };
    // Scan first: if a pinned module no longer validates, keep the previous config
    // and sniffer set together instead of activating routes that name sniffers the
    // old set cannot serve.
    let scanned = match scan_sniffers(&cfg, sniffer_loader) {
        Ok(scanned) => scanned,
        Err(e) => {
            metrics::counter!(m::CONFIG_RELOAD, "result" => "failed").increment(1);
            tracing::error!(
                source = source_desc,
                error = %e,
                "sniffer rescan failed; reload rejected, keeping the current configuration and sniffer set"
            );
            return Applied::Rejected;
        }
    };
    let prev = handle.current();
    let listeners_changed = cfg.listeners != prev.listeners;
    let resolvers_changed = cfg.resolvers != prev.resolvers;
    let next = Snapshot::build_with_sources(
        &cfg,
        Some(&prev),
        handle.backend_overlay(),
        handle.discovery(),
    );
    let sources_changed = next.sources != prev.sources;
    handle.store(next);
    // Straight after the snapshot swap, with no await in between.
    if let Some(map) = scanned {
        let count = map.len();
        sniffers.replace(map);
        tracing::info!(count, "sniffers rescanned");
    }
    metrics::counter!(m::CONFIG_RELOAD, "result" => "ok").increment(1);
    metrics::gauge!(m::CONFIG_VERSION).set(unix_now());
    crate::features::publish(&cfg.features, handle);
    live.rotate(&cfg, handle, source_desc);
    // Always reconcile, not only when the listener definitions changed: the
    // snapshot above is already stored, so after a failed (re)bind the running
    // group no longer matches it, and only a reconcile on a later reload — even
    // of the same file — retries the bind. With nothing to do it is a cheap diff.
    {
        let out = handle.reconcile_listeners().await;
        if listeners_changed || out.stopped > 0 || !out.failed.is_empty() {
            tracing::info!(
                running = out.running,
                stopped = out.stopped,
                failed = out.failed.len(),
                "listeners reconciled (added / removed / rebound)"
            );
        }
        for e in &out.failed {
            metrics::counter!(m::LISTENER_BIND_FAILURES, "listener" => e.listener.clone())
                .increment(1);
        }
    }
    if sources_changed {
        let (running, stopped) = handle.reconcile_sources().await;
        tracing::info!(
            running,
            stopped,
            "backend_sources changed; discovery refresh tasks reconciled \
             (added / removed / restarted)"
        );
    }
    if resolvers_changed {
        rebuild_resolvers(&cfg, resolvers);
    }
    tracing::info!(source = %source_desc, "configuration reloaded");
    Applied::Yes
}

/// Rebuild the external-resolver clients from the new `resolvers:` and swap the
/// whole set in (`Resolvers::replace`). Only called when `ResolverConfig`
/// actually changed, so a plain reload / discovery tick / overlay edit never
/// drops a `CachedResolver`'s LRU cache. A build error (bad endpoint URL) keeps
/// the previous set, same spirit as `rescan_sniffers`; the snapshot has already
/// swapped so pool / routing changes still land.
fn rebuild_resolvers(cfg: &wayhouse_config::Config, resolvers: &Resolvers) {
    match crate::resolver::build_resolvers(cfg) {
        Ok(map) => {
            let n = map.len();
            resolvers.replace(map);
            tracing::info!(count = n, "external resolvers rebuilt (caches reset)");
        }
        Err(e) => tracing::error!(
            error = %e,
            "resolver rebuild failed; keeping the previous resolver set"
        ),
    }
}

/// Phase 9 slice 4: `settings.sniffers.dir` is scanned on every reload — an
/// added module loads, a removed one drops, a changed one recompiles (its
/// hash no longer matches whatever's already registered under that name, so
/// it's simply a fresh `WasmSniffer`). The engine itself (hence
/// `call_timeout_ms` / `max_memory_bytes`) is startup-only, like
/// `settings.workers` — `sniffer_loader` is `None` unless `settings.sniffers`
/// was present at process start, and enabling/disabling the block itself
/// still needs a restart.
///
/// The scan runs *before* the new snapshot is activated (#214): a failing scan (a
/// pinned module that no longer validates) rejects the whole reload, so the new
/// routes never run against the old sniffer set.
pub(crate) fn scan_sniffers(
    cfg: &wayhouse_config::Config,
    sniffer_loader: Option<&SnifferLoader>,
) -> anyhow::Result<Option<std::collections::HashMap<String, Arc<dyn wayhouse_core::sniff::Sniffer>>>>
{
    match (sniffer_loader, &cfg.sniffers) {
        (Some(loader), Some(sc)) => loader.scan(sc).map(Some),
        (None, Some(_)) | (Some(_), None) => {
            tracing::warn!(
                "settings.sniffers presence changed; a restart is needed for that to take effect"
            );
            Ok(None)
        }
        (None, None) => Ok(None),
    }
}

fn spawn_watcher(path: &Path, trigger: Arc<Notify>) -> notify::Result<RecommendedWatcher> {
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        match res {
            // The admin API's own staging files (`/config/file`) are not a config edit.
            Ok(event)
                if event.paths.iter().all(|p| is_staging_file(p)) && !event.paths.is_empty() => {}
            Ok(_) => trigger.notify_one(),
            Err(_) => {}
        }
    })?;
    // Watch the parent directory so atomic-rename saves are seen; fall back to
    // the file itself when there is no parent component.
    let target = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| path.to_path_buf());
    watcher.watch(&target, RecursiveMode::NonRecursive)?;
    Ok(watcher)
}

/// A temp or backup file the admin API writes next to the config (`.<name>.<pid>.new.tmp`, `<name>.prev`).
fn is_staging_file(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == "tmp" || e == "prev")
}

/// Unix timestamp (whole seconds) for the `wayhouse_config_version` gauge.
pub fn unix_now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as f64)
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn settle_coalesces_events_spaced_inside_the_quiet_period() {
        let n = Arc::new(Notify::new());
        let n2 = n.clone();
        tokio::spawn(async move {
            // Events 150 ms apart: each is within one 200 ms window of the last.
            for _ in 0..4 {
                tokio::time::sleep(Duration::from_millis(150)).await;
                n2.notify_one();
            }
        });
        let start = tokio::time::Instant::now();
        settle(&n, Duration::from_millis(200), Duration::from_secs(2)).await;
        // Last event at 600 ms, then 200 ms of quiet.
        assert_eq!(start.elapsed(), Duration::from_millis(800));
    }

    #[tokio::test(start_paused = true)]
    async fn settle_is_capped_by_max() {
        let n = Arc::new(Notify::new());
        let n2 = n.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(100)).await;
                n2.notify_one();
            }
        });
        let start = tokio::time::Instant::now();
        settle(&n, Duration::from_millis(200), Duration::from_secs(1)).await;
        assert_eq!(start.elapsed(), Duration::from_secs(1));
    }

    /// A failed rebind is retried by reloading the *same* config once the port
    /// is free: `apply_config` must reconcile even though the listener
    /// definitions equal the already-stored snapshot.
    #[tokio::test]
    async fn reloading_an_unchanged_config_retries_a_failed_listener_bind() {
        let free = || {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap()
        };
        let (p1, p2) = (free(), free());
        let yaml = |bind: std::net::SocketAddr| {
            format!(
                "pools:\n  - name: p\n    targets: [\"127.0.0.1:9\"]\n    health_check:\n      type: none\n\
                 listeners:\n  - name: l1\n    bind: \"{bind}\"\n    pool: p\n"
            )
        };
        let parse = |bind| wayhouse_config::parse_str(&yaml(bind)).unwrap();
        let runtime =
            wayhouse_core::Runtime::start(Snapshot::from_config(&parse(p1)), Arc::default(), 1);
        let handle = runtime.handle();
        let resolvers = Resolvers::default();
        let sniffers = Sniffers::default();

        let blocker = std::net::TcpListener::bind(p2).unwrap();
        apply_config(
            parse(p2),
            &handle,
            &resolvers,
            None,
            &sniffers,
            &Live::default(),
            "test",
        )
        .await;
        assert!(
            tokio::net::TcpStream::connect(p1).await.is_ok(),
            "the old listener keeps serving after the failed rebind"
        );

        drop(blocker);
        apply_config(
            parse(p2),
            &handle,
            &resolvers,
            None,
            &sniffers,
            &Live::default(),
            "test",
        )
        .await;
        assert!(
            tokio::net::TcpStream::connect(p2).await.is_ok(),
            "reloading the same config retried the bind"
        );
        assert!(
            tokio::net::TcpStream::connect(p1).await.is_err(),
            "the old bind was released"
        );
        runtime
            .shutdown_with_grace(Duration::from_millis(100))
            .await;
    }

    /// A document from a newer schema is refused on the reload path too, and
    /// the running config keeps serving.
    #[tokio::test]
    async fn reload_with_newer_schema_keeps_running_config() {
        let bind = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap()
        };
        let yaml = |header: &str| {
            format!(
                "{header}pools:\n  - name: p\n    targets: [\"127.0.0.1:9\"]\n    health_check:\n      type: none\n\
                 listeners:\n  - name: l1\n    bind: \"{bind}\"\n    pool: p\n"
            )
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        std::fs::write(&path, yaml("")).unwrap();
        let cfg = wayhouse_config::load(&path).unwrap();
        let runtime = wayhouse_core::Runtime::start(Snapshot::from_config(&cfg), Arc::default(), 1);
        let handle = runtime.handle();
        // Not applied yet: start only builds the snapshot, so bring the listener up.
        apply_config(
            cfg,
            &handle,
            &Resolvers::default(),
            None,
            &Sniffers::default(),
            &Live::default(),
            "test",
        )
        .await;
        assert!(tokio::net::TcpStream::connect(bind).await.is_ok());

        std::fs::write(&path, yaml("schema_version: 5\n")).unwrap();
        apply(
            &path,
            &handle,
            &Resolvers::default(),
            None,
            &Sniffers::default(),
            &Live::default(),
        )
        .await;
        assert!(
            tokio::net::TcpStream::connect(bind).await.is_ok(),
            "the running config keeps serving after the newer document is refused"
        );
        assert_eq!(handle.snapshot().listeners.len(), 1);
        runtime
            .shutdown_with_grace(Duration::from_millis(100))
            .await;
    }

    const KEY: &str = "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=";
    const FIRST: &str = "first-admin-token-0123456789";
    const SECOND: &str = "second-admin-token-0123456789";

    /// A proxy on a file config with a sealed admin token and a gossip key, plus the pieces a
    /// test needs to rotate them: the secrets file, and the live token the admin API checks.
    struct Rig {
        _dir: tempfile::TempDir,
        store: PathBuf,
        live: Live,
        admin: Arc<wayhouse_http::rotating::RotatingSecret>,
        runtime: wayhouse_core::Runtime,
        handle: RuntimeHandle,
    }

    const SEALED: &str =
        "schema_version: 4\nsettings:\n  admin:\n    auth_token: { secret: admin-token }\n\
        pools:\n  - name: p\n    targets: [\"127.0.0.1:9\"]\n    health_check:\n      type: none\n\
        listeners:\n  - name: l1\n    bind: \"127.0.0.1:0\"\n    pool: p\n";

    fn seal(rig_store: &Path, name: &str, value: &str) {
        let ring = wayhouse_secrets::Keyring::parse(KEY).unwrap();
        crate::secret_store::set(rig_store, &ring, name, value.as_bytes()).unwrap();
    }

    fn rig() -> Rig {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let (store, keys) = (dir.path().join("secrets"), dir.path().join("keys"));
        std::fs::write(&keys, format!("{KEY}\n")).unwrap();
        std::fs::set_permissions(&keys, std::fs::Permissions::from_mode(0o600)).unwrap();
        seal(&store, "admin-token", FIRST);
        let admin = Arc::new(wayhouse_http::rotating::RotatingSecret::new(FIRST));
        let live = Live {
            source: crate::secrets::SecretSource {
                controller: None,
                file: Some(store.clone()),
                key_file: Some(keys),
            },
            admin: Some(admin.clone()),
        };
        let cfg = wayhouse_config::parse_str(SEALED).unwrap();
        let runtime = wayhouse_core::Runtime::start(Snapshot::from_config(&cfg), Arc::default(), 1);
        let handle = runtime.handle();
        Rig {
            _dir: dir,
            store,
            live,
            admin,
            runtime,
            handle,
        }
    }

    async fn reload(r: &Rig, yaml: &str) -> bool {
        reload_as(r, yaml).await == Applied::Yes
    }

    async fn reload_as(r: &Rig, yaml: &str) -> Applied {
        let cfg = wayhouse_config::parse_str(yaml).unwrap();
        apply_config(
            cfg,
            &r.handle,
            &Resolvers::default(),
            None,
            &Sniffers::default(),
            &r.live,
            "test",
        )
        .await
    }

    #[tokio::test]
    async fn a_reload_picks_up_a_rotated_admin_token_and_keeps_the_old_one_for_the_window() {
        let r = rig();
        seal(&r.store, "admin-token", SECOND);
        assert!(reload(&r, SEALED).await);
        assert!(r.admin.accepts(SECOND));
        assert!(
            r.admin.accepts(FIRST),
            "the old token works inside the overlap window"
        );
        r.runtime
            .shutdown_with_grace(Duration::from_millis(100))
            .await;
    }

    #[tokio::test]
    async fn a_reload_whose_secret_is_missing_is_rejected_and_the_token_stays() {
        let r = rig();
        std::fs::remove_file(&r.store).unwrap();
        let before = r.handle.snapshot().listeners.len();
        assert!(!reload(&r, &SEALED.replace("l1", "l2")).await);
        assert!(r.admin.accepts(FIRST));
        assert_eq!(r.handle.snapshot().listeners.len(), before);
        assert_eq!(
            r.handle.snapshot().listeners[0].name,
            "l1",
            "the rejected config is not live"
        );
        r.runtime
            .shutdown_with_grace(Duration::from_millis(100))
            .await;
    }

    #[tokio::test]
    async fn a_too_short_rotated_token_rejects_the_reload() {
        let r = rig();
        seal(&r.store, "admin-token", "only-ten-b");
        assert!(!reload(&r, SEALED).await);
        assert!(!r.admin.accepts("only-ten-b"));
        assert!(r.admin.accepts(FIRST));
        r.runtime
            .shutdown_with_grace(Duration::from_millis(100))
            .await;
    }

    #[tokio::test]
    async fn a_reload_cannot_remove_the_admin_token() {
        let r = rig();
        let open = "pools:\n  - name: p\n    targets: [\"127.0.0.1:9\"]\n    health_check:\n      type: none\n\
            listeners:\n  - name: l2\n    bind: \"127.0.0.1:0\"\n    pool: p\n";
        assert!(
            !reload(&r, open).await,
            "dropping auth is a restart-only change"
        );
        assert!(r.admin.accepts(FIRST));
        assert_eq!(r.handle.snapshot().listeners[0].name, "l1");
        r.runtime
            .shutdown_with_grace(Duration::from_millis(100))
            .await;
    }

    #[derive(Clone, Default)]
    struct Capture(Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for Capture {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Capture {
        type Writer = Capture;
        fn make_writer(&'a self) -> Capture {
            self.clone()
        }
    }

    /// No log line of a reload carries a secret value: not on a rotation, not when the new value
    /// is refused as too short, not when the secret is gone.
    #[tokio::test]
    async fn no_reload_log_line_carries_a_secret_value() {
        let log = Capture::default();
        let _guard = tracing::subscriber::set_default(
            tracing_subscriber::fmt()
                .with_max_level(tracing::Level::TRACE)
                .with_writer(log.clone())
                .with_ansi(false)
                .finish(),
        );
        let r = rig();
        seal(&r.store, "admin-token", SECOND);
        assert!(reload(&r, SEALED).await);
        seal(&r.store, "admin-token", "only-ten-b");
        assert!(!reload(&r, SEALED).await);
        std::fs::remove_file(&r.store).unwrap();
        assert!(!reload(&r, SEALED).await);
        let text = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
        assert!(text.contains("rotated"), "the rotation was logged: {text}");
        for secret in [FIRST, SECOND, "only-ten-b"] {
            assert!(!text.contains(secret), "a log line carries a secret value");
        }
        r.runtime
            .shutdown_with_grace(Duration::from_millis(100))
            .await;
    }
}
