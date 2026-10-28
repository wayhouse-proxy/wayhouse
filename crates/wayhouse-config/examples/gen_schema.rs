//! Prints the config JSON Schema; `schema/config.schema.json` is this output.

fn main() {
    println!(
        "{}",
        serde_json::to_string_pretty(&wayhouse_config::json_schema()).unwrap()
    );
}
