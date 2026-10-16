//! `wayhouse` -> real Caddy -> origin, with `wayhouse-web-sync` rendering the `web:` section
//! (slices 2 and 3 of the Caddy front-door model, #356/#357).
//!
//! Needs a Caddy binary of the pinned version (pinned in `deploy/Dockerfile`): set `CADDY_BIN` and run
//! `cargo test -p wayhouse-fleet-tests --test web -- --ignored`. The CI job `web-e2e`
//! does; `make check` skips it like the other tests that need outside tools.

use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpSocket, TcpStream};
use tokio::process::{Child, Command};
use wayhouse_fleet_tests::tls_front::{TEST_CA, TEST_LEAF, TEST_LEAF_KEY};
use wayhouse_fleet_tests::{
    build_web_bins, free_port, spawn_wayhouse, wait_until, Proc, WayhouseArgs,
};

/// The address the test connects from, so the real client IP differs from the
/// 127.0.0.1 every hop between the processes uses.
const CLIENT_IP: &str = "127.0.0.2";

struct Caddy {
    _child: Child,
    log: tempfile::TempPath,
}

fn spawn_caddy(bin: &str, dir: &Path, sock: &Path) -> Result<Caddy> {
    let base = dir.join("base.json");
    std::fs::write(
        &base,
        format!(
            r#"{{"admin":{{"listen":"unix/{}","config":{{"persist":false}}}}}}"#,
            sock.display()
        ),
    )?;
    let (file, log) = tempfile::NamedTempFile::new()?.into_parts();
    let file2 = file.try_clone()?;
    let child = Command::new(bin)
        .args(["run", "--config"])
        .arg(&base)
        .env("HOME", dir)
        .env("XDG_CONFIG_HOME", dir)
        .env("XDG_DATA_HOME", dir)
        .stdout(Stdio::from(file))
        .stderr(Stdio::from(file2))
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("spawning {bin}"))?;
    Ok(Caddy { _child: child, log })
}

/// An HTTP/1.1 origin that answers every request with the request headers it saw
/// (`name: value` lines) and, for a WebSocket upgrade, answers 101 and echoes.
async fn origin() -> Result<SocketAddr> {
    let l = TcpListener::bind("127.0.0.1:0").await?;
    let addr = l.local_addr()?;
    tokio::spawn(async move {
        while let Ok((mut s, _)) = l.accept().await {
            tokio::spawn(async move {
                let mut head = Vec::new();
                let mut buf = [0u8; 2048];
                while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                    match s.read(&mut buf).await {
                        Ok(n) if n > 0 => head.extend_from_slice(&buf[..n]),
                        _ => return,
                    }
                }
                let text = String::from_utf8_lossy(&head).to_lowercase();
                if text.contains("upgrade: websocket") {
                    let _ = s
                        .write_all(
                            b"HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\r\n",
                        )
                        .await;
                    while let Ok(n) = s.read(&mut buf).await {
                        if n == 0 || s.write_all(&buf[..n]).await.is_err() {
                            return;
                        }
                    }
                    return;
                }
                let body: String = text
                    .lines()
                    .skip(1)
                    .take_while(|l| !l.is_empty())
                    .map(|l| format!("{l}\n"))
                    .collect();
                let _ = s
                    .write_all(
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        )
                        .as_bytes(),
                    )
                    .await;
            });
        }
    });
    Ok(addr)
}

fn header<'a>(body: &'a str, name: &str) -> Option<&'a str> {
    body.lines()
        .find_map(|l| l.strip_prefix(&format!("{name}: ")))
}

fn client(resolve: Option<(&str, SocketAddr)>) -> Result<reqwest::Client> {
    let mut b = wayhouse_http::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .local_address(CLIENT_IP.parse::<IpAddr>()?)
        .add_root_certificate(reqwest::Certificate::from_pem(&std::fs::read(TEST_CA)?)?)
        .timeout(Duration::from_secs(5));
    if let Some((host, addr)) = resolve {
        b = b.resolve(host, addr);
    }
    Ok(b.build()?)
}

