//! Confirm-and-revert under `--ha-peers` (A4a): a config revision on trial is reverted by the
//! leader when nobody confirms it, also after a leader failover; a confirm on any replica ends
//! the trial everywhere. Plain `cargo test`.

use std::time::Duration;

use anyhow::{ensure, Context, Result};
use serde_json::Value;
use wayhouse_fleet_tests::{
    build_fleet_bins, free_port, spawn_controller_with, wait_http_up, wait_until, Proc,
};

const AUTH_TOKEN: &str = "ha-config-trial-test-client-token";

const BASE: &str = "pools:\n  - name: mc\n    targets: [\"127.0.0.1:25566\"]\nlisteners:\n  - name: main\n    bind: \"0.0.0.0:25565\"\n    protocol: tcp\n    pool: mc\n";
const MOVED_ADMIN: &str = "settings:\n  admin:\n    listen: \"0.0.0.0:9999\"\n";

/// The shortest window the API accepts, in seconds.
const WINDOW: u64 = 10;

struct Cluster {
    bases: Vec<String>,
    /// `None` once a node has been killed.
    procs: Vec<Option<Proc>>,
    _dirs: Vec<tempfile::TempDir>,
    http: reqwest::Client,
}

impl Cluster {
    async fn start() -> Result<Self> {
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
            procs.push(Some(spawn_controller_with(
                dir.path(),
                &format!("127.0.0.1:{p}"),
                &extra,
            )?));
            dirs.push(dir);
            wait_http_up(
                &format!("{}/healthz", bases[i]),
                wayhouse_fleet_tests::STARTUP_TIMEOUT,
            )
            .await?;
        }
        let c = Cluster {
            bases,
            procs,
            _dirs: dirs,
            http,
        };
        wait_until(
            || async {
                let mut seen = Vec::new();
                for i in 0..3 {
                    seen.push(c.leader_id(i).await);
                }
                Ok(seen[0].is_some() && seen.iter().all(|s| *s == seen[0]))
            },
            Duration::from_secs(20),
            "all nodes agree on a leader",
        )
        .await?;
        Ok(c)
    }

    async fn leader_id(&self, i: usize) -> Option<usize> {
        let r = self
            .http
            .get(format!("{}/admin/ha/members", self.bases[i]))
            .bearer_auth(AUTH_TOKEN)
            .send()
            .await
            .ok()?;
        let id = r.json::<Value>().await.ok()?["leader"].as_u64()?;
        Some(id as usize - 1)
    }

    async fn post(&self, i: usize, path: &str, body: &str) -> Result<(u16, String)> {
        let r = self
            .http
            .post(format!("{}{path}", self.bases[i]))
            .bearer_auth(AUTH_TOKEN)
            .body(body.to_string())
            .send()
            .await?;
        Ok((r.status().as_u16(), r.text().await?))
    }

    async fn pending(&self, i: usize) -> Result<Value> {
        let v: Value = self
            .http
            .get(format!("{}/config/pending", self.bases[i]))
            .bearer_auth(AUTH_TOKEN)
            .send()
            .await?
            .json()
            .await?;
        Ok(v["pending"].clone())
    }

    async fn current(&self, i: usize) -> Result<String> {
        Ok(self
            .http
            .get(format!("{}/config", self.bases[i]))
            .bearer_auth(AUTH_TOKEN)
            .send()
            .await?
            .text()
            .await?)
    }

    /// Waits until node `i` holds a trial on `revision`.
    async fn wait_trial(&self, i: usize, revision: u64) -> Result<()> {
        wait_until(
            || async { Ok(self.pending(i).await?["revision"] == revision) },
            Duration::from_secs(10),
            "the trial is on the replica",
        )
        .await
    }
}

