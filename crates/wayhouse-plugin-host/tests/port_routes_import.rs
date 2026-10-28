//! The `port_routes_set` and `port_routes_get` imports against a WAT guest.

use std::collections::BTreeMap;
use std::sync::Arc;

use wayhouse_plugin_host::{
    inspect, CallError, Limits, PluginHost, PortProtoSel, PortRouteEntry, StateSnapshot,
};

const CAPS: &str = r#"{"triggers":{"on_timer":true},"tick_interval_secs":30,"log":true,"port_routes":{"ports":["30000-30099"],"proto":["udp","tcp"],"backends":["10.0.0.0/16"],"max_entries":3}}"#;

/// Declares `doc` as the port route set, logs the import's return value as a digit (`0`
/// accepted, `1` refused), then logs whatever `port_routes_get` returns.
fn guest(caps: &str, doc: &str) -> Vec<u8> {
    let caps = caps.replace('"', "\\\"");
    let data = doc.replace('"', "\\\"");
    let len = doc.len();
    wat::parse_str(format!(
        r#"(module
  (@custom "wayhouse.plugin-abi" "\00\00\02\00")
  (@custom "wayhouse.plugin-caps" "{caps}")
  (import "wayhouse" "log" (func $log (param i32 i32 i32)))
  (import "wayhouse" "port_routes_set" (func $set (param i32 i32) (result i32)))
  (import "wayhouse" "port_routes_get" (func $get (param i32 i32) (result i32)))
  (memory (export "memory") 1)
  (data (i32.const 0) "{data}")
  (data (i32.const 2000) "01")
  (func (export "alloc") (param i32) (result i32) (i32.const 4096))
  (func (export "init") (param i32 i32))
  (func (export "on_timer")
    (local $n i32)
    (call $log (i32.const 2) (i32.add (i32.const 2000)
      (i32.lt_s (call $set (i32.const 0) (i32.const {len})) (i32.const 0))) (i32.const 1))
    (local.set $n (call $get (i32.const 8192) (i32.const 1024)))
    (if (i32.gt_s (local.get $n) (i32.const 0))
      (then (call $log (i32.const 2) (i32.const 8192) (local.get $n))))))"#
    ))
    .unwrap()
}

fn plugin(caps: &str, doc: &str) -> wayhouse_plugin_host::Plugin {
    let m = guest(caps, doc);
    let approved = inspect(&m).unwrap().caps;
    PluginHost::new(Limits::default())
        .unwrap()
        .load(&m, &approved)
        .unwrap()
}

fn run(caps: &str, doc: &str) -> Result<wayhouse_plugin_host::Effects, CallError> {
    plugin(caps, doc).on_timer(&StateSnapshot::new())
}

fn entry(key: &str, backend: &str, proto: PortProtoSel, prefer: Option<u16>) -> PortRouteEntry {
    PortRouteEntry {
        key: key.into(),
        backend: backend.into(),
        proto,
        prefer,
    }
}

#[test]
fn an_approved_set_is_returned_sorted_by_key() {
    let fx = run(
        CAPS,
        r#"[{"key":"b","backend":"10.0.1.2:7777","proto":"udp"},{"key":"a","backend":"10.0.1.3:25565","proto":"both","prefer":30010}]"#,
    )
    .unwrap();
    assert_eq!(fx.logs[0].msg, "0");
    assert_eq!(
        fx.port_routes.unwrap(),
        vec![
            entry("a", "10.0.1.3:25565", PortProtoSel::Both, Some(30010)),
            entry("b", "10.0.1.2:7777", PortProtoSel::Udp, None),
        ]
    );
}

#[test]
fn an_empty_set_clears_the_port_routes() {
    assert_eq!(run(CAPS, "[]").unwrap().port_routes, Some(vec![]));
}

#[test]
fn a_refused_set_changes_nothing_and_says_why() {
    for doc in [
        // backend outside the approved networks
        r#"[{"key":"a","backend":"192.168.1.2:1","proto":"udp"}]"#,
        // prefer outside the approved ports
        r#"[{"key":"a","backend":"10.0.1.2:1","proto":"udp","prefer":31000}]"#,
        // duplicate key
        r#"[{"key":"a","backend":"10.0.1.2:1","proto":"udp"},{"key":"a","backend":"10.0.1.3:1","proto":"udp"}]"#,
        // over the entry cap
        r#"[{"key":"a","backend":"10.0.1.2:1","proto":"udp"},{"key":"b","backend":"10.0.1.2:2","proto":"udp"},{"key":"c","backend":"10.0.1.2:3","proto":"udp"},{"key":"d","backend":"10.0.1.2:4","proto":"udp"}]"#,
        // unknown field, unknown proto, empty key, junk
        r#"[{"key":"a","backend":"10.0.1.2:1","proto":"udp","x":1}]"#,
        r#"[{"key":"a","backend":"10.0.1.2:1","proto":"sctp"}]"#,
        r#"[{"key":"","backend":"10.0.1.2:1","proto":"udp"}]"#,
        "not json",
    ] {
        let fx = run(CAPS, doc).unwrap();
        assert!(fx.port_routes.is_none(), "{doc}");
        assert_eq!(fx.logs.last().unwrap().msg, "1", "{doc}");
        assert!(
            fx.logs
                .iter()
                .any(|l| l.msg.starts_with("port_routes_set refused")),
            "{doc}"
        );
    }
}

#[test]
fn a_protocol_the_approval_lacks_is_refused() {
    let caps = CAPS.replace(r#""proto":["udp","tcp"]"#, r#""proto":["udp"]"#);
    for proto in ["tcp", "both"] {
        let doc = format!(r#"[{{"key":"a","backend":"10.0.1.2:1","proto":"{proto}"}}]"#);
        assert!(run(&caps, &doc).unwrap().port_routes.is_none(), "{proto}");
    }
}

#[test]
fn the_plugin_reads_its_assigned_ports_back() {
    let p = plugin(CAPS, "[]");
    let none = p.on_timer(&StateSnapshot::new()).unwrap();
    assert_eq!(none.logs.len(), 1, "nothing assigned, nothing logged");
    p.set_port_assignments(Arc::new(BTreeMap::from([
        ("a".to_string(), 30010u16),
        ("b".to_string(), 30011),
    ])));
    let fx = p.on_timer(&StateSnapshot::new()).unwrap();
    assert_eq!(fx.logs[1].msg, r#"{"a":30010,"b":30011}"#);
}

#[test]
fn port_routes_without_the_capability_trap_the_call() {
    let caps = r#"{"triggers":{"on_timer":true},"tick_interval_secs":30,"log":true}"#;
    assert!(matches!(
        run(caps, "[]"),
        Err(CallError::CapabilityDenied("port_routes"))
    ));
}

#[test]
fn a_module_cannot_declare_more_than_was_approved() {
    let m = guest(CAPS, "[]");
    let declared = inspect(&m).unwrap().caps;
    for widen in [
        |c: &mut wayhouse_plugin_host::PortRouteCap| c.max_entries = 2,
        |c: &mut wayhouse_plugin_host::PortRouteCap| c.ports = vec!["30000-30009".into()],
        |c: &mut wayhouse_plugin_host::PortRouteCap| c.proto.truncate(1),
        |c: &mut wayhouse_plugin_host::PortRouteCap| c.backends = vec!["10.0.0.0/24".into()],
    ] {
        // Shrinking the approval below the declaration must refuse the load.
        let mut approved = declared.clone();
        widen(approved.port_routes.as_mut().unwrap());
        assert!(PluginHost::new(Limits::default())
            .unwrap()
            .load(&m, &approved)
            .is_err());
    }
}
