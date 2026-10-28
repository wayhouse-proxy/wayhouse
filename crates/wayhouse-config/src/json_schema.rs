//! The config document's JSON Schema, derived from the raw serde types (`json-schema` feature).

use serde_json::Value;

/// Shown by the drift test when the checked-in schema is stale.
#[cfg(test)]
const REGENERATE: &str = "cargo run -p wayhouse-config --features json-schema --example gen_schema > crates/wayhouse-config/schema/config.schema.json";

/// JSON Schema (2020-12) for a config document, with `x-wayhouse-since` on each field of
/// [`FIELD_SINCE`](crate::version::FIELD_SINCE).
pub fn json_schema() -> Value {
    let mut schema = serde_json::to_value(schemars::schema_for!(crate::schema::RawConfig))
        .expect("a schema is plain JSON");
    for (path, since) in crate::version::FIELD_SINCE {
        let ptr = property_pointer(&schema, path)
            .unwrap_or_else(|| panic!("FIELD_SINCE path {path} is not in the schema"));
        schema
            .pointer_mut(&ptr)
            .expect("the pointer was just found")["x-wayhouse-since"] = (*since).into();
    }
    schema
}

/// JSON pointer to the property a dotted `FIELD_SINCE` path names. Like the path itself, it passes
/// through sequences, optionals and `$ref`s transparently.
fn property_pointer(schema: &Value, path: &str) -> Option<String> {
    let mut ptr = String::new();
    for seg in path.split('.') {
        ptr = unwrap_node(schema, ptr)?;
        schema.pointer(&ptr)?["properties"].get(seg)?;
        ptr = format!("{ptr}/properties/{seg}");
    }
    Some(ptr)
}

/// Follows `$ref`, array `items` and the non-null arm of an optional to the schema that holds
/// the properties.
fn unwrap_node(schema: &Value, mut ptr: String) -> Option<String> {
    loop {
        let node = schema.pointer(&ptr)?;
        if let Some(target) = node["$ref"].as_str() {
            ptr = target.strip_prefix('#')?.to_owned();
        } else if node.get("items").is_some() {
            ptr = format!("{ptr}/items");
        } else if let Some(arms) = node["anyOf"].as_array() {
            let i = arms.iter().position(|a| a["type"] != "null")?;
            ptr = format!("{ptr}/anyOf/{i}");
        } else {
            return Some(ptr);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{json_schema, REGENERATE};
    use serde_json::Value;

    fn def<'a>(schema: &'a Value, name: &str) -> &'a Value {
        &schema["$defs"][name]
    }

    #[test]
    fn schema_marks_fields_with_since() {
        let s = json_schema();
        assert_eq!(s["properties"]["web"]["x-wayhouse-since"], 3);
        assert_eq!(
            def(&s, "RawListener")["properties"]["plugin_routes"]["x-wayhouse-since"],
            2
        );
        assert!(s["properties"]["pools"].get("x-wayhouse-since").is_none());
    }

    #[test]
    fn checked_in_schema_matches_the_derived_one() {
        let checked_in: Value = serde_json::from_str(crate::CONFIG_JSON_SCHEMA).unwrap();
        assert_eq!(
            checked_in,
            json_schema(),
            "schema/config.schema.json is stale; regenerate it with:\n  {REGENERATE}"
        );
    }

    #[test]
    fn schema_carries_serde_defaults() {
        let s = json_schema();
        assert_eq!(
            def(&s, "RawSettings")["properties"]["shutdown_grace_sec"]["default"],
            30
        );
    }

    #[test]
    fn schema_covers_representative_types() {
        let s = json_schema();
        // Raw document, a listener, a pool and a web site: strict structs.
        for name in ["RawListener", "RawPool", "RawSite"] {
            assert_eq!(
                def(&s, name)["additionalProperties"],
                Value::Bool(false),
                "{name} must forbid unknown fields"
            );
        }
        // `RawConfig` is the root, so its properties live at the top level.
        assert_eq!(s["additionalProperties"], Value::Bool(false));
        // The untagged `ports` enum keeps both shapes (integer and string).
        let port = def(&s, "RawPort");
        let arms = port["anyOf"]
            .as_array()
            .or(port["oneOf"].as_array())
            .unwrap();
        assert_eq!(arms.len(), 2, "{port}");
        // The `present` helper field is optional, and typed as a whole number.
        assert!(!s["required"]
            .as_array()
            .is_some_and(|r| r.iter().any(|v| v == "schema_version")));
        assert_eq!(s["properties"]["schema_version"]["type"][0], "integer");
    }
}
