//! One-time-code enrollment (design `design-one-command-peer-setup-2026-11-12.md`, slice 1).
//!
//! An admin mints a code on the controller; a node redeems it once over TLS that is pinned by a
//! leaf fingerprint or a CA file and receives what its role needs (for an origin: its agent
//! token and a controller-made tunnel preshared key). This crate holds what both ends share:
//! the code format, the wire types, the on-disk layout and the client.

pub mod client;
pub mod code;
pub mod files;
pub mod input;

use serde::{Deserialize, Serialize};

pub use code::{Code, CodeError};

/// A node role a code can be minted for. Slice 1 knows only the origin agent; the other roles
/// arrive with the slices that deliver their grants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Agent,
}

/// `POST /enroll`. The code is the only credential.
#[derive(Serialize, Deserialize)]
pub struct RedeemRequest {
    pub code: String,
    pub role: Role,
}

/// What a redeemed agent code returns. Both values are secrets: this type has no `Debug`.
#[derive(Serialize, Deserialize)]
pub struct RedeemResponse {
    pub name: String,
    pub role: Role,
    pub agent_token: Option<String>,
    /// The origin's tunnel preshared key, base64 of 32 bytes.
    pub tunnel_psk: String,
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

/// A fresh tunnel PSK, base64 (the form `wg` and `defguard` take).
pub fn generate_psk() -> zeroize::Zeroizing<String> {
    use base64::Engine;
    use rand::RngCore;
    let mut k = zeroize::Zeroizing::new([0u8; 32]);
    rand::rngs::OsRng.fill_bytes(&mut *k);
    zeroize::Zeroizing::new(base64::engine::general_purpose::STANDARD.encode(*k))
}

/// Redeems `code` at `controller_url` (pin-checked) and writes the node's files into `dir`.
/// Returns the enrolled name. Nothing secret is returned or logged.
pub async fn enroll_agent(
    controller_url: &str,
    pin: &client::Pin,
    code: &Code,
    dir: &std::path::Path,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let r = client::redeem(controller_url, pin, code, Role::Agent).await?;
    let meta = files::Meta { controller_url: controller_url.to_owned(), name: r.name.clone(), role: r.role };
    files::write(dir, &meta, r.agent_token.as_deref(), &r.tunnel_psk)?;
    Ok(r.name)
}
