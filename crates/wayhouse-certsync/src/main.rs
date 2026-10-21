//! `wayhouse-certsync` binary: poll the controller for certificate bundles and keep a
//! directory of them current. See `lib.rs`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::Parser;
use tracing_subscriber::EnvFilter;

use wayhouse_certsync::client::Controller;
use wayhouse_certsync::sync::Syncer;

#[derive(Parser, Debug)]
#[command(
    name = "wayhouse-certsync",
    version = wayhouse_http::LONG_VERSION,
    about = "Keeps a directory of ACME certificate bundles in step with the controller (docs/acme.md)"
)]
struct Args {
    /// Base URL of a controller, or several separated by commas; they are tried in turn, so
    /// list every replica.
    #[arg(long, value_delimiter = ',', required = true)]
    controller: Vec<String>,

    /// File holding this gateway's token (`POST /acme/gateways` shows it once). It must not be
    /// readable by group or others.
    #[arg(long)]
    token_file: PathBuf,

    /// Directory the bundles go to: `<dir>/<site>/{fullchain.pem,privkey.pem,meta.json}`.
    #[arg(long)]
    dir: PathBuf,

    /// Time between passes: `60`, `60s`, `5m` or `2h`.
    #[arg(long, default_value = "60s", value_parser = wayhouse_certsync::parse_interval)]
    interval: Duration,

    /// Command to run (without a shell; split on spaces) after files changed, e.g.
    /// `caddy reload --config /etc/caddy/Caddyfile`. A failure is retried on the next pass.
    #[arg(long)]
    reload_cmd: Option<String>,

    /// Per-request timeout towards the controller, in seconds.
    #[arg(long, default_value_t = 15)]
    timeout_sec: u64,

    /// PEM file of extra CA certificates to trust for an https controller.
    #[arg(long)]
    ca_file: Option<PathBuf>,

    /// Allow a plain-http controller on a non-loopback address. The private keys travel in
    /// the response, so use this only on a network you trust completely.
    #[arg(long)]
    allow_insecure_secrets: bool,

    /// Run one pass and exit (non-zero when it failed), for cron or a systemd timer.
    #[arg(long)]
    once: bool,
}

/// The URLs, refusing plain http to a non-loopback host unless `allow_insecure`.
fn parse_urls(raw: &[String], allow_insecure: bool) -> Result<Vec<reqwest::Url>, String> {
    raw.iter()
        .map(|text| {
            let url = reqwest::Url::parse(text)
                .map_err(|e| format!("--controller {text:?} is not a URL: {e}"))?;
            if !url.username().is_empty() || url.password().is_some() {
                return Err("--controller must not carry a user name or password".into());
            }
            Ok(url)
        })
        .collect::<Result<Vec<_>, String>>()
        .and_then(|urls| {
            for url in &urls {
                check_transport(url, allow_insecure)?;
            }
            Ok(urls)
        })
}

fn check_transport(url: &reqwest::Url, allow_insecure: bool) -> Result<(), String> {
    match url.scheme() {
        "https" => Ok(()),
        "http" if allow_insecure || is_loopback(url) => Ok(()),
        "http" => Err(format!(
            "--controller {url} is plain http to a non-loopback host, which would send the \
             private keys in clear text: use https, or pass --allow-insecure-secrets on a network \
             you fully trust"
        )),
        other => Err(format!("--controller {url}: unsupported scheme {other}")),
    }
}

fn is_loopback(url: &reqwest::Url) -> bool {
    let host = url.host_str().unwrap_or("").trim_matches(['[', ']']);
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// The token in `path`; on unix the file must not be readable by group or others.
fn read_token(path: &Path) -> anyhow::Result<String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(path)
            .map_err(|e| anyhow::anyhow!("--token-file {}: {e}", path.display()))?
            .permissions()
            .mode();
        anyhow::ensure!(
            mode & 0o077 == 0,
            "--token-file {} is readable by group or others; chmod 600 it",
            path.display()
        );
    }
    let token = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("--token-file {}: {e}", path.display()))?;
    let token = token.trim();
    anyhow::ensure!(
        !token.is_empty(),
        "--token-file {} is empty",
        path.display()
    );
    Ok(token.to_string())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_env("WAYHOUSE_LOG").unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    let urls = parse_urls(&args.controller, args.allow_insecure_secrets)
        .map_err(|e| anyhow::anyhow!(e))?;
    let token = read_token(&args.token_file)?;
    if let Some(path) = &args.ca_file {
        let certs = wayhouse_http::init_ca_file(path)?;
        tracing::info!(certs, path = %path.display(), "trusting extra CAs from --ca-file");
    }
    let reload = args
        .reload_cmd
        .as_deref()
        .map(|c| c.split_whitespace().map(str::to_string).collect::<Vec<_>>())
        .filter(|c| !c.is_empty());
    std::fs::create_dir_all(&args.dir)
        .map_err(|e| anyhow::anyhow!("--dir {}: {e}", args.dir.display()))?;

    let controller = Controller::new(urls, token, Duration::from_secs(args.timeout_sec))?;
    let mut syncer = Syncer::new(controller, args.dir.clone(), reload);
    tracing::info!(dir = %args.dir.display(), "wayhouse-certsync started");

    if args.once {
        let report = syncer.run_once().await?;
        for (site, why) in &report.failed {
            tracing::warn!(%site, %why, "site not synced");
        }
        anyhow::ensure!(
            report.failed.is_empty(),
            "{} site(s) not synced",
            report.failed.len()
        );
        return Ok(());
    }

    let mut tick = tokio::time::interval(args.interval);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = tick.tick() => {}
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("shutting down; the files stay as they are");
                return Ok(());
            }
        }
        match syncer.run_once().await {
            Ok(report) => {
                for (site, why) in &report.failed {
                    tracing::warn!(%site, %why, "site not synced");
                }
            }
            Err(e) => tracing::warn!(error = %e, "sync failed; keeping the last good files"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn urls(list: &[&str], insecure: bool) -> Result<Vec<reqwest::Url>, String> {
        parse_urls(
            &list.iter().map(|s| (*s).to_string()).collect::<Vec<_>>(),
            insecure,
        )
    }

    #[test]
    fn plain_http_is_for_loopback_only_unless_allowed() {
        assert!(urls(&["https://ctl.example:9443"], false).is_ok());
        assert!(urls(
            &[
                "http://127.0.0.1:9443",
                "http://localhost:1",
                "http://[::1]:2"
            ],
            false
        )
        .is_ok());
        for bad in [
            "http://ctl.example:9443",
            "http://10.0.0.5:9443",
            "http://127.0.0.1.example.net",
            "ftp://x",
            "not a url",
            "https://user:pw@ctl.example",
        ] {
            assert!(urls(&[bad], false).is_err(), "{bad}");
        }
        assert!(urls(&["http://10.0.0.5:9443"], true).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn the_token_file_must_be_private_and_not_empty() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("token");
        std::fs::write(&path, "gwt_abc\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(read_token(&path)
            .unwrap_err()
            .to_string()
            .contains("chmod 600"));
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(read_token(&path).unwrap(), "gwt_abc");
        std::fs::write(&path, "  \n").unwrap();
        assert!(read_token(&path).unwrap_err().to_string().contains("empty"));
        assert!(read_token(&dir.path().join("missing")).is_err());
    }
}
