//! CIDR blocks, compiled prefix sets, and the per-listener source filters.

use std::net::IpAddr;

/// An IPv4 or IPv6 CIDR block, parsed from `addr/prefix`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cidr {
    base: IpAddr,
    prefix: u8,
}

impl std::fmt::Display for Cidr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.base, self.prefix)
    }
}

impl Cidr {
    /// Parse `"10.0.0.0/8"` / `"2001:db8::/32"`.
    pub fn parse(s: &str) -> Result<Self, String> {
        let (addr, prefix) = s
            .split_once('/')
            .ok_or_else(|| format!("CIDR {s:?} is missing a '/prefix'"))?;
        let base: IpAddr = addr
            .parse()
            .map_err(|_| format!("CIDR {s:?} has an invalid IP address"))?;
        let prefix: u8 = prefix
            .parse()
            .map_err(|_| format!("CIDR {s:?} has an invalid prefix length"))?;
        let max = if base.is_ipv4() { 32 } else { 128 };
        if prefix > max {
            return Err(format!("CIDR {s:?} prefix /{prefix} exceeds /{max}"));
        }
        Ok(Self { base, prefix })
    }

    /// The prefix length; 0 matches every address of the family.
    pub fn prefix_len(&self) -> u8 {
        self.prefix
    }

    /// Does `ip` fall within this block? An IPv4-mapped IPv6 address
    /// (`::ffff:a.b.c.d`) is matched as the IPv4 address it carries.
    pub fn contains(&self, ip: IpAddr) -> bool {
        match (self.canonical().base, ip.to_canonical()) {
            (IpAddr::V4(b), IpAddr::V4(x)) => bits_match(&b.octets(), &x.octets(), self.prefix),
            (IpAddr::V6(b), IpAddr::V6(x)) => bits_match(&b.octets(), &x.octets(), self.prefix),
            _ => false,
        }
    }
}

impl Cidr {
    /// A mapped-range block (`::ffff:a.b.c.d/p`, p >= 96) as the equivalent IPv4
    /// block, so it matches the canonical IPv4 form clients are seen as.
    fn canonical(&self) -> Self {
        if let IpAddr::V6(a) = self.base {
            if let (Some(v4), true) = (a.to_ipv4_mapped(), self.prefix >= 96) {
                return Self {
                    base: IpAddr::V4(v4),
                    prefix: self.prefix - 96,
                };
            }
        }
        self.clone()
    }
}

fn bits_match(a: &[u8], b: &[u8], prefix: u8) -> bool {
    let full = (prefix / 8) as usize;
    if a[..full] != b[..full] {
        return false;
    }
    let rem = prefix % 8;
    if rem == 0 {
        return true;
    }
    let mask = 0xffu8 << (8 - rem);
    (a[full] & mask) == (b[full] & mask)
}

/// A set of CIDR prefixes with O(prefix-length) membership tests: a binary radix
/// trie over address bits, IPv4 and IPv6 kept separate. Built once (per listener
/// spawn, through [`Acl`]) so a large deny / allow list — bogons plus a threat
/// feed, thousands of entries — costs a bounded bit-walk per connection instead
/// of a linear scan.
#[derive(Debug, Clone, Default)]
pub struct CidrSet {
    v4: Option<Box<TrieNode>>,
    v6: Option<Box<TrieNode>>,
    len: usize,
}

#[derive(Debug, Clone, Default)]
struct TrieNode {
    /// A prefix ends here — every address that reaches this node is covered.
    terminal: bool,
    children: [Option<Box<TrieNode>>; 2],
}

fn nth_bit(octets: &[u8], i: usize) -> usize {
    ((octets[i / 8] >> (7 - (i % 8))) & 1) as usize
}

