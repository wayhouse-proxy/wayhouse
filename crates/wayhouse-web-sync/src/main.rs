//! `wayhouse-web-sync` binary: poll the local `wayhouse` admin API, render the `web:`
//! section and load it into Caddy. See `lib.rs`.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use clap::Parser;
use tracing_subscriber::EnvFilter;

use wayhouse_web_sync::admin_addr::AdminAddr;
use wayhouse_web_sync::caddy::CaddyClient;
use wayhouse_web_sync::source::HttpSource;
use wayhouse_web_sync::sync::{Outcome, Syncer};

#[derive(Parser, Debug)]
#[command(
    name = "wayhouse-web-sync",
    version = wayhouse_http::LONG_VERSION,
    about = "Renders wayhouse's `web:` section into Caddy's configuration (docs/web-proxy.md)"
)]
struct Args {
    /// Base URL of the local `wayhouse` admin API.
    #[arg(long, default_value = "http://127.0.0.1:9900")]
    wayhouse_admin: String,

    /// Bearer token for `wayhouse`'s admin API (`settings.admin.auth_token`).
    #[arg(long, env = "WAYHOUSE_ADMIN_TOKEN", hide_env_values = true)]
    wayhouse_token: Option<String>,

    /// Caddy's admin API: a loopback `ip:port` or a unix socket (`/run/caddy/admin.sock`,
    /// or Caddy's `unix//run/caddy/admin.sock`). It is also written into the document as
    /// `admin.listen`, so Caddy's base configuration must use the same address.
    #[arg(long, default_value = "/run/caddy/admin.sock")]
    caddy_admin: String,

    /// Seconds between reads of `wayhouse` (pool changes and certificate renewals are
    /// picked up at this rate).
    #[arg(long, default_value_t = 5)]
    interval_sec: u64,

    /// Seconds between forced re-sends of an unchanged document, which restores a Caddy
    /// that restarted and lost its configuration. An identical load is a no-op.
    #[arg(long, default_value_t = 60)]
    resend_sec: u64,

    /// Per-request timeout towards `wayhouse` and Caddy.
    #[arg(long, default_value_t = 10)]
    timeout_sec: u64,

    /// Address of `GET /metrics`.
    #[arg(long, default_value = "127.0.0.1:9904")]
    metrics_listen: SocketAddr,

    /// Bearer token `GET /metrics` requires; mandatory for a non-loopback `--metrics-listen`.
    #[arg(long, env = "WAYHOUSE_WEB_SYNC_METRICS_TOKEN", hide_env_values = true)]
    metrics_token: Option<String>,

    /// PEM file of extra CA certificates to trust when `--wayhouse-admin` is https.
    #[arg(long)]
    ca_file: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_env("WAYHOUSE_LOG").unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    wayhouse_http::policy::check_optional_secret("--metrics-token", args.metrics_token.as_deref())
        .map_err(|e| anyhow::anyhow!(e))?;
    wayhouse_http::policy::check_exposure(
        "wayhouse-web-sync",
        args.metrics_listen,
        args.metrics_token.is_some(),
        false,
    )
    .map_err(|e| anyhow::anyhow!(e))?;
    if let Some(path) = &args.ca_file {
        let certs = wayhouse_http::init_ca_file(path)?;
        tracing::info!(certs, path = %path.display(), "trusting extra CAs from --ca-file");
    }
    let admin =
        AdminAddr::parse(&args.caddy_admin).map_err(|e| anyhow::anyhow!("--caddy-admin: {e}"))?;

    let prometheus =
        wayhouse_http::metrics::install("wayhouse-web-sync", env!("CARGO_PKG_VERSION"))?;
    let app = wayhouse_http::metrics::router(
        prometheus,
        wayhouse_http::server::BearerAuth::new(args.metrics_token.as_deref()),
    );
    let listener = tokio::net::TcpListener::bind(args.metrics_listen).await?;
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            tracing::error!(error = %e, "metrics server stopped");
        }
    });

    let timeout = Duration::from_secs(args.timeout_sec);
    let source = HttpSource::new(&args.wayhouse_admin, args.wayhouse_token, timeout)?;
    let caddy = CaddyClient::new(admin.clone(), timeout);
    let mut syncer = Syncer::new(source, caddy, admin);
    tracing::info!(
        wayhouse = %args.wayhouse_admin,
        caddy = %args.caddy_admin,
        "wayhouse-web-sync started; waiting for the first good web: section"
    );

    let resend = Duration::from_secs(args.resend_sec);
    let mut last_send = Instant::now();
    let mut tick = tokio::time::interval(Duration::from_secs(args.interval_sec.max(1)));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = tick.tick() => {}
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("shutting down; Caddy keeps its last configuration");
                return Ok(());
            }
        }
        let force = last_send.elapsed() >= resend;
        match syncer.run_once(force).await {
            Outcome::Loaded | Outcome::Unchanged if force => last_send = Instant::now(),
            _ => {}
        }
    }
}
