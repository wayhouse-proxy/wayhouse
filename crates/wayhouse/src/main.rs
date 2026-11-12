//! `wayhouse` — game-agnostic game server reverse proxy.
//!
//! Phase 1: load a YAML config, start the TCP listeners and the health
//! checker, serve the admin API (`/healthz`, `/readyz`, `/metrics`, `/pools`),
//! reload on SIGHUP / file change, and shut down cleanly on SIGINT/SIGTERM.

mod admin;
mod aggregator_client;
mod controller_client;
mod discovery;
#[cfg(feature = "dns-srv")]
mod dns_srv;
#[cfg(not(feature = "dns-srv"))]
#[path = "dns_srv_disabled.rs"]
mod dns_srv;
mod features;
#[cfg(feature = "grpc-resolver")]
mod grpc_resolver;
#[cfg(not(feature = "grpc-resolver"))]
#[path = "grpc_resolver_disabled.rs"]
mod grpc_resolver;
mod intent_client;
#[cfg(feature = "tunnel")]
mod live_interface;
#[cfg(feature = "tunnel")]
mod netlink_addr;
mod plugin_overlay;
mod procinfo;
#[cfg(feature = "tunnel")]
mod proxy_register;
mod reload;
mod resolver;
mod secret_store;
mod secrets;
#[cfg(feature = "wasm-sniffers")]
mod sniffer_loader;
#[cfg(not(feature = "wasm-sniffers"))]
#[path = "sniffer_loader_disabled.rs"]
mod sniffer_loader;
#[cfg(feature = "tunnel")]
mod tunnel_address;
#[cfg(feature = "tunnel")]
mod tunnel_boot;
#[cfg(not(feature = "tunnel"))]
#[path = "tunnel_boot_disabled.rs"]
mod tunnel_boot;
#[cfg(feature = "tunnel")]
mod tunnel_client;
#[cfg(feature = "tunnel")]
mod tunnel_source;
#[cfg(not(feature = "tunnel"))]
#[path = "tunnel_source_disabled.rs"]
mod tunnel_source;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use tracing_subscriber::EnvFilter;

use wayhouse_core::{Runtime, Snapshot};

#[derive(Parser, Debug)]
#[command(
    name = "wayhouse",
    version = wayhouse_http::LONG_VERSION,
    about = "Game-agnostic game server reverse proxy"
)]
struct Args {
    /// Directory with this program's settings file (`<program>.json`, written by the UI): its
    /// values fill in the flags this command line does not give. See `wayhouse-settings`.
    #[arg(long, value_name = "DIR")]
    settings_dir: Option<PathBuf>,

