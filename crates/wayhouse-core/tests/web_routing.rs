//! The `web:` front door on the data path (spec
//! `docs/superpowers/specs/2026-10-15-web-routing-design.md`): `port` and `sni` routes send
//! web traffic to the `web` pool with a PROXY v2 header, and a `via: wayhouse` upstream is an
//! ordinary loopback listener in front of a pool.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use wayhouse_config::{parse_str, WebUpstream};
use wayhouse_core::{Runtime, Snapshot};

static HANDED_OUT: std::sync::Mutex<std::collections::BTreeSet<u16>> =
    std::sync::Mutex::new(std::collections::BTreeSet::new());

/// A port on 127.0.0.1 that nothing listens on and no other test here was given.
fn free_port() -> SocketAddr {
    loop {
        let probe = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = probe.local_addr().unwrap();
        let fresh = HANDED_OUT
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(addr.port());
        if fresh {
            return addr;
        }
    }
}

/// An echo server; the proxy's health probes connect and close, which is fine.
async fn echo_backend() -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = l.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                while let Ok(n) = s.read(&mut buf).await {
                    if n == 0 || s.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                }
            });
        }
    });
    addr
}

const V2_SIGNATURE: [u8; 12] = *b"\r\n\r\n\0\r\nQUIT\n";

/// Stand-in for Caddy's PROXY listener: reads the 28-byte v2 header of a TCP/IPv4
/// connection, reports it, then echoes. Health probes (no bytes) are skipped.
async fn fake_caddy() -> (SocketAddr, tokio::sync::mpsc::UnboundedReceiver<[u8; 28]>) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = l.accept().await {
            let tx = tx.clone();
            tokio::spawn(async move {
                let mut hdr = [0u8; 28];
                if s.read_exact(&mut hdr).await.is_err() {
                    return;
                }
                let _ = tx.send(hdr);
                let mut buf = [0u8; 4096];
                while let Ok(n) = s.read(&mut buf).await {
                    if n == 0 || s.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                }
            });
        }
    });
    (addr, rx)
}

/// A minimal TLS ClientHello record carrying `sni`.
fn client_hello(sni: &str) -> Vec<u8> {
    let mut sn = Vec::new();
    sn.extend_from_slice(&((sni.len() + 3) as u16).to_be_bytes());
    sn.push(0x00);
    sn.extend_from_slice(&(sni.len() as u16).to_be_bytes());
    sn.extend_from_slice(sni.as_bytes());
    let mut ext = Vec::new();
    ext.extend_from_slice(&0u16.to_be_bytes());
    ext.extend_from_slice(&(sn.len() as u16).to_be_bytes());
    ext.extend_from_slice(&sn);
    let mut body = Vec::new();
    body.extend_from_slice(&[0x03, 0x03]);
    body.extend_from_slice(&[0u8; 32]);
    body.push(0x00);
    body.extend_from_slice(&2u16.to_be_bytes());
    body.extend_from_slice(&[0x00, 0x2f]);
    body.push(0x01);
    body.push(0x00);
    body.extend_from_slice(&(ext.len() as u16).to_be_bytes());
    body.extend_from_slice(&ext);
    let bl = body.len();
    let mut hs = vec![0x01, (bl >> 16) as u8, (bl >> 8) as u8, bl as u8];
    hs.extend_from_slice(&body);
    let mut rec = vec![0x16, 0x03, 0x01];
    rec.extend_from_slice(&(hs.len() as u16).to_be_bytes());
    rec.extend_from_slice(&hs);
    rec
}

fn start(yaml: &str) -> (wayhouse_config::Config, Runtime) {
    let cfg = parse_str(yaml).unwrap();
    let runtime = Runtime::start(Snapshot::from_config(&cfg), Arc::default(), 1);
    (cfg, runtime)
}

/// The header says: v2 PROXY, TCP over IPv4, and this client address and destination port.
fn assert_header(hdr: &[u8; 28], client: SocketAddr, dst_port: u16) {
    assert_eq!(hdr[..12], V2_SIGNATURE);
    assert_eq!(hdr[12], 0x21, "version 2, command PROXY");
    assert_eq!(hdr[13], 0x11, "TCP over IPv4");
    assert_eq!(u16::from_be_bytes([hdr[14], hdr[15]]), 12);
    assert_eq!(hdr[16..20], [127, 0, 0, 1]);
    assert_eq!(u16::from_be_bytes([hdr[24], hdr[25]]), client.port());
    assert_eq!(u16::from_be_bytes([hdr[26], hdr[27]]), dst_port);
}

#[tokio::test]
async fn a_port_route_sends_the_web_pool_a_proxy_v2_header_with_the_client_address() {
    let (caddy, mut seen) = fake_caddy().await;
    let entry = free_port();
    let (_cfg, runtime) = start(&format!(
        r#"
schema_version: 3
pools:
  - name: web
    targets: ["{caddy}"]
    proxy_protocol: v2
listeners:
  - name: http
    bind: "{entry}"
    routes:
      - match: {{ type: port, ports: [{}] }}
        action: {{ pool: web }}
web:
  listen: {{ http: "{caddy}", https: "{caddy}" }}
  sites:
    - hosts: [panel.example.com]
      upstreams: [{{ addr: "10.0.0.5:8080" }}]
"#,
        entry.port()
    ));
    tokio::time::sleep(Duration::from_millis(150)).await;

    let mut c = TcpStream::connect(entry).await.unwrap();
    let client = c.local_addr().unwrap();
    c.write_all(b"GET / HTTP/1.1\r\n").await.unwrap();
    let mut echo = [0u8; 16];
    c.read_exact(&mut echo).await.unwrap();
    assert_eq!(&echo, b"GET / HTTP/1.1\r\n");
    let hdr = tokio::time::timeout(Duration::from_secs(2), seen.recv())
        .await
        .unwrap()
        .unwrap();
    assert_header(&hdr, client, entry.port());
    runtime
        .shutdown_with_grace(Duration::from_millis(100))
        .await;
}

