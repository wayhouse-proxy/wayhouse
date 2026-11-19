//! The connection loop behind [`crate::tls::serve`]: what `axum::serve` does, plus the two
//! limits it lacks. A client that opens a connection and then sends nothing (or one header
//! byte at a time) used to hold it, and its task, for ever; and nothing capped how many
//! connections one server held at once.
//!
//! - **Header-read timeout** ([`HEADER_READ_TIMEOUT`]): HTTP/1 must deliver a full request
//!   head in time. A connection that is idle between requests is closed by the same timer;
//!   long-lived streams (SSE, WebSocket) are not affected, because the timer only runs while
//!   a request head is awaited.
//! - **HTTP/2 keep-alive** ([`H2_PING_EVERY`], [`H2_PING_TIMEOUT`]): a peer that stops
//!   answering pings is dropped, so a dead HTTP/2 connection does not linger.
//! - **Connection cap** ([`MAX_CONNECTIONS`]): the loop takes a permit before it accepts, so
//!   past the cap new peers wait in the kernel's accept queue instead of costing a task.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ConnectInfo;
use axum::{Extension, Router};
use hyper::body::Incoming;
use hyper::Request;
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use hyper_util::server::conn::auto::Builder;
use hyper_util::service::TowerToHyperService;
use tokio::sync::Semaphore;
use tower::ServiceExt as _;

use crate::tls::PeerAddr;

/// The time a client has to send a whole HTTP/1 request head, and the time an idle
/// keep-alive connection may wait for the next one.
pub const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(30);
/// An HTTP/2 connection is pinged this often...
pub const H2_PING_EVERY: Duration = Duration::from_secs(30);
/// ...and dropped when the ping is not answered within this.
pub const H2_PING_TIMEOUT: Duration = Duration::from_secs(20);
/// How many connections one server holds at once. The fleet's HTTP servers carry operator and
/// peer traffic, so this is generous; it only has to stop a flood from using memory.
pub const MAX_CONNECTIONS: usize = 4096;

/// Limits for [`serve_listener`]. The defaults are the constants above; tests shrink them.
#[derive(Debug, Clone, Copy)]
pub struct ConnLimits {
    pub header_read_timeout: Duration,
    pub max_connections: usize,
}

impl Default for ConnLimits {
    fn default() -> Self {
        Self {
            header_read_timeout: HEADER_READ_TIMEOUT,
            max_connections: MAX_CONNECTIONS,
        }
    }
}

/// Serve `app` on `listener` for ever (it only returns when accepting fails for good, which
/// `axum::serve::Listener` impls retry internally). Handlers read the peer through
/// `ConnectInfo<PeerAddr>`.
pub async fn serve_listener<L>(listener: L, app: Router, limits: ConnLimits) -> std::io::Result<()>
where
    L: axum::serve::Listener<Addr = SocketAddr>,
    L::Io: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let mut listener = listener;
    let slots = Arc::new(Semaphore::new(limits.max_connections));
    loop {
        // Before accepting: a full server leaves new peers in the kernel's queue.
        let Ok(permit) = slots.clone().acquire_owned().await else {
            return Ok(());
        };
        let (io, peer) = listener.accept().await;
        let service = app
            .clone()
            .layer(Extension(ConnectInfo(PeerAddr(peer))))
            .map_request(|req: Request<Incoming>| req.map(axum::body::Body::new));
        let service = TowerToHyperService::new(service);
        let io = TokioIo::new(io);
        tokio::spawn(async move {
            // Held for the life of the connection.
            let _permit = permit;
            let mut builder = Builder::new(TokioExecutor::new());
            builder
                .http1()
                .timer(TokioTimer::new())
                .header_read_timeout(limits.header_read_timeout);
            builder
                .http2()
                .timer(TokioTimer::new())
                .keep_alive_interval(H2_PING_EVERY)
                .keep_alive_timeout(H2_PING_TIMEOUT)
                // HTTP/2 WebSockets (RFC 8441), as `axum::serve` offers.
                .enable_connect_protocol();
            let _ = builder.serve_connection_with_upgrades(io, service).await;
        });
    }
}
