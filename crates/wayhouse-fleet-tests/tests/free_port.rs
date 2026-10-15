//! `free_port` must never hand the same port to two callers (#334): in a
//! three-replica cluster a duplicate made one replica die on `Address already
//! in use` and pointed its peers' RPCs at another replica.

use std::collections::HashSet;
use std::net::TcpListener;
use std::path::PathBuf;

use wayhouse_fleet_tests::free_port;

fn claim_dir() -> PathBuf {
    std::env::temp_dir().join("wayhouse-fleet-ports")
}

#[test]
fn concurrent_callers_never_share_a_port() {
    let threads: Vec<_> = (0..8)
        .map(|_| {
            std::thread::spawn(|| {
                (0..200)
                    .map(|_| free_port().expect("a free port"))
                    .collect::<Vec<_>>()
            })
        })
        .collect();
    let mut seen = HashSet::new();
    for t in threads {
        for port in t.join().expect("worker") {
            assert!(seen.insert(port), "port {port} handed out twice");
        }
    }
}

#[test]
fn ports_stay_out_of_the_ephemeral_range_and_are_claimed() {
    // Outgoing connections draw from the ephemeral range and can take a port
    // between `free_port` and the child's bind.
    let low = std::fs::read_to_string("/proc/sys/net/ipv4/ip_local_port_range")
        .ok()
        .and_then(|s| s.split_whitespace().next()?.parse::<u16>().ok());
    let port = free_port().expect("a free port");
    if let Some(low) = low {
        assert!(port < low, "port {port} is in the ephemeral range ({low}+)");
    }
    // The claim is what keeps other test processes off it.
    assert!(claim_dir().join(port.to_string()).exists());
}

#[test]
fn ports_that_are_bound_stay_distinct() {
    // Bind every port free_port hands out and keep it: every bind must succeed
    // and none may repeat.
    let held: Vec<(u16, TcpListener)> = (0..50)
        .map(|_| {
            let port = free_port().expect("a free port");
            (port, TcpListener::bind(("127.0.0.1", port)).expect("bind"))
        })
        .collect();
    let ports: HashSet<_> = held.iter().map(|(p, _)| *p).collect();
    assert_eq!(ports.len(), held.len());
}
