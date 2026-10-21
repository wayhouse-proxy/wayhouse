//! `AcmeCa` against the in-process fake CA: no network, no real DNS.

use wayhouse_acme::testing::{FailKind, FakeCa, MemDns};
use wayhouse_acme::{AcmeCa, IssueError};

fn ca(fake: &FakeCa, tos: bool) -> AcmeCa {
    AcmeCa::new(&fake.directory_url(), tos)
        .with_http(FakeCa::http_factory())
        .fast_retry()
}

fn names(n: &[&str]) -> Vec<String> {
    n.iter().map(ToString::to_string).collect()
}

#[tokio::test]
async fn issues_a_single_name_and_cleans_up() {
    let dns = MemDns::default();
    let fake = FakeCa::start(dns.clone()).await;
    let (bundle, creds) = ca(&fake, true)
        .issue(None, &names(&["panel.example.com"]), &dns, 1_000)
        .await
        .unwrap();
    assert!(bundle.chain_pem.contains("BEGIN CERTIFICATE"));
    assert!(bundle.key_pem.expose().contains("PRIVATE KEY"));
    assert_eq!(bundle.issued_at, 1_000);
    assert!(bundle.not_after > 1_000_000_000, "parsed from the leaf");
    assert!(!creds.account_url.is_empty());
    assert!(dns.is_empty(), "TXT removed after the order");
    assert_eq!(fake.issued(), 1);
}

#[tokio::test]
async fn wildcard_and_apex_present_two_values_at_one_name() {
    let dns = MemDns::default();
    let fake = FakeCa::start(dns.clone()).await;
    let (bundle, _) = ca(&fake, true)
        .issue(None, &names(&["example.com", "*.example.com"]), &dns, 5)
        .await
        .unwrap();
    assert_eq!(bundle.names.len(), 2);
    let log = dns.log.lock().unwrap().clone();
    let presents = log
        .iter()
        .filter(|(op, f)| *op == "present" && f == "_acme-challenge.example.com")
        .count();
    assert_eq!(presents, 2);
    assert!(dns.is_empty());
}

#[tokio::test]
async fn reuses_the_account() {
    let dns = MemDns::default();
    let fake = FakeCa::start(dns.clone()).await;
    let (_, creds) = ca(&fake, true)
        .issue(None, &names(&["a.example.com"]), &dns, 1)
        .await
        .unwrap();
    // The terms flag is irrelevant once an account exists.
    let (_, again) = ca(&fake, false)
        .issue(Some(&creds), &names(&["b.example.com"]), &dns, 2)
        .await
        .unwrap();
    assert_eq!(again.account_url, creds.account_url);
    assert_eq!(fake.accounts(), 1);
    assert_eq!(fake.issued(), 2);
}

#[tokio::test]
async fn refuses_to_create_an_account_without_the_terms() {
    let dns = MemDns::default();
    let fake = FakeCa::start(dns.clone()).await;
    let err = ca(&fake, false).ensure_account(None).await.unwrap_err();
    assert!(matches!(err, IssueError::TosNotAccepted), "{err}");
    assert_eq!(fake.accounts(), 0);
}

#[tokio::test]
async fn a_rejected_challenge_fails_the_order_and_still_cleans_up() {
    let dns = MemDns::default();
    let fake = FakeCa::start(dns.clone()).await;
    fake.fail_next(FailKind::Dns);
    let err = ca(&fake, true)
        .issue(None, &names(&["a.example.com"]), &dns, 1)
        .await
        .unwrap_err();
    assert!(matches!(err, IssueError::Validation(_)), "{err}");
    assert!(dns.is_empty());
    assert_eq!(fake.issued(), 0);
}

#[tokio::test]
async fn the_fake_ca_really_validates_the_txt_value() {
    // A solver that claims success without publishing the record is caught by the CA.
    struct Liar;
    #[async_trait::async_trait]
    impl wayhouse_acme::DnsSolver for Liar {
        async fn present(&self, _: &str, _: &str) -> Result<(), wayhouse_acme::SolverError> {
            Ok(())
        }
        async fn cleanup(&self, _: &str, _: &str) -> Result<(), wayhouse_acme::SolverError> {
            Ok(())
        }
        async fn wait_visible(
            &self,
            _: &str,
            _: &str,
            _: std::time::Duration,
        ) -> Result<(), wayhouse_acme::SolverError> {
            Ok(())
        }
    }
    let fake = FakeCa::start(MemDns::default()).await;
    let err = ca(&fake, true)
        .issue(None, &names(&["a.example.com"]), &Liar, 1)
        .await
        .unwrap_err();
    assert!(matches!(err, IssueError::Validation(_)), "{err}");
}

#[tokio::test]
async fn a_failing_solver_fails_the_order_with_its_message() {
    let dns = MemDns::default();
    let fake = FakeCa::start(dns.clone()).await;
    dns.fail_next_present("provider said no");
    let err = ca(&fake, true)
        .issue(None, &names(&["a.example.com"]), &dns, 1)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, IssueError::Solver(e) if e.0 == "provider said no"),
        "{err}"
    );
    assert!(dns.is_empty());
}

#[tokio::test]
async fn rate_limit_carries_the_retry_after() {
    let dns = MemDns::default();
    let fake = FakeCa::start(dns.clone()).await;
    fake.retry_after(7200);
    fake.fail_next(FailKind::RateLimited);
    let err = ca(&fake, true)
        .issue(None, &names(&["a.example.com"]), &dns, 1)
        .await
        .unwrap_err();
    assert!(
        matches!(err, IssueError::RateLimited { retry_after: Some(d) } if d == std::time::Duration::from_secs(7200)),
        "{err}"
    );
}

#[tokio::test]
async fn rate_limit_without_a_header_has_no_retry_after() {
    let dns = MemDns::default();
    let fake = FakeCa::start(dns.clone()).await;
    fake.fail_next(FailKind::RateLimited);
    let err = ca(&fake, true)
        .issue(None, &names(&["a.example.com"]), &dns, 1)
        .await
        .unwrap_err();
    assert!(
        matches!(err, IssueError::RateLimited { retry_after: None }),
        "{err}"
    );
}
