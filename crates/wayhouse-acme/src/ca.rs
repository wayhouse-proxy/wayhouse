//! The ACME client: account handling and one DNS-01 order.

use std::time::Duration;

use instant_acme::{
    Account, AccountBuilder, AccountCredentials, AuthorizationStatus, ChallengeType, HttpClient,
    Identifier, NewAccount, NewOrder, OrderStatus, RetryPolicy,
};
use serde::{Deserialize, Serialize};

use crate::model::{CertBundle, Redacted};
use crate::solver::{DnsSolver, SolverError};

/// What the controller persists (sealed) to reuse the account: the URL is public, the
/// credentials JSON holds the account key.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountCreds {
    pub account_url: String,
    pub credentials: Redacted,
}

#[derive(Debug, thiserror::Error)]
pub enum IssueError {
    #[error("the terms of service were not accepted")]
    TosNotAccepted,
    #[error("account: {0}")]
    Account(String),
    #[error("order: {0}")]
    Order(String),
    #[error("the CA rejected the challenge: {0}")]
    Validation(String),
    #[error("dns solver: {0}")]
    Solver(#[from] SolverError),
    #[error("certificate: {0}")]
    Cert(String),
}

type HttpFactory = Box<dyn Fn() -> Box<dyn HttpClient> + Send + Sync>;

pub struct AcmeCa {
    directory_url: String,
    accept_tos: bool,
    http: Option<HttpFactory>,
    propagation_timeout: Duration,
    retry: RetryPolicy,
}

impl AcmeCa {
    /// `directory_url` must be https (the fake CA in tests uses [`AcmeCa::with_http`]).
    pub fn new(directory_url: &str, accept_tos: bool) -> Self {
        Self {
            directory_url: directory_url.to_string(),
            accept_tos,
            http: None,
            propagation_timeout: Duration::from_secs(120),
            retry: RetryPolicy::new().timeout(Duration::from_secs(90)),
        }
    }

    /// Replaces the HTTP client; tests point it at a plain-HTTP fake CA.
    #[must_use]
    pub fn with_http(mut self, factory: HttpFactory) -> Self {
        self.http = Some(factory);
        self
    }

    /// How long the solver may take to make a record visible.
    #[must_use]
    pub fn propagation_timeout(mut self, d: Duration) -> Self {
        self.propagation_timeout = d;
        self
    }

    /// Short poll delays, for tests.
    #[must_use]
    pub fn fast_retry(mut self) -> Self {
        self.retry = RetryPolicy::new()
            .initial_delay(Duration::from_millis(20))
            .timeout(Duration::from_secs(20));
        self
    }

    fn builder(&self) -> Result<AccountBuilder, IssueError> {
        match &self.http {
            Some(f) => Ok(Account::builder_with_http(f())),
            None => Account::builder().map_err(|e| IssueError::Account(e.to_string())),
        }
    }

    /// Opens the stored account, or creates one (needs the terms accepted). Returns the
    /// credentials to persist, which are `existing` unchanged when there was one.
    pub async fn ensure_account(
        &self,
        existing: Option<&AccountCreds>,
    ) -> Result<AccountCreds, IssueError> {
        self.open(existing).await.map(|(_, c)| c)
    }

    async fn open(
        &self,
        existing: Option<&AccountCreds>,
    ) -> Result<(Account, AccountCreds), IssueError> {
        let builder = self.builder()?;
        if let Some(c) = existing {
            let creds: AccountCredentials = serde_json::from_str(c.credentials.expose())
                .map_err(|e| IssueError::Account(e.to_string()))?;
            let account = builder
                .from_credentials(creds)
                .await
                .map_err(|e| IssueError::Account(e.to_string()))?;
            return Ok((account, c.clone()));
        }
        if !self.accept_tos {
            return Err(IssueError::TosNotAccepted);
        }
        let (account, creds) = builder
            .create(
                &NewAccount {
                    contact: &[],
                    terms_of_service_agreed: true,
                    only_return_existing: false,
                },
                self.directory_url.clone(),
                None,
            )
            .await
            .map_err(|e| IssueError::Account(e.to_string()))?;
        let json = serde_json::to_string(&creds).map_err(|e| IssueError::Account(e.to_string()))?;
        let stored = AccountCreds {
            account_url: account.id().to_string(),
            credentials: Redacted::new(json),
        };
        Ok((account, stored))
    }

