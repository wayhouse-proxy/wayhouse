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

use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use axum::extract::ConnectInfo;
use axum::{Extension, Router};
use hyper::body::Incoming;
use hyper::Request;
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use hyper_util::server::conn::auto::Builder;
use hyper_util::service::TowerToHyperService;
use std::future::Future;
use tokio::sync::Semaphore;
use tower::ServiceExt as _;

use crate::tls::PeerAddr;

/// The time a client has to send a whole HTTP/1 request head, and the time an idle
/// keep-alive connection may wait for the next one.
///
/// 60 s, not 30: the fleet's agents poll every 30 s by default, and a shorter idle timer
/// would make each poll pay a fresh handshake.
pub const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(60);
/// The time a new connection has to send its first byte. hyper-util's version detection
/// reads those bytes with no timer of its own, so without this a silent peer holds a slot.
pub const FIRST_BYTE_TIMEOUT: Duration = Duration::from_secs(30);
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
    pub first_byte_timeout: Duration,
    pub max_connections: usize,
}

impl Default for ConnLimits {
    fn default() -> Self {
        Self {
            header_read_timeout: HEADER_READ_TIMEOUT,
            first_byte_timeout: FIRST_BYTE_TIMEOUT,
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
        let io = TokioIo::new(FirstByte::new(io, limits.first_byte_timeout));
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

/// An accepted stream whose reads fail with `TimedOut` if no byte has arrived by the
/// deadline, or if the bytes so far are still only the start of the HTTP/2 preface. Once the
/// protocol is clear the timer is off for good (the header-read timeout and the HTTP/2 pings
/// take over).
struct FirstByte<Io> {
    inner: Io,
    sleep: Option<Pin<Box<tokio::time::Sleep>>>,
    /// What has been read while the timer runs; only ever a prefix of [`H2_PREFACE`].
    head: Vec<u8>,
}

/// hyper-util keeps reading until the bytes either differ from this or complete it, so a
/// peer that stops partway through it is the one case a plain "first byte" timer misses.
const H2_PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

impl<Io> FirstByte<Io> {
    fn new(inner: Io, limit: Duration) -> Self {
        Self {
            inner,
            sleep: Some(Box::pin(tokio::time::sleep(limit))),
            head: Vec::new(),
        }
    }
}

impl<Io: tokio::io::AsyncRead + Unpin> tokio::io::AsyncRead for FirstByte<Io> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let before = buf.filled().len();
        match Pin::new(&mut self.inner).poll_read(cx, buf) {
            Poll::Ready(r) => {
                if self.sleep.is_some() {
                    let me = &mut *self;
                    me.head.extend_from_slice(&buf.filled()[before..]);
                    // Done once the bytes are no longer a proper prefix of the preface.
                    if !H2_PREFACE.starts_with(&me.head) || me.head.len() >= H2_PREFACE.len() {
                        me.sleep = None;
                    }
                }
                Poll::Ready(r)
            }
            Poll::Pending => {
                let expired = match self.sleep.as_mut() {
                    Some(sleep) => sleep.as_mut().poll(cx).is_ready(),
                    None => false,
                };
                if expired {
                    Poll::Ready(Err(io::ErrorKind::TimedOut.into()))
                } else {
                    Poll::Pending
                }
            }
        }
    }
}

impl<Io: tokio::io::AsyncWrite + Unpin> tokio::io::AsyncWrite for FirstByte<Io> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        b: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, b)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}
