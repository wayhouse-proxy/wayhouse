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

/// How a site proves it controls its names.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChallengeKind {
    /// A TXT record, set through the site's DNS plugin. Wildcards need this.
    #[default]
    #[serde(rename = "dns-01")]
    Dns01,
    /// A file the CA fetches over plain HTTP from the site's names, which the gateways
    /// forward to the controller.
    #[serde(rename = "http-01")]
    Http01,
}

impl ChallengeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dns01 => "dns-01",
            Self::Http01 => "http-01",
        }
    }
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
    /// The challenge the CA is answered with. Defaults to DNS-01.
    #[serde(default)]
    pub challenge: ChallengeKind,
    /// The plugin install that sets and removes the DNS TXT records (DNS-01 only).
    #[serde(default)]
    pub dns_plugin: String,
    /// A zone the `_acme-challenge` names are CNAMEd into: the TXT records are written under
    /// it (`_acme-challenge.<host>.<zone>`) instead of in the site's own zone.
    #[serde(default)]
    pub challenge_zone: Option<String>,
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
        match self.challenge {
            ChallengeKind::Dns01 if self.dns_plugin.is_empty() => {
                return Err("dns_plugin is required".into());
            }
            ChallengeKind::Http01 => {
                if self.names.iter().any(|n| n.starts_with("*.")) {
                    return Err("a wildcard name needs the dns-01 challenge".into());
                }
                if !self.dns_plugin.is_empty() || self.challenge_zone.is_some() {
                    return Err("dns_plugin and challenge_zone belong to dns-01 sites".into());
                }
            }
            ChallengeKind::Dns01 => {}
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
        if let Some(zone) = &self.challenge_zone {
            if !wayhouse_acme::valid_zone(zone) {
                return Err("challenge_zone must be a hostname such as acme.other.net".into());
            }
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

/// A gateway that may fetch certificate bundles. Only the hash of its token is stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Gateway {
    pub name: String,
    /// Lowercase hex SHA-256 of the bearer token.
    pub token_hash: String,
    pub created_at: u64,
}

/// An HTTP-01 answer the controller serves until `expires_at`. The key authorization is
/// public by design (the CA gets it from any client that asks), so it is stored in clear.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpChallenge {
    pub token: String,
    pub key_authorization: String,
    /// Unix seconds; the answer is neither served nor kept after this.
    pub expires_at: u64,
}

impl HttpChallenge {
    /// 1 to 128 of A-Z, a-z, 0-9, '-' and '_'.
    pub fn valid_token(token: &str) -> bool {
        !token.is_empty()
            && token.len() <= 128
            && token
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    }

    /// Why this cannot be stored, or `Ok`. A token is what the CA puts in a URL path, so only
    /// URL-safe base64 characters pass, and the key authorization must be `<token>.<thumbprint>`.
    pub fn validate(&self) -> Result<(), String> {
        let t = &self.token;
        if t.is_empty()
            || t.len() > 128
            || !t
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err("an HTTP-01 token is 1 to 128 of A-Z, a-z, 0-9, '-' and '_'".into());
        }
        let k = &self.key_authorization;
        if k.len() > 512
            || !k
                .strip_prefix(t.as_str())
                .is_some_and(|r| r.starts_with('.'))
        {
            return Err("the key authorization must be `<token>.<thumbprint>`".into());
        }
        Ok(())
    }
}

/// A replicated ACME write.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AcmeOp {
    SiteSet(Site),
    SiteDelete {
        name: String,
    },
    /// Stores the account unless one exists for the same CA (the first writer wins); an
    /// account for a different CA replaces the stored one.
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
    /// Registers a gateway; `Exists` if the name is taken (delete it first to rotate).
    GatewaySet(Gateway),
    GatewayDelete {
        name: String,
    },
    /// Publishes (or replaces) an HTTP-01 answer.
    ChallengeSet(HttpChallenge),
    ChallengeDelete {
        token: String,
    },
    /// Drops every answer that expired at `now`; the leader proposes it, so every replica
    /// drops the same ones.
    ChallengesPrune {
        now: u64,
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
    NoSuchGateway,
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
    pub gateways: Vec<Gateway>,
    #[serde(default)]
    pub challenges: Vec<HttpChallenge>,
    #[serde(default)]
    pub applied_index: Option<u64>,
}
