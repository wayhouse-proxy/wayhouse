//! The optional `wayhouse.plugin-manifest` section (ABI 0.2): a display name, the options
//! schema the install `config` is checked against, and the capability bindings that say
//! which option feeds which `"${opt:NAME}"` placeholder of the caps JSON. See
//! `docs/superpowers/specs/2026-10-28-plugin-manifest-and-bindings.md`.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::module::ModuleError;

/// Largest manifest section the host reads.
pub const MAX_MANIFEST_BYTES: usize = 16 * 1024;
/// Deepest nesting of the options schema.
pub const MAX_OPTIONS_DEPTH: usize = 6;
/// Most properties one schema object, and most entries one `enum`, may hold.
pub const MAX_SCHEMA_ENTRIES: usize = 64;

/// What a bound option's value must be, and so which caps positions it may feed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BindingKind {
    Hostname,
    RouteHostPattern,
    Cidr,
    PortRange,
    Int,
}

/// One option feeding capability placeholders.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BindingDecl {
    pub option: String,
    pub kind: BindingKind,
}

/// The restricted JSON Schema (an object schema) the install `config` must satisfy.
/// Only built through [`Manifest::parse`], which has checked the subset and the limits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OptionsSchema(Value);

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub name: String,
    pub options: OptionsSchema,
    #[serde(default)]
    pub bindings: Vec<BindingDecl>,
}

impl<'de> Deserialize<'de> for OptionsSchema {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Value::deserialize(d)?;
        check_schema(&v, 0, true).map_err(serde::de::Error::custom)?;
        Ok(Self(v))
    }
}

const SCHEMA_KEYS: &[&str] = &[
    "type",
    "properties",
    "required",
    "enum",
    "default",
    "minimum",
    "maximum",
    "items",
    "additionalProperties",
    // Display hints for the form renderer; the host ignores them.
    "title",
    "description",
    "group",
    "order",
    "x-wayhouse-secret",
];

/// Whether `name` can be an option name used in a `${opt:NAME}` placeholder.
pub fn valid_option_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && name.len() <= 32
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn check_schema(schema: &Value, depth: usize, root: bool) -> Result<(), String> {
    if depth > MAX_OPTIONS_DEPTH {
        return Err(format!(
            "options nest deeper than {MAX_OPTIONS_DEPTH} levels"
        ));
    }
    let o = schema.as_object().ok_or("a schema must be an object")?;
    if let Some(k) = o.keys().find(|k| !SCHEMA_KEYS.contains(&k.as_str())) {
        return Err(format!("unsupported schema keyword {k:?}"));
    }
    let ty = o.get("type").and_then(Value::as_str).unwrap_or("");
    if !["string", "integer", "number", "boolean", "array", "object"].contains(&ty) {
        return Err(
            "every schema needs a `type` (string, integer, number, boolean, array or object)"
                .into(),
        );
    }
    if root && ty != "object" {
        return Err("the options schema must have type object".into());
    }
    let props = match o.get("properties") {
        None => None,
        Some(Value::Object(p)) if ty == "object" && p.len() <= MAX_SCHEMA_ENTRIES => Some(p),
        Some(_) => {
            return Err(format!(
                "`properties` is for objects and holds at most {MAX_SCHEMA_ENTRIES} entries"
            ))
        }
    };
    for sub in props.into_iter().flat_map(|p| p.values()) {
        check_schema(sub, depth + 1, false)?;
    }
    match o.get("required") {
        None => {}
        Some(Value::Array(r)) if ty == "object" => {
            for name in r {
                let name = name.as_str().ok_or("`required` lists property names")?;
                if !props.is_some_and(|p| p.contains_key(name)) {
                    return Err(format!(
                        "`required` names {name:?}, which is not a property"
                    ));
                }
            }
        }
        Some(_) => return Err("`required` is a list of property names on an object".into()),
    }
    if let Some(items) = o.get("items") {
        if ty != "array" {
            return Err("`items` is for arrays".into());
        }
        check_schema(items, depth + 1, false)?;
    }
    if let Some(e) = o.get("enum") {
        if !e
            .as_array()
            .is_some_and(|e| !e.is_empty() && e.len() <= MAX_SCHEMA_ENTRIES)
        {
            return Err(format!("`enum` lists 1 to {MAX_SCHEMA_ENTRIES} values"));
        }
    }
    for k in ["minimum", "maximum"] {
        if let Some(n) = o.get(k) {
            if !(n.is_number() && matches!(ty, "integer" | "number")) {
                return Err(format!("`{k}` is a number on an integer or number"));
            }
        }
    }
    if o.get("additionalProperties")
        .is_some_and(|a| !a.is_boolean() || ty != "object")
    {
        return Err("`additionalProperties` is a boolean on an object".into());
    }
    for k in ["title", "description", "group", "x-wayhouse-secret"] {
        if o.get(k).is_some_and(|v| !v.is_string()) {
            return Err(format!("`{k}` is a string"));
        }
    }
    if o.get("order").is_some_and(|v| !v.is_number()) {
        return Err("`order` is a number".into());
    }
    if let Some(d) = o.get("default") {
        check_value(schema, d, "")
            .map_err(|e| format!("`default` does not fit its schema: {e}"))?;
    }
    Ok(())
}

