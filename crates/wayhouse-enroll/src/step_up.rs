//! The signed step-up assertion (#638, design `design-ui-scoped-token-638-2026-11-14.md`, part B).
//!
//! Minting a code for a role that hands out the keyring or the admin token needs more than the
//! admin token. The operator picks a **step-up passphrase** once; an Ed25519 key is derived from
//! it with argon2id and a random salt, and only the public key and the salt are given to the
//! controller (`--enroll-step-up-pubkey-file`). At mint time whoever holds the passphrase signs
//! the exact request with a short expiry and a nonce; the controller verifies it and keeps
//! nothing that could sign. A controller or a UI host that leaks cannot mint, and a captured
//! assertion cannot mint anything else.

use argon2::{Algorithm, Argon2, Params, Version};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
pub use ed25519_dalek::SigningKey;
use ed25519_dalek::{Signature, Signer, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

/// The shortest accepted passphrase, in characters.
pub const MIN_PASSPHRASE_LEN: usize = 16;
/// How far in the future an assertion's `exp` may lie, and so how long a nonce is remembered.
pub const MAX_ASSERTION_TTL_SECS: u64 = 60;

/// How long a signer makes its assertions last. Half the verifier's cap, so a signer whose clock
/// runs up to 30 s ahead of the controller's is still accepted.
pub const SIGN_TTL_SECS: u64 = 30;
const SALT_LEN: usize = 16;
const DOMAIN: &[u8] = b"wayhouse-step-up-v1\n";

/// The public half the controller holds: the salt (not secret, the signer needs it) and the
/// verifying key. One line, `v1 <salt base64> <public key base64>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepUpKey {
    pub salt: [u8; SALT_LEN],
    pub public: [u8; 32],
}

impl StepUpKey {
    /// A new key for `passphrase` with a random salt.
    pub fn generate(passphrase: &str) -> Result<Self, String> {
        let salt: [u8; SALT_LEN] = rand::random();
        let key = derive(passphrase, &salt)?;
        Ok(Self {
            salt,
            public: key.verifying_key().to_bytes(),
        })
    }

    pub fn render(&self) -> String {
        format!("v1 {} {}\n", B64.encode(self.salt), B64.encode(self.public))
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let mut it = text.split_whitespace();
        let bad = || "the step-up key file must be `v1 <salt> <public key>`".to_string();
        if it.next() != Some("v1") {
            return Err(bad());
        }
        let (salt, public) = (it.next().ok_or_else(bad)?, it.next().ok_or_else(bad)?);
        if it.next().is_some() {
            return Err(bad());
        }
        let decode = |s: &str| B64.decode(s).map_err(|_| bad());
        let salt: [u8; SALT_LEN] = decode(salt)?.try_into().map_err(|_| bad())?;
        let public: [u8; 32] = decode(public)?.try_into().map_err(|_| bad())?;
        VerifyingKey::from_bytes(&public).map_err(|_| bad())?;
        Ok(Self { salt, public })
    }

    /// The salt as the controller publishes it for signers.
    pub fn salt_b64(&self) -> String {
        B64.encode(self.salt)
    }
}

/// The salt a controller publishes (`GET /enrollments/step-up`), decoded.
pub fn decode_salt(b64: &str) -> Result<Vec<u8>, String> {
    let salt = B64
        .decode(b64)
        .map_err(|_| "the controller's step-up salt is not base64".to_string())?;
    if salt.len() != SALT_LEN {
        return Err("the controller's step-up salt has the wrong length".into());
    }
    Ok(salt)
}

/// The signing key for `passphrase` and `salt` (argon2id, 64 MiB, 3 passes). The same inputs
/// always give the same key.
pub fn derive(passphrase: &str, salt: &[u8]) -> Result<SigningKey, String> {
    if passphrase.chars().count() < MIN_PASSPHRASE_LEN {
        return Err(format!(
            "the step-up passphrase must be at least {MIN_PASSPHRASE_LEN} characters"
        ));
    }
    let params = Params::new(64 * 1024, 3, 1, Some(32)).map_err(|e| e.to_string())?;
    let mut seed = Zeroizing::new([0u8; 32]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(passphrase.as_bytes(), salt, &mut seed[..])
        .map_err(|e| e.to_string())?;
    Ok(SigningKey::from_bytes(&seed))
}

/// The mint request as it was sent, the part the assertion binds. `None` and an empty string
/// are different values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Request<'a> {
    pub role: &'a str,
    pub name: &'a str,
    pub ttl_secs: Option<u64>,
    pub uses: Option<u64>,
    pub name_prefix: Option<&'a str>,
    pub parent_url: Option<&'a str>,
}

