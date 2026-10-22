//! Where the HTTP server sends `/.well-known/acme-challenge/*`: to the controller, which
//! answers HTTP-01 challenges from its replicated store (`docs/acme.md`, "HTTP-01").

/// The path prefix the CA fetches an HTTP-01 answer from.
pub const CHALLENGE_PATH: &str = "/.well-known/acme-challenge/*";

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("--acme-challenge-upstream {value:?}: {why}")]
pub struct ForwardError {
    value: String,
    why: &'static str,
}

/// The controller nodes to forward to, all over http or all over https.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcmeForward {
    /// `host:port` dial strings, in the order given.
    pub dials: Vec<String>,
    pub tls: bool,
    /// A PEM file (a path on the Caddy side) to trust for an https controller.
    pub ca_file: Option<String>,
}

impl AcmeForward {
    /// `urls` is a comma-separated list such as `https://c1:7000,https://c2:7000`. Each is
    /// `http(s)://host[:port]` with no path, query or credentials.
    ///
    /// # Errors
    /// An empty list, a malformed URL, or a mix of http and https.
    pub fn parse(urls: &str, ca_file: Option<&str>) -> Result<Self, ForwardError> {
        let mut dials = Vec::new();
        let mut scheme: Option<bool> = None;
        for raw in urls.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            let bad = |why| ForwardError {
                value: raw.to_string(),
                why,
            };
            let (tls, rest) = if let Some(r) = raw.strip_prefix("https://") {
                (true, r)
            } else if let Some(r) = raw.strip_prefix("http://") {
                (false, r)
            } else {
                return Err(bad("must start with http:// or https://"));
            };
            if scheme.is_some_and(|s| s != tls) {
                return Err(bad("all controllers must use the same scheme"));
            }
            scheme = Some(tls);
            let rest = rest.strip_suffix('/').unwrap_or(rest);
            if rest.is_empty() || rest.contains(['/', '?', '#', '@']) {
                return Err(bad("must be http(s)://host[:port] only"));
            }
            // The URL parser rejects an empty host, a port above 65535 and malformed IPv6
            // literals (`http://:7000`, `c1:99999`, `http://[::1`, an unbracketed `::1`).
            let url =
                reqwest::Url::parse(&format!("{}://{rest}", if tls { "https" } else { "http" }))
                    .map_err(|_| bad("not a valid host[:port]"))?;
            let (Some(host), Some(port)) = (url.host_str(), url.port_or_known_default()) else {
                return Err(bad("not a valid host[:port]"));
            };
            let dial = format!("{host}:{port}");
            if !dials.contains(&dial) {
                dials.push(dial);
            }
        }
        let Some(tls) = scheme else {
            return Err(ForwardError {
                value: urls.to_string(),
                why: "no controller given",
            });
        };
        if ca_file.is_some() && !tls {
            return Err(ForwardError {
                value: urls.to_string(),
                why: "a CA file only applies to https controllers",
            });
        }
        Ok(Self {
            dials,
            tls,
            ca_file: ca_file.map(ToString::to_string),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(s: &str) -> AcmeForward {
        AcmeForward::parse(s, None).unwrap()
    }

    #[test]
    fn urls_become_dial_strings_with_default_ports() {
        let f = ok("https://c1.internal, https://c2.internal:7000/");
        assert_eq!(f.dials, ["c1.internal:443", "c2.internal:7000"]);
        assert!(f.tls);
        let f = ok("http://10.0.0.5:7000,http://10.0.0.5:7000,http://[::1]:7000,http://[::1]");
        assert_eq!(f.dials, ["10.0.0.5:7000", "[::1]:7000", "[::1]:80"]);
        assert!(!f.tls);
    }

    #[test]
    fn hosts_are_normalized_and_ports_range_checked() {
        assert_eq!(ok("https://C1.Internal:65535").dials, ["c1.internal:65535"]);
        assert_eq!(ok("http://10.0.0.5").dials, ["10.0.0.5:80"]);
        assert_eq!(
            ok("http://[2001:db8::1]:7000").dials,
            ["[2001:db8::1]:7000"]
        );
    }

    #[test]
    fn malformed_lists_are_refused() {
        for bad in [
            "",
            " , ",
            "c1:7000",
            "ftp://c1",
            "https://",
            "https://c1/path",
            "https://u:p@c1",
            "https://c1?x=1",
            "https://c1,http://c2",
            "http://:7000",
            "https://c1:99999",
            "http://[::1",
            "http://::1",
            "https://a b",
        ] {
            assert!(AcmeForward::parse(bad, None).is_err(), "{bad:?}");
        }
        assert!(AcmeForward::parse("http://c1", Some("/ca.pem")).is_err());
        assert!(AcmeForward::parse("https://c1", Some("/ca.pem")).is_ok());
    }
}
