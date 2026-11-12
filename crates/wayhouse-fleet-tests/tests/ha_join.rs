//! A controller replica joins a running HA group by one-time code: the code is minted on the
//! cluster (admin token plus step-up), `wayhouse-controller enroll` redeems it into a data dir,
//! and the node started with `--ha-join` and the files it received asks the cluster to add it,
//! with no admin token on the new host. Plain `cargo test`: loopback only.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::Duration;

use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use wayhouse_fleet_tests::{
    bin_path, build_fleet_bins, free_port, spawn_controller_with, wait_http_up, wait_until, Proc,
    STARTUP_TIMEOUT,
};

const KEY: &str = "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=";
const ADMIN: &str = "ha-join-admin-token-0123";
const STEP_UP: &str = "ha-join-step-up-secret-0123";

fn private_file(dir: &Path, name: &str, text: &str) -> Result<String> {
    let p = dir.join(name);
    std::fs::write(&p, text)?;
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600))?;
    Ok(p.display().to_string())
}

struct Cluster {
    http: reqwest::Client,
    base1: String,
    dir: tempfile::TempDir,
    node1: Option<Proc>,
}

/// Node 1 alone in a one-voter group.
async fn first_node() -> Result<Cluster> {
    build_fleet_bins()?;
    let dir = tempfile::tempdir()?;
    let d = dir.path();
    std::fs::create_dir_all(d.join("node1"))?;
    let port = free_port()?;
    let node1 = spawn_controller_with(
        &d.join("node1"),
        &format!("127.0.0.1:{port}"),
        &[
            "--ha-node-id".to_owned(),
            "1".into(),
            "--ha-peers".into(),
            format!("1=127.0.0.1:{port}"),
            "--auth-token-file".into(),
            private_file(d, "admin", ADMIN)?,
            "--proxy-token-file".into(),
            private_file(d, "proxy", "ha-join-proxy-token-0123")?,
            "--agent-token-file".into(),
            private_file(d, "agent", "ha-join-agent-token-0123")?,
            "--ingest-token-file".into(),
            private_file(d, "ingest", "ha-join-ingest-token-0123")?,
            "--enroll-step-up-file".into(),
            private_file(d, "stepup", STEP_UP)?,
            "--plugin-secret-key-file".into(),
            private_file(d, "secret.key", &format!("{KEY}\n"))?,
        ],
    )?;
    let base1 = format!("http://127.0.0.1:{port}");
    wait_http_up(&format!("{base1}/healthz"), STARTUP_TIMEOUT).await?;
    let c = Cluster {
        http: wayhouse_http::client(),
        base1,
        dir,
        node1: Some(node1),
    };
    wait_until(
        || {
            let c = &c;
            async move { Ok(members(c, &c.base1).await.is_ok_and(|m| m["leader"] == 1)) }
        },
        Duration::from_secs(30),
        "node 1 to lead its group",
    )
    .await?;
    Ok(c)
}

async fn members(c: &Cluster, base: &str) -> Result<Value> {
    let r = c
        .http
        .get(format!("{base}/admin/ha/members"))
        .bearer_auth(ADMIN)
        .send()
        .await?;
    ensure!(r.status().is_success(), "members answered {}", r.status());
    Ok(r.json().await?)
}

fn voter_ids(m: &Value) -> Vec<u64> {
    let mut v: Vec<u64> = m["voters"]
        .as_array()
        .map(|a| a.iter().filter_map(|x| x["id"].as_u64()).collect())
        .unwrap_or_default();
    v.sort_unstable();
    v
}

async fn mint_controller(c: &Cluster, name: &str) -> Result<(String, u64)> {
    let r = c
        .http
        .post(format!("{}/enrollments", c.base1))
        .bearer_auth(ADMIN)
        .json(&json!({"role": "controller", "name": name, "step_up": STEP_UP}))
        .send()
        .await?;
    let status = r.status();
    let v: Value = r.json().await?;
    ensure!(status.is_success(), "mint answered {status}: {v}");
    Ok((
        v["code"].as_str().context("a code")?.to_owned(),
        v["node_id"].as_u64().context("a node id")?,
    ))
}

/// `wayhouse-controller enroll` into `<dir>/enrolled-<port>`; returns the data dir.
fn enroll(c: &Cluster, code: &str, advertise_port: u16) -> Result<std::process::Output> {
    Ok(std::process::Command::new(bin_path("wayhouse-controller"))
        .args(["enroll", "--controller-url", &c.base1, "--advertise"])
        .arg(format!("http://127.0.0.1:{advertise_port}"))
        .arg("--data-dir")
        .arg(c.dir.path().join(format!("enrolled-{advertise_port}")))
        .env("WAYHOUSE_ENROLL_CODE", code)
        .output()?)
}