impl CidrSet {
    pub fn build(cidrs: &[Cidr]) -> Self {
        let mut set = Self::default();
        for c in cidrs {
            let c = c.canonical();
            match c.base {
                IpAddr::V4(a) => Self::insert(&mut set.v4, &a.octets(), c.prefix),
                IpAddr::V6(a) => Self::insert(&mut set.v6, &a.octets(), c.prefix),
            }
            set.len += 1;
        }
        set
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn len(&self) -> usize {
        self.len
    }

    /// Does any prefix in the set cover `ip`?
    pub fn contains(&self, ip: IpAddr) -> bool {
        match ip.to_canonical() {
            IpAddr::V4(a) => Self::walk(&self.v4, &a.octets()),
            IpAddr::V6(a) => Self::walk(&self.v6, &a.octets()),
        }
    }

    fn insert(root: &mut Option<Box<TrieNode>>, octets: &[u8], prefix: u8) {
        let mut cur = root
            .get_or_insert_with(|| Box::new(TrieNode::default()))
            .as_mut();
        for i in 0..prefix as usize {
            if cur.terminal {
                return; // already covered by a shorter prefix
            }
            cur = cur.children[nth_bit(octets, i)]
                .get_or_insert_with(|| Box::new(TrieNode::default()))
                .as_mut();
        }
        cur.terminal = true;
        cur.children = [None, None]; // this prefix subsumes anything longer
    }

    fn walk(root: &Option<Box<TrieNode>>, octets: &[u8]) -> bool {
        let mut node = match root {
            Some(n) => n.as_ref(),
            None => return false,
        };
        if node.terminal {
            return true; // a /0 in the set
        }
        for i in 0..octets.len() * 8 {
            match &node.children[nth_bit(octets, i)] {
                Some(c) => {
                    node = c;
                    if node.terminal {
                        return true;
                    }
                }
                None => return false,
            }
        }
        false
    }
}

/// Per-listener source-IP filter, checked before routing (phase 7). Empty =
/// admit everyone. `deny` is checked first and wins; a non-empty `allow` then
/// makes the listener default-deny for anything it does not cover. The `Cidr`
/// vecs are kept for equality (reload diffing) and display; matching goes
/// through the compiled [`CidrSet`]s.
#[derive(Debug, Clone, Default)]
pub struct Acl {
    pub allow: Vec<Cidr>,
    pub deny: Vec<Cidr>,
    allow_set: CidrSet,
    deny_set: CidrSet,
}

impl PartialEq for Acl {
    fn eq(&self, other: &Self) -> bool {
        self.allow == other.allow && self.deny == other.deny
    }
}
impl Eq for Acl {}

impl Acl {
    pub fn new(allow: Vec<Cidr>, deny: Vec<Cidr>) -> Self {
        let allow_set = CidrSet::build(&allow);
        let deny_set = CidrSet::build(&deny);
        Self {
            allow,
            deny,
            allow_set,
            deny_set,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.allow.is_empty() && self.deny.is_empty()
    }

    /// Is a connection / datagram from `ip` admitted?
    pub fn permits(&self, ip: IpAddr) -> bool {
        if self.deny_set.contains(ip) {
            return false;
        }
        if !self.allow.is_empty() && !self.allow_set.contains(ip) {
            return false;
        }
        true
    }
}

/// Per-listener GeoIP country filter (phase 7), checked after the CIDR [`Acl`]
/// on the client source IP. Codes are ISO 3166-1 alpha-2, upper-cased at parse.
/// Same precedence as `Acl`: `deny` wins; a non-empty `allow` is default-deny.
/// The country lookup itself lives in `wayhouse-core` (needs the MaxMind DB).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GeoAcl {
    pub allow: Vec<[u8; 2]>,
    pub deny: Vec<[u8; 2]>,
}

impl GeoAcl {
    /// Admit a client whose source IP resolves to `country` (`None` = the IP is
    /// not in the database). An unknown country is admitted only when there is
    /// no `allow` list to fail closed against.
    pub fn permits(&self, country: Option<[u8; 2]>) -> bool {
        match country {
            Some(cc) => {
                if self.deny.contains(&cc) {
                    return false;
                }
                if !self.allow.is_empty() && !self.allow.contains(&cc) {
                    return false;
                }
                true
            }
            None => self.allow.is_empty(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cidr_contains_v4_and_v6() {
        let c = Cidr::parse("10.0.0.0/8").unwrap();
        assert!(c.contains("10.255.1.1".parse().unwrap()));
        assert!(!c.contains("11.0.0.1".parse().unwrap()));
        let c6 = Cidr::parse("2001:db8::/32").unwrap();
        assert!(c6.contains("2001:db8:dead:beef::1".parse().unwrap()));
        assert!(!c6.contains("2001:db9::1".parse().unwrap()));
        assert!(!c.contains("2001:db8::1".parse().unwrap()));
    }

    #[test]
    fn cidr_set_membership_v4_v6_and_prefix_subsumption() {
        let cidrs: Vec<Cidr> = ["10.0.0.0/8", "192.168.1.0/24", "2001:db8::/32"]
            .iter()
            .map(|s| Cidr::parse(s).unwrap())
            .collect();
        let set = CidrSet::build(&cidrs);
        assert_eq!(set.len(), 3);
        assert!(set.contains("10.9.9.9".parse().unwrap()));
        assert!(set.contains("192.168.1.200".parse().unwrap()));
        assert!(!set.contains("192.168.2.1".parse().unwrap()));
        assert!(!set.contains("11.0.0.1".parse().unwrap()));
        assert!(set.contains("2001:db8:dead::1".parse().unwrap()));
        assert!(!set.contains("2001:db9::1".parse().unwrap()));

        // A shorter prefix already in the set makes a longer one redundant, and
        // the reverse insertion order still yields the covering answer.
        let s2 = CidrSet::build(
            &["10.1.2.0/24", "10.0.0.0/8"]
                .iter()
                .map(|s| Cidr::parse(s).unwrap())
                .collect::<Vec<_>>(),
        );
        assert!(s2.contains("10.1.2.3".parse().unwrap()));
        assert!(s2.contains("10.240.0.1".parse().unwrap()));

        // The empty set matches nothing; a /0 matches everything.
        assert!(!CidrSet::default().contains("8.8.8.8".parse().unwrap()));
        let all_v4 = CidrSet::build(&[Cidr::parse("0.0.0.0/0").unwrap()]);
        assert!(all_v4.contains("1.2.3.4".parse().unwrap()));
        assert!(!all_v4.contains("::1".parse().unwrap()));
    }

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn ipv4_mapped_spellings_hit_ipv4_deny_and_allow_lists() {
        let acl = Acl::new(
            vec![Cidr::parse("10.0.0.0/8").unwrap()],
            vec![Cidr::parse("10.9.0.0/16").unwrap()],
        );
        for denied in [
            "::ffff:10.9.1.2",
            "::FFFF:0a09:0102",
            "0:0:0:0:0:ffff:10.9.1.2",
        ] {
            assert!(!acl.permits(ip(denied)), "{denied} must be denied");
        }
        for allowed in ["::ffff:10.1.2.3", "::ffff:0a01:0203"] {
            assert!(acl.permits(ip(allowed)), "{allowed} must be allowed");
        }
        assert!(!acl.permits(ip("::ffff:192.0.2.1")), "outside allow list");
    }

    #[test]
    fn cidr_contains_sees_through_ipv4_mapped_addresses() {
        let c = Cidr::parse("203.0.113.0/24").unwrap();
        assert!(c.contains(ip("::ffff:203.0.113.9")));
        assert!(!c.contains(ip("::ffff:203.0.114.9")));
    }

    #[test]
    fn mapped_form_cidr_in_config_matches_plain_ipv4_clients() {
        let acl = Acl::new(vec![], vec![Cidr::parse("::ffff:203.0.113.0/120").unwrap()]);
        assert!(!acl.permits(ip("203.0.113.9")));
        assert!(!acl.permits(ip("::ffff:203.0.113.9")));
        assert!(acl.permits(ip("203.0.114.9")));
    }

    #[test]
    fn non_mapped_ipv6_is_untouched_by_canonicalisation() {
        let acl = Acl::new(vec![], vec![Cidr::parse("2001:db8::/32").unwrap()]);
        assert!(!acl.permits(ip("2001:db8::1")));
        assert!(acl.permits(ip("::1")));
        assert!(
            acl.permits(ip("::203.0.113.9")),
            "IPv4-compatible is not mapped"
        );
    }
}
