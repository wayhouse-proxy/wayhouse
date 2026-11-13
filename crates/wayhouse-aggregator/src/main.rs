//! `wayhouse-aggregator` binary: the subcommands that never start a server, then aggregator's
//! `serve::run` (shared with `wayhouse-control`, which runs it in-process).

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let argv: Vec<_> = std::env::args_os().collect();
    if wayhouse_enroll::cli::is_node_cmd(&argv) {
        return wayhouse_enroll::cli::run(
            wayhouse_enroll::Role::Aggregator,
            "wayhouse-aggregator",
            argv,
        )
        .await
        .map_err(|e| anyhow::anyhow!("{e}"));
    }
    wayhouse_aggregator::serve::run(argv).await
}
