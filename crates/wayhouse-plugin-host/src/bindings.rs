//! Capability bindings (ABI 0.2): `"${opt:NAME}"` placeholders in the caps JSON, resolved
//! from the install's option values into the effective [`Capabilities`] the operator
//! approves. Resolution only substitutes at fixed positions and then runs the ordinary
//! [`Capabilities::parse`], so a resolved set is always one a literal declaration could
//! have been. See `docs/superpowers/specs/2026-10-28-plugin-manifest-and-bindings.md`.

use serde_json::{Map, Value};

use crate::caps::Capabilities;
use crate::manifest::{valid_option_name, BindingKind, Manifest};
use crate::module::ModuleError;

/// The caps JSON as the module wrote it, placeholders included. Opaque on purpose: only
/// [`resolve_caps`] turns it into something the host acts on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredCaps(Value);

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum BindError {
    #[error(
        "{0}: a placeholder must be exactly \"${{opt:NAME}}\" (no text around it, none in keys)"
    )]
    BadPlaceholder(String),
    #[error("{path}: a placeholder is not allowed here")]
    NotBindable { path: String },
    #[error("{path}: option {option} has no binding in the manifest")]
    Unbound { path: String, option: String },
    #[error(
        "{path}: option {option} is bound as {declared:?} but this position takes {expected:?}"
    )]
    KindMismatch {
        path: String,
        option: String,
        declared: BindingKind,
        expected: BindingKind,
    },
    #[error("option {0} has no value and no default")]
    MissingValue(String),
    #[error("option {option}: {why}")]
    BadValue { option: String, why: String },
    #[error("{0}")]
    Caps(String),
}

const PLACEHOLDER_OPEN: &str = "${opt:";

/// Where a placeholder may stand, and what kind of value goes there.
const POSITIONS: &[(&str, BindingKind)] = &[
    ("http.hosts[].host", BindingKind::Hostname),
    ("secrets[].hosts[]", BindingKind::Hostname),
    ("routes.hosts[]", BindingKind::RouteHostPattern),
    ("routes.backends[]", BindingKind::Cidr),
    ("port_routes.backends[]", BindingKind::Cidr),
    ("port_routes.ports[]", BindingKind::PortRange),
    ("http.hosts[].port", BindingKind::Int),
    ("routes.max_entries", BindingKind::Int),
    ("port_routes.max_entries", BindingKind::Int),
];

fn contains_placeholder(v: &Value) -> bool {
    match v {
        Value::String(s) => s.contains(PLACEHOLDER_OPEN),
        Value::Array(a) => a.iter().any(contains_placeholder),
        Value::Object(o) => o
            .iter()
            .any(|(k, v)| k.contains(PLACEHOLDER_OPEN) || contains_placeholder(v)),
        _ => false,
    }
}

/// Parse a caps section payload without resolving anything.
pub fn parse_declared(bytes: &[u8]) -> Result<DeclaredCaps, ModuleError> {
    match serde_json::from_slice::<Value>(bytes) {
        Ok(v @ Value::Object(_)) => Ok(DeclaredCaps(v)),
        Ok(_) => Err(ModuleError::CapsInvalid("expected a JSON object".into())),
        Err(e) => Err(ModuleError::CapsInvalid(e.to_string())),
    }
}

fn valid_for(kind: BindingKind, v: &Value) -> Result<(), String> {
    let text = v.as_str();
    let ok = match kind {
        BindingKind::Hostname => text.is_some_and(crate::caps::valid_host),
        BindingKind::RouteHostPattern => text.is_some_and(crate::routes::valid_pattern),
        BindingKind::Cidr => text.is_some_and(|t| crate::routes::Net::parse(t).is_ok()),
        BindingKind::PortRange => text.is_some_and(|t| crate::port_routes::parse_range(t).is_ok()),
        BindingKind::Int => v.is_u64(),
    };
    if ok {
        Ok(())
    } else {
        Err(format!("{v} is not a valid {kind:?} value"))
    }
}

