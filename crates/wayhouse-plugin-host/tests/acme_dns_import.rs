//! The `on_acme_dns` export and the `acme_dns_result` import.

use std::sync::Arc;

use wayhouse_plugin_host::http::{Hop, WireResponse};
use wayhouse_plugin_host::{
    inspect, AcmeDnsOp, AcmeDnsRequest, CallError, HttpEngine, Limits, PluginHost, SecretSource,
    SecretValue, StateSnapshot, Transport,
};

const CAPS: &str =
    r#"{"triggers":{"on_timer":true,"on_acme_dns":true},"tick_interval_secs":30,"log":true}"#;
const HTTP_CAPS: &str = r#"{"triggers":{"on_timer":true,"on_acme_dns":true},"tick_interval_secs":30,
    "http":{"hosts":[{"host":"dns.example"}]},
    "secrets":[{"name":"DNS_TOKEN","hosts":["dns.example"]}]}"#;
const HTTP_REQUEST: &str = r#"{"method":"GET","url":"https://dns.example/x","headers":{"Authorization":"Bearer ${secret:DNS_TOKEN}"}}"#;

/// How the guest's `on_acme_dns` behaves.
enum Body<'a> {
    /// Log the request document, then answer `(ok, message)`.
    Answer(i32, &'a str),
    /// Never call `acme_dns_result`.
    Silent,
    Trap,
    /// Call the provider (which expands the secret), then answer with `message`.
    ViaHttp(&'a str),
}

fn guest(caps: &str, body: &Body) -> Vec<u8> {
    let caps = caps.replace('"', "\\\"").replace('\n', " ");
    let req = HTTP_REQUEST.replace('"', "\\\"").replace('$', "\\24");
    let req_len = HTTP_REQUEST.len();
    let (code, msg) = match body {
        Body::Answer(ok, m) => (
            format!(
                "(call $log (i32.const 2) (local.get $p) (local.get $n))
                 (drop (call $result (i32.const {ok}) (i32.const 50000) (i32.const {})))",
                m.len()
            ),
            *m,
        ),
        Body::Silent => (String::new(), ""),
        Body::Trap => ("unreachable".to_string(), ""),
        Body::ViaHttp(m) => (
            format!(
                "(drop (call $req (i32.const 0) (i32.const {req_len})))
                 (drop (call $result (i32.const 0) (i32.const 50000) (i32.const {})))",
                m.len()
            ),
            *m,
        ),
    };
    let msg = msg.replace('"', "\\\"");
    wat::parse_str(format!(
        r#"(module
  (@custom "wayhouse.plugin-abi" "\00\00\02\00")
  (@custom "wayhouse.plugin-caps" "{caps}")
  (import "wayhouse" "log" (func $log (param i32 i32 i32)))
  (import "wayhouse" "acme_dns_result" (func $result (param i32 i32 i32) (result i32)))
  (import "wayhouse" "http_request" (func $req (param i32 i32) (result i32)))
  (memory (export "memory") 2)
  (data (i32.const 0) "{req}")
  (data (i32.const 50000) "{msg}")
  (global $bump (mut i32) (i32.const 1024))
  (func (export "alloc") (param i32) (result i32) (local $p i32)
    (local.set $p (global.get $bump))
    (global.set $bump (i32.add (global.get $bump) (local.get 0)))
    (local.get $p))
  (func (export "init") (param i32 i32))
  (func (export "on_timer"))
  (func (export "on_acme_dns") (param $p i32) (param $n i32)
    {code}))"#
    ))
    .unwrap()
}

fn request(op: AcmeDnsOp) -> AcmeDnsRequest {
    AcmeDnsRequest {
        op,
        fqdn: "_acme-challenge.panel.example.com".into(),
        value: "txt-value".into(),
        zone_hint: "panel.example.com".into(),
    }
}

fn plugin(caps: &str, body: &Body) -> wayhouse_plugin_host::Plugin {
    let m = guest(caps, body);
    let approved = inspect(&m).unwrap().caps;
    PluginHost::new(Limits::default())
        .unwrap()
        .load(&m, &approved)
        .unwrap()
}