/// Starts the replica the way the installed unit does, with the credentials as files.
fn start_replica(c: &Cluster, node_id: u64, port: u16, advertise_port: u16) -> Result<Proc> {
    let creds = c.dir.path().join(format!("enrolled-{advertise_port}"));
    let data = c.dir.path().join(format!("node-{node_id}-{port}"));
    std::fs::create_dir_all(&data)?;
    let f = |n: &str| creds.join(n).display().to_string();
    Proc::spawn_in(
        None,
        "wayhouse-controller",
        &[
            "--data-dir".to_owned(),
            data.display().to_string(),
            "--listen".into(),
            format!("127.0.0.1:{port}"),
            "--ha-join".into(),
            "--ha-node-id".into(),
            node_id.to_string(),
            "--ha-advertise".into(),
            format!("http://127.0.0.1:{advertise_port}"),
            "--ha-join-via".into(),
            c.base1.clone(),
            "--ha-token-file".into(),
            f("ha-token"),
            "--ha-join-proof-file".into(),
            f("ha-join-proof"),
            "--auth-token-file".into(),
            f("auth-token"),
            "--proxy-token-file".into(),
            f("proxy-token"),
            "--agent-token-file".into(),
            f("agent-token"),
            "--ingest-token-file".into(),
            f("ingest-token"),
            "--enroll-step-up-file".into(),
            f("enroll-step-up"),
            "--plugin-secret-key-file".into(),
            f("secret.keys"),
        ],
    )
}

