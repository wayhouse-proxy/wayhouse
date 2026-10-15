//! A fake ACME CA and an in-memory DNS, for tests. Enabled by the `test-support` feature.
//!
//! The CA implements the RFC 8555 subset `instant-acme` uses (directory, nonce, account with
//! ES256 JWS verification, order, authorization, `dns-01`, finalize, certificate). It really
//! validates a challenge: the TXT value must be in the [`MemDns`] the test shares with it.

mod ca;
mod dns;

pub use ca::{FailKind, FakeCa};
pub use dns::MemDns;
