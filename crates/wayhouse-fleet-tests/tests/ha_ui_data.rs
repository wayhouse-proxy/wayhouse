//! UI records under `--ha-peers` (#483): a write on a follower is forwarded, committed through
//! Raft and readable on every replica; a rejected write costs nothing. Plain `cargo test`.

use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::{json, Value};
use wayhouse_fleet_tests::{
    build_fleet_bins, free_port, spawn_controller_with, wait_http_up, wait_until,
};

const AUTH_TOKEN: &str = "ha-ui-data-test-client-token";

async fn leader_id(http: &reqwest::Client, base: &str) -> Option<u64> {
    let r = http
        .get(format!("{base}/admin/ha/members"))
        .bearer_auth(AUTH_TOKEN)
        .send()
        .await
        .ok()?;
    r.json::<Value>().await.ok()?["leader"].as_u64()
}

async fn send(
    http: &reqwest::Client,
    method: reqwest::Method,
    base: &str,
    path: &str,
    body: Option<Value>,
) -> Result<(u16, Value)> {
    let mut req = http
        .request(method, format!("{base}{path}"))
        .bearer_auth(AUTH_TOKEN);
    if let Some(b) = body {
        req = req.body(b.to_string());
    }
    let r = req.send().await?;
    let status = r.status().as_u16();
    Ok((status, r.json().await.unwrap_or(Value::Null)))
}

#[tokio::test]
async fn a_write_on_a_follower_is_readable_on_every_replica() -> Result<()> {
    build_fleet_bins()?;
    let http = wayhouse_http::client();
    let ports = (0..3).map(|_| free_port()).collect::<Result<Vec<_>>>()?;
    let peers = ports
        .iter()
        .enumerate()
        .map(|(i, p)| format!("{}=127.0.0.1:{p}", i + 1))
        .collect::<Vec<_>>()
        .join(",");
    let bases: Vec<String> = ports
        .iter()
        .map(|p| format!("http://127.0.0.1:{p}"))
        .collect();
    let (mut procs, mut dirs) = (Vec::new(), Vec::new());
    for (i, p) in ports.iter().enumerate() {
        let dir = tempfile::tempdir()?;
        let extra = [
            "--ha-node-id",
            &(i + 1).to_string(),
            "--ha-peers",
            &peers,
            "--auth-token",
            AUTH_TOKEN,
        ]
        .map(String::from);
        procs.push(spawn_controller_with(
            dir.path(),
            &format!("127.0.0.1:{p}"),
            &extra,
        )?);
        dirs.push(dir);
        wait_http_up(
            &format!("{}/healthz", bases[i]),
            wayhouse_fleet_tests::STARTUP_TIMEOUT,
        )
        .await?;
    }
    wait_until(
        || async {
            let mut seen = Vec::new();
            for b in &bases {
                seen.push(leader_id(&http, b).await);
            }
            Ok(seen[0].is_some() && seen.iter().all(|s| *s == seen[0]))
        },
        Duration::from_secs(20),
        "all nodes agree on a leader",
    )
    .await?;
    let leader = leader_id(&http, &bases[0]).await.context("a leader")? as usize - 1;
    let (writer, reader) = (&bases[(leader + 1) % 3], &bases[(leader + 2) % 3]);

    let put = reqwest::Method::PUT;
    let dash = json!({"widgets": [{"id": "nodes", "width": 6}]});
    let (status, body) = send(
        &http,
        put.clone(),
        writer,
        "/ui/dashboards/ops",
        Some(dash.clone()),
    )
    .await?;
    assert_eq!(status, 200, "{body}");
    let (status, body) = send(
        &http,
        put.clone(),
        writer,
        "/ui/defaults",
        Some(json!({"dashboard": "ops"})),
    )
    .await?;
    assert_eq!(status, 200, "{body}");

    // Rejected on the follower (422) without reaching the log.
    let bad = json!({"widgets": [{"id": "nodes", "width": 99}]});
    let (status, body) = send(&http, put, writer, "/ui/dashboards/bad", Some(bad)).await?;
    assert_eq!(status, 422, "{body}");

    // Another replica sees both after the commit.
    wait_until(
        || async {
            let (s, d) = send(
                &http,
                reqwest::Method::GET,
                reader,
                "/ui/dashboards/ops",
                None,
            )
            .await?;
            let (_, defaults) =
                send(&http, reqwest::Method::GET, reader, "/ui/defaults", None).await?;
            Ok(s == 200 && d["widgets"] == dash["widgets"] && defaults["dashboard"] == "ops")
        },
        Duration::from_secs(10),
        "the other replica applies the dashboard and the default",
    )
    .await?;

    // Deleting the default dashboard resets the default on every replica.
    let (status, _) = send(
        &http,
        reqwest::Method::DELETE,
        writer,
        "/ui/dashboards/ops",
        None,
    )
    .await?;
    assert_eq!(status, 204);
    wait_until(
        || async {
            let (_, defaults) =
                send(&http, reqwest::Method::GET, reader, "/ui/defaults", None).await?;
            Ok(defaults == json!({}))
        },
        Duration::from_secs(10),
        "the default is reset on the other replica",
    )
    .await?;

    // A feature-flag override written on a follower applies on every replica.
    let (status, body) = send(
        &http,
        reqwest::Method::PUT,
        writer,
        "/features/overrides/plugin_webhooks",
        Some(json!({"value": false, "reason": "fleet test"})),
    )
    .await?;
    assert_eq!(status, 200, "{body}");
    wait_until(
        || async {
            let (_, f) = send(&http, reqwest::Method::GET, reader, "/features", None).await?;
            let row = f["flags"]
                .as_array()
                .and_then(|a| a.iter().find(|r| r["name"] == "plugin_webhooks"));
            Ok(row.is_some_and(|r| r["value"] == false && r["source"] == "override"))
        },
        Duration::from_secs(10),
        "the other replica serves the override",
    )
    .await?;

    // An operator's node tags written on a follower are listed on every replica.
    let (status, body) = send(
        &http,
        reqwest::Method::PUT,
        writer,
        "/ui/node-tags/fra-1",
        Some(json!({"tags": {"rack": "r12"}})),
    )
    .await?;
    assert_eq!(status, 200, "{body}");
    wait_until(
        || async {
            let (_, l) = send(&http, reqwest::Method::GET, reader, "/ui/node-tags", None).await?;
            Ok(l["items"][0]["tags"]["rack"] == "r12")
        },
        Duration::from_secs(10),
        "the other replica lists the node tags",
    )
    .await?;
    drop(procs);
    Ok(())
}
