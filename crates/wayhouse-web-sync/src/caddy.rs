//! Caddy's admin API, `POST /load` only. A minimal HTTP/1.1 client over a loopback
//! TCP stream or a unix socket: the sync needs one request shape, and reqwest has no
//! unix-socket transport.

use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpStream, UnixStream};

use crate::admin_addr::AdminAddr;

/// Longest response we read: Caddy's error bodies are a line of JSON.
const MAX_RESPONSE: u64 = 64 * 1024;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum LoadError {
    /// Nothing is listening, or the connection broke.
    #[error("caddy admin unreachable: {0}")]
    Unreachable(String),
    #[error("caddy admin did not answer in time")]
    Timeout,
    /// Caddy refused the document (4xx). It keeps its previous configuration.
    #[error("caddy rejected the config ({status}): {body}")]
    Rejected { status: u16, body: String },
    /// Any other non-2xx answer.
    #[error("caddy admin answered {status}: {body}")]
    Server { status: u16, body: String },
    #[error("caddy admin sent an unreadable response: {0}")]
    BadResponse(String),
}

#[derive(Debug, Clone)]
pub struct CaddyClient {
    addr: AdminAddr,
    timeout: Duration,
}

impl CaddyClient {
    pub fn new(addr: AdminAddr, timeout: Duration) -> Self {
        Self { addr, timeout }
    }

    /// `POST /load` with `doc`. Caddy swaps the configuration atomically; on a 4xx the
    /// previous one keeps serving.
    pub async fn load(&self, doc: &Value) -> Result<(), LoadError> {
        let body = serde_json::to_vec(doc).map_err(|e| LoadError::BadResponse(e.to_string()))?;
        let req = format!(
            "POST /load HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n",
            self.addr.host_header(),
            body.len()
        );
        let io = async {
            match &self.addr {
                AdminAddr::Tcp(a) => exchange(TcpStream::connect(a).await, req, &body).await,
                AdminAddr::Unix(p) => exchange(UnixStream::connect(p).await, req, &body).await,
            }
        };
        let raw = tokio::time::timeout(self.timeout, io)
            .await
            .map_err(|_| LoadError::Timeout)??;
        interpret(&raw)
    }
}

async fn exchange<S>(
    stream: std::io::Result<S>,
    head: String,
    body: &[u8],
) -> Result<Vec<u8>, LoadError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let unreachable = |e: std::io::Error| LoadError::Unreachable(e.to_string());
    let mut s = stream.map_err(unreachable)?;
    s.write_all(head.as_bytes()).await.map_err(unreachable)?;
    s.write_all(body).await.map_err(unreachable)?;
    s.flush().await.map_err(unreachable)?;
    let mut raw = Vec::new();
    (&mut s)
        .take(MAX_RESPONSE)
        .read_to_end(&mut raw)
        .await
        .map_err(unreachable)?;
    Ok(raw)
}

fn interpret(raw: &[u8]) -> Result<(), LoadError> {
    let text = String::from_utf8_lossy(raw);
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status: u16 = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|c| c.parse().ok())
        .ok_or_else(|| LoadError::BadResponse(head.chars().take(80).collect()))?;
    let body = body.trim().to_string();
    match status {
        200..=299 => Ok(()),
        400..=499 => Err(LoadError::Rejected { status, body }),
        _ => Err(LoadError::Server { status, body }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::net::{TcpListener, UnixListener};

    /// Accepts one connection, returns the request it read, answers with `reply`.
    async fn fake_tcp(reply: &'static str) -> (AdminAddr, tokio::task::JoinHandle<String>) {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = AdminAddr::Tcp(l.local_addr().unwrap());
        let h = tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            let mut got = Vec::new();
            let mut buf = [0u8; 4096];
            // Read until the declared body has arrived.
            loop {
                let n = s.read(&mut buf).await.unwrap();
                got.extend_from_slice(&buf[..n]);
                let t = String::from_utf8_lossy(&got).to_string();
                if let Some((h, b)) = t.split_once("\r\n\r\n") {
                    let want: usize = h
                        .lines()
                        .find_map(|l| l.strip_prefix("Content-Length: "))
                        .unwrap()
                        .parse()
                        .unwrap();
                    if b.len() >= want {
                        break;
                    }
                }
                assert!(n > 0, "client closed early");
            }
            s.write_all(reply.as_bytes()).await.unwrap();
            String::from_utf8(got).unwrap()
        });
        (addr, h)
    }

    fn client(a: AdminAddr) -> CaddyClient {
        CaddyClient::new(a, Duration::from_secs(2))
    }

    #[tokio::test]
    async fn posts_the_document_to_load_and_accepts_200() {
        let (a, h) = fake_tcp("HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n").await;
        client(a.clone()).load(&json!({"k": 1})).await.unwrap();
        let req = h.await.unwrap();
        assert!(req.starts_with("POST /load HTTP/1.1\r\n"), "{req}");
        assert!(req.contains(&format!("Host: {}\r\n", a.host_header())));
        assert!(req.contains("Content-Type: application/json\r\n"));
        assert!(req.ends_with("{\"k\":1}"), "{req}");
    }

    #[tokio::test]
    async fn a_400_is_a_rejection_with_caddys_message() {
        let body = "{\"error\":\"loading config: bad CIDR\"}";
        let reply: &'static str = Box::leak(
            format!(
                "HTTP/1.1 400 Bad Request\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            )
            .into_boxed_str(),
        );
        let (a, _h) = fake_tcp(reply).await;
        let e = client(a).load(&json!({})).await.unwrap_err();
        assert_eq!(
            e,
            LoadError::Rejected {
                status: 400,
                body: body.into()
            }
        );
    }

    #[tokio::test]
    async fn a_500_is_a_server_error_not_a_rejection() {
        let (a, _h) =
            fake_tcp("HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\n\r\n").await;
        let e = client(a).load(&json!({})).await.unwrap_err();
        assert!(matches!(e, LoadError::Server { status: 500, .. }), "{e}");
    }

    #[tokio::test]
    async fn nothing_listening_is_unreachable() {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let a = AdminAddr::Tcp(l.local_addr().unwrap());
        drop(l);
        let e = client(a).load(&json!({})).await.unwrap_err();
        assert!(matches!(e, LoadError::Unreachable(_)), "{e}");
    }

    #[tokio::test]
    async fn a_silent_server_times_out() {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let a = AdminAddr::Tcp(l.local_addr().unwrap());
        let _keep = tokio::spawn(async move {
            let (_s, _) = l.accept().await.unwrap();
            tokio::time::sleep(Duration::from_secs(30)).await;
        });
        let c = CaddyClient::new(a, Duration::from_millis(200));
        assert_eq!(c.load(&json!({})).await.unwrap_err(), LoadError::Timeout);
    }

    #[tokio::test]
    async fn garbage_is_a_bad_response() {
        let (a, _h) = fake_tcp("not http at all").await;
        let e = client(a).load(&json!({})).await.unwrap_err();
        assert!(matches!(e, LoadError::BadResponse(_)), "{e}");
    }

    #[tokio::test]
    async fn works_over_a_unix_socket() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("admin.sock");
        let l = UnixListener::bind(&p).unwrap();
        let h = tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let n = s.read(&mut buf).await.unwrap();
            s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                .await
                .unwrap();
            String::from_utf8_lossy(&buf[..n]).to_string()
        });
        client(AdminAddr::Unix(p))
            .load(&json!({"a": 1}))
            .await
            .unwrap();
        assert!(h.await.unwrap().contains("Host: localhost\r\n"));
    }
}
