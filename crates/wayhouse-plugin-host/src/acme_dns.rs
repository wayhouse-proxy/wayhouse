//! The `on_acme_dns` trigger's data: what the host hands a DNS-provider plugin and what it
//! accepts back (`docs/plugins.md` "ACME DNS-01").
//!
//! The plugin only sets or removes one TXT record. It never sees an ACME account or
//! certificate key.

use serde::Serialize;

/// Longest answer message kept; longer ones are truncated.
pub const MAX_MESSAGE_BYTES: usize = 512;

/// What the plugin is asked to do with the record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AcmeDnsOp {
    Present,
    Cleanup,
}

/// The request document `on_acme_dns` receives.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AcmeDnsRequest {
    pub op: AcmeDnsOp,
    /// The full TXT name, `_acme-challenge.<host>`.
    pub fqdn: String,
    /// The TXT value to set or remove.
    pub value: String,
    /// The host being validated (the name without `_acme-challenge.`); the plugin finds the
    /// provider zone that contains it.
    pub zone_hint: String,
}

/// What the plugin reported with `acme_dns_result`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcmeDnsAnswer {
    pub ok: bool,
    /// Secrets the call expanded are already scrubbed out.
    pub message: String,
}
