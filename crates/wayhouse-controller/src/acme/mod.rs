//! Controller-managed ACME certificates (design:
//! `docs/superpowers/specs/2026-10-15-controller-acme-design.md`).

pub mod api;
pub mod runner;
pub mod store;
pub mod types;

pub use store::{AcmeStore, AcmeStoreError};
pub use types::{AcmeOp, AcmeSnapshot, Applied, Attempt, Site, StoredAccount, StoredCert};

/// The rolling-upgrade gate for ACME writes under HA: every member must run a build that can
/// decode the entries. A member that does not answer blocks too, like a plugin install.
pub async fn check_support(ha: &crate::ha::HaHandle) -> Result<(), String> {
    for (id, answer) in crate::plugins::peer::whoami_all(ha).await {
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

/// The CA directory URL for `--acme-ca`: a preset name or an https URL (plain http only
/// for a loopback CA, which is what the tests and a local Pebble use).
pub fn resolve_ca(value: &str) -> Result<String, String> {
    match value {
        "letsencrypt" => Ok("https://acme-v02.api.letsencrypt.org/directory".into()),
        "letsencrypt-staging" => {
            Ok("https://acme-staging-v02.api.letsencrypt.org/directory".into())
        }
        url if url.starts_with("https://") => Ok(url.into()),
        url if url.starts_with("http://127.0.0.1") || url.starts_with("http://localhost") => {
            Ok(url.into())
        }
        _ => Err(format!(
            "--acme-ca {value:?} must be letsencrypt, letsencrypt-staging or an https:// URL"
        )),
    }
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
        assert!(resolve_ca("http://127.0.0.1:14000/dir").is_ok());
        assert!(resolve_ca("http://ca.example/dir").is_err());
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
}
