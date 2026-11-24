//! A plaintext admin token is scrubbed out of every retired config revision on every replica,
//! including one that joins late and installs a snapshot (#474): only the live revision keeps it,
//! and rolling back to a scrubbed revision is refused.
//!
//! Same shape as `ha_snapshots.rs`: the replicas build a snapshot every 10 entries
//! (`--ha-snapshot-after 10`) and the test writes past the 1000 entries `openraft` keeps behind
//! each, so the node that joins afterwards can only be caught up by snapshot.

use std::time::Duration;

use anyhow::{ensure, Result};
use wayhouse_fleet_tests::{
    build_fleet_bins, free_port, minimal_wayhouse_config, spawn_controller_with, wait_http_up,
    wait_until, Proc,
};

/// Comfortably past `max_in_snapshot_log_to_keep` (1000) plus one snapshot
/// interval, so the entries the joiner would need first are purged.
const WRITES: u16 = 1_100;
const HA_TOKEN: &str = "ha-secret-purge-test-token";
/// The secret the configs carry in plaintext; it must survive only in the live revision.
const SECRET: &str = "purge-me-0123456789abcdef";

/// `POST /config`: the new revision on 2xx, otherwise what the node answered (or why it did not).
async fn submit(client: &reqwest::Client, base: &str, body: String) -> Result<u64, String> {
    let resp = client
        .post(format!("{base}/config"))
        .body(body)
        .send()
        .await
        .map_err(|e| format!("no answer: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.map_err(|e| format!("{status}: {e}"))?;
    if !status.is_success() {
        return Err(format!("{status}: {text}"));
    }
    let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("{status}: {e}"))?;
    v["revision"].as_u64().ok_or(format!("{status}: {text}"))
}

/// `GET /config/revisions` as parsed JSON.
async fn revisions(client: &reqwest::Client, base: &str) -> Result<serde_json::Value> {
    Ok(client
        .get(format!("{base}/config/revisions"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}

/// A revision's raw text.
async fn revision_text(client: &reqwest::Client, base: &str, revision: u64) -> Result<String> {
    Ok(client
        .get(format!("{base}/config/revisions/{revision}"))
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?)
}

/// Same secret in every revision, so none of them is a lockout risk (no trial): each write
/// retires the one before it.
fn body(admin: u16, listen: u16, i: u16) -> String {
    minimal_wayhouse_config(admin, listen, 20_000 + i).replace(
        "  admin:\n",
        &format!("  admin:\n    auth_token: \"{SECRET}\"\n"),
    )
}

fn spawn_node(dir: &tempfile::TempDir, port: u16, extra: &[&str]) -> Result<Proc> {
    let mut args: Vec<String> = extra.iter().map(std::string::ToString::to_string).collect();
    args.extend(
        ["--ha-token", HA_TOKEN, "--ha-snapshot-after", "10"]
            .iter()
            .map(std::string::ToString::to_string),
    );
    spawn_controller_with(dir.path(), &format!("127.0.0.1:{port}"), &args)
}

#[tokio::test]
async fn retired_revisions_lose_the_plaintext_secret_on_every_replica_including_a_late_joiner(
) -> Result<()> {
    build_fleet_bins()?;
    let client = wayhouse_http::client();
    let ports = (0..3).map(|_| free_port()).collect::<Result<Vec<_>>>()?;
    let peers = ports
        .iter()
        .enumerate()
        .map(|(i, p)| format!("{}=127.0.0.1:{p}", i + 1))
        .collect::<Vec<_>>()
        .join(",");
    let mut procs = Vec::new();
    let mut dirs = Vec::new();
    for (i, p) in ports.iter().enumerate() {
        let dir = tempfile::tempdir()?;
        let id = (i + 1).to_string();
        procs.push(spawn_node(
            &dir,
            *p,
            &["--ha-node-id", &id, "--ha-peers", &peers],
        )?);
        dirs.push(dir);
    }
    let mut bases: Vec<String> = ports
        .iter()
        .map(|p| format!("http://127.0.0.1:{p}"))
        .collect();

    let (admin, listen) = (free_port()?, free_port()?);
    wait_until(
        || {
            let (client, base, body) = (client.clone(), bases[0].clone(), body(admin, listen, 0));
            async move { Ok(submit(&client, &base, body).await.is_ok()) }
        },
        Duration::from_secs(120),
        "a raft leader to accept a write",
    )
    .await?;
    for i in 1..WRITES {
        if let Err(why) = submit(&client, &bases[0], body(admin, listen, i)).await {
            anyhow::bail!("write {i} was refused: {why}");
        }
    }

    // The log has been purged on the leader, so the fourth node can only install a snapshot.
    let port4 = free_port()?;
    let dir4 = tempfile::tempdir()?;
    procs.push(spawn_node(
        &dir4,
        port4,
        &["--ha-join", "--ha-node-id", "4"],
    )?);
    dirs.push(dir4);
    let base4 = format!("http://127.0.0.1:{port4}");
    wait_http_up(
        &format!("{base4}/healthz"),
        wayhouse_fleet_tests::STARTUP_TIMEOUT,
    )
    .await?;
    let added = client
        .post(format!("{}/admin/ha/members", bases[0]))
        .json(&serde_json::json!({ "id": 4, "addr": format!("127.0.0.1:{port4}") }))
        .send()
        .await?;
    ensure!(
        added.status().is_success(),
        "adding node 4 failed: {}",
        added.text().await?
    );
    bases.push(base4.clone());

    let want = revisions(&client, &bases[0]).await?;
    let caught_up = wait_until(
        || {
            let (client, base4, want) = (client.clone(), base4.clone(), want.clone());
            async move { Ok(revisions(&client, &base4).await.ok() == Some(want)) }
        },
        // A snapshot of hundreds of revisions on a loaded CI runner took more than the old 30 s;
        // the wait ends as soon as node 4 matches, so a long ceiling costs nothing when it is fast.
        Duration::from_secs(240),
        "node 4 to hold the leader's revisions",
    )
    .await;
    if let Err(e) = caught_up {
        // Say how node 4 differs, and what the nodes logged, so a hang explains itself.
        let got = revisions(&client, &base4).await.unwrap_or_default();
        let diff: Vec<_> = want
            .as_array()
            .into_iter()
            .flatten()
            .zip(got.as_array().into_iter().flatten())
            .filter(|(a, b)| a != b)
            .take(5)
            .collect();
        let logs: Vec<String> = procs.iter().map(Proc::log).collect();
        anyhow::bail!(
            "{e}: leader holds {}, node 4 {}; first differing: {diff:?}\n{}",
            want.as_array().map_or(0, Vec::len),
            got.as_array().map_or(0, Vec::len),
            logs.join("\n=====\n")
        );
    }
    let on_node4: serde_json::Value = client
        .get(format!("{base4}/admin/ha/members"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    ensure!(
        on_node4["purged_index"].as_u64().is_some_and(|p| p > 0),
        "node 4 caught up without installing a snapshot: {on_node4}"
    );

    // Every replica: the live revision keeps the secret, no older one holds it.
    let last = u64::from(WRITES);
    for base in &bases {
        for revision in [1, 2, 500, last - 1] {
            let text = revision_text(&client, base, revision).await?;
            ensure!(
                !text.contains(SECRET),
                "{base}: revision {revision} still holds the secret"
            );
            ensure!(
                text.contains("[redacted]"),
                "{base}: revision {revision} is not marked:\n{text}"
            );
        }
        let live = revision_text(&client, base, last).await?;
        ensure!(
            live.contains(SECRET),
            "{base}: the live revision lost its secret"
        );
    }

    // Rolling back to a scrubbed revision is refused, and nothing was written.
    let refused = client
        .post(format!("{}/config/rollback/1", bases[0]))
        .send()
        .await?;
    ensure!(
        refused.status() == reqwest::StatusCode::CONFLICT,
        "rollback to a scrubbed revision: {}",
        refused.status()
    );
    ensure!(
        refused.text().await?.contains("redacted"),
        "the refusal does not say why"
    );
    let after = revisions(&client, &bases[0]).await?;
    ensure!(
        after.as_array().map(Vec::len) == want.as_array().map(Vec::len),
        "the refused rollback wrote a revision"
    );

    for p in &mut procs {
        ensure!(p.exit_code().is_none(), "a node exited:\n{}", p.log());
    }
    Ok(())
}
