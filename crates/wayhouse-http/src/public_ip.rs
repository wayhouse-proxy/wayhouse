//! The one special-use address allowlist shared by every outbound path that must
//! not be steered at internal infrastructure (the UI's registry client and the
//! plugin host's `http` capability), so the two cannot drift.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// True for an address a client may legitimately be sent to: anything that is
/// not loopback, private, link-local, unspecified, "this network", multicast,
/// broadcast, CGNAT, benchmarking, documentation, reserved, site-local or
/// unique-local. An IPv4-mapped IPv6 address is judged as its IPv4 form.
pub fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_public_v4(v4),
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => is_public_v4(v4),
            None => is_public_v6(v6),
        },
    }
}

fn is_public_v4(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    let cgnat = o[0] == 100 && (o[1] & 0xc0) == 64;
    let this_network = o[0] == 0; // 0.0.0.0/8
    let protocol_assignments = o[..3] == [192, 0, 0]; // 192.0.0.0/24
    let benchmarking = o[0] == 198 && (o[1] & 0xfe) == 18; // 198.18.0.0/15
    let reserved = o[0] >= 240; // 240.0.0.0/4 (255.255.255.255 is also broadcast)
    !(this_network
        || protocol_assignments
        || benchmarking
        || reserved
        || ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_documentation()
        || cgnat)
}

fn is_public_v6(ip: Ipv6Addr) -> bool {
    let s = ip.segments();
    let unique_local = (s[0] & 0xfe00) == 0xfc00;
    let link_local = (s[0] & 0xffc0) == 0xfe80;
    let site_local = (s[0] & 0xffc0) == 0xfec0; // fec0::/10, deprecated
    let documentation = s[0] == 0x2001 && s[1] == 0x0db8; // 2001:db8::/32
    let teredo = s[0] == 0x2001 && s[1] == 0; // 2001::/32
    let discard = s[0] == 0x0100 && s[1..4].iter().all(|&x| x == 0); // 100::/64
    let documentation_3fff = s[0] == 0x3fff && (s[1] & 0xf000) == 0; // 3fff::/20
    let benchmarking = s[0] == 0x2001 && s[1] == 2 && s[2] == 0; // 2001:2::/48
    let local_nat64 = s[0] == 0x0064 && s[1] == 0xff9b && s[2] == 1; // 64:ff9b:1::/48
    let orchid = s[0] == 0x2001 && matches!(s[1] & 0xfff0, 0x0010 | 0x0020); // 2001:10::/28, 2001:20::/28
    let srv6_sids = s[0] == 0x5f00; // 5f00::/16
    if ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || unique_local
        || link_local
        || site_local
        || documentation
        || teredo
        || discard
        || documentation_3fff
        || benchmarking
        || local_nat64
        || orchid
        || srv6_sids
    {
        return false;
    }
    // An IPv4 address carried inside IPv6 is judged as that IPv4 address: a DNS64/NAT64
    // gateway, a 6to4 relay or an old IPv4-compatible address would reach it.
    let embedded = |hi: u16, lo: u16| {
        let [a, b] = hi.to_be_bytes();
        let [c, d] = lo.to_be_bytes();
        Ipv4Addr::new(a, b, c, d)
    };
    let nat64 = s[0] == 0x0064 && s[1] == 0xff9b && s[2..6].iter().all(|&x| x == 0);
    let ipv4_compatible = s[..6].iter().all(|&x| x == 0);
    let siit = s[..4].iter().all(|&x| x == 0) && s[4] == 0xffff && s[5] == 0; // ::ffff:0:0:0/96
    if nat64 || ipv4_compatible || siit {
        return is_public_v4(embedded(s[6], s[7]));
    }
    if s[0] == 0x2002 {
        return is_public_v4(embedded(s[1], s[2]));
    }
    true
}

#[cfg(test)]
mod tests {
    use super::is_public;

    #[test]
    fn refuses_special_use_addresses() {
        for private in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "0.0.0.0",
            "100.64.0.1",
            "224.0.0.1",
            "255.255.255.255",
            "192.0.2.1",
            "192.0.0.8",
            "0.1.2.3",
            "198.18.0.1",
            "198.19.255.255",
            "240.0.0.1",
            "::1",
            "::",
            "fe80::1",
            "fec0::1",
            "fc00::1",
            "fd12:3456::1",
            "ff02::1",
            "::ffff:10.0.0.1",
            "::ffff:198.18.0.1",
            "2001:db8::1",
            "2001::1",
            "100::1",
            "64:ff9b::a00:1",
            "64:ff9b::a9fe:a9fe",
            "2002:a00:1::1",
            "::10.0.0.1",
            "::a9fe:a9fe",
            "2001:0:4136:e378:8000:63bf:3fff:fdd2",
            // Newer special-use IPv6 ranges (#240, #317.4, #294.4).
            "3fff::1",
            "3fff:fff:ffff::1",
            "2001:2::1",
            "2001:2:0:ffff::1",
            "64:ff9b:1::1",
            "64:ff9b:1:ffff::1",
            "::ffff:0:a00:1",
            "::ffff:0:7f00:1",
            "2001:10::1",
            "2001:1f:ffff::1",
            "2001:20::1",
            "2001:2f:ffff::1",
            "5f00::1",
            "5f00:ffff::1",
        ] {
            assert!(!is_public(private.parse().unwrap()), "{private}");
        }
    }

    #[test]
    fn keeps_public_addresses_and_neighbours_of_refused_ranges() {
        for public in [
            "1.1.1.1",
            "8.8.8.8",
            "140.82.112.3",
            "198.17.255.255",
            "198.20.0.1",
            "192.0.1.1",
            "223.255.255.255",
            "2606:4700::1111",
            "2001:4860:4860::8888",
            "2001:db9::1",
            "64:ff9b::808:808",
            "2002:808:808::",
            "::ffff:8.8.8.8",
            "::ffff:0:808:808",
            // Just outside each newly refused range.
            "3ffe::1",
            "4000::1",
            "2001:3::1",
            "2001:1::1",
            "2001:30::1",
            "2001:f::1",
            "64:ff9b:2::1",
            "5e00::1",
            "5f01::1",
        ] {
            assert!(is_public(public.parse().unwrap()), "{public}");
        }
    }
}
