//! The connection loop's limits (`wayhouse_http::conn`): a header-read timeout, a cap on open
//! connections, and no cost to a slow handler or a plain request.

use std::net::SocketAddr;
use std::time::Duration;

use axum::extract::ConnectInfo;
use axum::routing::get;
use axum::Router;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use wayhouse_http::conn::{serve_listener, ConnLimits};
use wayhouse_http::tls::PeerAddr;

async fn start(limits: ConnLimits) -> SocketAddr {
    let app = Router::new()
        .route(
            "/peer",
            get(
                |ConnectInfo(PeerAddr(peer)): ConnectInfo<PeerAddr>| async move {
                    peer.ip().to_string()
                },
            ),
        )
        .route(
            "/slow",
            get(|| async {
                tokio::time::sleep(Duration::from_millis(700)).await;
                "done"
            }),
        );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { serve_listener(listener, app, limits).await });
    addr
}

fn limits(header_ms: u64, max: usize) -> ConnLimits {
    ConnLimits {
        header_read_timeout: Duration::from_millis(header_ms),
        first_byte_timeout: Duration::from_millis(header_ms),
        idle_timeout: Duration::from_secs(30),
        max_connections: max,
    }
}

async fn get_path(s: &mut TcpStream, path: &str) {
    s.write_all(format!("GET {path} HTTP/1.1\r\nHost: t\r\n\r\n").as_bytes())
        .await
        .unwrap();
}

/// Reads until the response body ends (the tests use short, content-length bodies).
async fn read_some(s: &mut TcpStream, wait: Duration) -> Option<String> {
    let mut buf = vec![0u8; 1024];
    let n = tokio::time::timeout(wait, s.read(&mut buf))
        .await
        .ok()?
        .ok()?;
    (n > 0).then(|| String::from_utf8_lossy(&buf[..n]).into_owned())
}

#[tokio::test]
async fn a_plain_request_is_served_and_sees_the_peer() {
    let addr = start(ConnLimits::default()).await;
    let mut s = TcpStream::connect(addr).await.unwrap();
    get_path(&mut s, "/peer").await;
    let got = read_some(&mut s, Duration::from_secs(2)).await.unwrap();
    assert!(got.starts_with("HTTP/1.1 200"), "{got}");
    assert!(got.ends_with("127.0.0.1"), "{got}");
}

#[tokio::test]
async fn a_request_head_that_never_finishes_is_cut_off() {
    let addr = start(limits(300, 8)).await;
    let mut s = TcpStream::connect(addr).await.unwrap();
    // A head with no blank line at the end: the client then goes quiet.
    s.write_all(b"GET /peer HTTP/1.1\r\nHost: t\r\n")
        .await
        .unwrap();
    let mut buf = [0u8; 256];
    let started = std::time::Instant::now();
    let r = tokio::time::timeout(Duration::from_secs(3), s.read(&mut buf))
        .await
        .expect("the server closes the connection");
    // Closed (0 bytes) or an error status written before the close; never held open.
    assert!(r.is_ok());
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn a_slow_handler_is_not_cut_off_by_the_header_timeout() {
    let addr = start(limits(200, 8)).await;
    let mut s = TcpStream::connect(addr).await.unwrap();
    get_path(&mut s, "/slow").await;
    let got = read_some(&mut s, Duration::from_secs(3)).await.unwrap();
    assert!(
        got.starts_with("HTTP/1.1 200") && got.ends_with("done"),
        "{got}"
    );
}

#[tokio::test]
async fn past_the_cap_a_new_connection_waits_for_a_free_slot() {
    let addr = start(limits(30_000, 2)).await;
    let a = TcpStream::connect(addr).await.unwrap();
    let mut b = TcpStream::connect(addr).await.unwrap();
    // Let the server accept both, so the two slots are taken.
    get_path(&mut b, "/peer").await;
    assert!(read_some(&mut b, Duration::from_secs(2)).await.is_some());

    let mut c = TcpStream::connect(addr).await.unwrap();
    get_path(&mut c, "/peer").await;
    assert!(
        read_some(&mut c, Duration::from_millis(400))
            .await
            .is_none(),
        "the third connection is not served while two are open"
    );
    drop(a); // a slot frees up
    let got = read_some(&mut c, Duration::from_secs(2))
        .await
        .expect("served after a slot frees");
    assert!(got.starts_with("HTTP/1.1 200"), "{got}");
}

/// A peer that connects and sends nothing is cut off by the first-byte timer, and its
/// slot comes back (hyper's protocol detection has no timer of its own).
#[tokio::test]
async fn a_silent_connection_is_cut_off_and_frees_its_slot() {
    let addr = start(ConnLimits {
        header_read_timeout: Duration::from_secs(30),
        first_byte_timeout: Duration::from_millis(300),
        idle_timeout: Duration::from_secs(30),
        max_connections: 1,
    })
    .await;
    let mut silent = TcpStream::connect(addr).await.unwrap();
    let mut buf = [0u8; 16];
    let n = tokio::time::timeout(Duration::from_secs(3), silent.read(&mut buf))
        .await
        .expect("server closes the silent connection")
        .unwrap_or(0);
    assert_eq!(n, 0);
    let mut next = TcpStream::connect(addr).await.unwrap();
    get_path(&mut next, "/peer").await;
    assert!(read_some(&mut next, Duration::from_secs(3)).await.is_some());
}

/// One byte ("P") is not a request: the connection still ends once the header timer runs out.
#[tokio::test]
async fn one_byte_then_silence_is_cut_off() {
    let addr = start(ConnLimits {
        header_read_timeout: Duration::from_secs(30),
        first_byte_timeout: Duration::from_millis(300),
        idle_timeout: Duration::from_secs(30),
        max_connections: 4,
    })
    .await;
    let mut s = TcpStream::connect(addr).await.unwrap();
    s.write_all(b"P").await.unwrap();
    let mut buf = [0u8; 16];
    let n = tokio::time::timeout(Duration::from_secs(3), s.read(&mut buf))
        .await
        .expect("server closes the connection")
        .unwrap_or(0);
    assert_eq!(n, 0);
}

/// A connection that has finished its request and then sits idle is closed after the idle
/// timeout; one with a response still running is not.
#[tokio::test]
async fn an_idle_connection_is_closed_but_a_busy_one_is_not() {
    let addr = start(ConnLimits {
        header_read_timeout: Duration::from_secs(30),
        first_byte_timeout: Duration::from_secs(30),
        idle_timeout: Duration::from_millis(400),
        max_connections: 4,
    })
    .await;
    // Busy: /slow takes 700 ms, longer than the idle timeout, and still answers.
    let mut busy = TcpStream::connect(addr).await.unwrap();
    get_path(&mut busy, "/slow").await;
    let reply = read_some(&mut busy, Duration::from_secs(3)).await;
    assert!(
        reply.is_some_and(|r| r.contains("done")),
        "slow request must finish"
    );
    // Idle: one quick request, then nothing.
    let mut idle = TcpStream::connect(addr).await.unwrap();
    get_path(&mut idle, "/peer").await;
    assert!(read_some(&mut idle, Duration::from_secs(3)).await.is_some());
    let mut buf = [0u8; 16];
    let n = tokio::time::timeout(Duration::from_secs(3), idle.read(&mut buf))
        .await
        .expect("server closes the idle connection")
        .unwrap_or(0);
    assert_eq!(n, 0);
}
