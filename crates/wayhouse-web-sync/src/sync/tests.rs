use std::cell::RefCell;
use std::collections::VecDeque;

use wayhouse_config::{WebSiteView, WebUpstreamView, WebVia, WebView};

use super::*;
use crate::pools::PoolBackends;
use crate::source::SourceError;

#[derive(Default)]
struct FakeSource {
    web: RefCell<VecDeque<Result<Option<WebView>, String>>>,
    pools: RefCell<Option<Result<PoolBackends, String>>>,
}

impl Source for FakeSource {
    async fn web(&self) -> Result<Option<WebView>, SourceError> {
        self.web
            .borrow_mut()
            .pop_front()
            .expect("scripted web")
            .map_err(SourceError)
    }
    async fn pools(&self) -> Result<PoolBackends, SourceError> {
        self.pools
            .borrow()
            .clone()
            .expect("scripted pools")
            .map_err(SourceError)
    }
}

#[derive(Default)]
struct FakeLoader {
    replies: RefCell<VecDeque<Result<(), LoadError>>>,
    sent: RefCell<Vec<Value>>,
}

impl Loader for FakeLoader {
    async fn load(&self, doc: &Value) -> Result<(), LoadError> {
        self.sent.borrow_mut().push(doc.clone());
        self.replies.borrow_mut().pop_front().unwrap_or(Ok(()))
    }
}

fn site(upstream: WebUpstreamView) -> WebSiteView {
    WebSiteView {
        hosts: vec!["a.example.com".into()],
        upstreams: vec![upstream],
        tls: None,
        upstream_tls: None,
        health_path: None,
    }
}

fn web(sites: Vec<WebSiteView>) -> WebView {
    WebView {
        pool: "web".into(),
        listen_http: "127.0.0.1:8080".into(),
        listen_https: "127.0.0.1:8443".into(),
        trusted_proxy: vec![],
        sites,
    }
}

fn addr_site() -> WebSiteView {
    site(WebUpstreamView::Addr {
        addr: "10.0.0.1:80".into(),
    })
}

fn syncer(script: Vec<Result<Option<WebView>, String>>) -> Syncer<FakeSource, FakeLoader> {
    let src = FakeSource::default();
    *src.web.borrow_mut() = script.into();
    Syncer::new(
        src,
        FakeLoader::default(),
        AdminAddr::parse("127.0.0.1:2019").unwrap(),
    )
}

fn sent(s: &Syncer<FakeSource, FakeLoader>) -> usize {
    s.loader.sent.borrow().len()
}

#[tokio::test]
async fn a_failed_read_at_startup_sends_nothing() {
    let mut s = syncer(vec![Err("connection refused".into())]);
    assert_eq!(s.run_once(false).await, Outcome::Failed("read"));
    assert_eq!(sent(&s), 0);
}

#[tokio::test]
async fn no_web_section_and_an_empty_site_list_at_startup_send_nothing() {
    let mut s = syncer(vec![Ok(None), Ok(Some(web(vec![])))]);
    assert_eq!(s.run_once(false).await, Outcome::Idle);
    assert_eq!(s.run_once(false).await, Outcome::Idle);
    assert_eq!(sent(&s), 0);
}

#[tokio::test]
async fn loads_once_then_reports_unchanged() {
    let w = web(vec![addr_site()]);
    let mut s = syncer(vec![Ok(Some(w.clone())), Ok(Some(w))]);
    assert_eq!(s.run_once(false).await, Outcome::Loaded);
    assert_eq!(s.run_once(false).await, Outcome::Unchanged);
    assert_eq!(sent(&s), 1);
}

#[tokio::test]
async fn resend_reloads_an_identical_document() {
    let w = web(vec![addr_site()]);
    let mut s = syncer(vec![Ok(Some(w.clone())), Ok(Some(w))]);
    assert_eq!(s.run_once(false).await, Outcome::Loaded);
    assert_eq!(s.run_once(true).await, Outcome::Loaded);
    assert_eq!(sent(&s), 2);
}

#[tokio::test]
async fn a_failed_read_after_a_load_keeps_what_caddy_has() {
    let w = web(vec![addr_site()]);
    let mut s = syncer(vec![Ok(Some(w)), Err("boom".into())]);
    assert_eq!(s.run_once(false).await, Outcome::Loaded);
    assert_eq!(s.run_once(false).await, Outcome::Failed("read"));
    assert_eq!(sent(&s), 1);
}

#[tokio::test]
async fn removing_every_site_after_a_load_sends_a_complete_document_without_routes() {
    let mut s = syncer(vec![
        Ok(Some(web(vec![addr_site()]))),
        Ok(Some(web(vec![]))),
    ]);
    assert_eq!(s.run_once(false).await, Outcome::Loaded);
    assert_eq!(s.run_once(false).await, Outcome::Loaded);
    let sent = s.loader.sent.borrow();
    let doc = &sent[1];
    assert!(doc["admin"].is_object(), "never a bare {{}}");
    assert_eq!(
        doc["apps"]["http"]["servers"]["https"]["routes"],
        serde_json::json!([])
    );
}

