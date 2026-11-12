//! One-time-code enrollment (design `design-one-command-peer-setup-2026-11-12.md`, slice 1).
//!
//! An admin mints a code on the controller; a node redeems it once over TLS that is pinned by a
//! leaf fingerprint or a CA file and receives what its role needs (for an origin: its agent
//! token and a controller-made tunnel preshared key). This crate holds what both ends share:
//! the code format, the wire types, the on-disk layout and the client.

pub mod cli;
pub mod client;
pub mod code;
pub mod files;
pub mod input;
pub mod install;

use serde::{Deserialize, Serialize};

pub use code::{Code, CodeError};

/// A node role a code can be minted for. The role decides which files the redeem returns; the
/// names are the credential names each binary already resolves (`$CREDENTIALS_DIRECTORY/<name>`),
/// so a directory of them can be handed to systemd or mounted as `CREDENTIALS_DIRECTORY`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Agent,
    Proxy,
    Aggregator,
    Ui,
    /// Another replica of the controller's HA group (see `wayhouse-controller`'s `enroll` module).
    Controller,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Agent => "agent",
            Role::Proxy => "proxy",
            Role::Aggregator => "aggregator",
            Role::Ui => "ui",
            Role::Controller => "controller",
        }
    }

    /// The files a redeem for this role may carry; the client writes nothing else.
    pub fn grant_files(self) -> &'static [&'static str] {
        match self {
            Role::Agent => &[files::TOKEN_FILE, files::PSK_FILE],
            Role::Proxy => &["controller-token", "aggregator-token", files::KEYRING_FILE],
            Role::Aggregator => &["ingest-token"],
            Role::Ui => &["controller-token"],
            // A replica serves the same API, so it holds every token the first controller has;
            // `ha-join-proof` is what it later shows to be added to the Raft group.
            Role::Controller => &[
                "ha-token",
                files::KEYRING_FILE,
                "auth-token",
                "proxy-token",
                "agent-token",
                "ingest-token",
                "enroll-step-up",
                files::JOIN_PROOF_FILE,
            ],
        }
    }

    /// Roles whose grant is the cluster keyring or the admin token. Minting one needs the
    /// admin token and a fresh step-up secret (decision 4A of the design).
    pub fn needs_step_up(self) -> bool {
        matches!(self, Role::Proxy | Role::Ui | Role::Controller)
    }
}

/// `POST /enroll`. The code is the only credential.
#[derive(Serialize, Deserialize)]
pub struct RedeemRequest {
    pub code: String,
    pub role: Role,
}

/// What a redeemed code returns: the node's name and its secret files by name. Every value is
/// a secret: this type has no `Debug`.
#[derive(Serialize, Deserialize)]
pub struct RedeemResponse {
    pub name: String,
    pub role: Role,
    pub files: std::collections::BTreeMap<String, String>,
    /// An aggregator tier's parent, set by the admin at mint time (not a secret).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_url: Option<String>,
    /// A controller replica's Raft node id, picked by the cluster when the code was minted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<u64>,
    /// A controller replica's `--tunnel-network`, which must match the cluster's (not a secret).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tunnel_network: Option<String>,
}

/// The sealed cluster secret that holds an origin's tunnel PSK. Derived from the origin name so
/// it is always a valid secret name (origin names allow `.`, `_` and capitals; secret names do
/// not) and so the proxy never has to be told where to look.
pub fn psk_secret_name(origin: &str) -> String {
    use sha2::{Digest, Sha256};
    let d = Sha256::digest(origin.as_bytes());
    let hex: String = d[..8].iter().map(|b| format!("{b:02x}")).collect();
    format!("tunnel-psk-{hex}")
}

/// The sealed secret that holds generation `gen` of an origin's tunnel PSK. Generation 0 is
/// the one enrollment makes ([`psk_secret_name`]); a rotation makes `<base>-<gen>`. A new name
/// per generation lets the registration flip from one key to the next in a single write.
pub fn psk_secret_name_gen(origin: &str, gen: u64) -> String {
    match gen {
        0 => psk_secret_name(origin),
        n => format!("{}-{n}", psk_secret_name(origin)),
    }
}

