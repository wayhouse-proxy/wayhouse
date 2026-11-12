//! Enrollment over native TLS (slice 1): the redeem client sends the code only to a controller
//! whose certificate matches the pin, and a failed pin check leaves the code unspent.

use std::time::Duration;

use anyhow::{ensure, Result};
use sha2::{Digest, Sha256};
use wayhouse_enroll::client::{redeem, Pin};
use wayhouse_enroll::{Code, Role};
use wayhouse_fleet_tests::tls_front::{TEST_CA, TEST_LEAF, TEST_LEAF_KEY};
use wayhouse_fleet_tests::{build_fleet_bins, free_port, spawn_controller_with, wait_until};

fn leaf_fingerprint() -> Result<[u8; 32]> {
    use base64::Engine;
    // The first certificate in the PEM is the leaf the server presents.
    let pem = std::fs::read_to_string(TEST_LEAF)?;
    let body: String = pem
        .lines()
        .skip_while(|l| !l.starts_with("-----BEGIN CERTIFICATE"))
        .skip(1)
        .take_while(|l| !l.starts_with("-----END"))
        .collect();
    let der = base64::engine::general_purpose::STANDARD.decode(body)?;
    Ok(Sha256::digest(der).into())
}

#[tokio::test]
async fn the_code_goes_only_to_a_controller_matching_the_pin() -> Result<()> {
    build_fleet_bins()?;
    let dir = tempfile::tempdir()?;
    let key = dir.path().join("secret.key");
    std::fs::write(&key, "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=\n")?;
    std::fs::set_permissions(&key, std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
    std::fs::create_dir_all(dir.path().join("data"))?;
    let port = free_port()?;
    let ctl = spawn_controller_with(
        &dir.path().join("data"),
        &format!("127.0.0.1:{port}"),
        &[
            "--tls-cert",
            TEST_LEAF,
            "--tls-key",
            TEST_LEAF_KEY,
            "--secret-key-file",
            key.to_str().unwrap(),
        ]
        .map(String::from),
    )?;
    let ca = reqwest::Certificate::from_pem(&std::fs::read(TEST_CA)?)?;
    let admin = wayhouse_http::builder()
        .add_root_certificate(ca)
        .timeout(Duration::from_secs(10))
        .build()?;
    let base = format!("https://localhost:{port}");
    wait_until(
        || {
            let (c, b) = (admin.clone(), base.clone());
            async move { Ok(c.get(format!("{b}/healthz")).send().await.is_ok()) }
        },
        Duration::from_secs(30),
        "the controller to answer over HTTPS",
    )
    .await
    .map_err(|e| anyhow::anyhow!("{e}\ncontroller log:\n{}", ctl.log()))?;
    let mint = |origin: &'static str| {
        let (c, b) = (admin.clone(), base.clone());
        async move {
            let v: serde_json::Value = c
                .post(format!("{b}/enrollments"))
                .json(&serde_json::json!({ "origin": origin }))
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            anyhow::Ok(Code::parse(v["code"].as_str().unwrap())?)
        }
    };

    // A wrong fingerprint aborts the handshake: the code is never sent, never spent.
    let code = mint("home").await?;
    let wrong = Pin::Fingerprint([0u8; 32]);
    ensure!(redeem(&base, Some(&wrong), &code, Role::Agent)
        .await
        .is_err());
    let list: serde_json::Value = admin
        .get(format!("{base}/enrollments"))
        .send()
        .await?
        .json()
        .await?;
    ensure!(
        list["items"][0]["status"] == "pending",
        "a failed pin check spent the code: {list}"
    );

    // The right fingerprint redeems it, once.
    let pin = Pin::Fingerprint(leaf_fingerprint()?);
    let got = redeem(&base, Some(&pin), &code, Role::Agent)
        .await
        .ok()
        .ok_or_else(|| anyhow::anyhow!("pinned redeem failed"))?;
    ensure!(got.name == "home" && !got.tunnel_psk.is_empty());
    ensure!(
        redeem(&base, Some(&pin), &code, Role::Agent).await.is_err(),
        "a spent code redeemed twice"
    );

    // A CA file works instead of a fingerprint.
    let second = mint("away").await?;
    let by_ca = redeem(
        &base,
        Some(&Pin::CaFile(TEST_CA.into())),
        &second,
        Role::Agent,
    )
    .await;
    ensure!(
        by_ca.is_ok_and(|r| r.name == "away"),
        "redeem with --controller-ca failed"
    );
    Ok(())
}