#[tokio::test]
async fn removing_the_web_section_after_a_load_withdraws_the_sites() {
    let mut s = syncer(vec![Ok(Some(web(vec![addr_site()]))), Ok(None)]);
    assert_eq!(s.run_once(false).await, Outcome::Loaded);
    assert_eq!(s.run_once(false).await, Outcome::Loaded);
    let sent = s.loader.sent.borrow();
    assert_eq!(
        sent[1]["apps"]["http"]["servers"]["https"]["routes"],
        serde_json::json!([])
    );
}

#[tokio::test]
async fn a_rejected_document_is_counted_once_not_resent_every_cycle() {
    let w = web(vec![addr_site()]);
    let mut s = syncer(vec![Ok(Some(w.clone())), Ok(Some(w.clone())), Ok(Some(w))]);
    s.loader
        .replies
        .borrow_mut()
        .push_back(Err(LoadError::Rejected {
            status: 400,
            body: "bad".into(),
        }));
    assert_eq!(s.run_once(false).await, Outcome::Rejected);
    assert_eq!(s.run_once(false).await, Outcome::Rejected);
    assert_eq!(sent(&s), 1, "the same refused document is not sent again");
    // A different document goes out again.
    let mut s2 = syncer(vec![
        Ok(Some(web(vec![addr_site()]))),
        Ok(Some(web(vec![addr_site(), addr_site()]))),
    ]);
    s2.loader
        .replies
        .borrow_mut()
        .push_back(Err(LoadError::Rejected {
            status: 400,
            body: "bad".into(),
        }));
    assert_eq!(s2.run_once(false).await, Outcome::Rejected);
    assert_eq!(s2.run_once(false).await, Outcome::Loaded);
}

#[tokio::test]
async fn an_unreachable_caddy_is_retried_with_the_same_document() {
    let w = web(vec![addr_site()]);
    let mut s = syncer(vec![Ok(Some(w.clone())), Ok(Some(w))]);
    s.loader
        .replies
        .borrow_mut()
        .push_back(Err(LoadError::Unreachable("refused".into())));
    assert_eq!(s.run_once(false).await, Outcome::Failed("send"));
    assert_eq!(s.run_once(false).await, Outcome::Loaded);
    assert_eq!(sent(&s), 2);
}

fn direct_site() -> WebSiteView {
    site(WebUpstreamView::Pool {
        pool: "wings".into(),
        via: WebVia::Direct,
        local: None,
    })
}

#[tokio::test]
async fn pools_are_only_read_for_direct_upstreams_and_a_failed_read_sends_nothing() {
    let mut s = syncer(vec![Ok(Some(web(vec![addr_site()])))]);
    // No `pools` scripted: reading them here would panic.
    assert_eq!(s.run_once(false).await, Outcome::Loaded);

    let mut s = syncer(vec![Ok(Some(web(vec![direct_site()])))]);
    *s.source.pools.borrow_mut() = Some(Err("timeout".into()));
    assert_eq!(s.run_once(false).await, Outcome::Failed("read"));
    assert_eq!(sent(&s), 0);
}

#[tokio::test]
async fn a_drain_reaches_caddy_through_the_next_pools_read() {
    let w = web(vec![direct_site()]);
    let mut s = syncer(vec![Ok(Some(w.clone())), Ok(Some(w))]);
    let both = PoolBackends::from([(
        "wings".to_string(),
        vec!["10.0.0.6:1".parse().unwrap(), "10.0.0.7:1".parse().unwrap()],
    )]);
    *s.source.pools.borrow_mut() = Some(Ok(both));
    assert_eq!(s.run_once(false).await, Outcome::Loaded);
    let one = PoolBackends::from([("wings".to_string(), vec!["10.0.0.7:1".parse().unwrap()])]);
    *s.source.pools.borrow_mut() = Some(Ok(one));
    assert_eq!(s.run_once(false).await, Outcome::Loaded);
    let sent = s.loader.sent.borrow();
    let dials = |d: &Value| {
        d["apps"]["http"]["servers"]["https"]["routes"][0]["handle"][0]["upstreams"].clone()
    };
    assert_eq!(dials(&sent[0]).as_array().unwrap().len(), 2);
    assert_eq!(dials(&sent[1]), serde_json::json!([{"dial": "10.0.0.7:1"}]));
}

#[tokio::test]
async fn an_unreadable_certificate_sends_nothing() {
    let mut site = addr_site();
    site.tls = Some(wayhouse_config::WebSiteTlsView {
        cert: "/nonexistent/c.pem".into(),
        key: "/nonexistent/k.pem".into(),
    });
    let mut s = syncer(vec![Ok(Some(web(vec![site])))]);
    assert_eq!(s.run_once(false).await, Outcome::Failed("render"));
    assert_eq!(sent(&s), 0);
}