    /// Path to the YAML config file.
    #[arg(
        short,
        long,
        default_value = "config.yaml",
        conflicts_with = "controller"
    )]
    config: PathBuf,

    /// Fleet controller base URL (e.g. "http://127.0.0.1:9901") to pull
    /// structural config from instead of `--config`'s file (docs/10 "The
    /// controller"). Phase 10+11 PoC: one standalone controller, no HA or
    /// hierarchy yet — see docs/08-roadmap.md phase 10+11.
    #[arg(long)]
    controller: Option<String>,

    /// Bearer token to present to `--controller`, if it requires one (its
    /// own `--auth-token`).
    #[arg(long, value_name = "PATH")]
    controller_token_file: Option<std::path::PathBuf>,

    /// Resolved from `--controller-token-file`, `$CREDENTIALS_DIRECTORY/controller-token` or `$WAYHOUSE_CONTROLLER_TOKEN`;
    /// never a command-line value.
    #[arg(skip)]
    controller_token: Option<String>,

    /// Controller base URL to take plugin routes from while running from `--config`: the
    /// hostname and port routes its plugins declare are merged into listeners that opt in with
    /// `plugin_routes:` (docs/plugins.md "Routes"). With `--controller` this is already so.
    #[arg(long, value_name = "URL", conflicts_with = "controller")]
    plugin_overlay_url: Option<String>,

    /// Bearer token for `--plugin-overlay-url`, if the controller requires one.
    #[arg(long, value_name = "PATH", requires = "plugin_overlay_url")]
    plugin_overlay_token_file: Option<std::path::PathBuf>,

    /// Resolved from `--plugin-overlay-token-file`, `$CREDENTIALS_DIRECTORY/plugin-overlay-token` or `$WAYHOUSE_PLUGIN_OVERLAY_TOKEN`;
    /// never a command-line value.
    #[arg(skip)]
    plugin_overlay_token: Option<String>,

    /// Key file for `{ secret: name }` config values: one base64 32-byte key per line, the last
    /// one active (the same file format as the controller's `--plugin-secret-key-file`), or the
    /// `WAYHOUSE_PLUGIN_SECRET_KEY` environment variable. Needed only when the config uses a
    /// secret reference, which a proxy fetches (sealed) from `--controller`.
    #[arg(long, value_name = "FILE")]
    secret_key_file: Option<PathBuf>,

    /// The proxy's own sealed secret store, for `{ secret: name }` values on a file config
    /// (no controller). Created with `--secret-set`; sealed under `--secret-key-file`.
    #[arg(long, value_name = "FILE", conflicts_with = "controller")]
    secrets_file: Option<PathBuf>,

    /// Seal NAME into `--secrets-file` (the value is read from stdin, one trailing newline
    /// dropped) and exit.
    #[arg(long, value_name = "NAME", requires = "secrets_file", conflicts_with_all = ["secret_delete", "secret_list"])]
    secret_set: Option<String>,

    /// Remove NAME from `--secrets-file` and exit.
    #[arg(
        long,
        value_name = "NAME",
        requires = "secrets_file",
        conflicts_with = "secret_list"
    )]
    secret_delete: Option<String>,

    /// List the names in `--secrets-file` (never the values) and exit.
    #[arg(long, requires = "secrets_file")]
    secret_list: bool,

    /// Validate the config and exit without starting anything. Works with
    /// `--controller` too — fetches its current config and validates that.
    #[arg(long)]
    check: bool,

    /// Allow a non-loopback `settings.admin.listen` with no
    /// `settings.admin.auth_token`. Without it startup is refused. Only for
    /// deployments where the network boundary is the sole access control.
    #[arg(long)]
    insecure_no_auth: bool,

    /// Send bearer tokens to `--controller`, `--aggregator`, `--plugin-overlay-url` and
    /// `--tunnel-controller-url` over plain `http://` to a non-loopback host. Without it
    /// the start is refused. Only for a network that is private by construction.
    #[arg(long)]
    allow_insecure_controller: bool,

    /// Fleet aggregator base URL (e.g. "http://127.0.0.1:9902") to push
    /// periodic fleet-state summaries to (docs/10 "The aggregator").
    /// Optional and independent of `--controller` — pushing state and
    /// pulling config are unrelated axes.
    #[arg(long)]
    aggregator: Option<String>,

    /// Self-reported instance name in aggregator pushes. Defaults to this
    /// instance's `settings.admin.listen` address, which is already unique
    /// per running instance.
    #[arg(long)]
    aggregator_instance: Option<String>,

    /// Region this instance reports to the aggregator, for the fleet list's grouping and filters
    /// (letters, digits and `. _ - : / @ +`, at most 64).
    #[arg(long)]
    region: Option<String>,

    /// A `key=value` label this instance reports to the aggregator (repeatable, at most 16; keys
    /// are lowercase letters, digits, `. _ -`). The fleet list filters by them.
    #[arg(long = "tag", value_name = "KEY=VALUE")]
    tags: Vec<String>,

    /// Seconds between pushes to `--aggregator`.
    #[arg(long, default_value = "10")]
    aggregator_interval_sec: u64,

    /// Bearer token to present on every push to `--aggregator`, if it
    /// requires one (its own `--auth-token`).
    #[arg(long, value_name = "PATH")]
    aggregator_token_file: Option<std::path::PathBuf>,

    /// Resolved from `--aggregator-token-file`, `$CREDENTIALS_DIRECTORY/aggregator-token` or `$WAYHOUSE_AGGREGATOR_TOKEN`;
    /// never a command-line value.
    #[arg(skip)]
    aggregator_token: Option<String>,

    /// Base URL the aggregator should use to reach this instance's admin API
    /// for intent fan-out (drain, backend edits, route hints), e.g.
    /// `http://10.1.2.3:9900` or `https://wayhouse-1.example`. Defaults to
    /// `http(s)://<settings.admin.listen>`, which is unreachable from another
    /// host or container when the admin API binds `0.0.0.0` or loopback, or
    /// sits behind a NAT, port mapping or TLS terminator.
    #[arg(long, requires = "aggregator")]
    aggregator_admin_url: Option<String>,

    /// Enables phase 14's WireGuard backend transport (`docs/11`):
    /// brings up a local interface with this name and reconciles its peer
    /// list from `--tunnel-controller-url`'s backend-peers registry.
    /// Requires `--tunnel-controller-url`. Omit to
    /// leave this feature off entirely — today's exact behavior otherwise.
    #[arg(long)]
    tunnel_iface: Option<String>,

    /// UDP port the tunnel interface listens on.
    #[arg(long, default_value_t = 51820)]
    tunnel_listen_port: u16,

    /// Optional. This proxy's own tunnel-internal address as `ip/prefix`
    /// (e.g. `10.60.0.1/24`): pins that address with the controller (the
    /// prefix is used only when the controller reports no network). Omit to
    /// let the controller allocate one.
    #[arg(long)]
    tunnel_address: Option<String>,

    /// File holding this proxy's persisted WireGuard private key (created
    /// if missing).
    #[arg(long, default_value = "wayhouse-tunnel.key")]
    tunnel_key_file: PathBuf,

    /// File where the controller's feature-flag overrides are kept, so a restart while the
    /// controller is down keeps a kill switch as it was (used with `--controller`).
    #[arg(long, default_value = "wayhouse-feature-overrides.json")]
    feature_overrides_file: PathBuf,

    /// Base URL of the `wayhouse-controller` backend-peers registry to subscribe
    /// to — independent of `--controller` (a deployment may pull structural
    /// config from a file while still using a controller's tunnel registry,
    /// or vice versa). Required with `--tunnel-iface`.
    #[arg(long)]
    tunnel_controller_url: Option<String>,

    /// Bearer token for `--tunnel-controller-url`, if it requires one.
    #[arg(long, value_name = "PATH")]
    tunnel_controller_token_file: Option<std::path::PathBuf>,

    /// Resolved from `--tunnel-controller-token-file`, `$CREDENTIALS_DIRECTORY/tunnel-controller-token` or `$WAYHOUSE_TUNNEL_CONTROLLER_TOKEN`;
    /// never a command-line value.
    #[arg(skip)]
    tunnel_controller_token: Option<String>,

    /// Skip the kernel WireGuard backend and use boringtun userspace
    /// directly — see `wayhouse-agent --userspace`'s doc for why.
    #[arg(long)]
    tunnel_userspace: bool,

    /// This proxy's stable identity in `--tunnel-controller-url`'s
    /// proxy-peers registry (phase 14 slice 7) — arbitrary, just needs to
    /// be unique fleet-wide. Required with `--tunnel-iface`.
    #[arg(long)]
    tunnel_name: Option<String>,

    /// This proxy's public dial-out address to register (`ip:port`) — every
    /// origin's `wayhouse-agent` learns it from here and peers with it. Required
    /// (not optional like an origin's `--endpoint`): `docs/11`'s whole
    /// premise is that only the proxy side needs a stable public address.
    #[arg(long)]
    tunnel_endpoint: Option<String>,

    /// How often to re-register with `--tunnel-controller-url`'s
    /// proxy-peers registry.
    #[arg(long, default_value_t = 30)]
    tunnel_register_interval_sec: u64,

    /// PEM file of extra CA certificates to trust for outbound HTTPS, in
    /// addition to the built-in Mozilla roots.
    #[arg(long)]
    ca_file: Option<PathBuf>,
}

