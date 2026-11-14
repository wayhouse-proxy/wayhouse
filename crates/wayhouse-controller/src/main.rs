//! `wayhouse-controller` binary: the subcommands that never start a server, then controller's
//! `serve::run` (shared with `wayhouse-control`, which runs it in-process).

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // `wayhouse-controller init ...` generates a first controller's secrets and certificates and
    // exits; it never reads the settings layer or starts a server.
    if std::env::args_os().nth(1).is_some_and(|a| a == "init") {
        return wayhouse_controller::init::main(std::env::args_os().skip(2));
    }
    // `enroll` / `install --systemd`: join an existing HA group as a replica by one-time code.
    let argv: Vec<_> = std::env::args_os().collect();
    if wayhouse_enroll::cli::is_node_cmd(&argv) {
        return wayhouse_enroll::cli::run(
            wayhouse_enroll::Role::Controller,
            "wayhouse-controller",
            argv,
        )
        .await
        .map_err(|e| anyhow::anyhow!("{e}"));
    }
    wayhouse_controller::serve::run(argv).await
}
