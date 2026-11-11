//! The registry `index.json` for `kind = plugin`, and the checks on a downloaded plugin module.
//!
//! A plugin registry is a separate document from a sniffer registry (`plugins` instead of
//! `sniffers`), so [`crate::index::parse_index`] keeps refusing it with a pointer here. The
//! install path never trusts an entry: it checks size, sha256 and, for the official registry,
//! the minisign signature, and the controller still inspects and validates the module itself
//! on upload. Nothing installed from here is approved: the operator approves its capabilities.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::index::{
    check_https_url, parse_abi, valid_name, Kind, UrlProblem, INDEX_SCHEMA, MAX_INDEX_BYTES,
    MAX_MODULE_BYTES,
};

/// Largest `options_schema` an entry may carry once serialised (64 KiB).
pub const MAX_OPTIONS_SCHEMA_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginIndex {
    pub schema: u32,
    pub kind: Kind,
    pub name: String,
    pub plugins: Vec<PluginEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginEntry {
    pub name: String,
    pub description: String,
    pub license: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    pub versions: Vec<PluginVersion>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginVersion {
    pub version: semver::Version,
    /// The plugin ABI the module is built for, `major.minor` (the host matches it exactly
    /// while the major is 0).
    pub abi: String,
    pub url: String,
    pub sha256: String,
    pub size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature_url: Option<String>,
    /// A display hint for the options form; the module's own manifest stays authoritative.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options_schema: Option<serde_json::Value>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PluginIndexError {
    #[error("index is {0} bytes, over the {MAX_INDEX_BYTES} byte limit")]
    TooLarge(usize),
    #[error("index is not valid JSON for a plugin registry: {0}")]
    Json(String),
    #[error("unsupported index schema {0} (this client understands schema {INDEX_SCHEMA})")]
    UnsupportedSchema(u32),
    #[error("this is a {0:?} registry, not a plugin registry")]
    WrongKind(Kind),
    #[error("invalid plugin name {0:?}: use only lowercase letters, digits, '-' and '_'")]
    BadName(String),
    #[error("plugin {0:?} is listed twice")]
    DuplicatePlugin(String),
    #[error("plugin {plugin:?} lists version {version} twice")]
    DuplicateVersion { plugin: String, version: String },
    #[error("plugin {plugin:?}: versions must be newest first, but {later} follows {earlier}")]
    UnsortedVersions {
        plugin: String,
        earlier: String,
        later: String,
    },
    #[error("plugin {plugin:?} {version}: {field} must be an https:// URL, got {url:?}")]
    NotHttps {
        plugin: String,
        version: String,
        field: &'static str,
        url: String,
    },
    #[error("plugin {plugin:?} {version}: {field} must have a host and no user info, got {url:?}")]
    BadUrlHost {
        plugin: String,
        version: String,
        field: &'static str,
        url: String,
    },
    #[error("plugin {plugin:?} {version}: sha256 must be 64 lowercase hex characters")]
    BadSha256 { plugin: String, version: String },
    #[error("plugin {plugin:?} {version}: size {size} must be 1..={MAX_MODULE_BYTES} bytes")]
    BadSize {
        plugin: String,
        version: String,
        size: u64,
    },
    #[error("plugin {plugin:?} {version}: abi {abi:?} is not of the form major.minor")]
    BadAbi {
        plugin: String,
        version: String,
        abi: String,
    },
    #[error(
        "plugin {plugin:?} {version}: options_schema is over {MAX_OPTIONS_SCHEMA_BYTES} bytes"
    )]
    OptionsSchemaTooLarge { plugin: String, version: String },
}

/// Just the `kind`, read before the rest of the index.
#[derive(Deserialize)]
struct Head {
    kind: Option<Kind>,
}

