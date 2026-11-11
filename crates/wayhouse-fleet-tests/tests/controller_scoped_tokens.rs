//! `wayhouse-controller --proxy-token/--agent-token` against the real binary (#589): a proxy's
//! token fetches config but cannot write it, and an admin token without the scoped pair is refused.

use std::time::Duration;

use anyhow::{ensure, Result};
use tokio::process::Command;
use wayhouse_fleet_tests::{
    bin_path, build_fleet_bins, free_port, spawn_controller_with, wait_http_up,
};

const ADMIN: &str = "admin-token-0123456789";
const PROXY: &str = "proxy-token-0123456789";
const AGENT: &str = "agent-token-0123456789";

#[tokio::test]
async fn a_proxy_token_reads_config_but_cannot_write_it() -> Result<()> {
    build_fleet_bins()?;
    let dir = tempfile::tempdir()?;
    let port = free_port()?;
    let url = format!("http://127.0.0.1:{port}");
    let _controller = spawn_controller_with(
        dir.path(),
        &format!("127.0.0.1:{port}"),
        &[
            "--auth-token",
            ADMIN,
            "--proxy-token",
            PROXY,
            "--agent-token",
            AGENT,
        ]
        .map(String::from),
    )?;
    wait_http_up(
        &format!("{url}/healthz"),
        wayhouse_fleet_tests::STARTUP_TIMEOUT,
    )
    .await?;
    let c = wayhouse_http::client();
    let status = |req: reqwest::RequestBuilder| async move {
        Ok::<_, anyhow::Error>(req.send().await?.status().as_u16())
    };

    // A fresh controller has no config yet: 404 means the proxy token got past auth.
    ensure!(status(c.get(format!("{url}/config")).bearer_auth(PROXY)).await? != 401);
    ensure!(status(c.post(format!("{url}/config")).bearer_auth(PROXY).body("x")).await? == 401);
    ensure!(status(c.get(format!("{url}/secrets")).bearer_auth(PROXY)).await? == 401);
    ensure!(status(c.get(format!("{url}/config")).bearer_auth(AGENT)).await? == 401);
    ensure!(status(c.get(format!("{url}/config")).bearer_auth(ADMIN)).await? != 401);
    Ok(())
}

#[tokio::test]
async fn an_admin_token_needs_both_scoped_tokens() -> Result<()> {
    build_fleet_bins()?;
    let dir = tempfile::tempdir()?;
    let listen = format!("127.0.0.1:{}", free_port()?);
    let out = tokio::time::timeout(
        Duration::from_secs(30),
        Command::new(bin_path("wayhouse-controller"))
            .args(["--listen", &listen, "--data-dir"])
            .arg(dir.path())
            .args(["--auth-token", ADMIN, "--proxy-token", PROXY])
            .kill_on_drop(true)
            .output(),
    )
    .await??;
    let text = String::from_utf8_lossy(&out.stderr);
    ensure!(
        !out.status.success() && text.contains("--agent-token"),
        "{text}"
    );
    Ok(())
}
