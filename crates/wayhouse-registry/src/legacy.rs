//! Reading a schema 1 sniffer index so the generator can carry its versions forward.
//!
//! Schema 1 had no catalog fields. Only `wayhouse-registry-gen` uses this, to upgrade the
//! previous index it merges into; a client never reads schema 1 (see [`crate::parse_index`]).

use crate::index::{
    Index, IndexError, Kind, SnifferEntry, VersionEntry, INDEX_SCHEMA, MAX_INDEX_BYTES,
};
use serde::Deserialize;

#[derive(Deserialize)]
struct LegacyIndex {
    schema: u32,
    kind: Kind,
    name: String,
    sniffers: Vec<LegacySniffer>,
}

#[derive(Deserialize)]
struct LegacySniffer {
    name: String,
    description: String,
    license: String,
    #[serde(default)]
    homepage: Option<String>,
    versions: Vec<VersionEntry>,
}

/// Upgrade a schema 1 index to schema 2: the catalog fields come from each sniffer's newest
/// version, `updated` is `updated` for every entry (the date is unknown), and categories and
/// tags start empty until the sniffer is next published with a manifest that has them.
pub fn upgrade_v1(bytes: &[u8], updated: &str) -> Result<Index, IndexError> {
    if bytes.len() > MAX_INDEX_BYTES {
        return Err(IndexError::TooLarge(bytes.len()));
    }
    let old: LegacyIndex =
        serde_json::from_slice(bytes).map_err(|e| IndexError::Json(e.to_string()))?;
    if old.schema != 1 {
        return Err(IndexError::UnsupportedSchema(old.schema));
    }
    let mut sniffers = Vec::with_capacity(old.sniffers.len());
    for s in old.sniffers {
        let Some(newest) = s.versions.first() else {
            return Err(IndexError::Json(format!(
                "sniffer {:?} lists no versions",
                s.name
            )));
        };
        sniffers.push(SnifferEntry {
            latest: newest.version.clone(),
            abi: newest.abi.clone(),
            signed: newest.signature_url.is_some(),
            updated: updated.to_owned(),
            name: s.name,
            description: s.description,
            license: s.license,
            homepage: s.homepage,
            categories: Vec::new(),
            tags: Vec::new(),
            detail_url: None,
            versions: s.versions,
        });
    }
    let index = Index {
        schema: INDEX_SCHEMA,
        kind: old.kind,
        name: old.name,
        stats_url: None,
        sniffers,
    };
    index.validate()?;
    Ok(index)
}

#[cfg(test)]
mod tests {
    use super::*;

    const V1: &str = r#"{"schema":1,"kind":"sniffer","name":"old","sniffers":[
      {"name":"a2s","description":"d","license":"MIT","homepage":"https://example.com/a2s","versions":[
        {"version":"0.1.1","abi":"0.1","min_proxy":"0.1.0","url":"https://example.com/a.wasm",
         "sha256":"0000000000000000000000000000000000000000000000000000000000000000","size":1,
         "signature_url":"https://example.com/a.wasm.minisig",
         "limits":{"max_memory_bytes":1,"call_timeout_ms":1}},
        {"version":"0.1.0","abi":"0.1","min_proxy":"0.1.0","url":"https://example.com/b.wasm",
         "sha256":"1111111111111111111111111111111111111111111111111111111111111111","size":1,
         "limits":{"max_memory_bytes":1,"call_timeout_ms":1}}]}]}"#;

    #[test]
    fn a_schema_1_index_gains_the_catalog_fields_from_its_newest_version() {
        let index = upgrade_v1(V1.as_bytes(), "2026-11-12").unwrap();
        let e = &index.sniffers[0];
        assert_eq!(index.schema, 2);
        assert_eq!(
            (
                e.latest.to_string(),
                e.abi.as_str(),
                e.signed,
                e.updated.as_str()
            ),
            ("0.1.1".to_string(), "0.1", true, "2026-11-12")
        );
        assert_eq!(e.versions.len(), 2, "no version is lost");
        assert!(e.categories.is_empty() && e.detail_url.is_none());
    }

    #[test]
    fn only_schema_1_and_only_with_versions() {
        let v2 = V1.replace("\"schema\":1", "\"schema\":2");
        assert_eq!(
            upgrade_v1(v2.as_bytes(), "2026-11-12"),
            Err(IndexError::UnsupportedSchema(2))
        );
        let none = r#"{"schema":1,"kind":"sniffer","name":"o","sniffers":[{"name":"x","description":"","license":"MIT","versions":[]}]}"#;
        assert!(matches!(
            upgrade_v1(none.as_bytes(), "2026-11-12"),
            Err(IndexError::Json(_))
        ));
    }
}
