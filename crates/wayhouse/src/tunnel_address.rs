//! The tunnel address this proxy was last assigned, persisted next to its key
//! so a restart can come up while the controller is unreachable (spec:
//! "Persistence and offline behaviour"). The controller keeps an owner's
//! address sticky, so the saved value is still valid unless an operator
//! released it. Mirrored from `wayhouse-agent::address_store` rather than shared (the two
//! binaries have no common library; same precedent as `proxy_register.rs`).

use std::path::Path;

use anyhow::Context;

use crate::proxy_register::{RegisterError, Registered};

/// `10.60.0.5/16` → `10.60.0.5`.
pub fn ip_of(cidr: &str) -> &str {
    cidr.split_once('/').map(|(ip, _)| ip).unwrap_or(cidr)
}

/// Parses a bare tunnel IP, IPv4 or IPv6. IPv4-mapped (`::ffff:a.b.c.d`) and
/// IPv4-compatible (`::a.b.c.d`) IPv6 are refused, as the controller does:
/// they would make one host two addresses.
pub fn tunnel_ip(s: &str) -> Result<std::net::IpAddr, String> {
    let ip: std::net::IpAddr = s
        .parse()
        .map_err(|e| format!("{s:?} is not an IP address: {e}"))?;
    if let std::net::IpAddr::V6(a) = ip {
        let seg = a.segments();
        if seg[..5] == [0; 5] && (seg[5] == 0xffff || (seg[5] == 0 && seg[6] != 0)) {
            return Err(format!(
                "{s:?} is an IPv4 address written as IPv6: use the IPv4 form"
            ));
        }
    }
    Ok(ip)
}

/// The interface address: the assigned IP with the *network's* prefix, or — in
/// pin-only mode, when the controller reports no network — with the prefix of
/// the operator's pinned `--tunnel-address`.
pub fn interface_cidr(
    assigned_ip: &str,
    network: Option<&str>,
    pinned_cidr: Option<&str>,
) -> anyhow::Result<String> {
    let source = match (network, pinned_cidr) {
        (Some(n), _) => n,
        (None, Some(p)) => p,
        (None, None) => anyhow::bail!(
            "the controller reported no tunnel_network and no --tunnel-address was given whose \
             prefix could be used for the interface"
        ),
    };
    let ip = tunnel_ip(assigned_ip).map_err(anyhow::Error::msg)?;
    refuse_special_use(ip).map_err(anyhow::Error::msg)?;
    let (net_ip, prefix) = source
        .split_once('/')
        .with_context(|| format!("{source:?} has no /prefix"))?;
    let prefix: u8 = prefix
        .parse()
        .with_context(|| format!("{source:?} has no valid /prefix"))?;
    if network.is_some() {
        check_in_network(ip, net_ip, prefix)?;
    }
    Ok(format!("{assigned_ip}/{prefix}"))
}

/// The controller's own bounds for a tunnel network: IPv4 `/16`..`/30`, IPv6 `/64`..`/120`.
const V4_PREFIX: std::ops::RangeInclusive<u8> = 16..=30;
const V6_PREFIX: std::ops::RangeInclusive<u8> = 64..=120;

/// An address the interface must never be given, whatever a controller reports.
fn refuse_special_use(ip: std::net::IpAddr) -> Result<(), String> {
    let special = match ip {
        std::net::IpAddr::V4(a) => {
            a.is_unspecified()
                || a.is_loopback()
                || a.is_multicast()
                || a.is_link_local()
                || a.is_broadcast()
        }
        std::net::IpAddr::V6(a) => {
            a.is_unspecified()
                || a.is_loopback()
                || a.is_multicast()
                || a.segments()[0] & 0xffc0 == 0xfe80
        }
    };
    if special {
        Err(format!(
            "{ip} is a special-use address, not a tunnel address"
        ))
    } else {
        Ok(())
    }
}