#[tokio::test]
async fn an_sni_route_sends_the_web_pool_a_proxy_v2_header_and_the_untouched_hello() {
    let (caddy, mut seen) = fake_caddy().await;
    let entry = free_port();
    let (_cfg, runtime) = start(&format!(
        r#"
pools:
  - name: web
    targets: ["{caddy}"]
    proxy_protocol: v2
  - name: game
    targets: ["127.0.0.1:9"]
listeners:
  - name: shared
    bind: "{entry}"
    routes:
      - match: {{ type: sni, host: ["*.example.com"] }}
        action: {{ pool: web }}
      - match: {{ type: always }}
        action: {{ pool: game }}
"#
    ));
    tokio::time::sleep(Duration::from_millis(150)).await;

    let hello = client_hello("panel.example.com");
    let mut c = TcpStream::connect(entry).await.unwrap();
    let client = c.local_addr().unwrap();
    c.write_all(&hello).await.unwrap();
    let mut echo = vec![0u8; hello.len()];
    c.read_exact(&mut echo).await.unwrap();
    assert_eq!(echo, hello, "Caddy gets the ClientHello byte for byte");
    let hdr = tokio::time::timeout(Duration::from_secs(2), seen.recv())
        .await
        .unwrap()
        .unwrap();
    assert_header(&hdr, client, entry.port());
    runtime
        .shutdown_with_grace(Duration::from_millis(100))
        .await;
}

#[tokio::test]
async fn a_via_wayhouse_upstream_reaches_the_origin_through_its_pool() {
    let origin = echo_backend().await;
    let up = free_port();
    let (cfg, runtime) = start(&format!(
        r#"
schema_version: 3
pools:
  - name: web
    targets: ["127.0.0.1:8443"]
    proxy_protocol: v2
  - name: wings1
    targets: ["{origin}"]
listeners:
  - name: https
    bind: "{}"
    pool: web
web:
  internal_ports: "{p}-{p}"
  sites:
    - hosts: [wings1.example.com]
      upstreams: [{{ pool: wings1, via: wayhouse }}]
"#,
        free_port(),
        p = up.port()
    ));
    tokio::time::sleep(Duration::from_millis(150)).await;

    // What the renderer would hand Caddy as the upstream.
    let WebUpstream::Local { addr, .. } = &cfg.web.as_ref().unwrap().sites[0].upstreams[0] else {
        panic!("expected a local upstream");
    };
    assert_eq!(*addr, up);

    let mut c = TcpStream::connect(addr).await.unwrap();
    c.write_all(b"GET /console HTTP/1.1\r\n").await.unwrap();
    let mut echo = [0u8; 23];
    c.read_exact(&mut echo).await.unwrap();
    assert_eq!(
        &echo, b"GET /console HTTP/1.1\r\n",
        "the origin sees no PROXY header"
    );
    runtime
        .shutdown_with_grace(Duration::from_millis(100))
        .await;
}

#[tokio::test]
async fn a_via_wayhouse_upstream_fails_over_to_a_live_backend() {
    let alive = echo_backend().await;
    let dead = free_port();
    let up = free_port();
    let (cfg, runtime) = start(&format!(
        r#"
schema_version: 3
pools:
  - name: web
    targets: ["127.0.0.1:8443"]
    proxy_protocol: v2
  - name: wings1
    targets: ["{dead}", "{alive}"]
    balancer: round_robin
    health_check: {{ interval_sec: 1, timeout_ms: 200, rise: 1, fall: 1 }}
listeners:
  - name: https
    bind: "{}"
    pool: web
web:
  internal_ports: "{p}-{p}"
  sites:
    - hosts: [wings1.example.com]
      upstreams: [{{ pool: wings1, via: wayhouse }}]
"#,
        free_port(),
        p = up.port()
    ));
    tokio::time::sleep(Duration::from_millis(150)).await;
    let WebUpstream::Local { addr, .. } = &cfg.web.as_ref().unwrap().sites[0].upstreams[0] else {
        panic!("expected a local upstream");
    };

    let mut successes = 0;
    for _ in 0..10 {
        if let Ok(mut c) = TcpStream::connect(addr).await {
            if c.write_all(b"x").await.is_ok() {
                let mut b = [0u8; 1];
                if tokio::time::timeout(Duration::from_millis(300), c.read_exact(&mut b))
                    .await
                    .is_ok_and(|r| r.is_ok())
                {
                    successes += 1;
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        successes >= 8,
        "expected most requests to succeed, got {successes}/10"
    );
    runtime
        .shutdown_with_grace(Duration::from_millis(100))
        .await;
}
