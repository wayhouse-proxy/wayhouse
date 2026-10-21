//! `AcmeCa` against the in-process fake CA: no network, no real DNS.

use wayhouse_acme::testing::{FailKind, FakeCa, MemDns, MemHttp};
use wayhouse_acme::{AcmeCa, DelegatedSolver, HttpSolver, IssueError};

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

#[tokio::test]
async fn a_hung_solver_fails_the_order_and_still_cleans_up() {
    struct Hangs(MemDns);
    #[async_trait::async_trait]
    impl wayhouse_acme::DnsSolver for Hangs {
        async fn present(&self, f: &str, v: &str) -> Result<(), wayhouse_acme::SolverError> {
            self.0.present(f, v).await
        }
        async fn cleanup(&self, f: &str, v: &str) -> Result<(), wayhouse_acme::SolverError> {
            self.0.cleanup(f, v).await
        }
        async fn wait_visible(
            &self,
            _: &str,
            _: &str,
            _: std::time::Duration,
        ) -> Result<(), wayhouse_acme::SolverError> {
            std::future::pending().await
        }
    }
    let dns = MemDns::default();
    let fake = FakeCa::start(dns.clone()).await;
    let err = ca(&fake, true)
        .order_margin(std::time::Duration::from_millis(200))
        .issue_within(
            None,
            &names(&["a.example.com"]),
            &Hangs(dns.clone()),
            1,
            std::time::Duration::from_millis(100),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(&err, IssueError::Order(m) if m.contains("did not finish")),
        "{err}"
    );
    assert!(dns.is_empty());
}

const EAB_KEY: &str = "c2VjcmV0LWhtYWMta2V5LWZvci10ZXN0cw";

#[tokio::test]
async fn an_account_is_created_with_the_external_account_binding_the_ca_requires() {
    let dns = MemDns::default();
    let fake = FakeCa::start(dns.clone()).await;
    fake.require_eab("kid-1", EAB_KEY);
    let with = ca(&fake, true).external_account("kid-1", EAB_KEY).unwrap();
    let (_, creds) = with
        .issue(None, &names(&["a.example.com"]), &dns, 1)
        .await
        .unwrap();
    assert!(!creds.account_url.is_empty());
    assert_eq!(fake.accounts(), 1);
    // The standard-alphabet, padded spelling of the same key works too.
    let fake2 = FakeCa::start(dns.clone()).await;
    fake2.require_eab("kid-1", EAB_KEY);
    ca(&fake2, true)
        .external_account("kid-1", "c2VjcmV0LWhtYWMta2V5LWZvci10ZXN0cw==")
        .unwrap()
        .ensure_account(None)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_ca_that_requires_a_binding_refuses_an_account_without_a_valid_one() {
    let dns = MemDns::default();
    let fake = FakeCa::start(dns.clone()).await;
    fake.require_eab("kid-1", EAB_KEY);
    let none = ca(&fake, true).ensure_account(None).await.unwrap_err();
    assert!(none.to_string().contains("binding"), "{none}");
    let wrong_id = ca(&fake, true)
        .external_account("other", EAB_KEY)
        .unwrap()
        .ensure_account(None)
        .await
        .unwrap_err();
    assert!(wrong_id.to_string().contains("key id"), "{wrong_id}");
    let wrong_key = ca(&fake, true)
        .external_account("kid-1", "d3Jvbmcta2V5")
        .unwrap()
        .ensure_account(None)
        .await
        .unwrap_err();
    assert!(wrong_key.to_string().contains("verify"), "{wrong_key}");
    assert_eq!(fake.accounts(), 0);
}

#[tokio::test]
async fn a_malformed_binding_is_refused_before_any_request() {
    let fake_url = "https://ca.example/dir";
    assert!(AcmeCa::new(fake_url, true)
        .external_account("", EAB_KEY)
        .is_err());
    assert!(AcmeCa::new(fake_url, true)
        .external_account("kid", "***")
        .is_err());
    assert!(AcmeCa::new(fake_url, true)
        .external_account("kid", "")
        .is_err());
}

#[tokio::test]
async fn a_delegated_zone_gets_the_record_and_the_original_name_is_a_cname_to_it() {
    let dns = MemDns::default();
    let fake = FakeCa::start(dns.clone()).await;
    dns.cname(
        "_acme-challenge.panel.example.com",
        "_acme-challenge.panel.example.com.acme.other.net",
    );
    let solver = DelegatedSolver::new(dns.clone(), "acme.other.net");
    ca(&fake, true)
        .issue(None, &names(&["panel.example.com"]), &solver, 1)
        .await
        .unwrap();
    let log = dns.log.lock().unwrap().clone();
    assert_eq!(
        log,
        [
            (
                "present",
                "_acme-challenge.panel.example.com.acme.other.net".to_string()
            ),
            (
                "cleanup",
                "_acme-challenge.panel.example.com.acme.other.net".to_string()
            ),
        ],
        "only the delegated name is ever written"
    );
    assert!(dns.is_empty());
}

#[tokio::test]
async fn a_missing_cname_fails_the_order_with_a_hint_and_still_cleans_up() {
    let dns = MemDns::default();
    let fake = FakeCa::start(dns.clone()).await;
    let solver = DelegatedSolver::new(dns.clone(), "acme.other.net");
    let err = ca(&fake, true)
        .issue(None, &names(&["panel.example.com"]), &solver, 1)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("CNAME"), "{err}");
    assert!(dns.is_empty());
}

