//! The ACME client: account handling and one DNS-01 order.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use instant_acme::{
    Account, AccountBuilder, AccountCredentials, AuthorizationStatus, BodyWrapper, BytesResponse,
    ChallengeType, HttpClient, Identifier, NewAccount, NewOrder, OrderStatus, RetryPolicy,
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
    /// The CA answered 429 / `rateLimited`. `retry_after` is its `Retry-After` header, if any.
    #[error("the CA rate limit was hit{}", fmt_retry(*.retry_after))]
    RateLimited { retry_after: Option<Duration> },
}

fn fmt_retry(d: Option<Duration>) -> String {
    d.map_or_else(String::new, |d| format!(" (retry after {}s)", d.as_secs()))
}

/// Wraps the HTTP client to remember the `Retry-After` of the last 429 answer, which
/// `instant-acme` does not put into its error.
struct RetryAfterSlot {
    inner: Box<dyn HttpClient>,
    slot: Arc<Mutex<Option<Duration>>>,
}

impl HttpClient for RetryAfterSlot {
    fn request(
        &self,
        req: http::Request<BodyWrapper<Bytes>>,
    ) -> Pin<Box<dyn Future<Output = Result<BytesResponse, instant_acme::Error>> + Send>> {
        let fut = self.inner.request(req);
        let slot = self.slot.clone();
        Box::pin(async move {
            let rsp = fut.await?;
            if rsp.parts.status == http::StatusCode::TOO_MANY_REQUESTS {
                let wait = rsp
                    .parts
                    .headers
                    .get(http::header::RETRY_AFTER)
                    .and_then(|v| v.to_str().ok())
                    .and_then(parse_retry_after);
                *slot
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = wait;
            }
            Ok(rsp)
        })
    }
}

/// `Retry-After`: delay-seconds or an HTTP-date (a date in the past is zero).
fn parse_retry_after(value: &str) -> Option<Duration> {
    let value = value.trim();
    if let Ok(secs) = value.parse::<u64>() {
        return Some(Duration::from_secs(secs));
    }
    let at = httpdate::parse_http_date(value).ok()?;
    Some(at.duration_since(SystemTime::now()).unwrap_or_default())
}

/// The default client: HTTPS only, the platform's certificate verifier.
fn default_client() -> Result<Box<dyn HttpClient>, IssueError> {
    let connector = hyper_rustls::HttpsConnectorBuilder::new()
        .try_with_platform_verifier()
        .map_err(|e| IssueError::Account(e.to_string()))?
        .https_only()
        .enable_http1()
        .enable_http2()
        .build();
    Ok(Box::new(
        hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
            .build::<_, BodyWrapper<Bytes>>(connector),
    ))
}

type HttpFactory = Box<dyn Fn() -> Box<dyn HttpClient> + Send + Sync>;

pub struct AcmeCa {
    directory_url: String,
    accept_tos: bool,
    http: Option<HttpFactory>,
    propagation_timeout: Duration,
    /// Allowed on top of the propagation wait for the CA round trips and both polls.
    order_margin: Duration,
    retry: RetryPolicy,
    /// The `Retry-After` of the newest 429 answer, shared with the HTTP client wrapper.
    retry_after: Arc<Mutex<Option<Duration>>>,
    /// External account binding (RFC 8555 7.3.4) for CAs that tie accounts to a customer:
    /// the key id and the raw HMAC key. Used only when the account is created.
    eab: Option<(String, zeroize::Zeroizing<Vec<u8>>)>,
}

impl AcmeCa {
    /// `directory_url` must be https (the fake CA in tests uses [`AcmeCa::with_http`]).
    pub fn new(directory_url: &str, accept_tos: bool) -> Self {
        Self {
            directory_url: directory_url.to_string(),
            accept_tos,
            http: None,
            propagation_timeout: Duration::from_secs(120),
            order_margin: Duration::from_secs(300),
            retry: RetryPolicy::new().timeout(Duration::from_secs(90)),
            retry_after: Arc::default(),
            eab: None,
        }
    }

