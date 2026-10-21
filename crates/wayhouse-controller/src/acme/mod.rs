//! Controller-managed ACME certificates (design:
//! `docs/superpowers/specs/2026-10-15-controller-acme-design.md`).

pub mod api;
#[cfg(feature = "acme")]
pub mod dns_plugin;
#[cfg(feature = "acme")]
pub mod runner;
pub mod store;
pub mod types;

pub use store::{AcmeStore, AcmeStoreError};
pub use types::{AcmeOp, AcmeSnapshot, Applied, Attempt, Site, StoredAccount, StoredCert};

/// The rolling-upgrade gate for ACME writes under HA: every member must run a build that can
/// decode the entries. A member that does not answer blocks too, like a plugin install.
pub async fn check_support(ha: &crate::ha::HaHandle) -> Result<(), String> {
    check_support_answers(&crate::plugins::peer::whoami_all(ha).await)
}

/// `Ok` when every answer reports a build that decodes ACME entries.
pub fn check_support_answers(
    answers: &[(
        crate::ha::NodeId,
        Result<crate::ha::routes::Whoami, crate::ha::members::WhoamiError>,
    )],
) -> Result<(), String> {
    for (id, answer) in answers {
        match answer {
            Err(why) => {
                return Err(format!(
                    "node {id} did not answer ({why}); ACME changes wait until every member reports its build"
                ))
            }
            Ok(who) if who.acme_support < crate::ha::routes::ACME_SUPPORT => {
                return Err(format!(
                    "node {id} runs a build without ACME support; upgrade every replica before changing ACME sites"
                ))
            }
            Ok(_) => {}
        }
    }
    Ok(())
}

/// The CA directory URL for `--acme-ca`: a preset name or an https URL. The CA client is
/// https only, so plain http is refused here rather than failing every order later.
pub fn resolve_ca(value: &str) -> Result<String, String> {
    match value {
        "letsencrypt" => return Ok("https://acme-v02.api.letsencrypt.org/directory".into()),
        "letsencrypt-staging" => {
            return Ok("https://acme-staging-v02.api.letsencrypt.org/directory".into())
        }
        _ => {}
    }
    let bad = || {
        format!("--acme-ca {value:?} must be letsencrypt, letsencrypt-staging or an https:// URL")
    };
    let url = url::Url::parse(value).map_err(|_| bad())?;
    match url.scheme() {
        "https" if url.has_host() && url.username().is_empty() => Ok(url.into()),
        _ => Err(bad()),
    }
}

/// True when the URL's host is `localhost` or a loopback address (parsed, not a prefix test).
fn is_loopback_host(url: &url::Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(d)) => d.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

/// True when certificates from this CA directory are not publicly trusted: a staging CA (a
/// host or path that says so) or a loopback test CA. Their certificates are stored marked
/// `untrusted` and must never be served to real clients.
pub fn untrusted_ca(directory_url: &str) -> bool {
    let Ok(url) = url::Url::parse(directory_url) else {
        return true;
    };
    is_loopback_host(&url)
        || url
            .host_str()
            .is_some_and(|h| h.to_ascii_lowercase().contains("staging"))
        || url.path().to_ascii_lowercase().contains("staging")
}

/// `--acme` moves the CA account key and certificate keys through the API only as sealed
/// ciphertext, but the site and account endpoints still carry an admin token; a plain-HTTP
/// listener on a non-loopback address would send it in clear text.
pub fn check_listener(plain_remote: bool, allow_insecure: bool) -> Result<(), String> {
    if plain_remote && !allow_insecure {
        return Err(
            "--acme on a plain-HTTP, non-loopback --listen would send the admin token and ACME \
             site changes in clear text: serve TLS (--tls-cert/--tls-key), listen on loopback \
             behind a TLS terminator, or pass --allow-insecure-secrets"
                .into(),
        );
    }
    Ok(())
}

