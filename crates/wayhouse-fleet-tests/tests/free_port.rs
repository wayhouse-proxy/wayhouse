//! `free_port` must never hand the same port to two callers (#334): in a
//! three-replica cluster a duplicate made one replica die on `Address already
//! in use` and pointed its peers' RPCs at another replica.

use std::collections::HashSet;
use std::net::TcpListener;
use std::path::PathBuf;

use wayhouse_fleet_tests::free_port;

fn claim_dir() -> PathBuf {
    wayhouse_fleet_tests::port_claim_dir()
}

#[test]
fn concurrent_callers_never_share_a_port() {
    let threads: Vec<_> = (0..4)
        .map(|_| {
            std::thread::spawn(|| {
                (0..20)
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
    let held: Vec<(u16, TcpListener)> = (0..20)
        .map(|_| {
            let port = free_port().expect("a free port");
            (port, TcpListener::bind(("127.0.0.1", port)).expect("bind"))
        })
        .collect();
    let ports: HashSet<_> = held.iter().map(|(p, _)| *p).collect();
    assert_eq!(ports.len(), held.len());
}

fn age(path: &std::path::Path, secs: u64) {
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(secs);
    std::fs::File::options()
        .write(true)
        .open(path)
        .and_then(|f| f.set_modified(old))
        .expect("set mtime");
}

#[test]
fn an_expired_claim_is_taken_by_exactly_one_caller() {
    let dir = tempfile::tempdir().unwrap();
    let ttl = std::time::Duration::from_secs(60);
    for round in 0..20 {
        let claim = dir.path().join(format!("claim{round}"));
        std::fs::write(&claim, "").unwrap();
        age(&claim, 120);
        let wins: usize = (0..8)
            .map(|_| {
                let claim = claim.clone();
                std::thread::spawn(move || wayhouse_fleet_tests::take_stale(&claim, ttl))
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|t| usize::from(t.join().expect("worker")))
            .sum();
        assert_eq!(wins, 1, "round {round}");
        assert!(!claim.exists());
    }
}

#[test]
fn a_fresh_claim_that_was_moved_aside_is_put_back() {
    // The late loser of a race renames the winner's fresh claim; it must give it back.
    let dir = tempfile::tempdir().unwrap();
    let claim = dir.path().join("claim");
    std::fs::write(&claim, "").unwrap();
    assert!(!wayhouse_fleet_tests::take_stale(
        &claim,
        std::time::Duration::from_secs(60)
    ));
    assert!(claim.exists());
    let names: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
    assert_eq!(names.len(), 1, "no .stale leftovers: {names:?}");
}