/// What travels in the mint body's `step_up`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Assertion {
    /// Base64 Ed25519 signature over [`canonical`].
    pub sig: String,
    /// At least 16 characters, never reused within the assertion's life.
    pub nonce: String,
    /// Unix seconds; at most [`MAX_ASSERTION_TTL_SECS`] ahead of the verifier's clock.
    pub exp: u64,
}

fn field(out: &mut Vec<u8>, name: &str, value: Option<&str>) {
    out.extend_from_slice(name.as_bytes());
    match value {
        // Length-prefixed, so no value can run into the next field.
        Some(v) => out.extend_from_slice(format!("={}:{v}\n", v.len()).as_bytes()),
        None => out.extend_from_slice(b"=-\n"),
    }
}

/// The exact bytes that are signed: a domain line, the request's fields and the nonce and expiry.
pub fn canonical(req: &Request<'_>, nonce: &str, exp: u64) -> Vec<u8> {
    let mut out = DOMAIN.to_vec();
    field(&mut out, "role", Some(req.role));
    field(&mut out, "name", Some(req.name));
    field(
        &mut out,
        "ttl_secs",
        req.ttl_secs.map(|n| n.to_string()).as_deref(),
    );
    field(&mut out, "uses", req.uses.map(|n| n.to_string()).as_deref());
    field(&mut out, "name_prefix", req.name_prefix);
    field(&mut out, "parent_url", req.parent_url);
    field(&mut out, "nonce", Some(nonce));
    field(&mut out, "exp", Some(&exp.to_string()));
    out
}

/// Signs `req` with a fresh nonce, valid for [`SIGN_TTL_SECS`] from `now`.
pub fn sign(key: &SigningKey, req: &Request<'_>, now: u64) -> Assertion {
    let mut raw = [0u8; 16];
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut raw);
    let nonce = B64.encode(raw);
    let exp = now + SIGN_TTL_SECS;
    let sig = key.sign(&canonical(req, &nonce, exp));
    Assertion {
        sig: B64.encode(sig.to_bytes()),
        nonce,
        exp,
    }
}

/// Why an assertion was refused. The text is what the caller sees, so it names the problem and
/// not what would have passed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    Expired,
    TooFarAhead,
    BadNonce,
    BadSignature,
}

impl Refusal {
    pub fn as_str(self) -> &'static str {
        match self {
            Refusal::Expired => "the step-up assertion has expired",
            Refusal::TooFarAhead => "the step-up assertion expires too far ahead",
            Refusal::BadNonce => "the step-up nonce is not valid",
            Refusal::BadSignature => "the step-up assertion does not match this request",
        }
    }
}

