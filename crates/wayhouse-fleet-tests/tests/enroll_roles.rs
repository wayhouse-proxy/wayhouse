//! Enrollment for the proxy, aggregator and UI roles, through the real binaries: each node's
//! `enroll` subcommand redeems a code the controller minted for that role and leaves exactly
//! that role's files (0600) in its data dir. The keyring and the admin token are only minted
//! with the step-up secret.

use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

use anyhow::{ensure, Result};
use wayhouse_fleet_tests::{
    bin_path, build_fleet_bins, free_port, spawn_controller_with, wait_until,
};

const KEY: &str = "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=";
const ADMIN: &str = "admin-token-admin-token";
const PROXY: &str = "proxy-token-proxy-token";
const AGENT: &str = "agent-token-agent-token";
const INGEST: &str = "ingest-token-ingest-token";
const STEP_UP: &str = "step-up-step-up-step-up";

fn private_file(dir: &std::path::Path, name: &str, text: &str) -> Result<String> {
    let p = dir.join(name);
    std::fs::write(&p, text)?;
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600))?;
    Ok(p.display().to_string())
}

#[tokio::test]
async fn each_node_binary_enrolls_into_exactly_its_role_files() -> Result<()> {
    build_fleet_bins()?;
    let dir = tempfile::tempdir()?;
    let d = dir.path();
    std::fs::create_dir_all(d.join("data"))?;
    let port = free_port()?;
    let _ctl = spawn_controller_with(
        &d.join("data"),
        &format!("127.0.0.1:{port}"),
        &[
            "--auth-token-file",
            &private_file(d, "admin", ADMIN)?,
            "--proxy-token-file",
            &private_file(d, "proxy", PROXY)?,
            "--agent-token-file",
            &private_file(d, "agent", AGENT)?,
            "--ingest-token-file",
            &private_file(d, "ingest", INGEST)?,
            "--enroll-step-up-file",
            &private_file(d, "stepup", STEP_UP)?,
            "--plugin-secret-key-file",
            &private_file(d, "secret.key", &format!("{KEY}\n"))?,
        ]
        .map(String::from),
    )?;
    let base = format!("http://127.0.0.1:{port}");
    let http = wayhouse_http::client();
    wait_until(
        || {
            let (c, b) = (http.clone(), base.clone());
            async move { Ok(c.get(format!("{b}/healthz")).send().await.is_ok()) }
        },
        Duration::from_secs(30),
        "the controller to answer",
    )
    .await?;

    let mint = |body: serde_json::Value| {
        let (c, b) = (http.clone(), base.clone());
        async move {
            let r = c
                .post(format!("{b}/enrollments"))
                .bearer_auth(ADMIN)
                .json(&body)
                .send()
                .await?;
            anyhow::Ok((r.status().as_u16(), r.json::<serde_json::Value>().await?))
        }
    };
    // The keyring and admin roles are refused without the step-up secret.
    ensure!(
        mint(serde_json::json!({"role": "proxy", "name": "edge-1"}))
            .await?
            .0
            == 403
    );

    for (bin, role, expect) in [
        (
            "wayhouse",
            "proxy",
            &["aggregator-token", "controller-token", "secret.keys"][..],
        ),
        (
            "wayhouse-aggregator",
            "aggregator",
            &["auth-token", "ingest-token"][..],
        ),
        ("wayhouse-ui", "ui", &["controller-token"][..]),
    ] {
        let (st, m) =
            mint(serde_json::json!({"role": role, "name": "n1", "step_up": STEP_UP})).await?;
        ensure!(st == 200, "{role}: {m}");
        let data = d.join(format!("node-{role}"));
        let out = std::process::Command::new(bin_path(bin))
            .args(["enroll", "--controller-url", &base, "--data-dir"])
            .arg(&data)
            .env("WAYHOUSE_ENROLL_CODE", m["code"].as_str().unwrap())
            .output()?;
        ensure!(
            out.status.success(),
            "{bin} enroll: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let mut got: Vec<String> = std::fs::read_dir(&data)?
            .map(|e| e.map(|e| e.file_name().to_string_lossy().into_owned()))
            .collect::<Result<_, _>>()?;
        got.retain(|f| f != "enrollment.json");
        got.sort();
        ensure!(got == expect, "{role} got {got:?}, wanted {expect:?}");
        for f in &got {
            let mode = std::fs::metadata(data.join(f))?.permissions().mode() & 0o777;
            ensure!(mode == 0o600, "{role}/{f} has mode {mode:o}");
        }
        // The same code does not work twice.
        let again = std::process::Command::new(bin_path(bin))
            .args(["enroll", "--controller-url", &base, "--data-dir"])
            .arg(d.join("again"))
            .env("WAYHOUSE_ENROLL_CODE", m["code"].as_str().unwrap())
            .output()?;
        ensure!(
            !again.status.success(),
            "{role}: a spent code redeemed twice"
        );
        if role == "proxy" {
            ensure!(std::fs::read_to_string(data.join("secret.keys"))?.trim() == KEY);
            ensure!(std::fs::read_to_string(data.join("controller-token"))? == PROXY);
        }
        if role == "ui" {
            ensure!(std::fs::read_to_string(data.join("controller-token"))? == ADMIN);
        }
    }
    Ok(())
}
