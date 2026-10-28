//! Capability bindings through the loader: a bound module runs with its effective
//! capabilities and can never run with more than the operator approved.

use serde_json::{json, Value};
use wayhouse_plugin_host::{inspect, Limits, ModuleError, PluginHost, StateSnapshot};

const CAPS: &str = r#"{"triggers":{"on_timer":true},"tick_interval_secs":30,"log":true,"routes":{"hosts":["${opt:pat}"],"backends":["${opt:net}"],"max_entries":4}}"#;
const MANIFEST: &str = r#"{"name":"bound","options":{"type":"object","required":["pat"],"properties":{"pat":{"type":"string"},"net":{"type":"string","default":"10.0.0.0/16"}}},"bindings":[{"option":"pat","kind":"route_host_pattern"},{"option":"net","kind":"cidr"}]}"#;
const DOC: &str = r#"[{"host":"a.mc.example.com","backend":"10.0.1.2:25565"}]"#;

fn guest() -> Vec<u8> {
    let esc = |s: &str| s.replace('"', "\\\"");
    let (caps, manifest, data) = (esc(CAPS), esc(MANIFEST), esc(DOC));
    let len = DOC.len();
    wat::parse_str(format!(
        r#"(module
  (@custom "wayhouse.plugin-abi" "\00\00\02\00")
  (@custom "wayhouse.plugin-caps" "{caps}")
  (@custom "wayhouse.plugin-manifest" "{manifest}")
  (import "wayhouse" "log" (func $log (param i32 i32 i32)))
  (import "wayhouse" "routes_set" (func $set (param i32 i32) (result i32)))
  (memory (export "memory") 1)
  (data (i32.const 0) "{data}")
  (data (i32.const 2000) "01")
  (func (export "alloc") (param i32) (result i32) (i32.const 4096))
  (func (export "init") (param i32 i32))
  (func (export "on_timer")
    (call $log (i32.const 2) (i32.add (i32.const 2000)
      (i32.lt_s (call $set (i32.const 0) (i32.const {len})) (i32.const 0))) (i32.const 1))))"#
    ))
    .unwrap()
}

fn host() -> PluginHost {
    PluginHost::new(Limits::default()).unwrap()
}

/// What the operator approves for `config`: the module's effective set.
fn approval(m: &[u8], config: &Value) -> wayhouse_plugin_host::Capabilities {
    inspect(m).unwrap().effective_caps(config).unwrap()
}

#[test]
fn a_bound_module_runs_with_exactly_the_effective_caps() {
    let m = guest();
    let config = json!({"pat":"*.mc.example.com"});
    let approved = approval(&m, &config);
    let p = host().load_with_config(&m, &approved, &config).unwrap();
    let fx = p.on_timer(&StateSnapshot::new()).unwrap();
    assert_eq!(fx.logs[0].msg, "0", "inside the effective set");
    assert_eq!(fx.routes.unwrap().len(), 1);

    // Same module, an install whose pattern does not cover the route: the call is refused.
    let config = json!({"pat":"*.other.example.com"});
    let approved = approval(&m, &config);
    let p = host().load_with_config(&m, &approved, &config).unwrap();
    let fx = p.on_timer(&StateSnapshot::new()).unwrap();
    assert!(
        fx.logs[0].msg.contains("outside the approved hosts"),
        "{:?}",
        fx.logs
    );
    assert!(fx.routes.is_none());
}

#[test]
fn a_wider_config_cannot_load_under_an_older_approval() {
    let m = guest();
    let approved = approval(&m, &json!({"pat":"a.mc.example.com"}));
    let load = |config: Value| host().load_with_config(&m, &approved, &config).map(|_| ());
    for wider in [
        json!({"pat":"*.mc.example.com"}),
        json!({"pat":"b.mc.example.com"}),
        json!({"pat":"a.mc.example.com","net":"10.0.0.0/8"}),
        json!({"pat":"a.mc.example.com","net":"0.0.0.0/0"}),
    ] {
        assert!(
            matches!(load(wider.clone()), Err(ModuleError::CapsNotApproved(_))),
            "{wider}"
        );
    }
    // Narrowing (or repeating) the value stays inside the approval.
    load(json!({"pat":"a.mc.example.com"})).unwrap();
    load(json!({"pat":"a.mc.example.com","net":"10.0.1.0/24"})).unwrap();
}

#[test]
fn a_bound_module_loaded_without_its_options_fails_closed() {
    let m = guest();
    let approved = approval(&m, &json!({"pat":"a.mc.example.com"}));
    for r in [
        host().load(&m, &approved),
        host().load_with_config(&m, &approved, &json!({})),
        host().load_with_config(&m, &approved, &Value::Null),
    ] {
        assert!(matches!(r, Err(ModuleError::OptionsInvalid(_))));
    }
}

#[test]
fn the_static_part_alone_approves_nothing_for_a_bound_section() {
    let m = guest();
    let approved = inspect(&m).unwrap().caps; // routes left out
    let config = json!({"pat":"a.mc.example.com"});
    let r = host().load_with_config(&m, &approved, &config);
    assert!(matches!(r, Err(ModuleError::CapsNotApproved(_))));
}