/// Parse and validate a plugin index. Never panics on hostile input.
pub fn parse_plugin_index(bytes: &[u8]) -> Result<PluginIndex, PluginIndexError> {
    if bytes.len() > MAX_INDEX_BYTES {
        return Err(PluginIndexError::TooLarge(bytes.len()));
    }
    // Look at `kind` first, so a sniffer index is told apart from a malformed plugin one.
    if let Ok(Head {
        kind: Some(kind @ Kind::Sniffer),
    }) = serde_json::from_slice(bytes)
    {
        return Err(PluginIndexError::WrongKind(kind));
    }
    let index: PluginIndex =
        serde_json::from_slice(bytes).map_err(|e| PluginIndexError::Json(e.to_string()))?;
    index.validate()?;
    Ok(index)
}

impl PluginIndex {
    pub fn validate(&self) -> Result<(), PluginIndexError> {
        if self.schema != INDEX_SCHEMA {
            return Err(PluginIndexError::UnsupportedSchema(self.schema));
        }
        if self.kind != Kind::Plugin {
            return Err(PluginIndexError::WrongKind(self.kind));
        }
        let mut seen = std::collections::HashSet::new();
        for plugin in &self.plugins {
            if !valid_name(&plugin.name) {
                return Err(PluginIndexError::BadName(plugin.name.clone()));
            }
            if !seen.insert(plugin.name.as_str()) {
                return Err(PluginIndexError::DuplicatePlugin(plugin.name.clone()));
            }
            plugin.validate()?;
        }
        Ok(())
    }
}

impl PluginEntry {
    fn validate(&self) -> Result<(), PluginIndexError> {
        for pair in self.versions.windows(2) {
            let (earlier, later) = (&pair[0].version, &pair[1].version);
            if earlier == later {
                return Err(PluginIndexError::DuplicateVersion {
                    plugin: self.name.clone(),
                    version: earlier.to_string(),
                });
            }
            if earlier < later {
                return Err(PluginIndexError::UnsortedVersions {
                    plugin: self.name.clone(),
                    earlier: earlier.to_string(),
                    later: later.to_string(),
                });
            }
        }
        self.versions
            .iter()
            .try_for_each(|v| v.validate(&self.name))
    }
}

impl PluginVersion {
    fn validate(&self, plugin: &str) -> Result<(), PluginIndexError> {
        let version = self.version.to_string();
        let https = |field: &'static str, url: &str| match check_https_url(url) {
            Ok(()) => Ok(()),
            Err(UrlProblem::NotHttps) => Err(PluginIndexError::NotHttps {
                plugin: plugin.to_owned(),
                version: version.clone(),
                field,
                url: url.to_owned(),
            }),
            Err(UrlProblem::BadHost) => Err(PluginIndexError::BadUrlHost {
                plugin: plugin.to_owned(),
                version: version.clone(),
                field,
                url: url.to_owned(),
            }),
        };
        https("url", &self.url)?;
        if let Some(sig) = &self.signature_url {
            https("signature_url", sig)?;
        }
        if self.sha256.len() != 64
            || !self
                .sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(PluginIndexError::BadSha256 {
                plugin: plugin.to_owned(),
                version,
            });
        }
        if self.size == 0 || self.size > MAX_MODULE_BYTES {
            return Err(PluginIndexError::BadSize {
                plugin: plugin.to_owned(),
                version,
                size: self.size,
            });
        }
        if parse_abi(&self.abi).is_none() {
            return Err(PluginIndexError::BadAbi {
                plugin: plugin.to_owned(),
                version,
                abi: self.abi.clone(),
            });
        }
        if let Some(schema) = &self.options_schema {
            if serde_json::to_vec(schema).map_or(true, |b| b.len() > MAX_OPTIONS_SCHEMA_BYTES) {
                return Err(PluginIndexError::OptionsSchemaTooLarge {
                    plugin: plugin.to_owned(),
                    version,
                });
            }
        }
        Ok(())
    }
}

/// What a downloaded plugin module passed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PluginVerified {
    /// A minisign signature was present and verified against the official key.
    pub signed: bool,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PluginVerifyError {
    #[error("downloaded {got} bytes but the index says {expected}")]
    Size { expected: u64, got: usize },
    #[error("sha256 mismatch: index says {expected}, downloaded file is {got}")]
    Sha256 { expected: String, got: String },
    #[error("signature check failed: {0}")]
    Signature(String),
    #[error("not a plugin module: {0}")]
    Module(String),
}

