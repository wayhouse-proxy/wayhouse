//! `wayhouse-control`: see `lib.rs`. One process, three listeners; if any of the three stops, the
//! process exits non-zero so the supervisor (systemd, compose) restarts all of them together.

use tracing_subscriber::EnvFilter;
use wayhouse_control::{plan, Programs};

const USAGE: &str = "\
wayhouse-control: controller, aggregator and UI in one process (single node).

Shared flags: --settings-dir DIR  --tls-cert FILE  --tls-key FILE  --ca-file FILE
Everything else is namespaced: --controller-<flag>, --aggregator-<flag>, --ui-<flag> reach
<flag> of that program (see `wayhouse-controller --help` and so on). The UI is pointed at the
controller and aggregator automatically.";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let argv: Vec<_> = std::env::args_os().collect();
    if argv.iter().skip(1).any(|a| a == "--help" || a == "-h") {
        println!("{USAGE}");
        return Ok(());
    }
    if argv.iter().skip(1).any(|a| a == "--version" || a == "-V") {
        println!("wayhouse-control {}", wayhouse_http::LONG_VERSION);
        return Ok(());
    }
    let credentials = std::env::var_os("CREDENTIALS_DIRECTORY").map(std::path::PathBuf::from);
    let plan =
        plan(&argv, &Programs::linked(), credentials.as_deref()).map_err(anyhow::Error::msg)?;
    // The three programs would each read `$CREDENTIALS_DIRECTORY/auth-token` and
    // `WAYHOUSE_AUTH_TOKEN` for the same secret name, so those never apply here: per-program
    // credential files were mapped above, and an environment secret is refused instead of
    // handing one value to all three.
    if let Some((k, _)) = std::env::vars().find(|(k, _)| {
        k.starts_with("WAYHOUSE_")
            && ["TOKEN", "PASSWORD", "STEP_UP", "PROOF"]
                .iter()
                .any(|w| k.contains(w))
    }) {
        anyhow::bail!(
            "{k} is set, but its name is shared by all three programs; give the secret as \
             --<program>-<name>-file or a credential named <program>-<name>"
        );
    }
    std::env::remove_var("CREDENTIALS_DIRECTORY");
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_env("WAYHOUSE_LOG").unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .try_init();
    if let Some(path) = &plan.ca_file {
        let certs = wayhouse_http::init_ca_file(path)?;
        tracing::info!(certs, path = %path.display(), "trusting extra CAs from --ca-file");
    }
    // Controller first: the UI and aggregator retry their connections to it, so order is only
    // for the log. Each `run` serves until it fails.
    let (name, result) = tokio::select! {
        r = wayhouse_controller::serve::run(plan.controller) => ("controller", r),
        r = wayhouse_aggregator::serve::run(plan.aggregator) => ("aggregator", r),
        r = wayhouse_ui::serve::run(plan.ui) => ("ui", r),
    };
    match result {
        Ok(()) => anyhow::bail!("the {name} stopped"),
        Err(e) => Err(e.context(format!("the {name} stopped"))),
    }
}
