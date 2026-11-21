//! Accepted TCP streams carry keepalive and a user timeout, so a half-open or stalled client of
//! a long-lived stream lets go of its slot.

use axum::serve::Listener;
use wayhouse_http::conn::HardenedListener;

#[tokio::test]
async fn an_accepted_stream_has_keepalive_on() {
    let tcp = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = tcp.local_addr().unwrap();
    let mut listener = HardenedListener(tcp);
    let client = tokio::spawn(async move { tokio::net::TcpStream::connect(addr).await.unwrap() });
    let (io, _) = listener.accept().await;
    let sock = socket2::SockRef::from(&io);
    assert!(sock.keepalive().unwrap(), "SO_KEEPALIVE is on");
    #[cfg(any(target_os = "linux", target_os = "android"))]
    assert_eq!(
        sock.tcp_user_timeout().unwrap(),
        Some(std::time::Duration::from_secs(60))
    );
    drop(client.await.unwrap());
}