/// Checks the reported `network` has an allowed prefix and holds `ip` as a host
/// (neither the network's own nor its all-ones address).
fn check_in_network(ip: std::net::IpAddr, net_ip: &str, prefix: u8) -> anyhow::Result<()> {
    let net: std::net::IpAddr = net_ip
        .parse()
        .with_context(|| format!("tunnel network {net_ip:?} is not an IP address"))?;
    refuse_special_use(net).map_err(anyhow::Error::msg)?;
    let (bits, allowed, width) = match (ip, net) {
        (std::net::IpAddr::V4(a), std::net::IpAddr::V4(n)) => (
            (u128::from(u32::from(a)), u128::from(u32::from(n))),
            V4_PREFIX,
            32u32,
        ),
        (std::net::IpAddr::V6(a), std::net::IpAddr::V6(n)) => {
            ((u128::from(a), u128::from(n)), V6_PREFIX, 128u32)
        }
        _ => anyhow::bail!("tunnel address {ip} is not in the same family as network {net}"),
    };
    anyhow::ensure!(
        allowed.contains(&prefix),
        "tunnel network prefix /{prefix} is outside the allowed /{}../{}",
        allowed.start(),
        allowed.end()
    );
    let host_bits = width - u32::from(prefix);
    let all_ones = (1u128 << host_bits) - 1;
    let (ip_bits, net_bits) = bits;
    anyhow::ensure!(
        ip_bits >> host_bits == net_bits >> host_bits,
        "tunnel address {ip} is not inside network {net}/{prefix}"
    );
    anyhow::ensure!(
        ip_bits & all_ones != 0 && ip_bits & all_ones != all_ones,
        "tunnel address {ip} is the network or broadcast address of {net}/{prefix}"
    );
    Ok(())
}