    /// Runs one order for `names` over DNS-01. `now` stamps `issued_at`. The records the
    /// solver was asked to present are always cleaned up, also on failure.
    pub async fn issue(
        &self,
        existing: Option<&AccountCreds>,
        names: &[String],
        solver: &dyn DnsSolver,
        now: u64,
    ) -> Result<(CertBundle, AccountCreds), IssueError> {
        let (account, creds) = self.open(existing).await?;
        let mut presented: Vec<(String, String)> = Vec::new();
        let result = self
            .run_order(&account, names, solver, now, &mut presented)
            .await;
        for (fqdn, value) in &presented {
            if let Err(e) = solver.cleanup(fqdn, value).await {
                tracing::warn!(%fqdn, error = %e, "acme: dns cleanup failed");
            }
        }
        result.map(|b| (b, creds))
    }

    async fn run_order(
        &self,
        account: &Account,
        names: &[String],
        solver: &dyn DnsSolver,
        now: u64,
        presented: &mut Vec<(String, String)>,
    ) -> Result<CertBundle, IssueError> {
        let ids: Vec<Identifier> = names.iter().map(|n| Identifier::Dns(n.clone())).collect();
        let mut order = account
            .new_order(&NewOrder::new(&ids))
            .await
            .map_err(|e| IssueError::Order(e.to_string()))?;
        let mut authzs = order.authorizations();
        while let Some(next) = authzs.next().await {
            let mut authz = next.map_err(|e| classify(&e))?;
            match authz.status {
                AuthorizationStatus::Valid => continue,
                AuthorizationStatus::Pending => {}
                other => return Err(IssueError::Order(format!("authorization is {other:?}"))),
            }
            let mut challenge = authz
                .challenge(ChallengeType::Dns01)
                .ok_or_else(|| IssueError::Order("the CA offers no dns-01 challenge".into()))?;
            let Identifier::Dns(host) = challenge.identifier().identifier else {
                return Err(IssueError::Order("not a DNS identifier".into()));
            };
            let fqdn = format!("_acme-challenge.{host}");
            let value = challenge.key_authorization().dns_value();
            presented.push((fqdn.clone(), value.clone()));
            solver.present(&fqdn, &value).await?;
            solver
                .wait_visible(&fqdn, &value, self.propagation_timeout)
                .await?;
            challenge.set_ready().await.map_err(|e| classify(&e))?;
        }
        let status = order
            .poll_ready(&self.retry)
            .await
            .map_err(|e| classify(&e))?;
        if status != OrderStatus::Ready {
            return Err(IssueError::Validation(format!("order is {status:?}")));
        }
        let key_pem = order
            .finalize()
            .await
            .map_err(|e| IssueError::Order(e.to_string()))?;
        let chain_pem = order
            .poll_certificate(&self.retry)
            .await
            .map_err(|e| IssueError::Cert(e.to_string()))?;
        let (not_after, serial) = leaf_info(&chain_pem)?;
        Ok(CertBundle {
            chain_pem,
            key_pem: Redacted::new(key_pem),
            not_after,
            issued_at: now,
            serial,
            names: names.to_vec(),
        })
    }
}

/// A CA problem about the challenge itself (DNS lookup, unauthorized, CAA) is a validation
/// failure; anything else is an order error.
fn classify(e: &instant_acme::Error) -> IssueError {
    let text = e.to_string();
    let challenge_problem = [
        "error:dns",
        "error:unauthorized",
        "error:connection",
        "error:caa",
    ]
    .iter()
    .any(|k| text.contains(k));
    if challenge_problem || matches!(*e, instant_acme::Error::Timeout(_)) {
        IssueError::Validation(text)
    } else {
        IssueError::Order(text)
    }
}

/// `notAfter` (Unix seconds) and serial (hex) of the first certificate in a PEM chain.
pub fn leaf_info(chain_pem: &str) -> Result<(u64, String), IssueError> {
    let (_, pem) = x509_parser::pem::parse_x509_pem(chain_pem.as_bytes())
        .map_err(|e| IssueError::Cert(e.to_string()))?;
    let cert = pem
        .parse_x509()
        .map_err(|e| IssueError::Cert(e.to_string()))?;
    let not_after = u64::try_from(cert.validity().not_after.timestamp())
        .map_err(|_| IssueError::Cert("notAfter before 1970".into()))?;
    Ok((not_after, cert.raw_serial_as_string().replace(':', "")))
}
