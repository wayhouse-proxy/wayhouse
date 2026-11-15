//! Mixed-version fleets (#185, `docs/upgrading.md`): a node one minor behind
//! keeps working and is sent only baseline fields, and a node on another major
//! is refused readably. The older side is played either by a hand-built request
//! carrying the older header, or by a real binary started with
//! `WAYHOUSE_TEST_PROTOCOL_*` (the `test-protocol-override` feature every fleet
//! build enables, see `PROTOCOL_OVERRIDE_FEATURE`).

use std::time::Duration;

use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use wayhouse_fleet_tests::{
    build_fleet_bins, free_port, spawn_controller_env, spawn_controller_with, wait_http_up,
    wait_until, Proc,
};
use wayhouse_http::protocol::{ProtocolVersion, HEADER};

async fn controller() -> Result<(Proc, tempfile::TempDir, String)> {
    build_fleet_bins()?;
    let dir = tempfile::tempdir()?;
    let port = free_port()?;
    let extra = vec!["--tunnel-network".to_string(), "10.60.0.0/16".to_string()];
    let ctl = spawn_controller_with(dir.path(), &format!("127.0.0.1:{port}"), &extra)?;
    wait_http_up(
        &format!("http://127.0.0.1:{port}/healthz"),
        wayhouse_fleet_tests::STARTUP_TIMEOUT,
    )
    .await?;
    Ok((ctl, dir, format!("http://127.0.0.1:{port}")))
}

/// Register a proxy the way a peer announcing `protocol` would.
async fn register(base: &str, name: &str, protocol: &str) -> Result<(u16, Option<String>, Value)> {
    let resp = reqwest::Client::new()
        .post(format!("{base}/proxy-peers"))
        .header(HEADER, protocol)
        .json(&json!({"name": name, "pubkey": wayhouse_fleet_tests::test_key(name), "endpoint": "203.0.113.9:51820"}))
        .send()
        .await?;
    let status = resp.status().as_u16();
    let ours = resp
        .headers()
        .get(HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let text = resp.text().await?;
    let body = serde_json::from_str(&text).unwrap_or(Value::String(text));
    Ok((status, ours, body))
}

#[tokio::test]
async fn proxy_one_minor_behind_registers_and_gets_baseline_fields() -> Result<()> {
    let (_ctl, _dir, base) = controller().await?;
    let current = ProtocolVersion::CURRENT;
    assert!(
        current.minor >= 1,
        "this test plays a peer one minor behind"
    );
    // Minor 1 introduced the `capabilities` field; a peer below it gets the baseline.
    let older = format!("{}.0", current.major);

    let (status, ours, body) = register(&base, "old-proxy", &older).await?;
    assert_eq!(status, 200, "{body}");
    assert_eq!(ours.as_deref(), Some(current.to_string().as_str()));
    assert!(
        body.get("tunnel_address").is_some(),
        "baseline fields stay: {body}"
    );
    let mut keys: Vec<_> = body.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    assert_eq!(
        keys,
        ["revision", "tunnel_address", "tunnel_network"],
        "an older peer is sent exactly the baseline fields"
    );

    let (status, _, body) = register(&base, "new-proxy", &current.to_string()).await?;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["capabilities"], json!(["protocol-gating"]));
    Ok(())
}

#[tokio::test]
async fn other_major_peer_is_refused_with_readable_message_and_counter() -> Result<()> {
    let (_ctl, _dir, base) = controller().await?;
    let other = format!("{}.0", ProtocolVersion::CURRENT.major + 1);

    let (status, ours, body) = register(&base, "future-proxy", &other).await?;
    assert_eq!(status, 426);
    assert_eq!(
        ours.as_deref(),
        Some(ProtocolVersion::CURRENT.to_string().as_str())
    );
    let text = body.as_str().unwrap_or_default();
    assert!(
        text.contains(&other) && text.contains("upgrade the older side"),
        "{body}"
    );

    let metrics = wayhouse_http::client()
        .get(format!("{base}/metrics"))
        .send()
        .await?
        .text()
        .await?;
    assert!(
        metrics.contains("wayhouse_protocol_mismatch_total{route_group=\"controller\"} 1"),
        "{metrics}"
    );
    Ok(())
}

const DOC: &str = "pools:\n  - name: mc\n    targets: [\"127.0.0.1:25566\"]\nlisteners:\n  - name: main\n    bind: \"0.0.0.0:25565\"\n    protocol: tcp\n    pool: mc\n";

fn override_env(major: u16, minor: u16) -> Vec<(&'static str, String)> {
    vec![
        ("WAYHOUSE_TEST_PROTOCOL_MAJOR", major.to_string()),
        ("WAYHOUSE_TEST_PROTOCOL_MINOR", minor.to_string()),
    ]
}

