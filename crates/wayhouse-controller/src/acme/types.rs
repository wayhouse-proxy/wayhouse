//! What the controller stores and replicates about ACME: sites, the account, certificates
//! (private keys sealed) and the retry state of each site.

use serde::{Deserialize, Serialize};

use crate::plugins::secrets::Sealed;

/// The sealing scope of everything ACME stores: `install = "acme"`, and the slot below.
pub const SEAL_SCOPE: &str = "acme";
pub const ACCOUNT_SLOT: &str = "account";

/// The sealing slot of a certificate key, so a ciphertext copied to another site or version
/// fails to authenticate.
pub fn cert_slot(site: &str, version: u64) -> String {
    format!("cert:{site}@{version}")
}

/// A published site: the names one certificate covers and where it is served.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Site {
    pub name: String,
    /// Lowercase hostnames, optionally `*.zone`.
    pub names: Vec<String>,
    /// Gateways that may fetch this site's key. A wildcard needs at least one.
    #[serde(default)]
    pub gateways: Vec<String>,
    /// The plugin install that sets and removes the DNS TXT records.
    pub dns_plugin: String,
    #[serde(default = "default_propagation")]
    pub propagation_timeout_secs: u64,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_propagation() -> u64 {
    120
}

fn default_true() -> bool {
    true
}

impl Site {
    /// A site name: lowercase letters, digits and dashes, 1 to 63 bytes.
    pub fn valid_name(name: &str) -> bool {
        !name.is_empty()
            && name.len() <= 63
            && !name.starts_with('-')
            && name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    }

    /// Why this site cannot be stored, or `Ok`.
    pub fn validate(&self) -> Result<(), String> {
        if !Self::valid_name(&self.name) {
            return Err("a site name is 1 to 63 of a-z, 0-9 and '-'".into());
        }
        wayhouse_acme::validate_names(&self.names).map_err(|e| e.to_string())?;
        if self.dns_plugin.is_empty() {
            return Err("dns_plugin is required".into());
        }
        if self.gateways.iter().any(|g| !Self::valid_name(g)) {
            return Err("a gateway name is 1 to 63 of a-z, 0-9 and '-'".into());
        }
        let mut seen = std::collections::BTreeSet::new();
        if !self.gateways.iter().all(|g| seen.insert(g)) {
            return Err("duplicate gateway".into());
        }
        if self.names.iter().any(|n| n.starts_with("*.")) && self.gateways.is_empty() {
            return Err("a wildcard site must name at least one gateway".into());
        }
        if !(10..=3600).contains(&self.propagation_timeout_secs) {
            return Err("propagation_timeout_secs must be 10 to 3600".into());
        }
        Ok(())
    }
}

/// The cluster's one ACME account. The credentials (with the account key) are sealed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredAccount {
    pub directory_url: String,
    pub account_url: String,
    pub sealed: Sealed,
}

/// One issued certificate version. `names` are the names it covers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredCert {
    pub version: u64,
    pub chain_pem: String,
    pub sealed_key: Sealed,
    pub not_after: u64,
    pub issued_at: u64,
    pub serial: String,
    pub names: Vec<String>,
    /// Issued by a staging or local test CA: not publicly trusted, never to be served to
    /// real clients (see [`super::untrusted_ca`]).
    #[serde(default)]
    pub untrusted: bool,
}

/// Retry state of a site, replicated so a new leader continues the backoff.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attempt {
    /// Consecutive failures.
    pub failures: u32,
    pub next_attempt_at: u64,
    pub last_error: Option<String>,
    /// Start of the current hour-long window and the failures counted in it.
    pub window_start: u64,
    pub window_failures: u32,
}

/// A replicated ACME write.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AcmeOp {
    SiteSet(Site),
    SiteDelete {
        name: String,
    },
    /// Stores the account unless one exists (the first writer wins).
    AccountSet(StoredAccount),
    /// A finished order. Applied only if proposed in `tagged_term` (the leader's term),
    /// `cert.version` is the next one and `cert.names` are the site's current names.
    CertIssued {
        site: String,
        tagged_term: u64,
        cert: StoredCert,
    },
    Attempt {
        site: String,
        attempt: Attempt,
    },
}

/// What applying one op did. Everything but `Replayed` and `Done` changed nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Applied {
    Replayed,
    Done,
    /// `AccountSet` found an account already stored.
    Exists,
    /// Another site already covers this name.
    NameTaken(String),
    NoSuchSite,
    /// The entry was not appended in the term it was proposed in.
    WrongTerm,
    /// The version or names moved since the order started.
    Stale,
    Invalid(String),
}

/// Everything replicated about ACME, for an HA snapshot.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcmeSnapshot {
    pub sites: Vec<Site>,
    pub account: Option<StoredAccount>,
    /// `(site, cert)` pairs.
    pub certs: Vec<(String, StoredCert)>,
    pub attempts: Vec<(String, Attempt)>,
    #[serde(default)]
    pub applied_index: Option<u64>,
}
