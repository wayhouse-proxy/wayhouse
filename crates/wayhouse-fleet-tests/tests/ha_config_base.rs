//! The config `base` guard under `--ha-peers` (#447): a follower forwards `?base=` and the
//! leader answers `409`; `POST /config/validate` is read-only, so a follower answers it itself.
//! Plain `cargo test`.

use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::Value;
use wayhouse_fleet_tests::{
    build_fleet_bins, free_port, spawn_controller_with, wait_http_up, wait_until,
};

const AUTH_TOKEN: &str = "ha-config-base-test-client-token";

const DOC: &str = "pools:\n  - name: mc\n    targets: [\"127.0.0.1:25566\"]\nlisteners:\n  - name: main\n    bind: \"0.0.0.0:25565\"\n    protocol: tcp\n    pool: mc\n";

async fn leader_id(http: &reqwest::Client, base: &str) -> Option<u64> {
    let r = http
        .get(format!("{base}/admin/ha/members"))
        .bearer_auth(AUTH_TOKEN)
        .send()
        .await
        .ok()?;
    r.json::<Value>().await.ok()?["leader"].as_u64()
}

async fn post(
    http: &reqwest::Client,
    base: &str,
    path: &str,
    body: String,
) -> Result<(u16, String)> {
    let r = http
        .post(format!("{base}{path}"))
        .bearer_auth(AUTH_TOKEN)
        .body(body)
        .send()
        .await?;
    Ok((r.status().as_u16(), r.text().await?))
}

#[tokio::test]
async fn a_follower_forwards_base_and_the_leader_answers_409() -> Result<()> {
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
    let follower = &bases[(leader + 1) % 3];

    // First save from a follower: base 0 = "no revision yet".
    let (status, body) = post(&http, follower, "/config?base=0", DOC.into()).await?;
    assert_eq!(status, 200, "{body}");
    let (status, body) = post(&http, follower, "/config?base=1", DOC.into()).await?;
    assert_eq!(status, 200, "{body}");
    // base=1 is stale now; the follower relays the leader's 409 and its `current`.
    let (status, body) = post(&http, follower, "/config?base=1", DOC.into()).await?;
    assert_eq!(status, 409, "{body}");
    assert_eq!(serde_json::from_str::<Value>(&body)?["current"], 2);

    // Validate stores nothing and needs no leader.
    let (status, body) = post(&http, follower, "/config/validate", DOC.into()).await?;
    assert_eq!(status, 200, "{body}");
    let (status, _) = post(&http, follower, "/config/validate", "pools: 7".into()).await?;
    assert_eq!(status, 422);
    drop(procs);
    Ok(())
}
