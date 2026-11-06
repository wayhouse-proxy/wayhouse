//! The public key the official sniffer and plugin registries sign with (one key for both,
//! decision 2A of wave 3).
//!
//! The maintainer generated the key pair offline; only the public half lives here. A
//! version that lists a signature in the official registry is verified against it, and a
//! signature that does not verify is an error, never a quiet downgrade. A version without a
//! signature still installs, shown as unsigned (sniffers: as before; plugins: after an
//! explicit confirm).

/// Base64 minisign public key (the second line of a `.pub` file). Key id `C44491B8C8BF6083`.
pub const OFFICIAL_PUBKEY: Option<&str> =
    Some("RWSDYL/IuJFExBUcctEzBH93zm7TzFTfvFlWHjmvxb4wCYiZ3+BN+YIt");

/// The parsed official key, if one is configured and valid.
pub fn official_key() -> Option<minisign_verify::PublicKey> {
    OFFICIAL_PUBKEY.and_then(|k| minisign_verify::PublicKey::from_base64(k).ok())
}

#[cfg(test)]
mod tests {
    use base64::Engine;

    #[test]
    fn the_official_key_is_set_parses_and_has_the_published_key_id() {
        let k = super::OFFICIAL_PUBKEY.expect("the official key is configured");
        assert!(minisign_verify::PublicKey::from_base64(k).is_ok());
        assert!(super::official_key().is_some());
        // 2 bytes "Ed", the 8-byte key id (little endian), then the 32-byte key.
        let raw = base64::engine::general_purpose::STANDARD.decode(k).unwrap();
        assert_eq!(&raw[..2], b"Ed");
        let id: String = raw[2..10]
            .iter()
            .rev()
            .map(|b| format!("{b:02X}"))
            .collect();
        assert_eq!(id, "C44491B8C8BF6083");
    }
}