/// The generation `secret` holds for `origin`, or `None` when it is not one of that origin's
/// own tunnel key secrets (another origin's, a plain cluster secret, a malformed suffix).
pub fn psk_secret_gen(origin: &str, secret: &str) -> Option<u64> {
    let base = psk_secret_name(origin);
    if secret == base {
        return Some(0);
    }
    let n = secret.strip_prefix(&base)?.strip_prefix('-')?;
    // Canonical decimal only, so one generation has one name.
    let canonical = !n.starts_with('0') && n.len() <= 18 && n.bytes().all(|b| b.is_ascii_digit());
    canonical.then(|| n.parse().ok()).flatten()
}

/// A fresh tunnel PSK, base64 (the form `wg` and `defguard` take).
pub fn generate_psk() -> zeroize::Zeroizing<String> {
    use base64::Engine;
    use rand::RngCore;
    let mut k = zeroize::Zeroizing::new([0u8; 32]);
    rand::rngs::OsRng.fill_bytes(&mut *k);
    zeroize::Zeroizing::new(base64::engine::general_purpose::STANDARD.encode(*k))
}

/// Redeems `code` as `role` at `controller_url` (pin-checked) and writes the node's files into
/// `dir`. Returns the enrolled name. Nothing secret is returned or logged.
pub async fn enroll(
    controller_url: &str,
    pin: Option<&client::Pin>,
    code: &Code,
    role: Role,
    dir: &std::path::Path,
    advertise: Option<&str>,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let r = client::redeem(controller_url, pin, code, role).await?;
    let mut granted = r.files;
    if role == Role::Aggregator {
        // The aggregator's own admin token never leaves it, so it is made here, not sent.
        granted.insert("auth-token".into(), generate_token());
    }
    let meta = files::Meta {
        controller_url: controller_url.to_owned(),
        name: r.name.clone(),
        role: r.role,
        parent_url: r.parent_url,
        node_id: r.node_id,
        tunnel_network: r.tunnel_network,
        advertise: advertise.map(str::to_owned),
    };
    files::write(dir, &meta, &granted)?;
    Ok(r.name)
}

/// 32 random bytes as URL-safe base64 (43 characters), for a token that is born on the node.
pub fn generate_token() -> String {
    use base64::Engine;
    use rand::RngCore;
    let mut k = zeroize::Zeroizing::new([0u8; 32]);
    rand::rngs::OsRng.fill_bytes(&mut *k);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(*k)
}

#[cfg(test)]
mod gen_tests {
    use super::*;

    #[test]
    fn a_generation_has_exactly_one_name_and_only_its_origin_owns_it() {
        assert_eq!(psk_secret_name_gen("home", 0), psk_secret_name("home"));
        let g3 = psk_secret_name_gen("home", 3);
        assert_eq!(g3, format!("{}-3", psk_secret_name("home")));
        assert!(valid(&g3), "must stay a legal cluster secret name");
        assert_eq!(psk_secret_gen("home", &psk_secret_name("home")), Some(0));
        assert_eq!(psk_secret_gen("home", &g3), Some(3));
        assert_eq!(
            psk_secret_gen("other", &g3),
            None,
            "another origin's secret"
        );
        for bad in [
            "-0",
            "-03",
            "-",
            "-x",
            "-1-2",
            "-+1",
            "-99999999999999999999",
        ] {
            let name = format!("{}{bad}", psk_secret_name("home"));
            assert_eq!(psk_secret_gen("home", &name), None, "suffix {bad}");
        }
        assert_eq!(psk_secret_gen("home", "gossip-psk"), None);
    }

    fn valid(name: &str) -> bool {
        name.len() <= 63
            && name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    }
}
