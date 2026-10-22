//! The challenge side of an order: put a TXT record (DNS-01) or a key authorization (HTTP-01)
//! where the CA will look, and remove it.

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

/// Publishes and withdraws HTTP-01 answers. The controller's implementation replicates them
/// to every controller node, which serve them at `/.well-known/acme-challenge/<token>`.
#[async_trait]
pub trait HttpSolver: Send + Sync {
    /// Makes `GET /.well-known/acme-challenge/<token>` answer `key_authorization`.
    async fn present(&self, token: &str, key_authorization: &str) -> Result<(), SolverError>;
    /// Withdraws that answer. Always called after `present`, also after a failed order.
    async fn cleanup(&self, token: &str) -> Result<(), SolverError>;
}

#[async_trait]
impl<T: HttpSolver + ?Sized> HttpSolver for std::sync::Arc<T> {
    async fn present(&self, token: &str, key_authorization: &str) -> Result<(), SolverError> {
        (**self).present(token, key_authorization).await
    }

    async fn cleanup(&self, token: &str) -> Result<(), SolverError> {
        (**self).cleanup(token).await
    }
}

#[async_trait]
impl<T: DnsSolver + ?Sized> DnsSolver for std::sync::Arc<T> {
    async fn present(&self, fqdn: &str, value: &str) -> Result<(), SolverError> {
        (**self).present(fqdn, value).await
    }

    async fn cleanup(&self, fqdn: &str, value: &str) -> Result<(), SolverError> {
        (**self).cleanup(fqdn, value).await
    }

    async fn wait_visible(
        &self,
        fqdn: &str,
        value: &str,
        timeout: Duration,
    ) -> Result<(), SolverError> {
        (**self).wait_visible(fqdn, value, timeout).await
    }
}

/// A solver for a site whose `_acme-challenge` names are CNAMEs into another zone, so the DNS
/// provider that holds the real zone is never asked to change it. The operator creates, once,
/// `_acme-challenge.<host> CNAME _acme-challenge.<host>.<zone>`; this wrapper writes and
/// removes the TXT record at the second name. The visibility check still looks at the name the
/// CA queries, which proves the CNAME is in place.
pub struct DelegatedSolver<S> {
    inner: S,
    zone: String,
}

impl<S> DelegatedSolver<S> {
    /// `zone` is a hostname such as `acme.other.net`.
    pub fn new(inner: S, zone: &str) -> Self {
        Self {
            inner,
            zone: zone.trim_matches('.').to_ascii_lowercase(),
        }
    }

    /// `_acme-challenge.host` becomes `_acme-challenge.host.<zone>`.
    pub fn delegated(&self, fqdn: &str) -> String {
        format!("{}.{}", fqdn.trim_end_matches('.'), self.zone)
    }
}

#[async_trait]
impl<S: DnsSolver> DnsSolver for DelegatedSolver<S> {
    async fn present(&self, fqdn: &str, value: &str) -> Result<(), SolverError> {
        self.inner.present(&self.delegated(fqdn), value).await
    }

    async fn cleanup(&self, fqdn: &str, value: &str) -> Result<(), SolverError> {
        self.inner.cleanup(&self.delegated(fqdn), value).await
    }

    async fn wait_visible(
        &self,
        fqdn: &str,
        value: &str,
        timeout: Duration,
    ) -> Result<(), SolverError> {
        // The CA queries the original name; a resolver follows the CNAME to the record.
        self.inner
            .wait_visible(fqdn, value, timeout)
            .await
            .map_err(|e| {
                SolverError(format!(
                    "{e}; if the record was set, check that `{fqdn}` is a CNAME to `{}`",
                    self.delegated(fqdn)
                ))
            })
    }
}