/// The override works on a real spawned binary (#262): a controller started with an injected
/// version announces it in its response header and its build-info metric, and one on another
/// major refuses a peer on ours with the readable `426`.
#[tokio::test]
async fn a_spawned_binary_announces_its_injected_version_and_refuses_another_major() -> Result<()> {
    build_fleet_bins()?;
    let current = ProtocolVersion::CURRENT;
    assert!(
        current.minor >= 1,
        "this test plays a node one minor behind"
    );
    let http = wayhouse_http::client();

    // One minor behind: it announces that version, and still serves ours.
    let dir = tempfile::tempdir()?;
    let port = free_port()?;
    let _older = spawn_controller_env(
        dir.path(),
        &format!("127.0.0.1:{port}"),
        &["--tunnel-network".to_string(), "10.60.0.0/16".to_string()],
        &override_env(current.major, current.minor - 1),
    )?;
    let base = format!("http://127.0.0.1:{port}");
    wait_http_up(
        &format!("{base}/healthz"),
        wayhouse_fleet_tests::STARTUP_TIMEOUT,
    )
    .await?;
    let (status, announced, body) = register(&base, "peer-a", &current.to_string()).await?;
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        announced.as_deref(),
        Some(format!("{}.{}", current.major, current.minor - 1).as_str())
    );
    let metrics = http
        .get(format!("{base}/metrics"))
        .send()
        .await?
        .text()
        .await?;
    assert!(
        metrics.contains(&format!(
            "protocol=\"{}.{}\"",
            current.major,
            current.minor - 1
        )),
        "{metrics}"
    );

    // Another major: our request is refused readably.
    let dir = tempfile::tempdir()?;
    let port = free_port()?;
    let _future = spawn_controller_env(
        dir.path(),
        &format!("127.0.0.1:{port}"),
        &[],
        &override_env(current.major + 1, 0),
    )?;
    let base = format!("http://127.0.0.1:{port}");
    wait_http_up(
        &format!("{base}/healthz"),
        wayhouse_fleet_tests::STARTUP_TIMEOUT,
    )
    .await?;
    let (status, _, body) = register(&base, "peer-b", &current.to_string()).await?;
    assert_eq!(status, 426);
    let text = body.as_str().unwrap_or_default();
    assert!(text.contains("upgrade the older side"), "{body}");
    Ok(())
}

/// A `slave` tier one minor behind its parent relays the parent's config (#262, #263): the only
/// protocol-gated field today is the registration `capabilities`, which tiers do not use, so the
/// tier path's baseline rule comes down to the relay working across a minor difference. The
/// relay cursor survives the child restarting upgraded, and the next revision arrives once.
#[tokio::test]
async fn a_slave_tier_one_minor_behind_relays_and_resumes_its_cursor_after_upgrading() -> Result<()>
{
    build_fleet_bins()?;
    let current = ProtocolVersion::CURRENT;
    assert!(current.minor >= 1);
    let http = wayhouse_http::client();
    let dir = tempfile::tempdir()?;
    let (parent_port, child_port) = (free_port()?, free_port()?);
    let (parent_dir, child_dir) = (dir.path().join("parent"), dir.path().join("child"));
    std::fs::create_dir_all(&parent_dir)?;
    std::fs::create_dir_all(&child_dir)?;
    let parent_base = format!("http://127.0.0.1:{parent_port}");
    let child_base = format!("http://127.0.0.1:{child_port}");
    let _parent = spawn_controller_with(&parent_dir, &format!("127.0.0.1:{parent_port}"), &[])?;
    wait_http_up(
        &format!("{parent_base}/healthz"),
        wayhouse_fleet_tests::STARTUP_TIMEOUT,
    )
    .await?;
    let child_args = vec![
        "--role".to_string(),
        "slave".to_string(),
        "--parent-url".to_string(),
        parent_base.clone(),
    ];

    let submit = |doc: String| {
        let (http, base) = (http.clone(), parent_base.clone());
        async move {
            let r = http.post(format!("{base}/config")).body(doc).send().await?;
            ensure!(r.status().is_success(), "submit: {}", r.text().await?);
            anyhow::Ok(())
        }
    };
    let child_revision = |base: String| {
        let http = http.clone();
        async move {
            let r = http.get(format!("{base}/config")).send().await.ok()?;
            r.headers()
                .get("X-Config-Revision")?
                .to_str()
                .ok()?
                .parse::<u64>()
                .ok()
        }
    };

    submit(DOC.to_string()).await?;
    // The child is one minor behind its parent.
    let older = spawn_controller_env(
        &child_dir,
        &format!("127.0.0.1:{child_port}"),
        &child_args,
        &override_env(current.major, current.minor - 1),
    )?;
    wait_until(
        || {
            let (f, b) = (child_revision, child_base.clone());
            async move { Ok(f(b).await == Some(1)) }
        },
        Duration::from_secs(30),
        "the older child to relay the parent's first revision",
    )
    .await?;

    // The child upgrades (restarts on the same data dir, no override); the parent moves on.
    older.kill().await?;
    submit(DOC.replace("25566", "25567")).await?;
    let _upgraded = spawn_controller_env(
        &child_dir,
        &format!("127.0.0.1:{child_port}"),
        &child_args,
        &[],
    )?;
    wait_until(
        || {
            let (f, b) = (child_revision, child_base.clone());
            async move { Ok(f(b).await == Some(2)) }
        },
        Duration::from_secs(30),
        "the upgraded child to resume from its cursor and relay revision 2",
    )
    .await
    .context("the child did not catch up")?;
    // Exactly one new revision: the cursor kept it from replaying revision 1.
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(child_revision(child_base).await, Some(2));
    Ok(())
}