/// Where this process's config comes from, decided once at startup from
/// `Args`. `reload`/`controller_client` each own the live-update side of one
/// variant; nothing else branches on this after `run` dispatches on it once.
enum ConfigSource {
    File(PathBuf),
    Controller(String, Option<String>),
}

/// `--secret-set` / `--secret-delete` / `--secret-list`: one-shot management of `--secrets-file`.
/// Returns true when one ran (the proxy then exits without starting).
fn manage_secrets(args: &Args) -> anyhow::Result<bool> {
    let Some(path) = args.secrets_file.as_deref() else {
        return Ok(false);
    };
    if let Some(name) = &args.secret_set {
        let Some(ring) = secrets::load_keyring(args.secret_key_file.as_deref())? else {
            anyhow::bail!("--secret-set needs a key: give --secret-key-file or set WAYHOUSE_PLUGIN_SECRET_KEY");
        };
        let mut value = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut value)?;
        let value = value.strip_suffix('\n').unwrap_or(&value);
        secret_store::set(path, &ring, name, value.as_bytes())?;
        eprintln!("sealed the secret into {}", path.display());
    } else if let Some(name) = &args.secret_delete {
        let gone = secret_store::remove(path, name)?;
        // No name in the message: the caller just typed it, and a name is not worth a log line.
        eprintln!(
            "{}",
            if gone {
                "removed the secret"
            } else {
                "no such secret in the store"
            }
        );
    } else if args.secret_list {
        for name in secret_store::read(path)?.keys() {
            println!("{name}");
        }
    } else {
        return Ok(false);
    }
    Ok(true)
}

fn main() -> anyhow::Result<()> {
    let argv: Vec<_> = std::env::args_os().collect();
    if wayhouse_enroll::cli::is_node_cmd(&argv) {
        return tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(wayhouse_enroll::cli::run(
                wayhouse_enroll::Role::Proxy,
                "wayhouse",
                argv,
            ))
            .map_err(|e| anyhow::anyhow!("{e}"));
    }
    let layered = wayhouse_settings::layer(
        "wayhouse",
        &<Args as clap::CommandFactory>::command(),
        std::env::args_os().collect(),
    );
    let mut args = Args::parse_from(&layered.argv);
    args.controller_token = wayhouse_http::secret_arg::resolve(
        "controller-token",
        args.controller_token_file.as_deref(),
    )
    .map_err(anyhow::Error::msg)?;
    args.plugin_overlay_token = wayhouse_http::secret_arg::resolve(
        "plugin-overlay-token",
        args.plugin_overlay_token_file.as_deref(),
    )
    .map_err(anyhow::Error::msg)?;
    args.aggregator_token = wayhouse_http::secret_arg::resolve(
        "aggregator-token",
        args.aggregator_token_file.as_deref(),
    )
    .map_err(anyhow::Error::msg)?;
    args.tunnel_controller_token = wayhouse_http::secret_arg::resolve(
        "tunnel-controller-token",
        args.tunnel_controller_token_file.as_deref(),
    )
    .map_err(anyhow::Error::msg)?;
    if manage_secrets(&args)? {
        return Ok(());
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_env("WAYHOUSE_LOG").unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    if let Some(path) = &args.ca_file {
        let certs = wayhouse_http::init_ca_file(path)?;
        tracing::info!(certs, path = %path.display(), "trusting extra CAs from --ca-file");
    }

    // Fetching from a controller needs an async runtime, so config loading
    // itself now happens inside `block_on` rather than before it (file mode
    // is unaffected — `wayhouse_config::load` is still a plain sync read).
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async {
            layered.start();
            async_main(args).await
        })
}