/// Walk the declaration; with `values`, substitute each placeholder in place.
fn walk(
    v: &mut Value,
    path: &str,
    manifest: Option<&Manifest>,
    values: Option<&Map<String, Value>>,
) -> Result<(), BindError> {
    match v {
        Value::Object(o) => {
            for (k, child) in o.iter_mut() {
                let here = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{path}.{k}")
                };
                if k.contains(PLACEHOLDER_OPEN) {
                    return Err(BindError::BadPlaceholder(here));
                }
                walk(child, &here, manifest, values)?;
            }
        }
        Value::Array(a) => {
            for child in a {
                walk(child, &format!("{path}[]"), manifest, values)?;
            }
        }
        Value::String(s) if s.contains(PLACEHOLDER_OPEN) => {
            let name = s
                .strip_prefix(PLACEHOLDER_OPEN)
                .and_then(|r| r.strip_suffix('}'))
                .filter(|n| valid_option_name(n))
                .ok_or_else(|| BindError::BadPlaceholder(path.to_string()))?
                .to_string();
            let expected = POSITIONS
                .iter()
                .find(|(p, _)| *p == path)
                .map(|(_, k)| *k)
                .ok_or_else(|| BindError::NotBindable {
                    path: path.to_string(),
                })?;
            let declared = manifest
                .and_then(|m| m.bindings.iter().find(|b| b.option == name))
                .map(|b| b.kind)
                .ok_or_else(|| BindError::Unbound {
                    path: path.to_string(),
                    option: name.clone(),
                })?;
            if declared != expected {
                return Err(BindError::KindMismatch {
                    path: path.to_string(),
                    option: name,
                    declared,
                    expected,
                });
            }
            if let Some(values) = values {
                let value = values
                    .get(&name)
                    .ok_or_else(|| BindError::MissingValue(name.clone()))?;
                valid_for(expected, value)
                    .map_err(|why| BindError::BadValue { option: name, why })?;
                *v = value.clone();
            }
        }
        _ => {}
    }
    Ok(())
}

impl DeclaredCaps {
    /// Whether any value is a placeholder.
    pub fn has_placeholders(&self) -> bool {
        contains_placeholder(&self.0)
    }

    /// Check placeholder syntax, positions and bindings, without values.
    pub fn check(&self, manifest: Option<&Manifest>) -> Result<(), BindError> {
        walk(&mut self.0.clone(), "", manifest, None)
    }

    /// The part of the declaration that does not depend on options: the sections holding
    /// placeholders are left out (and `secrets` with `http`, which they point into).
    pub fn static_caps(&self) -> Result<Capabilities, ModuleError> {
        let mut v = self.0.clone();
        let o = v.as_object_mut().expect("parse_declared keeps an object");
        for k in ["http", "routes", "port_routes"] {
            if o.get(k).is_some_and(contains_placeholder) {
                o.remove(k);
            }
        }
        if o.get("secrets").is_some_and(contains_placeholder) || !o.contains_key("http") {
            o.remove("secrets");
        }
        parse_value(&v)
    }
}

fn parse_value(v: &Value) -> Result<Capabilities, ModuleError> {
    Capabilities::parse(&serde_json::to_vec(v).expect("a JSON value serializes"))
}

/// Substitute the option `values` into `declared` and validate the result.
pub fn resolve_caps(
    declared: &DeclaredCaps,
    manifest: Option<&Manifest>,
    values: &Map<String, Value>,
) -> Result<Capabilities, BindError> {
    let mut v = declared.0.clone();
    walk(&mut v, "", manifest, Some(values))?;
    parse_value(&v).map_err(|e| BindError::Caps(e.to_string()))
}

#[cfg(test)]
#[allow(clippy::needless_pass_by_value)] // test helpers take `json!` temporaries
mod tests {
    use super::*;
    use serde_json::json;

    fn manifest(bindings: Value) -> Manifest {
        serde_json::from_value(json!({"name":"t","options":{"type":"object","properties":{
            "host":{"type":"string"},"pat":{"type":"string"},"net":{"type":"string"},
            "ports":{"type":"string"},"n":{"type":"integer"},"tick":{"type":"integer"}}},
            "bindings":bindings}))
        .unwrap()
    }

    fn all_bound() -> Manifest {
        manifest(json!([
            {"option":"host","kind":"hostname"},{"option":"pat","kind":"route_host_pattern"},
            {"option":"net","kind":"cidr"},{"option":"ports","kind":"port_range"},
            {"option":"n","kind":"int"},{"option":"tick","kind":"int"}]))
    }

    fn declared(v: Value) -> DeclaredCaps {
        parse_declared(v.to_string().as_bytes()).unwrap()
    }