/// Check size, then sha256, then the signature, then the declared ABI, in that order.
///
/// As for sniffers, a signature that is present is checked whenever a key is given and a
/// failure is an error, never a quiet downgrade to unsigned; no signature (or no key) gives
/// `signed: false`, which the caller must make the operator confirm. The module's ABI section
/// is read without running anything; the controller validates the whole module on upload.
pub fn verify_plugin_artifact(
    name: &str,
    v: &PluginVersion,
    bytes: &[u8],
    signature: Option<&[u8]>,
    official_key: Option<&minisign_verify::PublicKey>,
) -> Result<PluginVerified, PluginVerifyError> {
    if bytes.len() as u64 != v.size {
        return Err(PluginVerifyError::Size {
            expected: v.size,
            got: bytes.len(),
        });
    }
    let got = Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    if got != v.sha256 {
        return Err(PluginVerifyError::Sha256 {
            expected: v.sha256.clone(),
            got,
        });
    }
    let signed = match (signature, official_key) {
        (Some(sig), Some(key)) => {
            let claim = crate::verify::signed_claim(
                "plugin",
                name,
                &v.version.to_string(),
                &v.abi,
                &v.sha256,
            );
            crate::verify::verify_claimed(key, sig, bytes, &claim)
                .map_err(PluginVerifyError::Signature)?;
            true
        }
        _ => false,
    };
    let declared = plugin_abi(bytes)?;
    if declared != v.abi {
        return Err(PluginVerifyError::Module(format!(
            "module declares ABI {declared} but the index says {}",
            v.abi
        )));
    }
    Ok(PluginVerified { signed })
}

