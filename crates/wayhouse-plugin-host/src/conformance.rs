//! Conformance check for a built plugin module: what the host would say about it.
//!
//! The module is loaded with its own declaration as the approved set, then `init` (empty
//! config, empty state) and, when declared, `on_timer` are called once under the default
//! limits. Anything the host would refuse, and any capability the plugin used without
//! declaring, is a problem. Used by `wayhouse-plugin-check` and plugin repo CI.

use crate::caps::Capabilities;
use crate::module::{inspect, ModuleInfo};
use crate::runtime::{CallError, Effects, Limits, PluginHost, StateSnapshot};

#[derive(Debug)]
pub struct Report {
    pub info: Option<ModuleInfo>,
    /// What the module asks for under the config given to the check.
    pub effective: Option<Capabilities>,
    pub problems: Vec<String>,
    pub init: Option<Effects>,
    pub on_timer: Option<Effects>,
}

impl Report {
    pub fn ok(&self) -> bool {
        self.problems.is_empty()
    }
}

fn describe(entry: &str, e: &CallError) -> String {
    match e {
        CallError::CapabilityDenied(cap) => {
            format!("{entry} used `{cap}` without declaring it in wayhouse.plugin-caps")
        }
        other => format!("{entry} failed: {other}"),
    }
}

/// Check `bytes` as the host would load it, with an empty install config.
pub fn check(bytes: &[u8]) -> Report {
    check_with_config(bytes, &serde_json::Value::Null)
}

/// Check `bytes` as the host would load it for an install with `config`: the config is
/// checked against the module's options and handed to `init`.
pub fn check_with_config(bytes: &[u8], config: &serde_json::Value) -> Report {
    let mut report = Report {
        info: None,
        effective: None,
        problems: Vec::new(),
        init: None,
        on_timer: None,
    };
    let info = match inspect(bytes) {
        Ok(i) => i,
        Err(e) => {
            report.problems.push(e.to_string());
            return report;
        }
    };
    report.info = Some(info.clone());
    let effective = match info.effective_caps(config) {
        Ok(c) => c,
        Err(e) => {
            report.problems.push(e.to_string());
            return report;
        }
    };
    report.effective = Some(effective.clone());
    let host = match PluginHost::new(Limits::default()) {
        Ok(h) => h,
        Err(e) => {
            report.problems.push(format!("building the host: {e}"));
            return report;
        }
    };
    let plugin = match host.load_with_config(bytes, &effective, config) {
        Ok(p) => p,
        Err(e) => {
            report.problems.push(e.to_string());
            return report;
        }
    };
    let init_config = if config.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(config).unwrap_or_default()
    };
    let state = StateSnapshot::new();
    match plugin.init(&init_config, &state) {
        Ok(fx) => report.init = Some(fx),
        Err(e) => report.problems.push(describe("init", &e)),
    }
    if effective.triggers.on_timer {
        match plugin.on_timer(&state) {
            Ok(fx) => report.on_timer = Some(fx),
            Err(e) => report.problems.push(describe("on_timer", &e)),
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guest(caps: &str, funcs: &str) -> Vec<u8> {
        let caps = caps.replace('"', "\\\"");
        wat::parse_str(format!(
            r#"(module
  (@custom "wayhouse.plugin-abi" "\00\00\02\00")
  (@custom "wayhouse.plugin-caps" "{caps}")
  (import "wayhouse" "log" (func $log (param i32 i32 i32)))
  (memory (export "memory") 1)
  (data (i32.const 0) "hello")
  (func (export "alloc") (param i32) (result i32) (i32.const 0))
  {funcs})"#
        ))
        .unwrap()
    }

    const TIMER: &str = r#"{"triggers":{"on_timer":true},"tick_interval_secs":30,"log":true}"#;

    #[test]
    fn a_good_module_passes_and_lists_used_capabilities() {
        let m = guest(
            TIMER,
            r#"(func (export "init") (param i32 i32))
               (func (export "on_timer") (call $log (i32.const 2) (i32.const 0) (i32.const 5)))"#,
        );
        let r = check(&m);
        assert!(r.ok(), "{:?}", r.problems);
        assert!(r.on_timer.unwrap().used_log);
    }

    #[test]
    fn a_bad_abi_section_fails_with_the_reason() {
        let m = wat::parse_str("(module)").unwrap();
        let r = check(&m);
        assert!(!r.ok());
        assert!(
            r.problems[0].contains("wayhouse.plugin-abi"),
            "{:?}",
            r.problems
        );
    }

    #[test]
    fn a_trap_in_on_timer_is_a_problem_not_a_panic() {
        let m = guest(
            TIMER,
            r#"(func (export "init") (param i32 i32))
               (func (export "on_timer") unreachable)"#,
        );
        let r = check(&m);
        assert!(!r.ok());
        assert!(
            r.problems[0].starts_with("on_timer failed"),
            "{:?}",
            r.problems
        );
    }

    #[test]
    fn using_log_without_declaring_it_is_a_capability_violation() {
        let m = guest(
            r#"{"triggers":{"on_timer":true},"tick_interval_secs":30}"#,
            r#"(func (export "init") (param i32 i32))
               (func (export "on_timer") (call $log (i32.const 2) (i32.const 0) (i32.const 5)))"#,
        );
        let r = check(&m);
        assert!(
            r.problems[0].contains("used `log` without declaring it"),
            "{:?}",
            r.problems
        );
    }

    fn bound_guest() -> Vec<u8> {
        let caps = r#"{"log":true,"routes":{"hosts":["${opt:pat}"],"backends":["10.0.0.0/8"],"max_entries":1}}"#;
        let manifest = r#"{"name":"b","options":{"type":"object","required":["pat"],"properties":{"pat":{"type":"string"}}},"bindings":[{"option":"pat","kind":"route_host_pattern"}]}"#;
        let (caps, manifest) = (caps.replace('"', "\\\""), manifest.replace('"', "\\\""));
        wat::parse_str(format!(
            r#"(module
  (@custom "wayhouse.plugin-abi" "\00\00\02\00")
  (@custom "wayhouse.plugin-caps" "{caps}")
  (@custom "wayhouse.plugin-manifest" "{manifest}")
  (memory (export "memory") 1)
  (func (export "alloc") (param i32) (result i32) (i32.const 4096))
  (func (export "init") (param i32 i32)))"#
        ))
        .unwrap()
    }

    #[test]
    fn a_bound_module_needs_its_options_and_reports_the_effective_caps() {
        let m = bound_guest();
        let r = check(&m);
        assert!(
            !r.ok() && r.problems[0].contains("options"),
            "{:?}",
            r.problems
        );
        let r = check_with_config(&m, &serde_json::json!({"pat":"*.mc.example.com"}));
        assert!(r.ok(), "{:?}", r.problems);
        assert_eq!(
            r.effective.unwrap().routes.unwrap().hosts,
            ["*.mc.example.com"]
        );
        let r = check_with_config(&m, &serde_json::json!({"pat":"*"}));
        assert!(!r.ok());
    }
}