#[tokio::test]
#[ignore = "needs CADDY_BIN (the pinned Caddy); run by the web-e2e CI job"]
async fn requests_reach_the_origin_with_the_real_client_ip() -> Result<()> {
    let caddy_bin = std::env::var("CADDY_BIN").context("set CADDY_BIN to the pinned caddy")?;
    build_web_bins()?;
    let dir = tempfile::tempdir()?;
    let sock = dir.path().join("admin.sock");
    let origin = origin().await?;
    let (admin, pub_http, pub_https, caddy_http, caddy_https, metrics) = (
        free_port()?,
        free_port()?,
        free_port()?,
        free_port()?,
        free_port()?,
        free_port()?,
    );

    let cfg = dir.path().join("wayhouse.yaml");
    let pool = |name: &str, port: u16| {
        format!(
            "  - name: {name}\n    targets: [\"127.0.0.1:{port}\"]\n    proxy_protocol: v2\n    \
             health_check: {{ type: tcp_connect, interval_sec: 1, timeout_ms: 500, rise: 1, fall: 3 }}\n"
        )
    };
    std::fs::write(
        &cfg,
        format!(
            "schema_version: 3\nsettings:\n  workers: 1\n  shutdown_grace_sec: 1\n  admin:\n    \
             listen: \"127.0.0.1:{admin}\"\npools:\n{}{}listeners:\n  - name: https-in\n    \
             bind: \"127.0.0.1:{pub_https}\"\n    protocol: tcp\n    pool: web\n  - name: http-in\n    \
             bind: \"127.0.0.1:{pub_http}\"\n    protocol: tcp\n    pool: web-http\nweb:\n  \
             listen: {{ http: \"127.0.0.1:{caddy_http}\", https: \"127.0.0.1:{caddy_https}\" }}\n  \
             sites:\n    - hosts: [localhost]\n      upstreams: [{{ addr: \"{origin}\" }}]\n      \
             tls: {{ cert: \"{TEST_LEAF}\", key: \"{TEST_LEAF_KEY}\" }}\n    - hosts: [plain.test]\n      \
             upstreams: [{{ addr: \"{origin}\" }}]\n",
            pool("web", caddy_https),
            pool("web-http", caddy_http),
        ),
    )?;

    let _caddy = spawn_caddy(&caddy_bin, dir.path(), &sock)?;
    wait_until(
        || async { Ok(sock.exists()) },
        Duration::from_secs(20),
        "caddy admin socket",
    )
    .await?;
    let _wayhouse = spawn_wayhouse(WayhouseArgs {
        config_path: Some(cfg),
        ..Default::default()
    })?;
    let _sync = Proc::spawn_in(
        None,
        "wayhouse-web-sync",
        &[
            "--wayhouse-admin".into(),
            format!("http://127.0.0.1:{admin}"),
            "--caddy-admin".into(),
            sock.display().to_string(),
            "--interval-sec".into(),
            "1".into(),
            "--metrics-listen".into(),
            format!("127.0.0.1:{metrics}"),
        ],
    )?;

    let http_addr: SocketAddr = format!("127.0.0.1:{pub_http}").parse()?;
    let https_addr: SocketAddr = format!("127.0.0.1:{pub_https}").parse()?;
    let plain = client(None)?;

    // Plain HTTP site: the origin sees the real client IP, and client-supplied
    // forwarding headers are replaced, not trusted.
    let url = format!("http://plain.test:{pub_http}/");
    let c = client(Some(("plain.test", http_addr)))?;
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    let body = loop {
        let sent = c
            .get(&url)
            .header("X-Forwarded-For", "1.2.3.4")
            .header("X-Real-IP", "9.9.9.9")
            .send()
            .await;
        if let Ok(r) = sent {
            if r.status().is_success() {
                break r.text().await?;
            }
        }
        anyhow::ensure!(
            std::time::Instant::now() < deadline,
            "the plain site never answered through wayhouse and Caddy\ncaddy: {}\nwayhouse: {}\nsync: {}",
            std::fs::read_to_string(&_caddy.log).unwrap_or_default(),
            _wayhouse.log(),
            _sync.log()
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    };
    assert_eq!(header(&body, "x-forwarded-for"), Some(CLIENT_IP), "{body}");
    assert_eq!(header(&body, "x-real-ip"), Some(CLIENT_IP), "{body}");
    drop(plain);

    // HTTPS site with the operator's certificate.
    let c = client(Some(("localhost", https_addr)))?;
    let r = c
        .get(format!("https://localhost:{pub_https}/"))
        .header("X-Forwarded-For", "1.2.3.4")
        .send()
        .await?;
    assert!(r.status().is_success(), "{}", r.status());
    let body = r.text().await?;
    assert_eq!(header(&body, "x-forwarded-for"), Some(CLIENT_IP), "{body}");

    // The same host on plain HTTP redirects to https.
    let c = client(Some(("localhost", http_addr)))?;
    let r = c
        .get(format!("http://localhost:{pub_http}/x?y=1"))
        .send()
        .await?;
    assert_eq!(r.status(), 308);
    assert_eq!(
        r.headers()["location"],
        "https://localhost/x?y=1",
        "{:?}",
        r.headers()
    );

    // WebSocket upgrade through wayhouse and Caddy.
    let mut ws = TcpStream::connect(http_addr).await?;
    ws.write_all(
        b"GET /ws HTTP/1.1\r\nHost: plain.test\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\
          Sec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n",
    )
    .await?;
    let mut buf = [0u8; 512];
    let n = tokio::time::timeout(Duration::from_secs(5), ws.read(&mut buf)).await??;
    assert!(
        String::from_utf8_lossy(&buf[..n]).starts_with("HTTP/1.1 101"),
        "{}",
        String::from_utf8_lossy(&buf[..n])
    );
    ws.write_all(b"ping").await?;
    let n = tokio::time::timeout(Duration::from_secs(5), ws.read(&mut buf)).await??;
    assert_eq!(&buf[..n], b"ping");

    // A PROXY header from a source outside `allow` drops the connection (REJECT), so
    // the address it claims is never used.
    let sock = TcpSocket::new_v4()?;
    sock.bind("127.0.0.2:0".parse()?)?;
    let mut spoof = sock
        .connect(format!("127.0.0.1:{caddy_http}").parse()?)
        .await?;
    let _ = spoof
        .write_all(
            b"PROXY TCP4 1.2.3.4 127.0.0.1 1111 80\r\nGET / HTTP/1.1\r\nHost: plain.test\r\n\r\n",
        )
        .await;
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), spoof.read_to_end(&mut out)).await;
    assert!(
        out.is_empty(),
        "got a response: {}",
        String::from_utf8_lossy(&out)
    );

    // The sync reports itself.
    let m = wayhouse_http::builder()
        .no_proxy()
        .build()?
        .get(format!("http://127.0.0.1:{metrics}/metrics"))
        .send()
        .await?
        .text()
        .await?;
    assert!(m.contains("wayhouse_web_sync_last_ok"), "{m}");
    Ok(())
}