async fn async_main(mut args: Args) -> anyhow::Result<()> {
    for (flag, url, token) in [
        ("--controller", &args.controller, &args.controller_token),
        (
            "--plugin-overlay-url",
            &args.plugin_overlay_url,
            &args.plugin_overlay_token,
        ),
        ("--aggregator", &args.aggregator, &args.aggregator_token),
        (
            "--tunnel-controller-url",
            &args.tunnel_controller_url,
            &args.tunnel_controller_token,
        ),
    ] {
        if let Some(url) = url {
            wayhouse_http::bearer::check_bearer_transport(
                flag,
                url,
                token.is_some(),
                args.allow_insecure_controller,
            )
            .map_err(anyhow::Error::msg)?;
        }
    }
    wayhouse_http::bearer::set_allow_insecure_controller(args.allow_insecure_controller);
    let tunnel_config = tunnel_boot::config(&args)?;
    if let Some(raw) = &args.plugin_overlay_url {
        let url = checked_http_url("--plugin-overlay-url", raw)?;
        wayhouse_http::policy::warn_plaintext_source("--plugin-overlay-url", &url);
        args.plugin_overlay_url = Some(url);
    }

    let (config_source, cfg, initial_revision) = match &args.controller {
        Some(url) => {
            let (revision, text) =
                controller_client::fetch_current(url, args.controller_token.as_deref()).await?;
            let cfg = wayhouse_config::parse_str(&plugin_overlay::merge(&text))
                .map_err(|e| anyhow::anyhow!("controller {url} revision {revision}: {e}"))?;
            (
                ConfigSource::Controller(url.clone(), args.controller_token.clone()),
                cfg,
                Some(revision),
            )
        }
        None => {
            let cfg = plugin_overlay::load(&args.config)?;
            (ConfigSource::File(args.config.clone()), cfg, None)
        }
    };
    tracing::info!(
        source = match &config_source {
            ConfigSource::File(p) => p.display().to_string(),
            ConfigSource::Controller(u, _) => u.clone(),
        },
        listeners = cfg.listeners.len(),
        pools = cfg.pools.len(),
        "configuration loaded"
    );

    for w in cfg.warnings() {
        tracing::warn!("{w}");
        if args.check {
            eprintln!("warning: {w}");
        }
    }

    // `{ secret: name }` becomes its value here, once, so nothing below sees a reference. A
    // reference that cannot be resolved stops the start with a message naming the secret rather
    // than running with an empty token. A file-config proxy reads its own `--secrets-file`.
    let cfg = secrets::resolve_config(
        cfg,
        match (&args.controller, &args.secrets_file) {
            (Some(url), _) => secrets::Store::Controller(url, args.controller_token.as_deref()),
            (None, Some(path)) => secrets::Store::File(path),
            (None, None) => secrets::Store::None,
        },
        args.secret_key_file.as_deref(),
    )
    .await?;
    check_admin_auth(&cfg, args.insecure_no_auth)?;
    check_gossip_feature(&cfg)?;

    // A configured GeoIP DB must load, both for `--check` and at startup.
    let geo_db = match &cfg.geo_db {
        Some(path) => Some(
            wayhouse_core::GeoDb::open(path)
                .map_err(|e| anyhow::anyhow!("settings.geo_db {path:?}: {e}"))?,
        ),
        None => None,
    };

    // A configured sniffer dir must load cleanly too (phase 9). The
    // loader (its wasmtime engine + epoch-ticker thread) is kept alive and
    // handed to the reload task so a later `dir` rescan reuses it instead of
    // spawning a fresh ticker thread per reload.
    let (sniffer_loader, sniffers) = match &cfg.sniffers {
        Some(sc) => {
            let (loader, registry) = sniffer_loader::build_sniffers(sc)
                .map_err(|e| anyhow::anyhow!("settings.sniffers: {e:#}"))?;
            (Some(Arc::new(loader)), Arc::new(registry))
        }
        None => (None, Arc::new(wayhouse_core::sniff::Sniffers::default())),
    };

    // A configured admin certificate must load too, for `--check` and at startup.
    let admin_tls = match &cfg.admin_tls {
        Some(t) => Some(wayhouse_http::tls::ReloadingCert::new(
            wayhouse_http::tls::TlsFiles::new(t.cert.clone().into(), t.key.clone().into())
                .named("settings.admin.tls.cert", "settings.admin.tls.key"),
        )?),
        None => None,
    };

    let admin_url = aggregator_admin_url(
        args.aggregator_admin_url.as_deref(),
        admin_tls.is_some(),
        cfg.admin_listen,
    )?;

    // The resolver clients and discovery adapters are built before `--check`
    // returns: constructing them is what refuses a setting this build cannot
    // run (a missing cargo feature, a bad endpoint URL, a malformed source),
    // and `--check` is the pre-deploy gate (issue #127). Building does not fetch.
    // Phase 14 slice 5 (docs/11): a `tunnel` backend_sources entry resolves
    // an origin's currently-registered backends from the same backend-peers
    // registry the tunnel reconcile task (below) subscribes to — reuses
    // `--tunnel-controller-url`/`--tunnel-controller-token` rather than a
    // second pair of flags, since it's the identical registry. Borrowed
    // (not moved) so `run` can still consume `tunnel_config` by value.
    let tunnel_registry = tunnel_config.as_ref().map(tunnel_boot::registry);

    let resolvers = Arc::new(wayhouse_core::Resolvers::from_map(
        resolver::build_resolvers(&cfg)?,
    ));
    let sources = discovery::build_sources(&cfg, tunnel_registry.as_ref())?;

    if args.check {
        println!(
            "config OK: {} listener(s), {} pool(s){}{}{}",
            cfg.listeners.len(),
            cfg.pools.len(),
            if admin_tls.is_some() {
                ", admin TLS loaded"
            } else {
                ""
            },
            if geo_db.is_some() {
                ", geo_db loaded"
            } else {
                ""
            },
            match &cfg.sniffers {
                Some(sc) => format!(", sniffers loaded from {}", sc.dir),
                None => String::new(),
            },
        );
        return Ok(());
    }

    let labels = aggregator_client::Labels::from_args(args.region, &args.tags)
        .map_err(|e| anyhow::anyhow!(e))?;
    let aggregator_push = args
        .aggregator
        .map(|base_url| aggregator_client::PushConfig {
            base_url,
            instance: args
                .aggregator_instance
                .unwrap_or_else(|| cfg.admin_listen.to_string()),
            admin_url,
            interval: Duration::from_secs(args.aggregator_interval_sec),
            token: args.aggregator_token,
            labels,
        });

    run(
        cfg,
        config_source,
        initial_revision,
        geo_db,
        sniffer_loader,
        sniffers,
        aggregator_push,
        tunnel_config,
        tunnel_registry,
        resolvers,
        sources,
        admin_tls,
        args.feature_overrides_file,
        args.plugin_overlay_url.zip(Some(args.plugin_overlay_token)),
    )
    .await
}