/// Startup checks for `--acme`. Returns the CA directory URL.
pub fn check_flags(ca: Option<&str>, have_keyring: bool, slave: bool) -> Result<String, String> {
    if slave {
        return Err("--acme cannot run on a --role slave controller".into());
    }
    if !have_keyring {
        return Err(
            "--acme needs the cluster secret key (--secret-key-file or WAYHOUSE_PLUGIN_SECRET_KEY): \
             the account and certificate keys are stored sealed"
                .into(),
        );
    }
    let ca = ca.ok_or(
        "--acme needs --acme-ca (letsencrypt, letsencrypt-staging or an https:// directory URL); \
         there is no default CA",
    )?;
    resolve_ca(ca)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ca_presets_and_urls() {
        assert!(resolve_ca("letsencrypt")
            .unwrap()
            .starts_with("https://acme-v02"));
        assert!(resolve_ca("letsencrypt-staging")
            .unwrap()
            .contains("staging"));
        assert_eq!(
            resolve_ca("https://ca.example/dir").unwrap(),
            "https://ca.example/dir"
        );
        assert!(resolve_ca("http://127.0.0.1:14000/dir").is_err());
        assert!(resolve_ca("https://user:pw@ca.example/dir").is_err());
        assert!(resolve_ca("http://ca.example/dir").is_err());
        assert!(resolve_ca("http://localhost.example.net/dir").is_err());
        assert!(resolve_ca("http://127.0.0.1.example.net/dir").is_err());
        assert!(resolve_ca("http://127.0.0.1@ca.example/dir").is_err());
        assert!(resolve_ca("https://localhost:14000/dir").is_ok());
        assert!(resolve_ca("zerossl").is_err());
    }

    #[test]
    fn acme_needs_a_keyring_a_ca_and_a_writable_tier() {
        assert!(check_flags(Some("letsencrypt"), true, false).is_ok());
        assert!(check_flags(Some("letsencrypt"), false, false)
            .unwrap_err()
            .contains("secret key"));
        assert!(check_flags(None, true, false)
            .unwrap_err()
            .contains("no default CA"));
        assert!(check_flags(Some("letsencrypt"), true, true)
            .unwrap_err()
            .contains("slave"));
    }

    #[test]
    fn acme_refuses_a_plain_remote_listener_unless_allowed() {
        assert!(check_listener(true, false)
            .unwrap_err()
            .contains("--allow-insecure-secrets"));
        assert!(check_listener(true, true).is_ok());
        assert!(check_listener(false, false).is_ok());
    }

    #[test]
    fn staging_and_local_cas_issue_untrusted_certificates() {
        assert!(untrusted_ca(&resolve_ca("letsencrypt-staging").unwrap()));
        assert!(untrusted_ca("https://acme.staging.example/dir"));
        assert!(untrusted_ca("http://127.0.0.1:14000/dir"));
        assert!(!untrusted_ca(&resolve_ca("letsencrypt").unwrap()));
        assert!(!untrusted_ca("https://ca.example/dir"));
    }

    #[test]
    fn writes_wait_until_every_member_reports_acme_support() {
        use crate::ha::members::WhoamiError;
        let who = |acme_support| crate::ha::routes::Whoami {
            node_id: 2,
            log_empty: false,
            pre_ha: crate::ha::import::PreHaSummary::default(),
            plugin_support: 0,
            secret_key_ids: Vec::new(),
            acme_support,
        };
        assert!(
            check_support_answers(&[]).is_ok(),
            "a single node has no peers"
        );
        assert!(check_support_answers(&[(2, Ok(who(1)))]).is_ok());
        let old = check_support_answers(&[(2, Ok(who(1))), (3, Ok(who(0)))]).unwrap_err();
        assert!(old.contains("node 3") && old.contains("upgrade"), "{old}");
        let silent = check_support_answers(&[(
            2,
            Err(WhoamiError::Unreachable("connection refused".into())),
        )])
        .unwrap_err();
        assert!(silent.contains("did not answer"), "{silent}");
    }
}
