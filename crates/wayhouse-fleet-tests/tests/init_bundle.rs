//! `wayhouse-controller init` against the real binary: what it writes starts a controller that
//! serves https, a client that trusts only the generated CA reaches it, an enrollment redeem
//! pinned to the fingerprint `init` printed succeeds, and nothing secret is printed.

use std::process::Command;
use std::time::Duration;

use anyhow::{ensure, Result};
use wayhouse_enroll::client::{parse_fingerprint, redeem, Pin};
use wayhouse_enroll::{Code, Role};
use wayhouse_fleet_tests::{
    bin_path, build_fleet_bins, free_port, spawn_controller_with, wait_until,
};

#[tokio::test]
async fn init_makes_a_controller_that_serves_https_and_can_be_pinned() -> Result<()> {
    build_fleet_bins()?;
    let dir = tempfile::tempdir()?;
    let base_dir = dir.path().join("etc");
    let init = |dir: &std::path::Path| {
        Command::new(bin_path("wayhouse-controller"))
            .args(["init", "--target", "systemd", "--dir"])
            .arg(dir)
            .output()
    };
    let out = init(&base_dir)?;
    ensure!(out.status.success(), "init failed: {out:?}");
    let shown = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let creds = base_dir.join("credentials");
    let tls = base_dir.join("tls");
    let read = |p: std::path::PathBuf| std::fs::read_to_string(p);
    for f in [
        "controller-token",
        "proxy-token",
        "agent-token",
        "ui-password",
    ] {
        let value = read(creds.join(f))?;
        ensure!(
            value.trim().len() >= 16 && !shown.contains(value.trim()),
            "{f} leaked"
        );
    }
    let fingerprint = shown
        .split_whitespace()
        .find(|w| w.starts_with("sha256:"))
        .ok_or_else(|| anyhow::anyhow!("no fingerprint in: {shown}"))?;

    // A second run changes nothing and says so.
    let before = read(creds.join("controller-token"))?;
    let again = init(&base_dir)?;
    ensure!(!again.status.success(), "a second init succeeded");
    ensure!(read(creds.join("controller-token"))? == before);

    let key = dir.path().join("secret.key");
    std::fs::write(&key, "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=\n")?;
    std::fs::set_permissions(&key, std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
    std::fs::create_dir_all(dir.path().join("data"))?;
    let port = free_port()?;
    let arg = |s: &str, p: std::path::PathBuf| [s.to_string(), p.display().to_string()];
    let extra: Vec<String> = [
        arg("--tls-cert", tls.join("controller.pem")),
        arg("--tls-key", tls.join("controller.key")),
        arg("--auth-token-file", creds.join("controller-token")),
        arg("--proxy-token-file", creds.join("proxy-token")),
        arg("--agent-token-file", creds.join("agent-token")),
        arg("--secret-key-file", key),
    ]
    .concat();
    let ctl = spawn_controller_with(
        &dir.path().join("data"),
        &format!("127.0.0.1:{port}"),
        &extra,
    )?;
    let ca = reqwest::Certificate::from_pem(&std::fs::read(tls.join("ca.pem"))?)?;
    let trusting = wayhouse_http::builder()
        .add_root_certificate(ca)
        .timeout(Duration::from_secs(10))
        .build()?;
    let base = format!("https://127.0.0.1:{port}");
    wait_until(
        || {
            let (c, b) = (trusting.clone(), base.clone());
            async move { Ok(c.get(format!("{b}/healthz")).send().await.is_ok()) }
        },
        Duration::from_secs(30),
        "the controller to answer over https",
    )
    .await
    .map_err(|e| anyhow::anyhow!("{e}\ncontroller log:\n{}", ctl.log()))?;

    // A client without the CA refuses it, and the admin token works over the trusted channel.
    ensure!(
        wayhouse_http::builder()
            .timeout(Duration::from_secs(10))
            .build()?
            .get(format!("{base}/healthz"))
            .send()
            .await
            .is_err(),
        "the certificate verified without the generated CA"
    );
    let admin = read(creds.join("controller-token"))?;
    let minted: serde_json::Value = trusting
        .post(format!("{base}/enrollments"))
        .bearer_auth(admin.trim())
        .json(&serde_json::json!({ "origin": "home" }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let code = Code::parse(minted["code"].as_str().unwrap())?;

    // The fingerprint `init` printed is the one a pinned redeem needs.
    let pin = Pin::Fingerprint(parse_fingerprint(fingerprint)?);
    let got = redeem(&base, Some(&pin), &code, Role::Agent, None)
        .await
        .map_err(|e| anyhow::anyhow!("pinned redeem failed: {e}"))?;
    ensure!(got.name == "home");
    Ok(())
}