#[allow(clippy::too_many_arguments)] // mirrors controller_client::run's shape; a struct doesn't earn its keep for one call site
async fn run(
    cfg: wayhouse_config::Config,
    config_source: ConfigSource,
    initial_revision: Option<u64>,
    geo_db: Option<Arc<wayhouse_core::GeoDb>>,
    sniffer_loader: Option<Arc<sniffer_loader::SnifferLoader>>,
    sniffers: Arc<wayhouse_core::sniff::Sniffers>,
    aggregator_push: Option<aggregator_client::PushConfig>,
    tunnel_config: Option<tunnel_boot::TunnelConfig>,
    tunnel_registry: Option<tunnel_source::TunnelRegistry>,
    resolvers: Arc<wayhouse_core::Resolvers>,
    sources: Vec<Arc<dyn wayhouse_core::BackendSource>>,
    admin_tls: Option<Arc<wayhouse_http::tls::ReloadingCert>>,
    feature_overrides_file: PathBuf,
    file_mode_overlay: Option<(String, Option<String>)>,
) -> anyhow::Result<()> {
    let prometheus = metrics_exporter_prometheus::PrometheusBuilder::new().install_recorder()?;

    if !resolvers.is_empty() {
        tracing::info!(count = resolvers.len(), "external resolvers ready");
    }

    // Backend discovery (phase 8): build one source per pool with a `source`,
    // do a best-effort initial fetch so the first snapshot has real backends,
    // then let the runtime run a refresh task per source.
    let discovery = Arc::new(wayhouse_core::Discovery::new());
    if !sources.is_empty() {
        tracing::info!(count = sources.len(), "backend discovery sources ready");
        for s in &sources {
            match tokio::time::timeout(Duration::from_secs(5), s.fetch()).await {
                Ok(Ok(addrs)) if !addrs.is_empty() => {
                    discovery.store(s.pool(), addrs);
                }
                Ok(Ok(_)) => tracing::warn!(
                    pool = s.pool(),
                    "initial discovery returned no addresses; starting from the seed"
                ),
                Ok(Err(e)) => tracing::warn!(
                    pool = s.pool(), error = %e,
                    "initial discovery failed; starting from the seed"
                ),
                Err(_) => tracing::warn!(
                    pool = s.pool(),
                    "initial discovery timed out; starting from the seed"
                ),
            }
        }
    }

    // After the best-effort initial fetch, hand a factory (not the prebuilt
    // sources) to the runtime: its `SourceManager` spawns one refresh task per
    // pool `source` and reconciles them on every reload.
    let source_factory: Option<Arc<dyn wayhouse_core::SourceFactory>> =
        cfg.pools.iter().any(|p| p.source.is_some()).then(|| {
            let f: Arc<dyn wayhouse_core::SourceFactory> =
                Arc::new(discovery::DiscoveryFactory::new(tunnel_registry.clone()));
            f
        });

    // Phase 14 slice 4 (docs/11): bring up the shared WireGuard interface
    // and start reconciling its peer list from the backend-peers registry,
    // before any listener binds — a failure here is loud, not silent, same
    // posture as `geo_db`/`sniffers` loading earlier: a misconfigured
    // `--tunnel-*` flag set shouldn't start a proxy that silently never
    // forwards tunneled traffic, and shouldn't leave listeners bound behind
    // a startup error either.
    let tunnel = match tunnel_config {
        Some(tc) => Some(tunnel_boot::start(tc).await?),
        None => None,
    };

    let snapshot: Arc<Snapshot> = Snapshot::build_with_sources(
        &cfg,
        None,
        &wayhouse_core::BackendOverlay::new(),
        &discovery,
    );
    let runtime = Runtime::start_with_discovery(
        snapshot,
        resolvers.clone(),
        geo_db,
        sniffers.clone(),
        discovery,
        source_factory,
        cfg.gossip.clone(),
        cfg.workers,
    )
    .map_err(|e| anyhow::anyhow!("cannot start listeners: {e}"))?;
    let handle = runtime.handle();
    metrics::gauge!(wayhouse_core::metrics_defs::CONFIG_VERSION).set(reload::unix_now());
    features::publish(&cfg.features, &handle);
    // Build identity + fd headroom (docs/06 "Planned / not yet built", now
    // built): version/commit are fixed for the process lifetime, so
    // `wayhouse_build_info` and `wayhouse_fd_limit` are set once; `wayhouse_fd_open` needs a
    // live sample, hence the periodic task.
    wayhouse_http::metrics::set_build_info("wayhouse", env!("CARGO_PKG_VERSION"));
    if let Some(limit) = procinfo::fd_limit() {
        metrics::gauge!(wayhouse_core::metrics_defs::FD_LIMIT).set(limit as f64);
    }
    let fd_gauge = procinfo::spawn_fd_gauge(Duration::from_secs(5));

    let (sniffer_pins, sniffer_validator): (admin::SnifferPins, Option<admin::SnifferValidator>) =
        match (&sniffer_loader, &cfg.sniffers) {
            (Some(loader), Some(sc)) => {
                let (pins_loader, check_loader) = (loader.clone(), loader.clone());
                let max_memory = sc.max_memory_bytes;
                (
                    Arc::new(move || pins_loader.pins()),
                    Some(Arc::new(move |bytes: &[u8]| {
                        check_loader
                            .validate(bytes, max_memory)
                            .map_err(|e| e.to_string())
                    })),
                )
            }
            _ => (Arc::new(Vec::new), None),
        };
    let admin = tokio::spawn(admin::serve(
        cfg.admin_listen,
        admin_tls.map(|cert| (cert, admin::handshake_limits(cfg.admin_tls.as_ref()))),
        handle.clone(),
        prometheus.clone(),
        admin_token(&cfg)?,
        cfg.sniffers.as_ref().map(|sc| PathBuf::from(&sc.dir)),
        sniffers.clone(),
        sniffer_pins,
        sniffer_validator,
    ));
    let aggregator = aggregator_push.map(|push_cfg| {
        tracing::info!(
            aggregator = %push_cfg.base_url,
            instance = %push_cfg.instance,
            interval_sec = push_cfg.interval.as_secs(),
            "pushing fleet-state summaries to aggregator"
        );
        tokio::spawn(aggregator_client::run(
            push_cfg,
            handle.clone(),
            prometheus.clone(),
        ))
    });
    // The intent log (phase 12 slice 3) lives on the same controller
    // instance as structural config, so it only makes sense to subscribe
    // alongside `--controller` — an "unrelated axis" the other way from
    // `aggregator_client` (that one is independent of `--controller`; this
    // one is a second stream off the *same* connection target).
    let intent = if let ConfigSource::Controller(url, token) = &config_source {
        tracing::info!(controller = %url, "subscribing to controller intent updates");
        Some(tokio::spawn(intent_client::run(
            url.clone(),
            token.clone(),
            handle.clone(),
        )))
    } else {
        None
    };

    // See `controller_client::watch_admin_reloads`'s doc: in `--controller`
    // mode, nothing otherwise rebuilds the snapshot after an admin-API or
    // intent-op backend overlay change (`reload::run`, which normally does,
    // only runs in file-config mode).
    let overlay_source = match &config_source {
        ConfigSource::Controller(url, token) => Some((url.clone(), token.clone())),
        ConfigSource::File(_) => file_mode_overlay,
    };
    let plugin_overlay = overlay_source
        .map(|(url, token)| tokio::spawn(plugin_overlay::run(url, token, handle.clone())));
    let feature_overrides = if let ConfigSource::Controller(url, token) = &config_source {
        Some(tokio::spawn(features::run(
            url.clone(),
            token.clone(),
            handle.clone(),
            feature_overrides_file.clone(),
        )))
    } else {
        None
    };
    let admin_reload_watch = if let ConfigSource::Controller(url, token) = &config_source {
        Some(tokio::spawn(controller_client::watch_admin_reloads(
            url.clone(),
            token.clone(),
            handle.clone(),
            resolvers.clone(),
            sniffer_loader.clone(),
            sniffers.clone(),
        )))
    } else {
        None
    };

    let reload = match config_source {
        ConfigSource::File(path) => tokio::spawn(reload::run(
            path,
            handle,
            resolvers,
            sniffer_loader,
            sniffers,
        )),
        ConfigSource::Controller(url, token) => tokio::spawn(controller_client::run(
            url,
            token,
            {
                // The first revision is already live (it built the first snapshot).
                if let Some(r) = initial_revision {
                    controller_client::note_applied(r);
                }
                initial_revision.unwrap_or(0)
            },
            handle,
            resolvers,
            sniffer_loader,
            sniffers,
        )),
    };

    wait_for_shutdown().await;
    tracing::info!(
        grace_sec = cfg.shutdown_grace.as_secs(),
        "shutdown signal received; draining in-flight connections"
    );

    runtime.shutdown_with_grace(cfg.shutdown_grace).await;
    reload.abort();
    if let Some(intent) = intent {
        intent.abort();
    }
    if let Some(feature_overrides) = feature_overrides {
        feature_overrides.abort();
    }
    if let Some(plugin_overlay) = plugin_overlay {
        plugin_overlay.abort();
    }
    if let Some(admin_reload_watch) = admin_reload_watch {
        admin_reload_watch.abort();
    }
    admin.abort();
    if let Some(aggregator) = aggregator {
        aggregator.abort();
    }
    fd_gauge.abort();
    if let Some(tunnel) = tunnel {
        tunnel.stop();
    }
    tracing::info!("stopped");
    Ok(())
}

