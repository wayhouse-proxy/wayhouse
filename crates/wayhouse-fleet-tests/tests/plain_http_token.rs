//! A bearer token is never sent over plain `http://` beyond loopback unless the operator
//! passes `--allow-insecure-controller`. Checked against the real binaries.

use std::process::{Command, Stdio};

use anyhow::Result;
use wayhouse_fleet_tests::{bin_path, build_fleet_bins, free_port, wait_http_up, STARTUP_TIMEOUT};

const REMOTE: &str = "http://10.255.255.1:9901";

fn token_file(dir: &tempfile::TempDir) -> Result<String> {
    let path = dir.path().join("token");
    std::fs::write(&path, "token-0123456789abcdef\n")?;
    std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
    Ok(path.to_str().unwrap().to_owned())
}

#[test]
fn every_client_binary_refuses_a_token_over_plain_http_to_a_remote_host() -> Result<()> {
    build_fleet_bins()?;
    let dir = tempfile::tempdir()?;
    let tok = token_file(&dir)?;
    let data = dir.path().to_str().unwrap();
    let cases: [(&str, Vec<String>); 6] = [
        (
            "wayhouse",
            vec![
                format!("--controller={REMOTE}"),
                format!("--controller-token-file={tok}"),
            ],
        ),
        (
            "wayhouse-agent",
            vec![
                format!("--controller-url={REMOTE}"),
                format!("--controller-token-file={tok}"),
                "--name=o".into(),
                format!("--data-dir={data}"),
            ],
        ),
        (
            "wayhouse-aggregator",
            vec![
                format!("--parent-url={REMOTE}"),
                format!("--parent-token-file={tok}"),
                "--tier-name=t".into(),
            ],
        ),
        (
            "wayhouse-ui",
            vec![
                format!("--controller-url={REMOTE}"),
                format!("--controller-token-file={tok}"),
            ],
        ),
        (
            "wayhouse-controller",
            vec![
                "--role=slave".into(),
                format!("--parent-url={REMOTE}"),
                format!("--parent-token-file={tok}"),
                format!("--data-dir={data}/slave"),
            ],
        ),
        (
            "wayhouse-controller",
            vec![
                "--ha-node-id=1".into(),
                format!("--ha-peers=1=127.0.0.1:1,2={}", &REMOTE["http://".len()..]),
                format!("--ha-token-file={tok}"),
                format!("--data-dir={data}/ha"),
            ],
        ),
    ];
    for (bin, args) in cases {
        let out = Command::new(bin_path(bin)).args(&args).output()?;
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "{bin} started: {err}");
        assert!(err.contains("--allow-insecure-controller"), "{bin}: {err}");
    }
    Ok(())
}

#[tokio::test]
async fn the_opt_out_lets_the_ui_start() -> Result<()> {
    build_fleet_bins()?;
    let dir = tempfile::tempdir()?;
    let tok = token_file(&dir)?;
    let port = free_port()?;
    let mut ui = Command::new(bin_path("wayhouse-ui"))
        .args([
            &format!("--listen=127.0.0.1:{port}"),
            &format!("--controller-url={REMOTE}"),
            &format!("--controller-token-file={tok}"),
            "--allow-insecure-controller",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let up = wait_http_up(&format!("http://127.0.0.1:{port}/healthz"), STARTUP_TIMEOUT).await;
    let _ = ui.kill();
    let _ = ui.wait();
    up
}
