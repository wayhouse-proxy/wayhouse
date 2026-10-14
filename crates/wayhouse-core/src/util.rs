//! Small shared helpers.

use std::sync::LazyLock;
use std::time::Instant;

static START: LazyLock<Instant> = LazyLock::new(Instant::now);

/// Milliseconds elapsed since process start. Monotonic; used for cheap
/// "is this due yet?" comparisons stored in atomics.
pub fn mono_ms() -> u64 {
    START.elapsed().as_millis() as u64
}

/// Peer address with an IPv4-mapped IPv6 address (`::ffff:a.b.c.d`, how a
/// dual-stack `[::]` socket reports IPv4 clients) turned back into plain IPv4,
/// so ACLs, matchers, rate buckets and route hints see one spelling per client.
/// Use for policy decisions; reply I/O keeps the address the socket reported.
pub fn canonical_addr(a: std::net::SocketAddr) -> std::net::SocketAddr {
    std::net::SocketAddr::new(a.ip().to_canonical(), a.port())
}

/// Whether a socket error is this process running out of a local resource
/// (file descriptors, memory, buffers, ephemeral ports) rather than the
/// backend misbehaving. Health logic treats these as "unknown" so that a
/// proxy-side shortage never ejects backends.
pub fn is_local_resource_error(e: &std::io::Error) -> bool {
    use nix::errno::Errno;
    matches!(
        e.raw_os_error().map(Errno::from_raw),
        Some(Errno::EMFILE | Errno::ENFILE | Errno::ENOMEM | Errno::ENOBUFS | Errno::EADDRNOTAVAIL)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_addr_unmaps_ipv4_mapped_and_keeps_the_rest() {
        let c = |s: &str| canonical_addr(s.parse().unwrap());
        assert_eq!(
            c("[::ffff:203.0.113.9]:80"),
            "203.0.113.9:80".parse().unwrap()
        );
        assert_eq!(
            c("[::FFFF:cb00:7109]:80"),
            "203.0.113.9:80".parse().unwrap()
        );
        assert_eq!(c("[2001:db8::1]:80"), "[2001:db8::1]:80".parse().unwrap());
        assert_eq!(c("[::1]:80"), "[::1]:80".parse().unwrap());
        assert_eq!(c("1.2.3.4:80"), "1.2.3.4:80".parse().unwrap());
    }

    #[test]
    fn local_resource_errors_are_told_apart_from_backend_errors() {
        for code in [24, 23, 12, 105, 99] {
            assert!(is_local_resource_error(&std::io::Error::from_raw_os_error(
                code
            )));
        }
        assert!(!is_local_resource_error(
            &std::io::Error::from_raw_os_error(111)
        )); // ECONNREFUSED
        assert!(!is_local_resource_error(&std::io::Error::other("x")));
    }
}
