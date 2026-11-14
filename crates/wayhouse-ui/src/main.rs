//! `wayhouse-ui` binary: the subcommands that never start a server, then ui's
//! `serve::run` (shared with `wayhouse-control`, which runs it in-process).

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let argv: Vec<_> = std::env::args_os().collect();
    if wayhouse_enroll::cli::is_node_cmd(&argv) {
        return wayhouse_enroll::cli::run(wayhouse_enroll::Role::Ui, "wayhouse-ui", argv)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"));
    }
    wayhouse_ui::serve::run(argv).await
}