/// The admin token as plaintext. A reference that was not resolved is an error, never `None`:
/// `None` means an open API.
fn admin_token(cfg: &wayhouse_config::Config) -> anyhow::Result<Option<String>> {
    cfg.admin_auth_token
        .as_ref()
        .map(|t| t.resolve(None))
        .transpose()
        .map_err(|e| anyhow::anyhow!("{e}"))
}

/// Refuse an open admin API on a routable address and weak admin/gossip
/// secrets.
fn check_admin_auth(cfg: &wayhouse_config::Config, insecure_no_auth: bool) -> anyhow::Result<()> {
    let token = admin_token(cfg)?;
    let token = token.as_deref();
    // The same rule the controller applies to a submission; references are resolved by now.
    cfg.check_secret_strength()
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    wayhouse_http::policy::check_exposure(
        "wayhouse admin API",
        cfg.admin_listen,
        token.is_some(),
        insecure_no_auth,
    )
    .map_err(|e| anyhow::anyhow!(e))
}

/// `settings.gossip` needs the `gossip` cargo feature; a build without it must
/// refuse the config, not silently run without the regional health fabric.
#[cfg_attr(feature = "gossip", allow(clippy::unnecessary_wraps))] // the refusal only exists without the feature
fn check_gossip_feature(cfg: &wayhouse_config::Config) -> anyhow::Result<()> {
    #[cfg(not(feature = "gossip"))]
    if cfg.gossip.is_some() {
        anyhow::bail!(
            "settings.gossip: this build of wayhouse was compiled without the `gossip` cargo \
             feature, so it cannot join the regional health fabric; remove \
             `settings.gossip` or use a full build"
        );
    }
    #[cfg(feature = "gossip")]
    let _ = cfg;
    Ok(())
}

