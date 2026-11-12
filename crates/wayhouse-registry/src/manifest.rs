//! The per-sniffer source manifest (`sniffers/<name>/manifest.toml` in the sniffers repo).

use crate::catalog::{check_labels, normalize_tags, CatalogProblem};
use crate::index::{parse_abi, valid_name, Limits};
use serde::Deserialize;

/// What a sniffer author writes. Strict (`deny_unknown_fields`): a misspelt key in
/// source the author controls is an error, unlike the forward-compatible index.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub name: String,
    pub description: String,
    pub license: String,
    pub version: semver::Version,
    pub abi: String,
    pub min_proxy: semver::Version,
    pub limits: Limits,
    #[serde(default)]
    pub config: Option<String>,
    #[serde(default)]
    pub homepage: Option<String>,
    /// 1 to 3, from the registry's `categories.toml` (the generator enforces the list).
    pub categories: Vec<String>,
    /// 0 to 8 free-form labels; trimmed and lowercased on parse.
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ManifestError {
    #[error("manifest.toml: {0}")]
    Toml(String),
    #[error("manifest.toml: abi {0:?} is not of the form major.minor")]
    BadAbi(String),
    #[error("manifest.toml: {0}")]
    Catalog(CatalogProblem),
    #[error("manifest.toml: list at least one category")]
    NoCategory,
    #[error("manifest.toml: name {0:?} must match [a-z0-9_-]+")]
    BadName(String),
}

pub fn parse_manifest(text: &str) -> Result<Manifest, ManifestError> {
    let mut m: Manifest = toml::from_str(text).map_err(|e| ManifestError::Toml(e.to_string()))?;
    m.tags = normalize_tags(&m.tags);
    if m.categories.is_empty() {
        return Err(ManifestError::NoCategory);
    }
    check_labels(&m.categories, &m.tags).map_err(ManifestError::Catalog)?;
    if !valid_name(&m.name) {
        return Err(ManifestError::BadName(m.name));
    }
    if parse_abi(&m.abi).is_none() {
        return Err(ManifestError::BadAbi(m.abi));
    }
    Ok(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) const MANIFEST: &str = r#"
name = "minecraft"
description = "Minecraft handshake virtual hosts"
license = "MIT OR Apache-2.0"
version = "0.1.0"
abi = "0.1"
min_proxy = "0.1.0"
homepage = "https://github.com/wayhouse-proxy/sniffers/tree/main/sniffers/minecraft"
categories = ["game"]
tags = ["Minecraft", "java-edition"]
config = "optional documentation"

[limits]
max_memory_bytes = 1048576
call_timeout_ms = 50
"#;

    #[test]
    fn manifest_round_trip() {
        let m = parse_manifest(MANIFEST).unwrap();
        assert_eq!(m.name, "minecraft");
        assert_eq!(m.version.to_string(), "0.1.0");
        assert_eq!(m.limits.call_timeout_ms, 50);
        assert_eq!(m.config.as_deref(), Some("optional documentation"));
    }

    #[test]
    fn manifest_normalizes_tags_and_checks_labels() {
        assert_eq!(
            parse_manifest(MANIFEST).unwrap().tags,
            ["minecraft", "java-edition"]
        );
        let bad = MANIFEST.replace("\"Minecraft\"", "\"mc!\"");
        assert!(matches!(
            parse_manifest(&bad),
            Err(ManifestError::Catalog(_))
        ));
        let none = MANIFEST.replace("categories = [\"game\"]", "categories = []");
        assert_eq!(parse_manifest(&none), Err(ManifestError::NoCategory));
        let many = MANIFEST.replace("[\"game\"]", "[\"aa\", \"bb\", \"cc\", \"dd\"]");
        assert!(matches!(
            parse_manifest(&many),
            Err(ManifestError::Catalog(_))
        ));
    }

    #[test]
    fn manifest_rejects_unknown_abi_format() {
        let text = MANIFEST.replace("abi = \"0.1\"", "abi = \"v1\"");
        assert_eq!(
            parse_manifest(&text),
            Err(ManifestError::BadAbi("v1".into()))
        );
    }

    #[test]
    fn manifest_rejects_a_name_that_could_escape_its_directory() {
        for bad in ["../other", "a/b", "", "has space", "..", ".", "Minecraft"] {
            let text = MANIFEST.replace("name = \"minecraft\"", &format!("name = {bad:?}"));
            assert_eq!(
                parse_manifest(&text),
                Err(ManifestError::BadName(bad.into())),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn manifest_rejects_a_misspelt_key() {
        let text = MANIFEST.replace("min_proxy", "min_proxie");
        assert!(matches!(parse_manifest(&text), Err(ManifestError::Toml(_))));
    }

    #[test]
    fn manifest_rejects_missing_required_field() {
        let text = MANIFEST.replace("license = \"MIT OR Apache-2.0\"\n", "");
        assert!(
            matches!(parse_manifest(&text), Err(ManifestError::Toml(m)) if m.contains("license"))
        );
    }

    #[test]
    fn manifest_rejects_a_non_semver_version() {
        let text = MANIFEST.replace("version = \"0.1.0\"", "version = \"1\"");
        assert!(matches!(parse_manifest(&text), Err(ManifestError::Toml(_))));
    }
}