/// Check `value` against an already-checked schema; `Err` names the JSON path.
fn check_value(schema: &Value, value: &Value, path: &str) -> Result<(), String> {
    let at = if path.is_empty() { "/" } else { path };
    let ty = schema["type"].as_str().unwrap_or("");
    let fits = match ty {
        "string" => value.is_string(),
        "integer" => value.is_i64() || value.is_u64(),
        "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        "array" => value.is_array(),
        _ => value.is_object(),
    };
    if !fits {
        return Err(format!("{at}: expected {ty}"));
    }
    if let Some(e) = schema.get("enum").and_then(Value::as_array) {
        if !e.contains(value) {
            return Err(format!("{at}: not one of the allowed values"));
        }
    }
    if let Some(n) = value.as_f64() {
        if schema
            .get("minimum")
            .and_then(Value::as_f64)
            .is_some_and(|m| n < m)
            || schema
                .get("maximum")
                .and_then(Value::as_f64)
                .is_some_and(|m| n > m)
        {
            return Err(format!("{at}: out of range"));
        }
    }
    if let (Some(items), Some(arr)) = (schema.get("items"), value.as_array()) {
        for (i, v) in arr.iter().enumerate() {
            check_value(items, v, &format!("{path}/{i}"))?;
        }
    }
    if let Some(obj) = value.as_object() {
        let props = schema.get("properties").and_then(Value::as_object);
        for name in schema
            .get("required")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let name = name.as_str().unwrap_or("");
            if !obj.contains_key(name) {
                return Err(format!("{path}/{name}: required"));
            }
        }
        for (k, v) in obj {
            match props.and_then(|p| p.get(k)) {
                Some(sub) => check_value(sub, v, &format!("{path}/{k}"))?,
                None if schema.get("additionalProperties") == Some(&Value::Bool(false)) => {
                    return Err(format!("{path}/{k}: unknown option"));
                }
                None => {}
            }
        }
    }
    Ok(())
}

impl Manifest {
    /// Parse and check a manifest section payload.
    pub fn parse(bytes: &[u8]) -> Result<Self, ModuleError> {
        let bad = |why: String| ModuleError::ManifestInvalid(why);
        if bytes.len() > MAX_MANIFEST_BYTES {
            return Err(bad(format!(
                "{} bytes, over the {MAX_MANIFEST_BYTES} byte limit",
                bytes.len()
            )));
        }
        let m: Self = serde_json::from_slice(bytes).map_err(|e| bad(e.to_string()))?;
        if m.name.is_empty() || m.name.chars().count() > 64 {
            return Err(bad("name is 1 to 64 characters".into()));
        }
        for (i, b) in m.bindings.iter().enumerate() {
            if m.bindings[..i].iter().any(|o| o.option == b.option) {
                return Err(bad(format!("option {} is bound twice", b.option)));
            }
            let ty = m.options.0["properties"][&b.option]["type"].as_str();
            let want = if b.kind == BindingKind::Int {
                "integer"
            } else {
                "string"
            };
            if !valid_option_name(&b.option) || ty != Some(want) {
                return Err(bad(format!(
                    "binding {}: needs an option named a-z, 0-9, _ of type {want} for kind {:?}",
                    b.option, b.kind
                )));
            }
        }
        Ok(m)
    }

    /// Check `config` (null counts as `{}`) against the options and return the option values:
    /// its top-level entries plus the schema defaults of absent ones. `Err` names the JSON path.
    pub fn option_values(&self, config: &Value) -> Result<Map<String, Value>, String> {
        let config = if config.is_null() {
            &Value::Object(Map::new())
        } else {
            config
        };
        check_value(&self.options.0, config, "")?;
        let mut values = config.as_object().cloned().unwrap_or_default();
        for (name, sub) in self.options.0["properties"]
            .as_object()
            .into_iter()
            .flatten()
        {
            if let (false, Some(d)) = (values.contains_key(name), sub.get("default")) {
                values.insert(name.clone(), d.clone());
            }
        }
        Ok(values)
    }
}

