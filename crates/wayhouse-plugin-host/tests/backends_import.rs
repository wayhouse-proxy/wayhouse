//! The `backends_get` import and the `backend_health` event declaration against a WAT guest.

use std::sync::Arc;

use wayhouse_plugin_host::{
    inspect, BackendStatus, CallError, Capabilities, Limits, ModuleError, PluginHost, StateSnapshot,
};

const CAPS: &str = r#"{"triggers":{"on_timer":true,"on_event":["backend_health"]},"tick_interval_secs":30,"log":true,"backends":{"networks":["10.0.0.0/16"],"max_entries":8}}"#;
const NO_CAP: &str = r#"{"triggers":{"on_timer":true},"tick_interval_secs":30,"log":true}"#;

/// Logs whatever `backends_get` returns.
fn guest(caps: &str) -> Vec<u8> {
    let on_event = if caps.contains("on_event") {
        r#"(func (export "on_event") (param i32 i32 i32 i32))"#
    } else {
        ""
    };
    let caps = caps.replace('"', "\\\"");
    wat::parse_str(format!(
        r#"(module
  (@custom "wayhouse.plugin-abi" "\00\00\03\00")
  (@custom "wayhouse.plugin-caps" "{caps}")
  (import "wayhouse" "log" (func $log (param i32 i32 i32)))
  (import "wayhouse" "backends_get" (func $get (param i32 i32) (result i32)))
  (memory (export "memory") 1)
  (func (export "alloc") (param i32) (result i32) (i32.const 4096))
  (func (export "init") (param i32 i32))
  {on_event}
  (func (export "on_timer")
    (local $n i32)
    (local.set $n (call $get (i32.const 8192) (i32.const 1024)))
    (if (i32.gt_s (local.get $n) (i32.const 0))
      (then (call $log (i32.const 2) (i32.const 8192) (local.get $n))))))"#
    ))
    .unwrap()
}

fn status(backend: &str, up: bool) -> BackendStatus {
    BackendStatus {
        backend: backend.into(),
        up,
        since: 7,
    }
}

#[test]
fn a_plugin_sees_only_the_backends_inside_its_networks() {
    let m = guest(CAPS);
    let approved = inspect(&m).unwrap().caps;
    let plugin = PluginHost::new(Limits::default())
        .unwrap()
        .load(&m, &approved)
        .unwrap();
    plugin.set_backend_status(Arc::new(vec![
        status("10.0.0.5:25565", false),
        status("192.168.1.9:25565", true),
    ]));
    let fx = plugin.on_timer(&StateSnapshot::new()).unwrap();
    let doc = &fx.logs[0].msg;
    assert!(doc.contains("10.0.0.5:25565"), "{doc}");
    assert!(!doc.contains("192.168.1.9"), "{doc}");
}

#[test]
fn without_the_capability_the_import_is_denied() {
    let m = guest(NO_CAP);
    let approved = inspect(&m).unwrap().caps;
    let plugin = PluginHost::new(Limits::default())
        .unwrap()
        .load(&m, &approved)
        .unwrap();
    assert!(matches!(
        plugin.on_timer(&StateSnapshot::new()),
        Err(CallError::CapabilityDenied("backends"))
    ));
}

#[test]
fn suspending_backends_denies_the_import() {
    let m = guest(CAPS);
    let approved = inspect(&m).unwrap().caps;
    let plugin = PluginHost::new(Limits::default())
        .unwrap()
        .load(&m, &approved)
        .unwrap();
    plugin.set_suspended(["backends".to_string()]);
    assert!(matches!(
        plugin.on_timer(&StateSnapshot::new()),
        Err(CallError::CapabilityDenied("backends"))
    ));
}

#[test]
fn the_event_needs_the_capability_and_a_wider_network_needs_a_new_approval() {
    let no_cap = CAPS.replace(
        r#","backends":{"networks":["10.0.0.0/16"],"max_entries":8}"#,
        "",
    );
    assert!(matches!(
        Capabilities::parse(no_cap.as_bytes()),
        Err(ModuleError::CapsInvalid(_))
    ));
    let want = Capabilities::parse(CAPS.as_bytes()).unwrap();
    let wide = Capabilities::parse(CAPS.replace("10.0.0.0/16", "10.0.0.0/8").as_bytes()).unwrap();
    assert!(want.check_within(&want).is_ok());
    assert!(wide.check_within(&want).is_err());
}

#[test]
fn a_suspended_backends_capability_silences_the_event() {
    let m = guest(CAPS);
    let approved = inspect(&m).unwrap().caps;
    let plugin = PluginHost::new(Limits::default())
        .unwrap()
        .load(&m, &approved)
        .unwrap();
    // The guest's on_event is empty; what matters is that a suspended plugin is not called
    // and the call does not fail.
    plugin.set_suspended(["backends".to_string()]);
    let fx = plugin
        .on_event(&StateSnapshot::new(), "backend_health", b"{}")
        .unwrap();
    assert!(fx.logs.is_empty() && fx.state_puts.is_empty());
}