/// Saves `BASE`, then a risky revision with an explicit window, through a follower.
async fn start_trial(c: &Cluster, follower: usize) -> Result<()> {
    let (status, body) = c.post(follower, "/config", BASE).await?;
    ensure!(status == 200, "{body}");
    // The follower judges the next revision against its own copy of the history.
    wait_until(
        || async { Ok(c.current(follower).await? == BASE) },
        Duration::from_secs(10),
        "the follower holds the first revision",
    )
    .await?;
    let risky = format!("{BASE}{MOVED_ADMIN}");
    let (status, body) = c
        .post(
            follower,
            &format!("/config?confirm_within={WINDOW}"),
            &risky,
        )
        .await?;
    ensure!(status == 200, "{body}");
    ensure!(
        serde_json::from_str::<Value>(&body)?["revision"] == 2,
        "{body}"
    );
    Ok(())
}

#[tokio::test]
async fn an_unconfirmed_trial_is_reverted_also_after_the_leader_dies() -> Result<()> {
    let mut c = Cluster::start().await?;
    let leader = c.leader_id(0).await.context("a leader")?;
    let follower = (leader + 1) % 3;
    let other = (leader + 2) % 3;
    start_trial(&c, follower).await?;
    for i in 0..3 {
        c.wait_trial(i, 2).await?;
    }

    // The window is open and the leader dies with it: a survivor takes over the timer.
    c.procs[leader]
        .take()
        .context("leader proc")?
        .kill()
        .await?;
    wait_until(
        || async { Ok(c.leader_id(follower).await.is_some_and(|l| l != leader)) },
        Duration::from_secs(30),
        "a new leader is elected",
    )
    .await?;
    for i in [follower, other] {
        wait_until(
            || async { Ok(c.current(i).await? == BASE && c.pending(i).await?.is_null()) },
            Duration::from_secs(60),
            "the risky revision is reverted on the survivor",
        )
        .await?;
    }

    // The revert is a new revision: 1 base, 2 risky, 3 base again.
    let revisions: Value = c
        .http
        .get(format!("{}/config/revisions", c.bases[follower]))
        .bearer_auth(AUTH_TOKEN)
        .send()
        .await?
        .json()
        .await?;
    ensure!(
        revisions.as_array().is_some_and(|a| a.len() == 3),
        "{revisions}"
    );
    Ok(())
}

#[tokio::test]
async fn a_confirm_on_a_follower_ends_the_trial_everywhere() -> Result<()> {
    let c = Cluster::start().await?;
    let leader = c.leader_id(0).await.context("a leader")?;
    let follower = (leader + 1) % 3;
    start_trial(&c, follower).await?;
    for i in 0..3 {
        c.wait_trial(i, 2).await?;
    }

    // Revision 1 was never on trial; revision 2 is.
    let (status, _) = c.post(follower, "/config/confirm/1", "").await?;
    ensure!(status == 409, "confirming revision 1: {status}");
    let (status, body) = c.post(follower, "/config/confirm/2", "").await?;
    ensure!(status == 200, "{body}");
    for i in 0..3 {
        wait_until(
            || async { Ok(c.pending(i).await?.is_null()) },
            Duration::from_secs(10),
            "the trial ends on every replica",
        )
        .await?;
    }

    // Past the deadline nothing reverts.
    tokio::time::sleep(Duration::from_secs(WINDOW + 4)).await;
    for i in 0..3 {
        ensure!(
            c.current(i).await? == format!("{BASE}{MOVED_ADMIN}"),
            "node {i} reverted a confirmed revision"
        );
    }
    Ok(())
}

#[tokio::test]
async fn revert_now_on_a_follower_reverts_without_waiting() -> Result<()> {
    let c = Cluster::start().await?;
    let leader = c.leader_id(0).await.context("a leader")?;
    let follower = (leader + 1) % 3;
    start_trial(&c, follower).await?;
    c.wait_trial(follower, 2).await?;

    let (status, body) = c.post(follower, "/config/revert", "").await?;
    ensure!(status == 200, "{body}");
    for i in 0..3 {
        wait_until(
            || async { Ok(c.current(i).await? == BASE && c.pending(i).await?.is_null()) },
            Duration::from_secs(10),
            "every replica serves the old bytes",
        )
        .await?;
    }
    Ok(())
}
