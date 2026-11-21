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
            "/stream",
            get(|| async {
                // A response that keeps streaming for longer than the idle timeouts below.
                let (tx, rx) = tokio::sync::mpsc::channel(1);
                tokio::spawn(async move {
                    for i in 0..4u8 {
                        tokio::time::sleep(Duration::from_millis(400)).await;
                        if tx
                            .send(Ok::<_, std::convert::Infallible>(format!("c{i};")))
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                });
                let chunks = tokio_stream::wrappers::ReceiverStream::new(rx);
                axum::body::Body::from_stream(chunks)
            }),
        )
        .route(
            "/upgrade",
            get(|mut req: axum::extract::Request| async move {
                let on = hyper::upgrade::on(&mut req);
                tokio::spawn(async move {
                    let up = on.await.unwrap();
                    let mut io = hyper_util::rt::TokioIo::new(up);
                    io.write_all(b"hello").await.unwrap();
                    tokio::time::sleep(Duration::from_millis(1500)).await;
                    let _ = io.write_all(b"later").await;
                });
                axum::response::Response::builder()
                    .status(101)
                    .header("connection", "upgrade")
                    .header("upgrade", "test")
                    .body(axum::body::Body::empty())
                    .unwrap()
            }),
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

fn idle_limits() -> ConnLimits {
    ConnLimits {
        header_read_timeout: Duration::from_secs(30),
        first_byte_timeout: Duration::from_secs(30),
        idle_timeout: Duration::from_millis(400),
        max_connections: 4,
    }
}

/// A response still streaming after several idle timeouts is delivered whole.
#[tokio::test]
async fn a_response_streaming_past_the_idle_timeout_survives() {
    let addr = start(idle_limits()).await;
    let mut s = TcpStream::connect(addr).await.unwrap();
    get_path(&mut s, "/stream").await;
    let mut all = String::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
    while !all.contains("c3;") && tokio::time::Instant::now() < deadline {
        if let Some(part) = read_some(&mut s, Duration::from_secs(2)).await {
            all.push_str(&part);
        } else {
            break;
        }
    }
    assert!(all.contains("c0;") && all.contains("c3;"), "got: {all:?}");
}

/// An upgraded connection (WebSocket-style) is not touched by the idle watchdog even though
/// its 101 response body ended at once.
#[tokio::test]
async fn an_upgraded_connection_survives_the_idle_timeout() {
    let addr = start(idle_limits()).await;
    let mut s = TcpStream::connect(addr).await.unwrap();
    s.write_all(
        b"GET /upgrade HTTP/1.1\r\nHost: t\r\nConnection: upgrade\r\nUpgrade: test\r\n\r\n",
    )
    .await
    .unwrap();
    let mut all = String::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
    while !all.contains("later") && tokio::time::Instant::now() < deadline {
        match read_some(&mut s, Duration::from_secs(3)).await {
            Some(part) => all.push_str(&part),
            None => break,
        }
    }
    assert!(all.contains("101") && all.contains("hello"), "got: {all:?}");
    assert!(
        all.contains("later"),
        "upgraded stream was cut off: {all:?}"
    );
}

/// Short requests on one keep-alive connection, each finished between two watchdog checks, still
/// count as activity: the connection outlives several idle timeouts.
#[tokio::test]
async fn a_busy_keep_alive_connection_is_not_closed() {
    let addr = start(idle_limits()).await;
    let mut s = TcpStream::connect(addr).await.unwrap();
    let end = tokio::time::Instant::now() + Duration::from_secs(2);
    let mut answered = 0;
    while tokio::time::Instant::now() < end {
        get_path(&mut s, "/peer").await;
        let reply = read_some(&mut s, Duration::from_secs(1)).await;
        assert!(
            reply.is_some(),
            "connection closed after {answered} requests"
        );
        answered += 1;
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert!(answered >= 6, "only {answered} requests");
}

/// An upgraded connection (a WebSocket) keeps its slot for as long as it lives: hyper's
/// connection future ends at the upgrade, but the cap counts open connections.
#[tokio::test]
async fn an_upgraded_connection_still_counts_against_the_cap() {
    let addr = start(limits(5_000, 1)).await;
    let mut a = TcpStream::connect(addr).await.unwrap();
    a.write_all(
        b"GET /upgrade HTTP/1.1\r\nHost: t\r\nConnection: upgrade\r\nUpgrade: test\r\n\r\n",
    )
    .await
    .unwrap();
    let got = read_some(&mut a, Duration::from_secs(2)).await.unwrap();
    assert!(got.starts_with("HTTP/1.1 101"), "{got}");

    // The only slot is taken by the upgraded connection: a second client waits in the queue.
    let mut b = TcpStream::connect(addr).await.unwrap();
    get_path(&mut b, "/peer").await;
    assert!(
        read_some(&mut b, Duration::from_millis(600))
            .await
            .is_none(),
        "served while the upgraded connection was still open"
    );
    // The upgraded task ends at about 1.5 s and drops the stream; then the slot is free.
    let got = read_some(&mut b, Duration::from_secs(4)).await.unwrap();
    assert!(got.starts_with("HTTP/1.1 200"), "{got}");
}
