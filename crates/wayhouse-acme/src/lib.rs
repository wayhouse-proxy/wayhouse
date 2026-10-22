//! ACME certificate issuance for the controller. See the crate's `Cargo.toml` for scope.

#[cfg(feature = "client")]
pub mod ca;
pub mod model;
pub mod renew;
pub mod solver;
#[cfg(feature = "test-support")]
pub mod testing;

#[cfg(feature = "client")]
pub use ca::{AccountCreds, AcmeCa, IssueError};
pub use model::{valid_zone, validate_names, CertBundle, NameError, Redacted, MAX_NAMES};
pub use renew::{backoff, due, JITTER_MAX};
pub use solver::{DelegatedSolver, DnsSolver, HttpSolver, SolverError};