    fn vals(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    fn full() -> DeclaredCaps {
        declared(json!({
            "http":{"hosts":[{"host":"${opt:host}","port":"${opt:n}"}]},
            "secrets":[{"name":"TOKEN","hosts":["${opt:host}"]}],
            "routes":{"hosts":["${opt:pat}"],"backends":["${opt:net}"],"max_entries":"${opt:n}"},
            "port_routes":{"ports":["${opt:ports}"],"proto":["udp"],"backends":["${opt:net}"],"max_entries":"${opt:n}"}}))
    }

    fn good_values() -> Map<String, Value> {
        vals(
            json!({"host":"panel.example","pat":"*.mc.example.com","net":"10.0.0.0/8",
            "ports":"30000-30999","n":8443,"tick":30}),
        )
    }

    #[test]
    fn a_resolved_set_equals_the_same_literal_declaration() {
        let d = declared(json!({
            "http":{"hosts":[{"host":"${opt:host}","port":"${opt:n}"}]},
            "secrets":[{"name":"TOKEN","hosts":["${opt:host}"]}],
            "routes":{"hosts":["${opt:pat}"],"backends":["${opt:net}"],"max_entries":3},
            "port_routes":{"ports":["${opt:ports}"],"proto":["udp"],"backends":["${opt:net}"],"max_entries":3}}));
        let got = resolve_caps(&d, Some(&all_bound()), &good_values()).unwrap();
        let literal = Capabilities::parse(
            json!({
                "http":{"hosts":[{"host":"panel.example","port":8443}]},
                "secrets":[{"name":"TOKEN","hosts":["panel.example"]}],
                "routes":{"hosts":["*.mc.example.com"],"backends":["10.0.0.0/8"],"max_entries":3},
                "port_routes":{"ports":["30000-30999"],"proto":["udp"],"backends":["10.0.0.0/8"],"max_entries":3}})
            .to_string()
            .as_bytes(),
        )
        .unwrap();
        assert_eq!(got, literal);
    }

    #[test]
    fn every_kind_resolves_to_what_the_literal_parser_accepts() {
        let d = declared(json!({
            "http":{"hosts":[{"host":"${opt:host}"}]},
            "routes":{"hosts":["${opt:pat}"],"backends":["${opt:net}"],"max_entries":"${opt:n}"},
            "port_routes":{"ports":["${opt:ports}"],"proto":["tcp"],"backends":["${opt:net}"],"max_entries":"${opt:n}"}}));
        let mut v = good_values();
        v.insert("n".into(), json!(4));
        let got = resolve_caps(&d, Some(&all_bound()), &v).unwrap();
        assert_eq!(got.http.as_ref().unwrap().hosts[0].host, "panel.example");
        assert_eq!(got.http.as_ref().unwrap().hosts[0].port, 443);
        assert_eq!(got.routes.as_ref().unwrap().hosts, ["*.mc.example.com"]);
        assert_eq!(got.routes.as_ref().unwrap().max_entries, 4);
        assert_eq!(got.port_routes.as_ref().unwrap().ports, ["30000-30999"]);
        // The effective set survives a round trip through the ordinary parser.
        let again = Capabilities::parse(&serde_json::to_vec(&got).unwrap()).unwrap();
        assert_eq!(again, got);
    }

    #[test]
    fn a_wildcard_in_a_hostname_binding_is_refused() {
        let d = declared(json!({"http":{"hosts":[{"host":"${opt:host}"}]}}));
        for bad in ["*.example.com", "*", "A.example", "a b", "", "https://x"] {
            let r = resolve_caps(&d, Some(&all_bound()), &vals(json!({"host":bad})));
            assert!(
                matches!(r, Err(BindError::BadValue { .. })),
                "{bad:?}: {r:?}"
            );
        }
    }

    #[test]
    fn a_hostname_option_cannot_make_a_host_private_capable() {
        let d = declared(json!({"http":{"hosts":[{"host":"${opt:host}"}]}}));
        let got = resolve_caps(&d, Some(&all_bound()), &vals(json!({"host":"10.0.0.5"}))).unwrap();
        assert!(!got.http.unwrap().hosts[0].allow_private);
    }

    #[test]
    fn a_badly_shaped_value_is_refused_for_every_kind() {
        let d = full();
        for (k, bad) in [
            ("pat", json!("*.")),
            ("pat", json!("a.*.b")),
            ("net", json!("10.0.0.0/33")),
            ("net", json!("not-an-ip")),
            ("ports", json!("0")),
            ("ports", json!("9-3")),
            ("n", json!("8")),
            ("n", json!(-1)),
            ("n", json!(1.5)),
            ("host", json!(7)),
        ] {
            let mut v = good_values();
            v.insert(k.into(), bad.clone());
            let r = resolve_caps(&d, Some(&all_bound()), &v);
            assert!(r.is_err(), "{k}={bad}");
        }
    }

    #[test]
    fn an_unbound_placeholder_is_refused() {
        let d = declared(json!({"http":{"hosts":[{"host":"${opt:host}"}]}}));
        let none = manifest(json!([]));
        assert!(matches!(
            d.check(Some(&none)),
            Err(BindError::Unbound { .. })
        ));
        assert!(matches!(d.check(None), Err(BindError::Unbound { .. })));
        assert!(matches!(
            resolve_caps(&d, None, &good_values()),
            Err(BindError::Unbound { .. })
        ));
    }

    #[test]
    fn a_missing_value_is_refused_not_guessed() {
        let d = declared(json!({"http":{"hosts":[{"host":"${opt:host}"}]}}));
        let r = resolve_caps(&d, Some(&all_bound()), &Map::new());
        assert_eq!(r, Err(BindError::MissingValue("host".into())));
    }

    #[test]
    fn a_placeholder_in_a_non_bindable_field_is_refused() {
        for caps in [
            json!({"triggers":{"on_timer":true},"tick_interval_secs":"${opt:tick}"}),
            json!({"http":{"hosts":[{"host":"a.example","allow_private":"${opt:n}"}]}}),
            json!({"secrets":[{"name":"${opt:host}","hosts":["a.example"]}],"http":{"hosts":[{"host":"a.example"}]}}),
            json!({"state":{"max_bytes":"${opt:n}"}}),
            json!({"log":"${opt:n}"}),
            json!({"port_routes":{"ports":["1"],"proto":["${opt:host}"],"backends":["10.0.0.1"],"max_entries":1}}),
            json!({"triggers":{"on_event":["${opt:host}"]}}),
        ] {
            let d = declared(caps.clone());
            assert!(
                matches!(
                    d.check(Some(&all_bound())),
                    Err(BindError::NotBindable { .. })
                ),
                "{caps}"
            );
        }
    }

    #[test]
    fn a_placeholder_in_a_key_is_refused() {
        let d = declared(json!({"${opt:host}":true}));
        assert!(matches!(
            d.check(Some(&all_bound())),
            Err(BindError::BadPlaceholder(_))
        ));
    }

    #[test]
    fn a_placeholder_embedded_in_a_longer_string_is_not_substituted() {
        for s in [
            "x${opt:host}",
            "${opt:host}.example.com",
            "${opt:host}${opt:host}",
            "${opt:}",
            "${opt:A}",
            "${opt:host",
        ] {
            let d = declared(json!({"http":{"hosts":[{"host":s}]}}));
            let r = d.check(Some(&all_bound()));
            assert!(
                matches!(r, Err(BindError::BadPlaceholder(_))) || s == "${opt:host",
                "{s}: {r:?}"
            );
        }
        // An unterminated `${opt:host` is just a bad literal host, caught by the ordinary parser.
        let d = declared(json!({"http":{"hosts":[{"host":"${opt:host"}]}}));
        assert!(resolve_caps(&d, Some(&all_bound()), &good_values()).is_err());
    }

    #[test]
    fn a_text_field_may_mention_the_syntax_without_being_a_placeholder() {
        let d = declared(json!({"http":{"hosts":[{"host":"a.example"}]},
            "secrets":[{"name":"T","description":"use ${secret:T}","hosts":["a.example"]}]}));
        assert!(!d.has_placeholders());
        d.check(None).unwrap();
    }

    #[test]
    fn a_kind_that_does_not_match_the_position_is_refused() {
        let d = declared(
            json!({"routes":{"hosts":["${opt:net}"],"backends":["10.0.0.0/8"],"max_entries":1}}),
        );
        assert!(matches!(
            d.check(Some(&all_bound())),
            Err(BindError::KindMismatch { .. })
        ));
    }

    #[test]
    fn static_caps_leave_out_what_depends_on_options() {
        let d = declared(
            json!({"triggers":{"on_timer":true},"tick_interval_secs":30,"log":true,
            "http":{"hosts":[{"host":"${opt:host}"}]},
            "secrets":[{"name":"T","hosts":["${opt:host}"]}],
            "routes":{"hosts":["a.example"],"backends":["10.0.0.0/8"],"max_entries":1}}),
        );
        let c = d.static_caps().unwrap();
        assert!(c.log && c.triggers.on_timer);
        assert!(c.http.is_none() && c.secrets.is_empty());
        assert!(c.routes.is_some());
    }

    #[test]
    fn a_declaration_without_placeholders_resolves_to_itself() {
        let d = declared(json!({"log":true}));
        assert!(!d.has_placeholders());
        let got = resolve_caps(&d, None, &Map::new()).unwrap();
        assert!(got.log);
    }

    #[test]
    fn growing_a_bound_value_leaves_the_old_approval() {
        let d = declared(
            json!({"routes":{"hosts":["${opt:pat}"],"backends":["10.0.0.0/8"],"max_entries":1}}),
        );
        let m = all_bound();
        let approved = resolve_caps(&d, Some(&m), &vals(json!({"pat":"a.example.com"}))).unwrap();
        let wider = resolve_caps(&d, Some(&m), &vals(json!({"pat":"*.example.com"}))).unwrap();
        let narrower = resolve_caps(&d, Some(&m), &vals(json!({"pat":"a.example.com"}))).unwrap();
        assert!(wider.check_within(&approved).is_err());
        assert!(narrower.check_within(&approved).is_ok());
        let tight = resolve_caps(&d, Some(&m), &vals(json!({"pat":"x.example.com"}))).unwrap();
        assert!(
            tight.check_within(&approved).is_err(),
            "a different host is not within"
        );
    }
}
