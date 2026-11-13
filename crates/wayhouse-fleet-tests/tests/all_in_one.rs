//! `wayhouse-control`: controller, aggregator and UI in one process (single node). The UI must
//! reach the other two over loopback with their own tokens, plain and over TLS.

use std::time::Duration;

use anyhow::{ensure, Context, Result};
use wayhouse_fleet_tests::tls_front::{TEST_CA, TEST_LEAF, TEST_LEAF_KEY};
use wayhouse_fleet_tests::{build_fleet_bins, free_port, wait_until, Proc, STARTUP_TIMEOUT};

const PASSWORD: &str = "ui-login-0123456789";

/// Writes `value` to a file in `dir` and returns its path.
fn file(dir: &std::path::Path, name: &str, value: &str) -> Result<String> {
    let p = dir.join(name);
    std::fs::write(&p, value)?;
    Ok(p.display().to_string())
}

async fn all_in_one_serves_and_wires_the_ui(tls: bool) -> Result<()> {
    build_fleet_bins()?;
    let dir = tempfile::tempdir()?;
    let d = dir.path();
    let (c, a, u) = (free_port()?, free_port()?, free_port()?);
    let mut args: Vec<String> = [
        (
            "--controller-data-dir",
            d.join("data").display().to_string(),
        ),
        ("--controller-listen", format!("127.0.0.1:{c}")),
        (
            "--controller-auth-token-file",
            file(d, "c-admin", "controller-admin-0123456789")?,
        ),
        (
            "--controller-proxy-token-file",
            file(d, "c-proxy", "controller-proxy-0123456789")?,
        ),
        (
            "--controller-agent-token-file",
            file(d, "c-agent", "controller-agent-0123456789")?,
        ),
        ("--aggregator-listen", format!("127.0.0.1:{a}")),
        (
            "--aggregator-auth-token-file",
            file(d, "a-admin", "aggregator-admin-0123456789")?,
        ),
        (
            "--aggregator-ingest-token-file",
            file(d, "a-ingest", "aggregator-ingest-0123456789")?,
        ),
        ("--ui-listen", format!("127.0.0.1:{u}")),
        ("--ui-ui-password-file", file(d, "ui-pw", PASSWORD)?),
    ]
    .into_iter()
    .flat_map(|(k, v)| [k.to_string(), v])
    .collect();
    if tls {
        args.extend(
            [
                "--tls-cert",
                TEST_LEAF,
                "--tls-key",
                TEST_LEAF_KEY,
                "--ui-tls-cert",
                TEST_LEAF,
                "--ui-tls-key",
                TEST_LEAF_KEY,
                "--ca-file",
                TEST_CA,
            ]
            .map(String::from),
        );
    }
    let control = Proc::spawn_in(None, "wayhouse-control", &args)?;

    let mut builder = wayhouse_http::builder().timeout(Duration::from_secs(10));
    if tls {
        builder =
            builder.add_root_certificate(reqwest::Certificate::from_pem(&std::fs::read(TEST_CA)?)?);
    }
    let client = builder.build()?;
    let scheme = if tls { "https" } else { "http" };
    for port in [c, a, u] {
        // Not `wait_http_up`: its client does not trust the test CA.
        wait_until(
            || {
                let (client, url) = (
                    client.clone(),
                    format!("{scheme}://localhost:{port}/healthz"),
                );
                async move { Ok(client.get(url).send().await.is_ok()) }
            },
            STARTUP_TIMEOUT,
            &format!("port {port} to answer\n{}", control.log()),
        )
        .await?;
    }

    let base = format!("{scheme}://localhost:{u}");
    let login = client
        .post(format!("{base}/ui/login"))
        .json(&serde_json::json!({ "password": PASSWORD }))
        .send()
        .await?
        .error_for_status()?;
    let cookie = login
        .headers()
        .get(reqwest::header::SET_COOKIE)
        .context("login set no cookie")?
        .to_str()?
        .split(';')
        .next()
        .unwrap_or_default()
        .to_string();
    // Each answer needs the UI to have reached its program with that program's own token.
    let mut pools = 0;
    for _ in 0..50 {
        pools = client
            .get(format!("{base}/api/fleet/pools"))
            .header(reqwest::header::COOKIE, &cookie)
            .send()
            .await?
            .status()
            .as_u16();
        if pools == 200 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    ensure!(
        pools == 200,
        "/api/fleet/pools (aggregator): {pools}\n{}",
        control.log()
    );
    let config = client
        .get(format!("{base}/api/config"))
        .header(reqwest::header::COOKIE, &cookie)
        .send()
        .await?
        .status()
        .as_u16();
    // A fresh controller has no config yet; 401/502/503 would mean the UI could not use it.
    ensure!(
        config == 200 || config == 404,
        "/api/config (controller): {config}\n{}",
        control.log()
    );
    Ok(())
}

#[tokio::test]
async fn the_ui_reaches_the_other_two_over_plain_loopback() -> Result<()> {
    all_in_one_serves_and_wires_the_ui(false).await
}

#[tokio::test]
async fn the_ui_reaches_the_other_two_over_tls() -> Result<()> {
    all_in_one_serves_and_wires_the_ui(true).await
}
