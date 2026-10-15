//! The DNS-01 side of an order: put a TXT record where the CA will look, and remove it.

use std::time::Duration;

use async_trait::async_trait;

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct SolverError(pub String);

/// Sets and removes `_acme-challenge` TXT records. The controller's implementation calls a
/// DNS-provider plugin; tests use an in-memory one.
#[async_trait]
pub trait DnsSolver: Send + Sync {
    /// Adds a TXT record `fqdn` = `value`. Several values may coexist at one name (a wildcard
    /// and its apex).
    async fn present(&self, fqdn: &str, value: &str) -> Result<(), SolverError>;
    /// Removes that record. Always called after `present`, also after a failed order.
    async fn cleanup(&self, fqdn: &str, value: &str) -> Result<(), SolverError>;
    /// Returns once the record is visible to a resolver, or fails after `timeout`.
    async fn wait_visible(
        &self,
        fqdn: &str,
        value: &str,
        timeout: Duration,
    ) -> Result<(), SolverError>;
}