pub fn load(path: &Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Saves `cidr` by writing a temporary file beside `path`, syncing it and
/// renaming it over `path`, so a crash or a full disk leaves the previous
/// address, never an empty or partial one.
pub fn save(path: &Path, cidr: &str) -> anyhow::Result<()> {
    write_atomically(path, format!("{cidr}\n").as_bytes())
        .with_context(|| format!("saving the tunnel address to {}", path.display()))
}

fn write_atomically(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    let tmp = path.with_file_name(name);
    let written = std::fs::File::create(&tmp).and_then(|mut f| {
        f.write_all(contents)?;
        f.sync_all()
    });
    match written.and_then(|()| std::fs::rename(&tmp, path)) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

#[derive(Debug)]
pub enum Source {
    Controller,
    /// The controller could not be reached; `cause` is the last transient
    /// error, and `pin_ignored` says so when a pinned `--tunnel-address` differs from the
    /// saved address it lost to.
    Saved {
        cause: String,
        pin_ignored: Option<String>,
    },
}

#[derive(Debug)]
pub struct StartupAddress {
    pub cidr: String,
    pub source: Source,
}

/// Decides the interface address at startup from the registration outcome:
/// the controller's answer wins; a *transient* failure falls back to the saved
/// address (or fails loudly if there is none); a *rejection* (409/422) never
/// falls back — a stale saved value must not mask an address conflict.
pub fn resolve_startup(
    outcome: Result<Registered, RegisterError>,
    pinned_cidr: Option<&str>,
    saved: Option<String>,
) -> anyhow::Result<StartupAddress> {
    match outcome {
        Ok(r) => Ok(StartupAddress {
            cidr: interface_cidr(&r.tunnel_address, r.tunnel_network.as_deref(), pinned_cidr)?,
            source: Source::Controller,
        }),
        Err(e @ RegisterError::Rejected(_)) => {
            Err(anyhow::Error::new(e).context("the controller refused this tunnel registration"))
        }
        Err(e) => match saved {
            Some(cidr) => Ok(StartupAddress {
                source: Source::Saved {
                    cause: format!("{e:#}"),
                    pin_ignored: pinned_cidr.filter(|p| ip_of(p) != ip_of(&cidr)).map(|p| {
                        format!(
                            "--tunnel-address {p} differs from the saved tunnel address {cidr}; \
                                 keeping {cidr} — restart once the controller is reachable to apply the pin"
                        )
                    }),
                },
                cidr,
            }),
            None => Err(anyhow::Error::new(e).context(
                "could not register with the controller and there is no saved tunnel address \
                 to fall back on",
            )),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ipv6_interface_address_round_trips_through_the_saved_file() {
        let cidr = interface_cidr("fd49::5", Some("fd49::/64"), None).unwrap();
        assert_eq!(cidr, "fd49::5/64");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tunnel-address");
        save(&path, &cidr).unwrap();
        assert_eq!(load(&path).as_deref(), Some("fd49::5/64"));
        let mask: defguard_wireguard_rs::net::IpAddrMask = cidr.parse().unwrap();
        assert_eq!(mask.cidr, 64);
    }

    #[test]
    fn tunnel_ip_accepts_both_families_but_not_ipv4_written_as_ipv6() {
        assert_eq!(tunnel_ip("10.60.0.5").unwrap().to_string(), "10.60.0.5");
        assert_eq!(tunnel_ip("fd49:0::5").unwrap().to_string(), "fd49::5");
        assert!(tunnel_ip("::ffff:10.60.0.5").is_err());
        assert!(tunnel_ip("::10.60.0.5").is_err());
        assert!(tunnel_ip("nonsense").is_err());
    }
    use crate::proxy_register::{RegisterError, Registered};

    fn registered(ip: &str, net: Option<&str>) -> Registered {
        Registered {
            revision: 1,
            tunnel_address: ip.into(),
            tunnel_network: net.map(str::to_string),
        }
    }

    #[test]
    fn a_controller_reported_address_outside_the_network_is_refused() {
        assert!(interface_cidr("10.99.0.5", Some("10.60.0.0/16"), None).is_err());
        assert!(interface_cidr("fd49::5", Some("10.60.0.0/16"), None).is_err());
        assert!(interface_cidr("10.60.0.5", Some("10.60.0.0/16"), None).is_ok());
    }

    #[test]
    fn the_networks_own_and_broadcast_addresses_are_refused() {
        assert!(interface_cidr("10.60.0.0", Some("10.60.0.0/16"), None).is_err());
        assert!(interface_cidr("10.60.255.255", Some("10.60.0.0/16"), None).is_err());
    }

    #[test]
    fn a_too_short_or_too_long_prefix_is_refused() {
        assert!(interface_cidr("10.0.0.5", Some("10.0.0.0/8"), None).is_err());
        assert!(interface_cidr("10.60.0.1", Some("10.60.0.0/31"), None).is_err());
        assert!(interface_cidr("fd49::5", Some("fd49::/48"), None).is_err());
        assert!(interface_cidr("fd49::5", Some("fd49::/64"), None).is_ok());
    }

    #[test]
    fn special_use_addresses_and_networks_are_refused() {
        for ip in [
            "127.0.0.1",
            "0.0.0.0",
            "224.0.0.5",
            "169.254.1.1",
            "fe80::5",
            "ff02::5",
            "::1",
        ] {
            assert!(
                interface_cidr(ip, None, Some("10.60.0.5/16")).is_err(),
                "{ip}"
            );
        }
        assert!(interface_cidr("127.0.0.5", Some("127.0.0.0/16"), None).is_err());
    }

    #[test]
    fn a_non_ip_address_or_a_malformed_network_is_refused() {
        assert!(interface_cidr("not-an-ip", Some("10.60.0.0/16"), None).is_err());
        assert!(interface_cidr("10.60.0.5", Some("10.60.0.0/x"), None).is_err());
        assert!(interface_cidr("::ffff:10.60.0.5", Some("10.60.0.0/16"), None).is_err());
    }

    #[test]
    fn ip_of_strips_the_prefix() {
        assert_eq!(ip_of("10.60.0.5/16"), "10.60.0.5");
        assert_eq!(ip_of("10.60.0.5"), "10.60.0.5");
    }

    #[test]
    fn the_networks_prefix_wins_over_a_pinned_one() {
        assert_eq!(
            interface_cidr("10.60.0.5", Some("10.60.0.0/16"), Some("10.60.0.5/24")).unwrap(),
            "10.60.0.5/16"
        );
    }

    #[test]
    fn pin_only_mode_uses_the_pinned_prefix_and_errors_without_one() {
        assert_eq!(
            interface_cidr("10.60.0.5", None, Some("10.60.0.5/24")).unwrap(),
            "10.60.0.5/24"
        );
        let err = interface_cidr("10.60.0.5", None, None)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("--tunnel-address"),
            "wrong flag name in: {err}"
        );
    }

    #[test]
    fn a_saved_address_round_trips_and_blank_or_missing_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tunnel-address");
        assert_eq!(load(&path), None);
        save(&path, "10.60.0.5/16").unwrap();
        assert_eq!(load(&path).as_deref(), Some("10.60.0.5/16"));
        std::fs::write(&path, "  \n").unwrap();
        assert_eq!(load(&path), None);
    }

    #[test]
    fn startup_uses_the_controllers_answer_when_it_registers() {
        let s = resolve_startup(
            Ok(registered("10.60.0.5", Some("10.60.0.0/16"))),
            None,
            Some("10.60.0.9/16".into()),
        )
        .unwrap();
        assert_eq!(s.cidr, "10.60.0.5/16");
        assert!(matches!(s.source, Source::Controller));
    }

    #[test]
    fn startup_falls_back_to_the_saved_address_when_the_controller_is_unreachable() {
        let s = resolve_startup(
            Err(RegisterError::Transient(anyhow::anyhow!(
                "connection refused"
            ))),
            None,
            Some("10.60.0.5/16".into()),
        )
        .unwrap();
        assert_eq!(s.cidr, "10.60.0.5/16");
        match s.source {
            Source::Saved { cause, pin_ignored } => {
                assert!(cause.contains("connection refused"), "{cause}");
                assert_eq!(pin_ignored, None);
            }
            other => panic!("expected Saved, got {other:?}"),
        }
    }

    #[test]
    fn a_fallback_says_when_it_overrides_a_changed_pin() {
        let s = resolve_startup(
            Err(RegisterError::Transient(anyhow::anyhow!(
                "connection refused"
            ))),
            Some("10.60.0.7/16"),
            Some("10.60.0.5/16".into()),
        )
        .unwrap();
        assert_eq!(s.cidr, "10.60.0.5/16");
        let Source::Saved { pin_ignored, .. } = s.source else {
            panic!("expected Saved");
        };
        let msg = pin_ignored.expect("a changed pin must be reported");
        assert!(
            msg.contains("10.60.0.7") && msg.contains("10.60.0.5"),
            "{msg}"
        );
        assert!(msg.contains("--tunnel-address"), "{msg}");

        // The same IP under another prefix is not a changed pin.
        let s = resolve_startup(
            Err(RegisterError::Transient(anyhow::anyhow!("down"))),
            Some("10.60.0.5/24"),
            Some("10.60.0.5/16".into()),
        )
        .unwrap();
        assert!(matches!(
            s.source,
            Source::Saved {
                pin_ignored: None,
                ..
            }
        ));
    }

    #[test]
    fn startup_fails_without_a_saved_address_when_the_controller_is_unreachable() {
        let err = resolve_startup(
            Err(RegisterError::Transient(anyhow::anyhow!(
                "connection refused"
            ))),
            None,
            None,
        )
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("no saved tunnel address"),
            "{err:#}"
        );
    }

    #[test]
    fn startup_never_falls_back_when_the_controller_refuses_the_registration() {
        // A 409 (address held) must not be papered over by a stale saved value.
        let err = resolve_startup(
            Err(RegisterError::Rejected(
                "already held by origin \"x\"".into(),
            )),
            None,
            Some("10.60.0.5/16".into()),
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("already held"), "{err:#}");
    }

    #[test]
    fn save_replaces_the_file_atomically_and_leaves_no_temporary_behind() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tunnel-address");
        save(&path, "10.60.0.5/16").unwrap();
        save(&path, "10.60.0.6/16").unwrap();
        assert_eq!(load(&path).as_deref(), Some("10.60.0.6/16"));
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from("tunnel-address")]);
    }

    #[test]
    fn a_failed_save_leaves_the_previous_file_intact() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tunnel-address");
        save(&path, "10.60.0.5/16").unwrap();
        // A directory squatting on the temporary's name makes the write fail
        // before the rename.
        std::fs::create_dir(dir.path().join("tunnel-address.tmp")).unwrap();
        assert!(save(&path, "10.60.0.6/16").is_err());
        assert_eq!(load(&path).as_deref(), Some("10.60.0.5/16"));
    }
}
