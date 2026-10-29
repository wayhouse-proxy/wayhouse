//! Region and tags travel from `wayhouse --region/--tag` through the aggregator's push endpoint
//! into the paged `GET /fleet/nodes` list. Plain `cargo test`.

use std::time::Duration;

use anyhow::{ensure, Result};
use serde_json::Value;
use wayhouse_fleet_tests::{
    build_fleet_bins, free_port, minimal_wayhouse_config, spawn_aggregator, spawn_wayhouse,
    wait_http_up, wait_until, Proc, WayhouseArgs,
};

const UP: Duration = Duration::from_secs(30);

fn proxy(agg: &str, name: &str, dir: &std::path::Path, labels: &[&str]) -> Result<Proc> {
    let path = dir.join(format!("{name}.yaml"));
    std::fs::write(
        &path,
        minimal_wayhouse_config(free_port()?, free_port()?, free_port()?),
    )?;
    spawn_wayhouse(WayhouseArgs {
        config_path: Some(path),
        aggregator_url: Some(agg.into()),
        aggregator_instance: Some(name.into()),
        extra: labels.iter().map(ToString::to_string).collect(),
        ..Default::default()
    })
}

async fn nodes(base: &str, query: &str) -> Result<Value> {
    Ok(wayhouse_http::client()
        .get(format!("{base}/fleet/nodes?{query}"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}

#[tokio::test]
async fn region_and_tags_reach_the_node_list_and_filter_it() -> Result<()> {
    build_fleet_bins()?;
    let dir = tempfile::tempdir()?;
    let port = free_port()?;
    let base = format!("http://127.0.0.1:{port}");
    let _agg = spawn_aggregator(port)?;
    wait_http_up(&format!("{base}/healthz"), UP).await?;
    let _a = proxy(
        &base,
        "fra-1",
        dir.path(),
        &["--region", "eu", "--tag", "env=prod", "--tag", "gpu=a100"],
    )?;
    let _b = proxy(
        &base,
        "fra-2",
        dir.path(),
        &["--region", "eu", "--tag", "env=stage"],
    )?;
    let _c = proxy(
        &base,
        "nyc-1",
        dir.path(),
        &["--region", "us", "--tag", "env=prod"],
    )?;
    let _d = proxy(&base, "bare", dir.path(), &[])?;

    wait_until(
        || async { Ok(nodes(&base, "").await?["total"] == 4) },
        UP,
        "all four proxies listed",
    )
    .await?;

    let v = nodes(&base, "tag=env%3Dprod").await?;
    let names: Vec<_> = v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["instance"].as_str().unwrap())
        .collect();
    ensure!(names == ["fra-1", "nyc-1"], "{v}");
    ensure!(v["items"][0]["tags"]["gpu"] == "a100", "{v}");

    let v = nodes(&base, "group_by=region&size=2").await?;
    ensure!(
        v["total"] == 4 && v["items"].as_array().unwrap().len() == 2,
        "{v}"
    );
    let groups: Vec<_> = v["groups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| (g["region"].clone(), g["nodes"].as_u64().unwrap()))
        .collect();
    ensure!(
        groups == [("eu".into(), 2), ("us".into(), 1), (Value::Null, 1)],
        "{v}"
    );
    Ok(())
}

#[tokio::test]
async fn a_malformed_label_stops_the_proxy_at_startup() -> Result<()> {
    build_fleet_bins()?;
    let dir = tempfile::tempdir()?;
    let config = dir.path().join("bad.yaml");
    std::fs::write(
        &config,
        minimal_wayhouse_config(free_port()?, free_port()?, free_port()?),
    )?;
    let argv = ["--config", config.to_str().unwrap(), "--tag", "Env=prod"].map(String::from);
    let mut p = Proc::spawn_in(None, "wayhouse", &argv)?;
    let deadline = std::time::Instant::now() + UP;
    while p.exit_code().is_none() {
        ensure!(
            std::time::Instant::now() < deadline,
            "the proxy kept running"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    ensure!(p.exit_code() != Some(0), "exited 0");
    ensure!(p.log().contains("--region/--tag"), "{}", p.log());
    Ok(())
}