    /// Creates the account with an external account binding. `hmac_key` is the key as the CA
    /// hands it out: base64url (standard base64 and padding are accepted too).
    ///
    /// # Errors
    /// [`IssueError::Account`] if `kid` is empty or `hmac_key` is not base64.
    pub fn external_account(mut self, kid: &str, hmac_key: &str) -> Result<Self, IssueError> {
        use base64::Engine;
        if kid.trim().is_empty() {
            return Err(IssueError::Account("the EAB key id is empty".into()));
        }
        let text = hmac_key.trim().trim_end_matches('=');
        let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(text.replace('+', "-").replace('/', "_"))
            .map_err(|_| IssueError::Account("the EAB HMAC key is not base64".into()))?;
        if raw.is_empty() {
            return Err(IssueError::Account("the EAB HMAC key is empty".into()));
        }
        self.eab = Some((kid.trim().to_string(), zeroize::Zeroizing::new(raw)));
        Ok(self)
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

    /// Time allowed on top of the propagation wait before an order is abandoned.
    #[must_use]
    pub fn order_margin(mut self, d: Duration) -> Self {
        self.order_margin = d;
        self
    }

    /// Short poll delays, for tests.
    #[must_use]
    pub fn fast_retry(mut self) -> Self {
        self.retry = RetryPolicy::new()
            .initial_delay(Duration::from_millis(20))
            .timeout(Duration::from_secs(20));
        self.order_margin = Duration::from_secs(60);
        self
    }

    fn builder(&self) -> Result<AccountBuilder, IssueError> {
        let inner = match &self.http {
            Some(f) => f(),
            None => default_client()?,
        };
        Ok(Account::builder_with_http(Box::new(RetryAfterSlot {
            inner,
            slot: self.retry_after.clone(),
        })))
    }

    /// An error from the CA's API: a rate limit keeps the CA's `Retry-After`; anything else
    /// is `other`.
    fn api_error(&self, e: &instant_acme::Error, other: fn(String) -> IssueError) -> IssueError {
        if is_rate_limit(e) {
            let retry_after = self
                .retry_after
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
            return IssueError::RateLimited { retry_after };
        }
        other(e.to_string())
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
        let eab = self
            .eab
            .as_ref()
            .map(|(kid, key)| instant_acme::ExternalAccountKey::new(kid.clone(), key));
        let (account, creds) = builder
            .create(
                &NewAccount {
                    contact: &[],
                    terms_of_service_agreed: true,
                    only_return_existing: false,
                },
                self.directory_url.clone(),
                eab.as_ref(),
            )
            .await
            .map_err(|e| self.api_error(&e, IssueError::Account))?;
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
        self.issue_within(existing, names, solver, now, self.propagation_timeout)
            .await
    }

    /// [`AcmeCa::issue`] with a per-order `propagation` wait. The whole order is bounded by
    /// that wait plus the two CA polls, so one hung request cannot stall the runner; the
    /// cleanup below still runs when the limit fires.
    pub async fn issue_within(
        &self,
        existing: Option<&AccountCreds>,
        names: &[String],
        solver: &dyn DnsSolver,
        now: u64,
        propagation: Duration,
    ) -> Result<(CertBundle, AccountCreds), IssueError> {
        self.retry_after
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        let (account, creds) = self.open(existing).await?;
        let mut presented: Vec<(String, String)> = Vec::new();
        let limit = propagation + self.order_margin;
        let result = match tokio::time::timeout(
            limit,
            self.run_order(&account, names, solver, now, propagation, &mut presented),
        )
        .await
        {
            Ok(r) => r,
            Err(_) => Err(IssueError::Order(format!(
                "the order did not finish within {}s",
                limit.as_secs()
            ))),
        };
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
        propagation: Duration,
        presented: &mut Vec<(String, String)>,
    ) -> Result<CertBundle, IssueError> {
        let ids: Vec<Identifier> = names.iter().map(|n| Identifier::Dns(n.clone())).collect();
        let mut order = account
            .new_order(&NewOrder::new(&ids))
            .await
            .map_err(|e| self.api_error(&e, IssueError::Order))?;
        let mut authzs = order.authorizations();
        while let Some(next) = authzs.next().await {
            let mut authz = next.map_err(|e| self.classify(&e))?;
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
            solver.wait_visible(&fqdn, &value, propagation).await?;
            challenge.set_ready().await.map_err(|e| self.classify(&e))?;
        }
        let status = order
            .poll_ready(&self.retry)
            .await
            .map_err(|e| self.classify(&e))?;
        if status != OrderStatus::Ready {
            return Err(IssueError::Validation(format!("order is {status:?}")));
        }
        let key_pem = order
            .finalize()
            .await
            .map_err(|e| self.api_error(&e, IssueError::Order))?;
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

/// True for the CA's `rateLimited` problem or any 429 problem.
fn is_rate_limit(e: &instant_acme::Error) -> bool {
    matches!(e, instant_acme::Error::Api(p)
        if p.status == Some(429)
            || p.r#type.as_deref().is_some_and(|t| t.ends_with(":rateLimited")))
}

impl AcmeCa {
    /// A CA problem about the challenge itself (DNS lookup, unauthorized, CAA) is a validation
    /// failure; a rate limit keeps its `Retry-After`; anything else is an order error.
    fn classify(&self, e: &instant_acme::Error) -> IssueError {
        if is_rate_limit(e) {
            return self.api_error(e, IssueError::Order);
        }
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
