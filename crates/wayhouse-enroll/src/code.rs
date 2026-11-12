//! The code format: `<id>.<secret>`. The id (8 hex chars) finds the record; only a SHA-256 of
//! the secret (256 random bits, so no slow hash is needed) is ever stored or replicated.

use rand::RngCore;
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CodeError {
    #[error("not a wayhouse enrollment code")]
    Malformed,
}

/// A parsed or fresh code. No `Debug`: it is a credential until it is redeemed.
pub struct Code {
    id: String,
    secret: Zeroizing<String>,
}

const ID_LEN: usize = 8;
const SECRET_LEN: usize = 43; // base64url of 32 bytes, unpadded

impl Code {
    pub fn generate() -> Self {
        use base64::Engine;
        let mut id = [0u8; ID_LEN / 2];
        let mut secret = Zeroizing::new([0u8; 32]);
        rand::rngs::OsRng.fill_bytes(&mut id);
        rand::rngs::OsRng.fill_bytes(&mut *secret);
        Self {
            id: hex(&id),
            secret: Zeroizing::new(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(*secret)),
        }
    }

    pub fn parse(text: &str) -> Result<Self, CodeError> {
        let (id, secret) = text.trim().split_once('.').ok_or(CodeError::Malformed)?;
        let id_ok = id.len() == ID_LEN && id.bytes().all(|b| b.is_ascii_hexdigit());
        let secret_ok = secret.len() == SECRET_LEN
            && secret
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
        if !(id_ok && secret_ok) {
            return Err(CodeError::Malformed);
        }
        Ok(Self {
            id: id.to_ascii_lowercase(),
            secret: Zeroizing::new(secret.to_owned()),
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    /// The text an admin hands to the node. Call only to show it once.
    pub fn expose(&self) -> Zeroizing<String> {
        Zeroizing::new(format!("{}.{}", self.id, &*self.secret))
    }

    /// What the controller stores: hex SHA-256 of the secret half.
    pub fn hash(&self) -> String {
        hex(&Sha256::digest(self.secret.as_bytes()))
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_generated_code_round_trips_through_its_text() {
        let c = Code::generate();
        let p = Code::parse(&c.expose()).unwrap();
        assert_eq!((p.id(), p.hash()), (c.id(), c.hash()));
    }

    #[test]
    fn the_hash_does_not_contain_the_secret_and_codes_differ() {
        let (a, b) = (Code::generate(), Code::generate());
        assert_ne!(a.hash(), b.hash());
        let text = a.expose();
        let secret = text.split_once('.').unwrap().1;
        assert!(!a.hash().contains(secret));
        assert_eq!(a.hash().len(), 64);
    }

    #[test]
    fn malformed_codes_are_refused() {
        for bad in ["", "abc", "zzzzzzzz.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA", "abcd1234.short", "abcd1234.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA/"] {
            assert!(Code::parse(bad).is_err(), "{bad}");
        }
    }
}
