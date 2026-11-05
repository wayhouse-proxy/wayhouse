//! Shared helpers for the integration tests (`mod common;`).
//!
//! A "free port" read from `bind(0)` and then released is the kernel's next ephemeral port:
//! another test process, or an outgoing connection, can be handed it before this test binds it
//! again (#439). These ports come from a range below the kernel's ephemeral one
//! (`ip_local_port_range`, 32768 to 60999 by default), start at an offset that differs per
//! process (nextest runs each test as its own process), and are probed by binding.
#![allow(dead_code)]

use std::net::{SocketAddr, TcpListener, UdpSocket};
use std::sync::atomic::{AtomicU32, Ordering};

const LOW: u32 = 20_000;
const SPAN: u32 = 10_000;
static CALLS: AtomicU32 = AtomicU32::new(0);

/// The next port to try: this process's own starting point, one further per call. 37 is
/// coprime with `SPAN` and larger than the few ports a test takes, so two processes started
/// close together do not walk the same ports.
fn candidate() -> u16 {
    let at = std::process::id() * 37 + CALLS.fetch_add(1, Ordering::Relaxed);
    u16::try_from(LOW + at % SPAN).expect("below 65536")
}

fn addr(port: u16) -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], port))
}

/// A TCP address on 127.0.0.1 that is free now.
pub fn free_tcp_addr() -> SocketAddr {
    loop {
        let a = addr(candidate());
        if TcpListener::bind(a).is_ok() {
            return a;
        }
    }
}

/// A UDP address on 127.0.0.1 that is free now.
pub fn free_udp_addr() -> SocketAddr {
    loop {
        let a = addr(candidate());
        if UdpSocket::bind(a).is_ok() {
            return a;
        }
    }
}

#[test]
fn free_addresses_stay_below_the_ephemeral_range_and_are_not_repeated() {
    let mut seen = std::collections::HashSet::new();
    for _ in 0..200 {
        for a in [free_tcp_addr(), free_udp_addr()] {
            assert!((20_000..30_000).contains(&a.port()), "{a}");
        }
        assert!(seen.insert(free_tcp_addr().port()));
    }
}

#[test]
fn a_port_in_use_is_skipped() {
    let taken = free_tcp_addr();
    // Walk the sequence until it would reach `taken`: it must be skipped, not returned.
    let held = TcpListener::bind(taken).unwrap();
    for _ in 0..SPAN {
        assert_ne!(free_tcp_addr(), held.local_addr().unwrap());
    }
}