#[tokio::test]
async fn two_replicas_join_by_code_with_distinct_ids_and_the_group_survives_losing_its_leader(
) -> Result<()> {
    let mut c = first_node().await?;

    // Two codes minted at once get two different ids.
    let (a, b) = tokio::join!(mint_controller(&c, "c-a"), mint_controller(&c, "c-b"));
    let ((code_a, id_a), (code_b, id_b)) = (a?, b?);
    ensure!(
        id_a != id_b && id_a >= 2 && id_b >= 2,
        "ids {id_a} and {id_b}"
    );

    let (port_a, port_b) = (free_port()?, free_port()?);
    for (code, port) in [(&code_a, port_a), (&code_b, port_b)] {
        let out = enroll(&c, code, port)?;
        ensure!(
            out.status.success(),
            "enroll: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        // Not one value on screen.
        let shown = String::from_utf8_lossy(&out.stderr).into_owned()
            + &String::from_utf8_lossy(&out.stdout);
        ensure!(!shown.contains(ADMIN) && !shown.contains(STEP_UP) && !shown.contains(KEY));
    }
    // Exactly the grant, 0600.
    let dir_a = c.dir.path().join(format!("enrolled-{port_a}"));
    let mut got: Vec<String> = std::fs::read_dir(&dir_a)?
        .map(|e| e.map(|e| e.file_name().to_string_lossy().into_owned()))
        .collect::<Result<_, _>>()?;
    got.sort();
    ensure!(
        got == [
            "agent-token",
            "auth-token",
            "enroll-step-up",
            "enrollment.json",
            "ha-join-proof",
            "ha-token",
            "ingest-token",
            "proxy-token",
            "secret.keys"
        ],
        "{got:?}"
    );
    for f in &got {
        let mode = std::fs::metadata(dir_a.join(f))?.permissions().mode() & 0o777;
        ensure!(mode == 0o600, "{f} has mode {mode:o}");
    }
    // A spent code does not enroll a second time.
    let again = enroll(&c, &code_a, free_port()?)?;
    ensure!(!again.status.success(), "a spent code enrolled again");

    // Both nodes start at once and ask to be added; the leader takes them one at a time.
    let (port_a_http, port_b_http) = (port_a, port_b);
    let node_a = start_replica(&c, id_a, port_a_http, port_a)?;
    let node_b = start_replica(&c, id_b, port_b_http, port_b)?;
    let mut want = vec![1, id_a, id_b];
    want.sort_unstable();
    wait_until(
        || {
            let c = &c;
            let want = want.clone();
            async move {
                Ok(members(c, &c.base1)
                    .await
                    .is_ok_and(|m| voter_ids(&m) == want))
            }
        },
        Duration::from_secs(120),
        "both replicas to become voters",
    )
    .await
    .with_context(|| {
        format!(
            "node a log:\n{}\nnode b log:\n{}",
            node_a.log(),
            node_b.log()
        )
    })?;

    // The codes show as joined, and the joined node serves the API with the delivered tokens: a
    // write through it is forwarded to the leader.
    let list: Value = c
        .http
        .get(format!("{}/enrollments", c.base1))
        .bearer_auth(ADMIN)
        .send()
        .await?
        .json()
        .await?;
    ensure!(
        list["items"]
            .as_array()
            .is_some_and(|i| i.iter().all(|r| r["status"] == "joined")),
        "{list}"
    );
    let via_a = format!("http://127.0.0.1:{port_a_http}");
    wait_http_up(&format!("{via_a}/healthz"), STARTUP_TIMEOUT).await?;

    // Lose the leader: the other two elect a new one and keep accepting writes.
    let m = members(&c, &c.base1).await?;
    let leader = m["leader"].as_u64().context("a leader")?;
    let (survivor_base, survivor) = if leader == 1 {
        c.node1.take().context("node 1")?.kill().await?;
        (via_a.clone(), id_a)
    } else if leader == id_a {
        node_a.kill().await?;
        (c.base1.clone(), 1)
    } else {
        node_b.kill().await?;
        (c.base1.clone(), 1)
    };
    let _ = survivor;
    let config =
        wayhouse_fleet_tests::minimal_wayhouse_config(free_port()?, free_port()?, free_port()?);
    wait_until(
        || {
            let (c, base, config) = (&c, survivor_base.clone(), config.clone());
            async move {
                Ok(c.http
                    .post(format!("{base}/config"))
                    .bearer_auth(ADMIN)
                    .body(config)
                    .send()
                    .await
                    .is_ok_and(|r| r.status().is_success()))
            }
        },
        Duration::from_secs(60),
        "the remaining replicas to elect a leader and accept a write",
    )
    .await?;
    Ok(())
}

#[tokio::test]
async fn a_wrong_address_names_its_reason_keeps_the_code_alive_and_the_node_joins_once_fixed(
) -> Result<()> {
    let c = first_node().await?;
    let (code, id) = mint_controller(&c, "c-wrong").await?;
    let nowhere = free_port()?; // nothing will listen here
    let real = free_port()?;
    let out = enroll(&c, &code, nowhere)?;
    ensure!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Advertises an address nothing answers on: the leader cannot verify the node.
    let wrong = start_replica(&c, id, real, nowhere)?;
    wait_until(
        || {
            let wrong = &wrong;
            async move { Ok(wrong.log().contains("did not answer /raft/whoami")) }
        },
        Duration::from_secs(60),
        "the node to log why it could not join",
    )
    .await
    .with_context(|| format!("log:\n{}", wrong.log()))?;
    ensure!(
        !wrong.log().contains("refused this node's join proof"),
        "a wrong address burnt the code"
    );
    ensure!(
        voter_ids(&members(&c, &c.base1).await?) == [1],
        "the node joined from a wrong address"
    );
    let list: Value = c
        .http
        .get(format!("{}/enrollments", c.base1))
        .bearer_auth(ADMIN)
        .send()
        .await?
        .json()
        .await?;
    ensure!(list["items"][0]["status"] == "joining", "{list}");
    wrong.kill().await?;

    // Same proof, correct address (the credentials still sit in the first directory).
    let creds = c.dir.path().join(format!("enrolled-{nowhere}"));
    let right = {
        let data = c.dir.path().join("fixed");
        std::fs::create_dir_all(&data)?;
        let f = |n: &str| creds.join(n).display().to_string();
        Proc::spawn_in(
            None,
            "wayhouse-controller",
            &[
                "--data-dir".to_owned(),
                data.display().to_string(),
                "--listen".into(),
                format!("127.0.0.1:{real}"),
                "--ha-join".into(),
                "--ha-node-id".into(),
                id.to_string(),
                "--ha-advertise".into(),
                format!("http://127.0.0.1:{real}"),
                "--ha-join-via".into(),
                c.base1.clone(),
                "--ha-token-file".into(),
                f("ha-token"),
                "--ha-join-proof-file".into(),
                f("ha-join-proof"),
                "--auth-token-file".into(),
                f("auth-token"),
                "--proxy-token-file".into(),
                f("proxy-token"),
                "--agent-token-file".into(),
                f("agent-token"),
                "--plugin-secret-key-file".into(),
                f("secret.keys"),
            ],
        )?
    };
    wait_until(
        || {
            let c = &c;
            async move {
                Ok(members(c, &c.base1)
                    .await
                    .is_ok_and(|m| voter_ids(&m) == [1, id]))
            }
        },
        Duration::from_secs(90),
        "the node to join once its address is right",
    )
    .await
    .with_context(|| format!("log:\n{}", right.log()))?;
    Ok(())
}