/// The admin URL to report in aggregator pushes: `--aggregator-admin-url` if
/// given (an `http`/`https` base URL, optionally with a path prefix; trailing
/// slashes are trimmed because the fan-out appends absolute paths), else this
/// instance's own admin listener.
fn aggregator_admin_url(
    override_url: Option<&str>,
    admin_tls: bool,
    admin_listen: impl std::fmt::Display,
) -> anyhow::Result<String> {
    let Some(raw) = override_url else {
        let scheme = if admin_tls { "https" } else { "http" };
        return Ok(format!("{scheme}://{admin_listen}"));
    };
    checked_http_url("--aggregator-admin-url", raw)
}

/// `raw` as a base URL: http or https, a host, no query or fragment, no trailing slash.
fn checked_http_url(flag: &str, raw: &str) -> anyhow::Result<String> {
    let bad = |why: &str| anyhow::anyhow!("{flag} {raw:?}: {why}");
    let url = reqwest::Url::parse(raw).map_err(|e| bad(&e.to_string()))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(bad("the scheme must be http or https"));
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err(bad("missing host"));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(bad("must not have a query or fragment"));
    }
    Ok(raw.trim_end_matches('/').to_string())
}

#[cfg(unix)]
async fn wait_for_shutdown() {
    use tokio::signal::unix::{signal, SignalKind};
    let mut term = signal(SignalKind::terminate()).expect("install SIGTERM handler");
    let mut int = signal(SignalKind::interrupt()).expect("install SIGINT handler");
    tokio::select! {
        _ = term.recv() => {}
        _ = int.recv() => {}
    }
}

#[cfg(not(unix))]
async fn wait_for_shutdown() {
    let _ = tokio::signal::ctrl_c().await;
}

#[cfg(test)]
mod tests {
    use super::{admin_token, aggregator_admin_url, check_admin_auth, check_gossip_feature};

    fn cfg(settings: &str) -> wayhouse_config::Config {
        wayhouse_config::parse_str(&format!(
            "settings:\n{settings}pools:\n  - name: p\n    targets: [\"127.0.0.1:1\"]\n\
             listeners:\n  - name: l\n    bind: \"0.0.0.0:7777\"\n    pool: p\n"
        ))
        .unwrap()
    }