/// Checks the shape, the clock and the signature of `a` against `req`. Replay is the caller's:
/// it must remember `a.nonce` until `a.exp`.
pub fn verify(
    public: &[u8; 32],
    req: &Request<'_>,
    a: &Assertion,
    now: u64,
) -> Result<(), Refusal> {
    if a.nonce.len() < 16 || a.nonce.len() > 64 || !a.nonce.is_ascii() {
        return Err(Refusal::BadNonce);
    }
    if a.exp <= now {
        return Err(Refusal::Expired);
    }
    if a.exp > now + MAX_ASSERTION_TTL_SECS {
        return Err(Refusal::TooFarAhead);
    }
    let key = VerifyingKey::from_bytes(public).map_err(|_| Refusal::BadSignature)?;
    let sig: [u8; 64] = B64
        .decode(&a.sig)
        .ok()
        .and_then(|b| b.try_into().ok())
        .ok_or(Refusal::BadSignature)?;
    key.verify(
        &canonical(req, &a.nonce, a.exp),
        &Signature::from_bytes(&sig),
    )
    .map_err(|_| Refusal::BadSignature)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PASS: &str = "correct horse battery staple";

    fn req() -> Request<'static> {
        Request {
            role: "proxy",
            name: "edge-1",
            ttl_secs: None,
            uses: Some(1),
            name_prefix: None,
            parent_url: None,
        }
    }

    fn fixture() -> (SigningKey, StepUpKey) {
        let salt: [u8; SALT_LEN] = rand::random();
        let key = derive(PASS, &salt).unwrap();
        let public = key.verifying_key().to_bytes();
        (key, StepUpKey { salt, public })
    }

    #[test]
    fn a_signed_request_verifies_and_the_key_file_round_trips() {
        let (key, k) = fixture();
        let a = sign(&key, &req(), 1000);
        verify(&k.public, &req(), &a, 1000).unwrap();
        assert_eq!(StepUpKey::parse(&k.render()).unwrap(), k);
    }

    #[test]
    fn the_same_passphrase_and_salt_give_the_same_key_and_another_salt_does_not() {
        let (_, k) = fixture();
        assert_eq!(
            derive(PASS, &k.salt).unwrap().verifying_key().to_bytes(),
            k.public
        );
        assert_ne!(
            derive(PASS, &rand::random::<[u8; SALT_LEN]>())
                .unwrap()
                .verifying_key()
                .to_bytes(),
            k.public
        );
    }

    #[test]
    fn an_assertion_binds_every_field_of_the_request() {
        let (key, k) = fixture();
        let a = sign(&key, &req(), 1000);
        let wrong = [
            Request {
                role: "ui",
                ..req()
            },
            Request {
                name: "edge-2",
                ..req()
            },
            Request {
                uses: Some(2),
                ..req()
            },
            Request {
                uses: None,
                ..req()
            },
            Request {
                ttl_secs: Some(60),
                ..req()
            },
            Request {
                name_prefix: Some("edge-"),
                ..req()
            },
            Request {
                parent_url: Some("https://x"),
                ..req()
            },
        ];
        for r in wrong {
            assert_eq!(
                verify(&k.public, &r, &a, 1000),
                Err(Refusal::BadSignature),
                "{r:?}"
            );
        }
    }

    #[test]
    fn fields_cannot_run_into_each_other() {
        let n = crate::generate_token();
        let a = canonical(
            &Request {
                name: "ab",
                name_prefix: Some("c-"),
                ..req()
            },
            &n,
            1,
        );
        let b = canonical(
            &Request {
                name: "ab",
                name_prefix: Some("c-"),
                uses: Some(2),
                ..req()
            },
            &n,
            1,
        );
        assert_ne!(a, b);
        let c = canonical(
            &Request {
                name: "a",
                name_prefix: Some("b-c-"),
                ..req()
            },
            &n,
            1,
        );
        assert_ne!(a, c);
    }

    #[test]
    fn a_signer_whose_clock_is_ahead_is_still_accepted() {
        let (key, k) = fixture();
        // The signer reads 1010 while the controller reads 1000.
        let a = sign(&key, &req(), 1010);
        verify(&k.public, &req(), &a, 1000).unwrap();
        // And one 20 s behind still has 10 s left.
        let b = sign(&key, &req(), 980);
        verify(&k.public, &req(), &b, 1000).unwrap();
    }

    #[test]
    fn expiry_and_nonce_shape_are_checked() {
        let (key, k) = fixture();
        let a = sign(&key, &req(), 1000);
        assert_eq!(verify(&k.public, &req(), &a, 1060), Err(Refusal::Expired));
        assert_eq!(
            verify(&k.public, &req(), &a, 900),
            Err(Refusal::TooFarAhead)
        );
        let short = Assertion {
            nonce: a.nonce[..4].to_owned(),
            ..a.clone()
        };
        assert_eq!(
            verify(&k.public, &req(), &short, 1000),
            Err(Refusal::BadNonce)
        );
        let junk = Assertion {
            sig: "!!".into(),
            ..a
        };
        assert_eq!(
            verify(&k.public, &req(), &junk, 1000),
            Err(Refusal::BadSignature)
        );
    }

    #[test]
    fn another_passphrase_cannot_sign_and_a_short_one_is_refused() {
        let (_, k) = fixture();
        let other = derive("a different passphrase!", &k.salt).unwrap();
        let a = sign(&other, &req(), 1000);
        assert_eq!(
            verify(&k.public, &req(), &a, 1000),
            Err(Refusal::BadSignature)
        );
        assert!(derive("too short", &k.salt).is_err());
    }

    #[test]
    fn a_malformed_key_file_is_refused() {
        for bad in ["", "v2 a b", "v1 a", "v1 AAAA AAAA", "v1 a b c"] {
            assert!(StepUpKey::parse(bad).is_err(), "{bad:?}");
        }
    }
}
