//! Web front door, data path: the entry route to the `web` pool (PROXY v2) and
//! the generated `via: wayhouse` loopback listener (spec
//! `docs/superpowers/specs/2026-10-15-web-routing-design.md`).

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use wayhouse_config::{parse_str, WebUpstream};
use wayhouse_core::pool::AdminState;
use wayhouse_core::{Runtime, Snapshot};

/// Longer than the 12-byte PROXY signature, so the backend can tell the two apart.
const REQUEST: &[u8] = b"GET / HTTP/1.1\r\n";

const V2_SIGNATURE: [u8; 12] = *b"\r\n\r\n\0\r\nQUIT\n";

/// A TLS ClientHello carrying `sni` (same record as `tcp_forward.rs`).
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

fn free_addr() -> SocketAddr {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
}

/// What a backend saw at the start of one connection.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Seen {
    /// Source and destination port of a PROXY v2 TCP/IPv4 header, if one came first.
    proxy_v2: Option<(SocketAddr, SocketAddr)>,
    /// The bytes after the header (or from the start when there was none).
    rest: Vec<u8>,
}

/// Parses an optional PROXY v2 header (TCP over IPv4) off `s`, then reads `want`
/// payload bytes.
async fn read_seen(s: &mut TcpStream, want: usize) -> Seen {
    // A probe may send fewer than 12 bytes (or none): keep what arrived as payload.
    let mut first = Vec::with_capacity(12);
    while first.len() < 12 {
        let mut b = [0u8; 12];
        let n = s.read(&mut b[..12 - first.len()]).await.unwrap_or(0);
        if n == 0 {
            break;
        }
        first.extend_from_slice(&b[..n]);
    }
    let mut rest = Vec::new();
    let mut proxy_v2 = None;
    if first == V2_SIGNATURE {
        let mut hdr = [0u8; 4];
        s.read_exact(&mut hdr).await.unwrap();
        assert_eq!(hdr[0], 0x21, "version 2, command PROXY");
        assert_eq!(hdr[1], 0x11, "TCP over IPv4");
        let len = u16::from_be_bytes([hdr[2], hdr[3]]) as usize;
        assert_eq!(len, 12, "IPv4 address block");
        let mut a = [0u8; 12];
        s.read_exact(&mut a).await.unwrap();
        let src = SocketAddr::from(([a[0], a[1], a[2], a[3]], u16::from_be_bytes([a[8], a[9]])));
        let dst = SocketAddr::from(([a[4], a[5], a[6], a[7]], u16::from_be_bytes([a[10], a[11]])));
        proxy_v2 = Some((src, dst));
    } else {
        rest.extend_from_slice(&first);
    }
    while rest.len() < want {
        let mut b = vec![0u8; want - rest.len()];
        let n = s.read(&mut b).await.unwrap();
        if n == 0 {
            break;
        }
        rest.extend_from_slice(&b[..n]);
    }
    rest.truncate(want);
    Seen { proxy_v2, rest }
}

/// Backend that records what each connection started with (after `want` payload
/// bytes) and then echoes them. Health probes that send nothing are ignored.
async fn recording_backend(want: usize) -> (SocketAddr, Arc<Mutex<Vec<Seen>>>) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let log = Arc::new(Mutex::new(Vec::new()));
    let out = log.clone();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = l.accept().await {
            let out = out.clone();
            tokio::spawn(async move {
                let seen = read_seen(&mut s, want).await;
                if seen.rest.len() == want {
                    let _ = s.write_all(&seen.rest).await;
                    out.lock().unwrap().push(seen);
                }
                let mut sink = [0u8; 64];
                while s.read(&mut sink).await.is_ok_and(|n| n > 0) {}
            });
        }
    });
    (addr, log)
}

/// Backend that answers every connection with one identifying byte.
async fn marker_backend(mark: u8) -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = l.accept().await {
            tokio::spawn(async move {
                let _ = s.write_all(&[mark]).await;
                let mut sink = [0u8; 64];
                while s.read(&mut sink).await.is_ok_and(|n| n > 0) {}
            });
        }
    });
    addr
}

async fn start(yaml: &str) -> (wayhouse_config::Config, Runtime) {
    let cfg = parse_str(yaml).unwrap();
    let rt = Runtime::start(Snapshot::from_config(&cfg), Arc::default(), 1);
    (cfg, rt)
}

