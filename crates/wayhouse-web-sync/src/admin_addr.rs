//! Where Caddy's admin API listens. Only loopback TCP or a unix socket is accepted:
//! the admin API can replace the whole configuration, so the sync refuses to talk to
//! (or render) anything reachable from the network.

use std::net::SocketAddr;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdminAddr {
    Tcp(SocketAddr),
    Unix(PathBuf),
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AdminAddrError {
    #[error("{0:?} is not a loopback ip:port, a unix socket path, or unix//<path>")]
    Unrecognised(String),
    #[error(
        "{0} is not a loopback address; Caddy's admin API must not be reachable from the network"
    )]
    NotLoopback(SocketAddr),
}

impl AdminAddr {
    /// `127.0.0.1:2019`, `[::1]:2019`, `/run/caddy/admin.sock` or Caddy's own `unix//run/caddy/admin.sock`.
    pub fn parse(s: &str) -> Result<Self, AdminAddrError> {
        let s = s.trim();
        if let Some(path) = s.strip_prefix("unix/") {
            return Self::unix(path, s);
        }
        if s.starts_with('/') {
            return Self::unix(s, s);
        }
        let addr: SocketAddr = s
            .parse()
            .map_err(|_| AdminAddrError::Unrecognised(s.to_string()))?;
        if !addr.ip().is_loopback() {
            return Err(AdminAddrError::NotLoopback(addr));
        }
        Ok(Self::Tcp(addr))
    }

    fn unix(path: &str, whole: &str) -> Result<Self, AdminAddrError> {
        if path.starts_with('/') && path.len() > 1 {
            Ok(Self::Unix(PathBuf::from(path)))
        } else {
            Err(AdminAddrError::Unrecognised(whole.to_string()))
        }
    }

    /// The value Caddy's `admin.listen` takes for this address.
    pub fn caddy_listen(&self) -> String {
        match self {
            Self::Tcp(a) => a.to_string(),
            Self::Unix(p) => format!("unix/{}", p.display()),
        }
    }

    /// The `Host` header to send: Caddy checks it against the listen address on TCP.
    pub fn host_header(&self) -> String {
        match self {
            Self::Tcp(a) => a.to_string(),
            Self::Unix(_) => "localhost".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_loopback_tcp_and_unix_sockets() {
        assert_eq!(
            AdminAddr::parse("127.0.0.1:2019").unwrap(),
            AdminAddr::Tcp("127.0.0.1:2019".parse().unwrap())
        );
        assert_eq!(
            AdminAddr::parse("[::1]:2019").unwrap().caddy_listen(),
            "[::1]:2019"
        );
        for s in ["/run/caddy/admin.sock", "unix//run/caddy/admin.sock"] {
            let a = AdminAddr::parse(s).unwrap();
            assert_eq!(a, AdminAddr::Unix("/run/caddy/admin.sock".into()));
            assert_eq!(a.caddy_listen(), "unix//run/caddy/admin.sock");
            assert_eq!(a.host_header(), "localhost");
        }
    }

    #[test]
    fn refuses_anything_reachable_from_the_network() {
        assert!(matches!(
            AdminAddr::parse("0.0.0.0:2019"),
            Err(AdminAddrError::NotLoopback(_))
        ));
        assert!(matches!(
            AdminAddr::parse("10.0.0.5:2019"),
            Err(AdminAddrError::NotLoopback(_))
        ));
        for s in ["localhost:2019", "caddy", "", "unix/relative.sock", "unix/"] {
            assert!(
                matches!(AdminAddr::parse(s), Err(AdminAddrError::Unrecognised(_))),
                "{s}"
            );
        }
    }
}