#[cfg(test)]
#[allow(clippy::needless_pass_by_value)] // test helpers take `json!` temporaries
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(v: Value) -> Result<Manifest, ModuleError> {
        Manifest::parse(v.to_string().as_bytes())
    }

    fn basic() -> Value {
        json!({"name":"demo","options":{"type":"object",
            "required":["panel"],
            "properties":{
                "panel":{"type":"string","title":"Panel host","x-wayhouse-secret":"TOKEN"},
                "nets":{"type":"array","items":{"type":"string"},"default":["10.0.0.0/8"]},
                "max":{"type":"integer","minimum":1,"maximum":10,"default":5},
                "mode":{"type":"string","enum":["a","b"]}}},
            "bindings":[{"option":"panel","kind":"hostname"},{"option":"max","kind":"int"}]})
    }

    #[test]
    fn a_valid_manifest_parses() {
        let m = parse(basic()).unwrap();
        assert_eq!(m.name, "demo");
        assert_eq!(m.bindings.len(), 2);
    }

    #[test]
    fn unknown_manifest_fields_are_refused() {
        let mut v = basic();
        v["extra"] = json!(1);
        assert!(matches!(parse(v), Err(ModuleError::ManifestInvalid(_))));
        let mut v = basic();
        v["bindings"][0]["extra"] = json!(1);
        assert!(matches!(parse(v), Err(ModuleError::ManifestInvalid(_))));
    }

    #[test]
    fn unsupported_schema_keywords_are_refused() {
        for kw in [
            json!({"pattern":"a"}),
            json!({"$ref":"#"}),
            json!({"oneOf":[]}),
        ] {
            let mut v = basic();
            v["options"]["properties"]["panel"] = {
                let mut s = json!({"type":"string"});
                s.as_object_mut()
                    .unwrap()
                    .extend(kw.as_object().unwrap().clone());
                s
            };
            assert!(
                matches!(parse(v), Err(ModuleError::ManifestInvalid(_))),
                "{kw}"
            );
        }
    }

    #[test]
    fn a_manifest_over_16_kib_is_refused() {
        let mut v = basic();
        v["name"] = json!("x".repeat(MAX_MANIFEST_BYTES));
        assert!(matches!(parse(v), Err(ModuleError::ManifestInvalid(_))));
    }

    #[test]
    fn options_nesting_over_the_depth_limit_is_refused() {
        let mut s = json!({"type":"string"});
        for _ in 0..=MAX_OPTIONS_DEPTH {
            s = json!({"type":"array","items":s});
        }
        let mut v = basic();
        v["options"]["properties"]["deep"] = s;
        assert!(matches!(parse(v), Err(ModuleError::ManifestInvalid(_))));
    }

    #[test]
    fn the_root_must_be_an_object_schema() {
        let mut v = basic();
        v["options"] = json!({"type":"string"});
        assert!(matches!(parse(v), Err(ModuleError::ManifestInvalid(_))));
    }

    #[test]
    fn a_default_that_breaks_its_own_schema_is_refused() {
        let mut v = basic();
        v["options"]["properties"]["max"]["default"] = json!(99);
        assert!(matches!(parse(v), Err(ModuleError::ManifestInvalid(_))));
    }

    #[test]
    fn bindings_must_name_a_declared_option_of_a_fitting_type() {
        let mut v = basic();
        v["bindings"] = json!([{"option":"nope","kind":"hostname"}]);
        assert!(matches!(parse(v), Err(ModuleError::ManifestInvalid(_))));
        let mut v = basic();
        v["bindings"] = json!([{"option":"max","kind":"cidr"}]); // integer option, string kind
        assert!(matches!(parse(v), Err(ModuleError::ManifestInvalid(_))));
        let mut v = basic();
        v["bindings"] =
            json!([{"option":"panel","kind":"hostname"},{"option":"panel","kind":"cidr"}]);
        assert!(matches!(parse(v), Err(ModuleError::ManifestInvalid(_))));
    }

    #[test]
    fn config_is_checked_and_defaults_fill_absent_options() {
        let m = parse(basic()).unwrap();
        let vals = m.option_values(&json!({"panel":"p.example"})).unwrap();
        assert_eq!(vals["panel"], json!("p.example"));
        assert_eq!(vals["max"], json!(5));
        assert_eq!(vals["nets"], json!(["10.0.0.0/8"]));
        assert!(!vals.contains_key("mode"));
    }

    #[test]
    fn config_violations_name_the_path() {
        let m = parse(basic()).unwrap();
        let why = |c: Value| m.option_values(&c).unwrap_err();
        assert!(why(json!({})).contains("panel"), "required");
        assert!(why(json!({"panel":1})).contains("/panel"));
        assert!(why(json!({"panel":"x","max":11})).contains("/max"));
        assert!(why(json!({"panel":"x","max":1.5})).contains("/max"));
        assert!(why(json!({"panel":"x","mode":"c"})).contains("/mode"));
        assert!(why(json!({"panel":"x","nets":["a",3]})).contains("/nets/1"));
        assert!(why(json!([1])).contains("object"));
    }
}
