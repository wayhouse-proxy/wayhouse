//! A fake ACME CA and an in-memory DNS, for tests. Enabled by the `test-support` feature.
//!
//! The CA implements the RFC 8555 subset `instant-acme` uses (directory, nonce, account with
//! ES256 JWS verification, order, authorization, `dns-01` and `http-01`, finalize, certificate).
//! It really validates a challenge: the TXT value must be in the [`MemDns`] the test shares
//! with it, the HTTP answer in the [`MemHttp`] given to `FakeCa::serve_http_challenges`.

mod ca;
mod dns;
mod http;

pub use ca::{FailKind, FakeCa};
pub use dns::MemDns;
pub use http::MemHttp;