const WAIT: std::time::Duration = std::time::Duration::from_secs(5);

#[tokio::test]
async fn http_01_publishes_each_token_and_withdraws_it() {
    let (dns, http) = (MemDns::default(), MemHttp::default());
    let fake = FakeCa::start(dns.clone()).await;
    fake.serve_http_challenges(http.clone());
    let (bundle, _) = ca(&fake, true)
        .issue_http_within(
            None,
            &names(&["a.example.com", "b.example.com"]),
            &http,
            7,
            WAIT,
        )
        .await
        .unwrap();
    assert_eq!(bundle.names.len(), 2);
    assert_eq!(fake.issued(), 1);
    let log = http.log.lock().unwrap().clone();
    let count = |op: &str| log.iter().filter(|(o, _)| *o == op).count();
    assert_eq!((count("present"), count("cleanup")), (2, 2));
    assert!(http.is_empty(), "answers withdrawn after the order");
    assert!(dns.is_empty(), "no DNS record was involved");
}

#[tokio::test]
async fn http_01_refuses_a_wildcard_before_asking_the_ca() {
    let (dns, http) = (MemDns::default(), MemHttp::default());
    let fake = FakeCa::start(dns).await;
    fake.serve_http_challenges(http.clone());
    let err = ca(&fake, true)
        .issue_http_within(None, &names(&["*.example.com"]), &http, 1, WAIT)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("wildcard"), "{err}");
    assert_eq!(fake.accounts(), 0);
}

#[tokio::test]
async fn http_01_fails_when_the_answer_is_not_served() {
    struct Liar;
    #[async_trait::async_trait]
    impl HttpSolver for Liar {
        async fn present(&self, _: &str, _: &str) -> Result<(), wayhouse_acme::SolverError> {
            Ok(())
        }
        async fn cleanup(&self, _: &str) -> Result<(), wayhouse_acme::SolverError> {
            Ok(())
        }
    }
    let fake = FakeCa::start(MemDns::default()).await;
    fake.serve_http_challenges(MemHttp::default());
    let err = ca(&fake, true)
        .issue_http_within(None, &names(&["a.example.com"]), &Liar, 1, WAIT)
        .await
        .unwrap_err();
    assert!(matches!(err, IssueError::Validation(_)), "{err}");
    assert_eq!(fake.issued(), 0);
}

#[tokio::test]
async fn a_failing_http_solver_fails_the_order_and_still_cleans_up() {
    let http = MemHttp::default();
    let fake = FakeCa::start(MemDns::default()).await;
    fake.serve_http_challenges(http.clone());
    http.fail_next_present("replication failed");
    let err = ca(&fake, true)
        .issue_http_within(None, &names(&["a.example.com"]), &http, 1, WAIT)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, IssueError::Solver(e) if e.0 == "replication failed"),
        "{err}"
    );
    assert!(http.is_empty());
}
