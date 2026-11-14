//! The rule that a bearer token never travels over plain `http://` beyond loopback, shared by
//! every client binary (the `server` feature is not needed: `wayhouse-agent` is a client only).

static ALLOW_INSECURE_CONTROLLER: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Records the operator's `--allow-insecure-controller` for call sites deep in a binary that
/// have no `Args` (a later fetch builds its URL itself). Set once, at start, by `main`.
pub fn set_allow_insecure_controller(allow: bool) {
    ALLOW_INSECURE_CONTROLLER.store(allow, std::sync::atomic::Ordering::Relaxed);
}

/// Refuses to start a client that would send `token` over plain `http://` to a non-loopback
/// host: anyone on the path could read it and then act as this process. `flag` names the URL
/// option. `--allow-insecure-controller` turns the refusal into a warning, for a network that
/// is private by construction (a container network, a trusted overlay).
pub fn check_bearer_transport(
    flag: &str,
    url: &str,
    has_token: bool,
    allow_insecure: bool,
) -> Result<(), String> {
    if !has_token || !sends_bearer_in_clear(url) {
        return Ok(());
    }
    let url = redact_userinfo(url);
    if allow_insecure {
        tracing::warn!(
            flag,
            url = %url,
            "bearer token is sent over plain http to a non-loopback host (--allow-insecure-controller)"
        );
        return Ok(());
    }
    Err(format!(
        "{flag} is {url}: its bearer token would travel over plain http. Use an https:// URL, \
         or pass --allow-insecure-controller if this network is private"
    ))
}

/// [`check_bearer_transport`] for a call site with no `Args`: uses the flag recorded by
/// [`set_allow_insecure_controller`].
pub fn check_bearer_transport_global(what: &str, url: &str, has_token: bool) -> Result<(), String> {
    check_bearer_transport(
        what,
        url,
        has_token,
        ALLOW_INSECURE_CONTROLLER.load(std::sync::atomic::Ordering::Relaxed),
    )
}

/// Only `scheme://host[:port]` of `url`: no userinfo, path, query or fragment,
/// any of which may carry a credential. Safe to log.
pub(crate) fn redact_userinfo(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_string();
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = authority.rsplit('@').next().unwrap_or_default();
    format!("{scheme}://{host}")
}

/// Decides from the same parse every request goes through (`url::Url`), so a spelling the
/// parser accepts cannot slip past a hand-cut check. An unparsable URL counts as in the clear.
pub(crate) fn sends_bearer_in_clear(url: &str) -> bool {
    let Ok(url) = url::Url::parse(url) else {
        return true;
    };
    if url.scheme() != "http" {
        return false;
    }
    let loopback = match url.host() {
        Some(url::Host::Domain(d)) => d.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.to_canonical().is_loopback(),
        None => false,
    };
    !loopback
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plaintext_bearer_is_flagged_only_for_non_loopback_http() {
        for clear in [
            "http://controller.example:8080",
            "http://10.0.0.5:8080/path",
            "HTTP://[2001:db8::1]:8080",
            "http://user@host.example",
            // Forms the `url` crate (what a request goes through) accepts but a hand-cut
            // parse misreads: backslash, missing slashes, leading space, embedded tab.
            "http://10.0.0.4\\@127.0.0.1:9900",
            "http:10.0.0.4:9900",
            "http:/10.0.0.4",
            " http://10.0.0.4",
            "ht\ttp://10.0.0.4",
            // Unparsable: treated as in the clear rather than waved through.
            "http://",
        ] {
            assert!(sends_bearer_in_clear(clear), "{clear}");
        }
        for fine in [
            "https://controller.example",
            "http://127.0.0.1:8080",
            "http://localhost:8080",
            "http://[::1]:8080/x",
            "http://[::ffff:127.0.0.1]:80",
            "unix:///run/wayhouse.sock",
        ] {
            assert!(!sends_bearer_in_clear(fine), "{fine}");
        }
    }

    #[test]
    fn a_token_over_plain_http_beyond_loopback_is_refused_unless_opted_out() {
        let e = check_bearer_transport("--controller", "http://c.example:9901", true, false)
            .unwrap_err();
        assert!(
            e.contains("--controller") && e.contains("--allow-insecure-controller"),
            "{e}"
        );
        for ok in [
            ("http://c.example", false, false),
            ("https://c.example", true, false),
            ("http://127.0.0.1:1", true, false),
            ("http://c.example", true, true),
        ] {
            assert!(
                check_bearer_transport("--x", ok.0, ok.1, ok.2).is_ok(),
                "{ok:?}"
            );
        }
    }

    #[test]
    fn logged_urls_carry_no_credentials() {
        assert_eq!(
            redact_userinfo("http://user:pw@host.example:8080/a?token=s3cret#c"),
            "http://host.example:8080"
        );
        assert_eq!(
            redact_userinfo("http://host.example/x@y"),
            "http://host.example"
        );
        assert_eq!(redact_userinfo("not a url"), "not a url");
    }
}
