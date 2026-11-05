//! The data-plane feature flags (`wayhouse_core::flags`) against real listeners: a flag that is
//! off switches its filter off for new connections, and on again restores it. The flags are
//! process-wide, so these tests take turns.

mod common;

use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};

use wayhouse_config::parse_str;
use wayhouse_core::{flags, Runtime, Snapshot};

static TURN: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn set(off: &[&str]) {
    flags::set_all(|name| !off.contains(&name));
}

async fn tcp_echo() -> std::net::SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = l.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 64];
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

/// Whether a connection to `proxy` gets its bytes echoed (true) or is dropped (false).
async fn echoed(proxy: std::net::SocketAddr) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut c = loop {
        match TcpStream::connect(proxy).await {
            Ok(c) => break c,
            Err(e) if std::time::Instant::now() > deadline => panic!("no listener: {e}"),
            Err(_) => tokio::time::sleep(Duration::from_millis(10)).await,
        }
    };
    let _ = c.write_all(b"hi").await;
    let mut buf = [0u8; 2];
    matches!(
        tokio::time::timeout(Duration::from_secs(2), c.read_exact(&mut buf)).await,
        Ok(Ok(_))
    )
}

#[tokio::test]
async fn acl_geo_and_limits_follow_their_flags_on_tcp() {
    let _t = TURN.lock().await;
    let backend = tcp_echo().await;
    let proxy = common::free_tcp_addr();
    let yaml = format!(
        "settings:\n  geo_db: \"{}/tests/data/GeoIP2-Country-Test.mmdb\"\n\
         pools:\n  - name: p\n    targets: [\"{backend}\"]\n\
         listeners:\n  - name: l\n    bind: \"{proxy}\"\n    pool: p\n    deny: [\"127.0.0.1/32\"]\n",
        env!("CARGO_MANIFEST_DIR")
    );
    let cfg = parse_str(&yaml).unwrap();
    set(&[]);
    let rt = Runtime::start(Snapshot::from_config(&cfg), Arc::default(), 1);
    assert!(!echoed(proxy).await, "acl on: the denied source is dropped");
    set(&["acl"]);
    assert!(echoed(proxy).await, "acl off: the same source gets through");
    set(&[]);
    assert!(!echoed(proxy).await, "acl back on");
    rt.shutdown_with_grace(Duration::from_millis(100)).await;
    set(&[]);
}

#[tokio::test]
async fn geo_flag_off_admits_a_country_the_allow_list_refuses() {
    let _t = TURN.lock().await;
    let backend = tcp_echo().await;
    let proxy = common::free_tcp_addr();
    let yaml = format!(
        "settings:\n  geo_db: \"{}/tests/data/GeoIP2-Country-Test.mmdb\"\n\
         pools:\n  - name: p\n    targets: [\"{backend}\"]\n\
         listeners:\n  - name: l\n    bind: \"{proxy}\"\n    pool: p\n\
         \x20   geo:\n      allow: [\"SE\"]\n",
        env!("CARGO_MANIFEST_DIR")
    );
    let cfg = parse_str(&yaml).unwrap();
    let geo = wayhouse_core::GeoDb::open(cfg.geo_db.as_ref().unwrap()).unwrap();
    set(&[]);
    let rt = Runtime::start_with_geo(Snapshot::from_config(&cfg), Arc::default(), Some(geo), 1);
    assert!(
        !echoed(proxy).await,
        "geo on: loopback is in no allowed country"
    );
    set(&["geo"]);
    assert!(echoed(proxy).await, "geo off");
    rt.shutdown_with_grace(Duration::from_millis(100)).await;
    set(&[]);
}

#[tokio::test]
async fn acl_flag_off_lets_a_new_udp_session_in() {
    let _t = TURN.lock().await;
    let backend = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let backend_addr = backend.local_addr().unwrap();
    tokio::spawn(async move {
        let mut buf = [0u8; 64];
        while let Ok((n, from)) = backend.recv_from(&mut buf).await {
            let _ = backend.send_to(&buf[..n], from).await;
        }
    });
    let proxy = common::free_udp_addr();
    let yaml = format!(
        "pools:\n  - name: p\n    targets: [\"{backend_addr}\"]\n    health_check: {{ type: none }}\n\
         listeners:\n  - name: l\n    bind: \"{proxy}\"\n    protocol: udp\n    pool: p\n    deny: [\"127.0.0.1/32\"]\n"
    );
    let cfg = parse_str(&yaml).unwrap();
    set(&[]);
    let rt = Runtime::start(Snapshot::from_config(&cfg), Arc::default(), 1);
    tokio::time::sleep(Duration::from_millis(200)).await;
    let ask = || async {
        let c = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        c.send_to(b"hi", proxy).await.unwrap();
        let mut buf = [0u8; 8];
        tokio::time::timeout(Duration::from_millis(500), c.recv_from(&mut buf))
            .await
            .is_ok()
    };
    assert!(!ask().await, "acl on: dropped");
    set(&["acl"]);
    assert!(ask().await, "acl off: relayed");
    rt.shutdown_with_grace(Duration::from_millis(100)).await;
    set(&[]);
}
