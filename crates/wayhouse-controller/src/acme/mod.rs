//! Controller-managed ACME certificates (design:
//! `docs/superpowers/specs/2026-10-15-controller-acme-design.md`).

pub mod store;
pub mod types;

pub use store::{AcmeStore, AcmeStoreError};
pub use types::{AcmeOp, AcmeSnapshot, Applied, Attempt, Site, StoredAccount, StoredCert};