async fn connect_retry(addr: SocketAddr) -> TcpStream {
    for _ in 0..200 {
        if let Ok(s) = TcpStream::connect(addr).await {
            return s;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("nothing listening on {addr}");
}

async fn stop(rt: Runtime) {
    rt.shutdown_with_grace(Duration::from_millis(100)).await;
}

#[tokio::test]
async fn entry_routes_send_a_proxy_v2_header_and_the_client_hello_intact() {
    let hello = client_hello("panel.example.com");
    let (caddy, log) = recording_backend(hello.len()).await;
    let port_route = free_addr();
    let sni_route = free_addr();
    let yaml = format!(
        r#"
schema_version: 3
pools:
  - name: web
    targets: ["{caddy}"]
    proxy_protocol: v2
listeners:
  - name: by-port
    bind: "{port_route}"
    routes:
      - match: {{ type: port, ports: ["{pp}-{pp}"] }}
        action: {{ pool: web }}
  - name: by-sni
    bind: "{sni_route}"
    routes:
      - match: {{ type: sni, host: ["*.example.com"] }}
        action: {{ pool: web }}
web:
  sites:
    - hosts: [panel.example.com]
      upstreams:
        - addr: 10.0.0.9:8080
"#,
        pp = port_route.port()
    );
    let (_cfg, rt) = start(&yaml).await;

    for (entry, label) in [(port_route, "port"), (sni_route, "sni")] {
        let mut c = connect_retry(entry).await;
        let client = c.local_addr().unwrap();
        c.write_all(&hello).await.unwrap();
        let mut echoed = vec![0u8; hello.len()];
        tokio::time::timeout(Duration::from_secs(5), c.read_exact(&mut echoed))
            .await
            .unwrap_or_else(|_| panic!("{label} route: no reply"))
            .unwrap();
        assert_eq!(echoed, hello, "{label} route");

        let seen = log
            .lock()
            .unwrap()
            .iter()
            .find(|s| s.proxy_v2.is_some_and(|(src, _)| src == client))
            .cloned()
            .unwrap_or_else(|| panic!("{label} route: no PROXY v2 header for {client}"));
        let (src, dst) = seen.proxy_v2.unwrap();
        assert_eq!(src, client, "{label}: client source");
        assert_eq!(
            dst, entry,
            "{label}: destination is the entry address and port"
        );
        assert_eq!(
            seen.rest, hello,
            "{label}: ClientHello follows byte for byte"
        );
    }
    stop(rt).await;
}

/// The loopback address the validator generated for `pool`.
fn local_of(cfg: &wayhouse_config::Config, pool: &str) -> SocketAddr {
    cfg.web.as_ref().unwrap().sites[0]
        .upstreams
        .iter()
        .find_map(|u| match u {
            WebUpstream::Local { pool: p, addr } if p == pool => Some(*addr),
            _ => None,
        })
        .unwrap()
}

#[tokio::test]
async fn via_wayhouse_reaches_the_origin_through_its_pool_without_a_proxy_header() {
    let (origin, log) = recording_backend(REQUEST.len()).await;
    let entry = free_addr();
    let pin = free_addr().port();
    let yaml = format!(
        r#"
schema_version: 3
pools:
  - name: web
    targets: ["127.0.0.1:1"]
    proxy_protocol: v2
  - name: origin
    targets: ["{origin}"]
listeners:
  - name: https
    bind: "{entry}"
    pool: web
web:
  internal_ports: "{pin}-{pin}"
  sites:
    - hosts: [panel.example.com]
      upstreams:
        - {{ pool: origin, via: wayhouse }}
"#
    );
    let (cfg, rt) = start(&yaml).await;
    let local = local_of(&cfg, "origin");
    assert_eq!(local.port(), pin);

    // What Caddy does: connect to the generated listener and speak plain HTTP.
    let mut c = connect_retry(local).await;
    c.write_all(REQUEST).await.unwrap();
    let mut got = [0u8; REQUEST.len()];
    tokio::time::timeout(Duration::from_secs(5), c.read_exact(&mut got))
        .await
        .expect("origin reply")
        .unwrap();
    assert_eq!(&got[..], REQUEST);
    let seen = log.lock().unwrap().clone();
    assert_eq!(seen.len(), 1);
    assert_eq!(
        seen[0].proxy_v2, None,
        "the origin must see no PROXY header"
    );
    stop(rt).await;
}

#[tokio::test]
async fn via_wayhouse_fails_over_and_honours_drains() {
    let dead = free_addr();
    let a = marker_backend(b'A').await;
    let b = marker_backend(b'B').await;
    let entry = free_addr();
    let yaml = format!(
        r#"
schema_version: 3
pools:
  - name: web
    targets: ["127.0.0.1:1"]
    proxy_protocol: v2
  - name: origin
    targets: ["{dead}", "{a}", "{b}"]
    balancer: round_robin
    health_check: {{ interval_sec: 1, timeout_ms: 200, rise: 1, fall: 1 }}
listeners:
  - name: https
    bind: "{entry}"
    pool: web
web:
  sites:
    - hosts: [panel.example.com]
      upstreams:
        - {{ pool: origin, via: wayhouse }}
"#
    );
    let (cfg, rt) = start(&yaml).await;
    let local = local_of(&cfg, "origin");

    let mark = |addr: SocketAddr| async move {
        let mut c = TcpStream::connect(addr).await.ok()?;
        let mut m = [0u8; 1];
        tokio::time::timeout(Duration::from_millis(500), c.read_exact(&mut m))
            .await
            .ok()?
            .ok()?;
        Some(m[0])
    };

    // First backend is dead: most connections still succeed.
    connect_retry(local).await;
    let mut ok = 0;
    for _ in 0..12 {
        if mark(local).await.is_some() {
            ok += 1;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    assert!(
        ok >= 10,
        "expected most connections to succeed, got {ok}/12"
    );

    // Drain A: new connections go to B only.
    rt.handle()
        .snapshot()
        .pool("origin")
        .unwrap()
        .backend(a)
        .unwrap()
        .set_admin_state(AdminState::Draining);
    for _ in 0..8 {
        assert_eq!(
            mark(local).await,
            Some(b'B'),
            "a drained backend takes no new connections"
        );
    }
    stop(rt).await;
}