/// The ABI (`major.minor`) in the module's `wayhouse.plugin-abi` custom section.
pub fn plugin_abi(bytes: &[u8]) -> Result<String, PluginVerifyError> {
    let bad = |e: String| PluginVerifyError::Module(e);
    wasmparser::Validator::new()
        .validate_all(bytes)
        .map_err(|e| bad(e.to_string()))?;
    let mut abi: Option<&[u8]> = None;
    for payload in wasmparser::Parser::new(0).parse_all(bytes) {
        match payload.map_err(|e| bad(e.to_string()))? {
            wasmparser::Payload::Version { encoding, .. }
                if encoding != wasmparser::Encoding::Module =>
            {
                return Err(bad("not a core WebAssembly module".into()));
            }
            wasmparser::Payload::CustomSection(c) if c.name() == "wayhouse.plugin-abi" => {
                if abi.is_some() {
                    return Err(bad("more than one wayhouse.plugin-abi section".into()));
                }
                abi = Some(c.data());
            }
            _ => {}
        }
    }
    match abi {
        Some(&[a, b, c, d]) => Ok(format!(
            "{}.{}",
            u16::from_le_bytes([a, b]),
            u16::from_le_bytes([c, d])
        )),
        Some(other) => Err(bad(format!(
            "wayhouse.plugin-abi section is {} bytes, expected 4",
            other.len()
        ))),
        None => Err(bad(
            "module declares no plugin ABI version (custom section wayhouse.plugin-abi)".into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn module(abi: &str) -> Vec<u8> {
        wat::parse_str(format!(
            r#"(module (memory (export "memory") 1) (@custom "wayhouse.plugin-abi" "{abi}"))"#
        ))
        .unwrap()
    }

    fn sha(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    fn version_for(bytes: &[u8]) -> PluginVersion {
        PluginVersion {
            version: semver::Version::new(0, 1, 0),
            abi: "0.2".into(),
            url: "https://example.com/p.wasm".into(),
            sha256: sha(bytes),
            size: bytes.len() as u64,
            signature_url: None,
            options_schema: None,
        }
    }

    fn doc(extra: &str) -> String {
        format!(
            r#"{{"schema":1,"kind":"plugin","name":"official plugins","plugins":[{{"name":"demo","description":"d","license":"MIT","versions":[{{"version":"0.1.0","abi":"0.2","url":"https://example.com/p.wasm","sha256":"{}","size":10{extra}}}]}}]}}"#,
            "a".repeat(64)
        )
    }

    #[test]
    fn a_plugin_index_parses_and_a_sniffer_index_is_told_apart() {
        let idx = parse_plugin_index(doc("").as_bytes()).unwrap();
        assert_eq!(idx.plugins[0].name, "demo");
        let sniffer = br#"{"schema":1,"kind":"sniffer","name":"s","sniffers":[]}"#;
        assert_eq!(
            parse_plugin_index(sniffer),
            Err(PluginIndexError::WrongKind(Kind::Sniffer))
        );
        // And the sniffer parser still refuses a plugin index, pointing at its kind.
        assert!(matches!(
            crate::index::parse_index(doc("").as_bytes()),
            Err(crate::index::IndexError::UnsupportedKind(Kind::Plugin))
        ));
    }

    #[test]
    fn an_index_with_a_bad_entry_is_refused() {
        let bad = |from: &str, to: &str| {
            parse_plugin_index(doc("").replace(from, to).as_bytes()).unwrap_err()
        };
        assert!(matches!(
            bad("\"demo\"", "\"Demo\""),
            PluginIndexError::BadName(_)
        ));
        assert!(matches!(
            bad("https://example.com", "http://example.com"),
            PluginIndexError::NotHttps { .. }
        ));
        assert!(matches!(
            bad("https://example.com", "https://u:p@example.com"),
            PluginIndexError::BadUrlHost { .. }
        ));
        assert!(matches!(
            bad(&"a".repeat(64), "abc"),
            PluginIndexError::BadSha256 { .. }
        ));
        assert!(matches!(
            bad("\"size\":10", "\"size\":0"),
            PluginIndexError::BadSize { .. }
        ));
        assert!(matches!(
            bad("\"abi\":\"0.2\"", "\"abi\":\"two\""),
            PluginIndexError::BadAbi { .. }
        ));
        assert!(matches!(
            bad("\"schema\":1", "\"schema\":2"),
            PluginIndexError::UnsupportedSchema(2)
        ));
    }

    #[test]
    fn duplicate_plugins_and_unsorted_versions_are_refused() {
        let one = doc("");
        let entry = one
            .split_once("\"plugins\":[")
            .unwrap()
            .1
            .strip_suffix("]}")
            .unwrap()
            .to_string();
        let twice =
            format!(r#"{{"schema":1,"kind":"plugin","name":"n","plugins":[{entry},{entry}]}}"#);
        assert!(matches!(
            parse_plugin_index(twice.as_bytes()),
            Err(PluginIndexError::DuplicatePlugin(_))
        ));
        let ver = |v: &str| {
            format!(
                r#"{{"version":"{v}","abi":"0.2","url":"https://example.com/p","sha256":"{}","size":1}}"#,
                "a".repeat(64)
            )
        };
        let sorted_wrong = format!(
            r#"{{"schema":1,"kind":"plugin","name":"n","plugins":[{{"name":"d","description":"","license":"MIT","versions":[{},{}]}}]}}"#,
            ver("0.1.0"),
            ver("0.2.0")
        );
        assert!(matches!(
            parse_plugin_index(sorted_wrong.as_bytes()),
            Err(PluginIndexError::UnsortedVersions { .. })
        ));
    }

    #[test]
    fn an_oversized_options_schema_is_refused() {
        let big = "x".repeat(MAX_OPTIONS_SCHEMA_BYTES + 1);
        let extra = format!(r#","options_schema":{{"description":"{big}"}}"#);
        assert!(matches!(
            parse_plugin_index(doc(&extra).as_bytes()),
            Err(PluginIndexError::OptionsSchemaTooLarge { .. })
        ));
    }

    #[test]
    fn a_tampered_module_is_refused_by_size_then_hash() {
        let good = module(r"\00\00\02\00");
        let v = version_for(&good);
        assert_eq!(
            verify_plugin_artifact("demo", &v, &good, None, None),
            Ok(PluginVerified { signed: false })
        );
        // Same length, different bytes: the hash catches it.
        let mut tampered = good.clone();
        let last = tampered.len() - 1;
        tampered[last] ^= 1;
        assert!(matches!(
            verify_plugin_artifact("demo", &v, &tampered, None, None),
            Err(PluginVerifyError::Sha256 { .. })
        ));
        // A different length: the size check goes first.
        let mut longer = good.clone();
        longer.push(0);
        assert!(matches!(
            verify_plugin_artifact("demo", &v, &longer, None, None),
            Err(PluginVerifyError::Size { .. })
        ));
    }

    #[test]
    fn the_declared_abi_must_match_the_index() {
        let m = module(r"\00\00\01\00");
        let v = version_for(&m); // index says 0.2, module says 0.1
        assert!(matches!(
            verify_plugin_artifact("demo", &v, &m, None, None),
            Err(PluginVerifyError::Module(_))
        ));
        let none = wat::parse_str("(module)").unwrap();
        assert!(matches!(
            plugin_abi(&none),
            Err(PluginVerifyError::Module(_))
        ));
        assert_eq!(plugin_abi(&module(r"\00\00\02\00")).unwrap(), "0.2");
    }

    #[test]
    fn a_signature_that_does_not_verify_is_an_error_not_unsigned() {
        let m = module(r"\00\00\02\00");
        let v = version_for(&m);
        // A key from the sniffer tests' fixtures is not available here; a garbage signature
        // against any key must fail loudly, not downgrade to unsigned.
        let key = minisign_verify::PublicKey::from_base64(
            "RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3",
        )
        .unwrap();
        assert!(matches!(
            verify_plugin_artifact("demo", &v, &m, Some(b"not a signature"), Some(&key)),
            Err(PluginVerifyError::Signature(_))
        ));
        // Without a key to check it with, a signature is ignored: unsigned.
        assert_eq!(
            verify_plugin_artifact("demo", &v, &m, Some(b"not a signature"), None),
            Ok(PluginVerified { signed: false })
        );
    }

    #[test]
    fn a_valid_signature_gives_signed_and_tampered_bytes_under_it_fail() {
        let m = module(r"\00\00\02\00");
        let v = version_for(&m);
        let kp = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let claim = crate::verify::signed_claim(
            "plugin",
            "demo",
            &v.version.to_string(),
            &v.abi,
            &v.sha256,
        );
        let sig = minisign::sign(
            Some(&kp.pk),
            &kp.sk,
            std::io::Cursor::new(&m),
            Some(&claim),
            None,
        )
        .unwrap()
        .to_string()
        .into_bytes();
        let key = minisign_verify::PublicKey::decode(&kp.pk.to_box().unwrap().to_string()).unwrap();
        assert_eq!(
            verify_plugin_artifact("demo", &v, &m, Some(&sig), Some(&key)),
            Ok(PluginVerified { signed: true })
        );
        // The same signature replayed under another plugin name or version is refused (#594).
        assert!(matches!(
            verify_plugin_artifact("other", &v, &m, Some(&sig), Some(&key)),
            Err(PluginVerifyError::Signature(_))
        ));
        let mut newer = v.clone();
        newer.version = "9.9.9".parse().unwrap();
        assert!(matches!(
            verify_plugin_artifact("demo", &newer, &m, Some(&sig), Some(&key)),
            Err(PluginVerifyError::Signature(_))
        ));
        // Same size, hash fixed up to match: only the signature can catch it now.
        let mut other = m.clone();
        *other.last_mut().unwrap() ^= 1;
        let mut v2 = version_for(&other);
        v2.abi = v.abi.clone();
        assert!(matches!(
            verify_plugin_artifact("demo", &v2, &other, Some(&sig), Some(&key)),
            Err(PluginVerifyError::Signature(_))
        ));
    }
}
