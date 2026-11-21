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
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use axum::extract::ConnectInfo;
use axum::{Extension, Router};
use hyper::body::Incoming;
use hyper::body::{Body as HttpBody, Frame, SizeHint};
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
/// A connection that has had no request in flight for this long is closed. An HTTP/2 peer can
/// answer pings for ever without opening a stream, so only counting requests catches it. Open
/// streams (SSE, long polls) keep a connection alive for as long as their response runs.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(60);
/// An HTTP/2 connection is pinged this often...
pub const H2_PING_EVERY: Duration = Duration::from_secs(30);
/// ...and dropped when the ping is not answered within this.
pub const H2_PING_TIMEOUT: Duration = Duration::from_secs(20);
/// How many connections one server holds at once. The fleet's HTTP servers carry operator and
/// peer traffic, so this is generous; it only has to stop a flood from using memory.
pub const MAX_CONNECTIONS: usize = 4096;

/// An accepted TCP stream is probed after this much silence...
const KEEPALIVE_IDLE: Duration = Duration::from_secs(30);
/// ...every this often...
const KEEPALIVE_EVERY: Duration = Duration::from_secs(10);
/// ...and a peer that leaves written data unacknowledged this long (a client that stopped
/// reading an SSE stream, or vanished) is dropped by the kernel, which frees its slot.
const USER_TIMEOUT: Duration = Duration::from_secs(60);

/// Turns on TCP keepalive and a user timeout for `stream`. HTTP/1 has no pings, so without this
/// a half-open or stalled client of a long-lived stream (SSE) would hold its connection, and the
/// slot its handler took, for ever. Best effort: a failure is logged and the stream is served.
pub fn harden(stream: &tokio::net::TcpStream) {
    let sock = socket2::SockRef::from(stream);
    let keepalive = socket2::TcpKeepalive::new()
        .with_time(KEEPALIVE_IDLE)
        .with_interval(KEEPALIVE_EVERY);
    if let Err(e) = sock.set_tcp_keepalive(&keepalive) {
        tracing::debug!(error = %e, "TCP keepalive not set");
    }
    #[cfg(any(target_os = "linux", target_os = "android"))]
    if let Err(e) = sock.set_tcp_user_timeout(Some(USER_TIMEOUT)) {
        tracing::debug!(error = %e, "TCP user timeout not set");
    }
}

/// A plain [`tokio::net::TcpListener`] whose accepted streams are [`harden`]ed.
pub struct HardenedListener(pub tokio::net::TcpListener);

impl axum::serve::Listener for HardenedListener {
    type Io = tokio::net::TcpStream;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        let (io, addr) = axum::serve::Listener::accept(&mut self.0).await;
        harden(&io);
        (io, addr)
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        self.0.local_addr()
    }
}

/// Limits for [`serve_listener`]. The defaults are the constants above; tests shrink them.
#[derive(Debug, Clone, Copy)]
pub struct ConnLimits {
    pub header_read_timeout: Duration,
    pub first_byte_timeout: Duration,
    /// A connection with no request in flight for this long is closed (gracefully).
    pub idle_timeout: Duration,
    pub max_connections: usize,
}

impl Default for ConnLimits {
    fn default() -> Self {
        Self {
            header_read_timeout: HEADER_READ_TIMEOUT,
            first_byte_timeout: FIRST_BYTE_TIMEOUT,
            idle_timeout: IDLE_TIMEOUT,
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
        let inflight = Arc::new(Activity::default());
        let router = app
            .clone()
            .layer(Extension(ConnectInfo(PeerAddr(peer))))
            .map_request(|req: Request<Incoming>| req.map(axum::body::Body::new));
        let counted = inflight.clone();
        let service = TowerToHyperService::new(tower::service_fn(move |req| {
            // Counted from the request head until the response body is finished or dropped.
            let guard = InFlight::new(counted.clone());
            let router = router.clone();
            async move {
                let res = router.oneshot(req).await?;
                Ok::<_, std::convert::Infallible>(res.map(|body| {
                    axum::body::Body::new(Guarded {
                        body,
                        _guard: guard,
                    })
                }))
            }
        }));
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
            let conn = builder.serve_connection_with_upgrades(io, service);
            tokio::pin!(conn);
            tokio::select! {
                _ = conn.as_mut() => {}
                _ = idle_watch(&inflight, limits.idle_timeout) => {
                    conn.as_mut().graceful_shutdown();
                    let _ = conn.await;
                }
            }
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

/// What the idle watchdog looks at: requests in flight now, and a counter bumped whenever one
/// starts or ends, so a request that came and went between two checks still counts as activity.
#[derive(Default)]
struct Activity {
    inflight: AtomicUsize,
    events: AtomicU64,
}

/// Resolves once the connection has had no request in flight, and no request start or end,
/// for `limit`. Checked a few times per `limit`.
async fn idle_watch(activity: &Activity, limit: Duration) {
    let tick = (limit / 4).max(Duration::from_millis(10));
    let mut idle_since = tokio::time::Instant::now();
    let mut seen = activity.events.load(Ordering::Acquire);
    loop {
        tokio::time::sleep(tick).await;
        let now_seen = activity.events.load(Ordering::Acquire);
        if activity.inflight.load(Ordering::Acquire) > 0 || now_seen != seen {
            idle_since = tokio::time::Instant::now();
            seen = now_seen;
        } else if idle_since.elapsed() >= limit {
            return;
        }
    }
}

/// One request in flight on a connection; dropping it ends the count.
struct InFlight(Arc<Activity>);

impl InFlight {
    fn new(a: Arc<Activity>) -> Self {
        a.inflight.fetch_add(1, Ordering::AcqRel);
        a.events.fetch_add(1, Ordering::AcqRel);
        Self(a)
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        self.0.inflight.fetch_sub(1, Ordering::AcqRel);
        self.0.events.fetch_add(1, Ordering::AcqRel);
    }
}

/// A response body that keeps its request counted until the body is finished or dropped.
struct Guarded {
    body: axum::body::Body,
    _guard: InFlight,
}

impl HttpBody for Guarded {
    type Data = <axum::body::Body as HttpBody>::Data;
    type Error = <axum::body::Body as HttpBody>::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        Pin::new(&mut self.body).poll_frame(cx)
    }
    fn is_end_stream(&self) -> bool {
        self.body.is_end_stream()
    }
    fn size_hint(&self) -> SizeHint {
        self.body.size_hint()
    }
}