    #[test]
    fn a_secret_reference_never_turns_into_an_open_admin_api() {
        let sealed = wayhouse_config::parse_str(
            "schema_version: 4\nsettings:\n  admin:\n    listen: \"127.0.0.1:9900\"\n    auth_token: { secret: admin-token }\n\
             pools:\n  - name: p\n    targets: [\"127.0.0.1:1\"]\nlisteners:\n  - name: l\n    bind: \"0.0.0.0:7777\"\n    pool: p\n",
        )
        .unwrap();
        // Unresolved: the token is an error, not `None` (which would mean no auth).
        let e = admin_token(&sealed).unwrap_err().to_string();
        assert!(
            e.contains("`admin-token`") && e.contains("no secret store"),
            "{e}"
        );
        assert!(check_admin_auth(&sealed, true).is_err());
        // Resolving without a store stops the start with the same message.
        let e = sealed.resolve_secrets(None).unwrap_err().to_string();
        assert!(e.contains("`admin-token`"), "{e}");
    }

    #[test]
    fn open_admin_on_loopback_is_fine_elsewhere_refused() {
        assert!(check_admin_auth(&cfg(""), false).is_ok());
        let public = cfg("  admin:\n    listen: \"0.0.0.0:9900\"\n");
        let e = check_admin_auth(&public, false).unwrap_err().to_string();
        assert!(e.contains("--insecure-no-auth"), "{e}");
        assert!(check_admin_auth(&public, true).is_ok());
    }

    #[test]
    fn admin_token_makes_a_public_bind_ok_unless_short() {
        let ok =
            cfg("  admin:\n    listen: \"0.0.0.0:9900\"\n    auth_token: \"0123456789abcdef\"\n");
        assert!(check_admin_auth(&ok, false).is_ok());
        let short = cfg("  admin:\n    listen: \"0.0.0.0:9900\"\n    auth_token: \"short\"\n");
        let e = check_admin_auth(&short, true).unwrap_err().to_string();
        assert!(e.contains("settings.admin.auth_token"), "{e}");
    }

    #[test]
    fn config_and_http_agree_on_the_minimum_secret_length() {
        assert_eq!(
            wayhouse_config::MIN_SECRET_LEN,
            wayhouse_http::policy::MIN_SECRET_LEN
        );
    }

    #[test]
    fn short_gossip_psk_is_refused() {
        let gossip = |psk: &str| {
            cfg(&format!(
                "  failure_domain: \"r1\"\n  gossip:\n    bind: \"127.0.0.1:7946\"\n    psk: \"{psk}\"\n"
            ))
        };
        let e = check_admin_auth(&gossip("short"), false)
            .unwrap_err()
            .to_string();
        assert!(e.contains("settings.gossip.psk"), "{e}");
        assert!(check_admin_auth(&gossip("0123456789abcdef"), false).is_ok());
    }

    #[cfg(not(feature = "gossip"))]
    #[test]
    fn a_gossip_section_is_refused_with_a_message_naming_the_feature() {
        let with = cfg(
            "  failure_domain: \"r1\"\n  gossip:\n    bind: \"127.0.0.1:7946\"\n    psk: \"0123456789abcdef\"\n",
        );
        let e = check_gossip_feature(&with).unwrap_err().to_string();
        assert!(e.contains("`gossip` cargo feature"), "{e}");
        assert!(check_gossip_feature(&cfg("")).is_ok());
    }

    #[cfg(feature = "gossip")]
    #[test]
    fn a_gossip_section_is_accepted_in_a_full_build() {
        let with = cfg(
            "  failure_domain: \"r1\"\n  gossip:\n    bind: \"127.0.0.1:7946\"\n    psk: \"0123456789abcdef\"\n",
        );
        assert!(check_gossip_feature(&with).is_ok());
    }

    #[test]
    fn defaults_to_the_admin_listen_address() {
        let listen = "127.0.0.1:9900";
        assert_eq!(
            aggregator_admin_url(None, false, listen).unwrap(),
            "http://127.0.0.1:9900"
        );
        assert_eq!(
            aggregator_admin_url(None, true, listen).unwrap(),
            "https://127.0.0.1:9900"
        );
    }

    #[test]
    fn an_override_replaces_it_whatever_the_admin_tls() {
        for tls in [false, true] {
            assert_eq!(
                aggregator_admin_url(Some("http://10.1.2.3:9900"), tls, "0.0.0.0:9900").unwrap(),
                "http://10.1.2.3:9900"
            );
        }
    }

    #[test]
    fn an_override_keeps_a_path_prefix_and_loses_trailing_slashes() {
        assert_eq!(
            aggregator_admin_url(
                Some("https://edge.example/wayhouse-1//"),
                false,
                "0.0.0.0:9900"
            )
            .unwrap(),
            "https://edge.example/wayhouse-1"
        );
        assert_eq!(
            aggregator_admin_url(Some("https://edge.example/"), false, "0.0.0.0:9900").unwrap(),
            "https://edge.example"
        );
    }

    #[test]
    fn a_bad_override_is_an_error_naming_the_flag() {
        for bad in [
            "10.1.2.3:9900",
            "ftp://10.1.2.3",
            "http://",
            "http://host/?x=1",
            "http://host/#frag",
            "not a url",
        ] {
            let err = aggregator_admin_url(Some(bad), false, "0.0.0.0:9900")
                .expect_err(bad)
                .to_string();
            assert!(err.contains("--aggregator-admin-url"), "{bad}: {err}");
        }
    }
}