#[test]
fn the_guest_gets_the_request_and_answers_ok() {
    let fx = plugin(CAPS, &Body::Answer(1, "record set"))
        .on_acme_dns(&StateSnapshot::new(), &request(AcmeDnsOp::Present))
        .unwrap();
    let seen = &fx.logs[0].msg;
    assert!(
        seen.contains(r#""op":"present""#)
            && seen.contains("_acme-challenge.panel.example.com")
            && seen.contains("txt-value")
            && seen.contains(r#""zone_hint":"panel.example.com""#),
        "{seen}"
    );
    let a = fx.acme_dns.unwrap();
    assert!(a.ok);
    assert_eq!(a.message, "record set");
}

#[test]
fn the_guest_can_report_a_failure_with_a_message() {
    let fx = plugin(CAPS, &Body::Answer(0, "zone not found"))
        .on_acme_dns(&StateSnapshot::new(), &request(AcmeDnsOp::Cleanup))
        .unwrap();
    assert!(fx.logs[0].msg.contains(r#""op":"cleanup""#));
    let a = fx.acme_dns.unwrap();
    assert!(!a.ok);
    assert_eq!(a.message, "zone not found");
}

#[test]
fn a_silent_guest_has_no_answer() {
    let fx = plugin(CAPS, &Body::Silent)
        .on_acme_dns(&StateSnapshot::new(), &request(AcmeDnsOp::Present))
        .unwrap();
    assert!(fx.acme_dns.is_none());
}

#[test]
fn a_trapping_guest_is_a_call_error() {
    let err = plugin(CAPS, &Body::Trap)
        .on_acme_dns(&StateSnapshot::new(), &request(AcmeDnsOp::Present))
        .unwrap_err();
    assert!(matches!(err, CallError::Trap(_)), "{err}");
}

#[test]
fn an_undeclared_trigger_is_never_called_and_the_import_is_denied() {
    // Declares no on_acme_dns, so there is no export to require and the host refuses.
    let no_trigger = r#"{"triggers":{"on_timer":true},"tick_interval_secs":30}"#;
    let m = wat::parse_str(
        r#"(module
  (@custom "wayhouse.plugin-abi" "\00\00\02\00")
  (@custom "wayhouse.plugin-caps" "{\"triggers\":{\"on_timer\":true},\"tick_interval_secs\":30}")
  (import "wayhouse" "acme_dns_result" (func $result (param i32 i32 i32) (result i32)))
  (memory (export "memory") 1)
  (func (export "alloc") (param i32) (result i32) (i32.const 1024))
  (func (export "init") (param i32 i32))
  (func (export "on_timer") (drop (call $result (i32.const 1) (i32.const 0) (i32.const 0)))))"#,
    )
    .unwrap();
    let approved = inspect(&m).unwrap().caps;
    assert_eq!(approved, serde_json::from_str(no_trigger).unwrap());
    let p = PluginHost::new(Limits::default())
        .unwrap()
        .load(&m, &approved)
        .unwrap();
    assert!(matches!(
        p.on_acme_dns(&StateSnapshot::new(), &request(AcmeDnsOp::Present)),
        Err(CallError::CapabilityDenied("on_acme_dns"))
    ));
    assert!(matches!(
        p.on_timer(&StateSnapshot::new()),
        Err(CallError::CapabilityDenied("on_acme_dns"))
    ));
}

#[test]
fn a_module_that_declares_the_trigger_needs_the_export() {
    let m = wat::parse_str(
        r#"(module
  (@custom "wayhouse.plugin-abi" "\00\00\02\00")
  (@custom "wayhouse.plugin-caps" "{\"triggers\":{\"on_timer\":true,\"on_acme_dns\":true},\"tick_interval_secs\":30}")
  (memory (export "memory") 1)
  (func (export "alloc") (param i32) (result i32) (i32.const 1024))
  (func (export "init") (param i32 i32))
  (func (export "on_timer")))"#,
    )
    .unwrap();
    let approved = inspect(&m).unwrap().caps;
    assert!(PluginHost::new(Limits::default())
        .unwrap()
        .load(&m, &approved)
        .is_err());
}

#[test]
fn a_module_approved_without_the_trigger_cannot_load_with_it() {
    let m = guest(CAPS, &Body::Silent);
    let mut approved = inspect(&m).unwrap().caps;
    approved.triggers.on_acme_dns = false;
    let err = PluginHost::new(Limits::default())
        .unwrap()
        .load(&m, &approved)
        .err()
        .unwrap();
    assert!(err.to_string().contains("on_acme_dns"), "{err}");
}

struct Echo;

impl Transport for Echo {
    fn send(&self, _: &Hop) -> Result<WireResponse, String> {
        Ok(WireResponse {
            status: 200,
            headers: vec![],
            body: vec![],
        })
    }
}

struct Token;

impl SecretSource for Token {
    fn get(&self, slot: &str) -> Result<Option<SecretValue>, String> {
        Ok((slot == "DNS_TOKEN").then(|| SecretValue::new(b"s3cr3t-token-value".to_vec())))
    }
}

#[test]
fn an_answer_message_never_carries_an_expanded_secret() {
    let body = Body::ViaHttp("provider rejected token s3cr3t-token-value");
    let m = guest(HTTP_CAPS, &body);
    let caps = inspect(&m).unwrap().caps;
    let engine = Arc::new(HttpEngine::new(
        caps.http.clone().unwrap(),
        caps.secrets.clone(),
        Arc::new(Token),
        Arc::new(Echo),
    ));
    let fx = PluginHost::new(Limits::default())
        .unwrap()
        .load(&m, &caps)
        .unwrap()
        .with_http(engine)
        .on_acme_dns(&StateSnapshot::new(), &request(AcmeDnsOp::Present))
        .unwrap();
    assert!(fx.used_http);
    let a = fx.acme_dns.unwrap();
    assert!(!a.ok);
    assert_eq!(a.message, "provider rejected token [redacted]");
}
