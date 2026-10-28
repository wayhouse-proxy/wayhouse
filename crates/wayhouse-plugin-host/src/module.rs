//! Reading a plugin module's ABI version and capabilities without running it.

use sha2::{Digest, Sha256};
use wayhouse_plugin_abi::{ABI_MAJOR, ABI_MINOR, ABI_SECTION, CAPS_SECTION, MANIFEST_SECTION};

use crate::bindings::{parse_declared, resolve_caps, DeclaredCaps};
use crate::caps::Capabilities;
use crate::manifest::Manifest;

/// Largest module the host accepts (as for sniffers).
pub const MAX_MODULE_BYTES: usize = 8 * 1024 * 1024;

/// The ABI version a plugin declares, or the host speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AbiVersion {
    pub major: u16,
    pub minor: u16,
}

impl std::fmt::Display for AbiVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

/// The ABI this host speaks.
pub const HOST_ABI: AbiVersion = AbiVersion {
    major: ABI_MAJOR,
    minor: ABI_MINOR,
};

/// Static facts about a module, read from its bytes alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleInfo {
    pub abi: AbiVersion,
    /// The capabilities that do not depend on install options: all of them for a module
    /// without placeholders, otherwise the declaration minus the sections that hold
    /// placeholders. What a module may do is [`ModuleInfo::effective_caps`], never this.
    pub caps: Capabilities,
    /// The caps declaration as written, placeholders included.
    pub declared: DeclaredCaps,
    /// The optional manifest: install options and capability bindings.
    pub manifest: Option<Manifest>,
    /// Lower-case hex sha256 of the module bytes; approvals are recorded against it.
    pub sha256: String,
}

/// Why a byte string is not a loadable plugin module.
#[derive(Debug, thiserror::Error)]
pub enum ModuleError {
    #[error("the module is empty")]
    Empty,
    #[error("the module is {len} bytes, over the {max} byte limit")]
    TooLarge { len: usize, max: usize },
    #[error(
        "the module has no `wayhouse.plugin-abi` section; link wayhouse-plugin-abi {HOST_ABI}"
    )]
    AbiMissing,
    #[error("the `wayhouse.plugin-abi` section is malformed ({0} bytes, or declared twice)")]
    AbiMalformed(usize),
    #[error("the plugin declares ABI {plugin}, this host speaks {host}")]
    AbiIncompatible {
        plugin: AbiVersion,
        host: AbiVersion,
    },
    #[error("the module has no `wayhouse.plugin-caps` section")]
    CapsMissing,
    #[error("the module declares `wayhouse.plugin-caps` more than once")]
    CapsDuplicate,
    #[error("invalid capability declaration: {0}")]
    CapsInvalid(String),
    #[error("the module declares `wayhouse.plugin-manifest` more than once")]
    ManifestDuplicate,
    #[error("invalid plugin manifest: {0}")]
    ManifestInvalid(String),
    #[error("invalid capability binding: {0}")]
    BindingInvalid(String),
    #[error("the install config does not fit the plugin's options: {0}")]
    OptionsInvalid(String),
    #[error("the module asks for {0}, which the operator has not approved")]
    CapsNotApproved(String),
    #[error("`on_timer` is declared without a `tick_interval_secs`")]
    UndeclaredTickInterval,
    #[error("`tick_interval_secs` is {0}, below the 10 second minimum")]
    TickIntervalTooShort(u64),
    #[error("`tick_interval_secs` is {0}, above the one week maximum")]
    TickIntervalTooLong(u64),
    #[error("the module is too complex to compile safely: {0}")]
    TooComplex(String),
    #[error("the module does not compile: {0}")]
    Compile(String),
    #[error("the module imports `{0}`, which is not part of the plugin ABI")]
    UnexpectedImport(String),
    #[error("the module does not export `{0}`")]
    MissingExport(&'static str),
    #[error("the export `{0}` has the wrong type")]
    WrongSignature(&'static str),
    #[error("the module does not instantiate: {0}")]
    Instantiate(String),
}

/// Check size, ABI version and capability declaration of `bytes`. Nothing is compiled
/// or instantiated, so no guest code runs.
pub fn inspect(bytes: &[u8]) -> Result<ModuleInfo, ModuleError> {
    if bytes.is_empty() {
        return Err(ModuleError::Empty);
    }
    if bytes.len() > MAX_MODULE_BYTES {
        return Err(ModuleError::TooLarge {
            len: bytes.len(),
            max: MAX_MODULE_BYTES,
        });
    }
    let mut abi: Option<&[u8]> = None;
    let mut caps: Option<&[u8]> = None;
    let mut manifest: Option<&[u8]> = None;
    for payload in wasmparser::Parser::new(0).parse_all(bytes) {
        let payload = payload.map_err(|e| ModuleError::Compile(e.to_string()))?;
        if let wasmparser::Payload::CustomSection(c) = payload {
            if c.name() == ABI_SECTION {
                if abi.is_some() {
                    return Err(ModuleError::AbiMalformed(c.data().len()));
                }
                abi = Some(c.data());
            } else if c.name() == CAPS_SECTION {
                if caps.is_some() {
                    return Err(ModuleError::CapsDuplicate);
                }
                caps = Some(c.data());
            } else if c.name() == MANIFEST_SECTION {
                if manifest.is_some() {
                    return Err(ModuleError::ManifestDuplicate);
                }
                manifest = Some(c.data());
            }
        }
    }
    let abi = match abi {
        None => return Err(ModuleError::AbiMissing),
        Some(&[a, b, c, d]) => AbiVersion {
            major: u16::from_le_bytes([a, b]),
            minor: u16::from_le_bytes([c, d]),
        },
        Some(other) => return Err(ModuleError::AbiMalformed(other.len())),
    };
    // Exact match while the major is 0: a minor bump may break the ABI.
    if abi != HOST_ABI {
        return Err(ModuleError::AbiIncompatible {
            plugin: abi,
            host: HOST_ABI,
        });
    }
    let caps = caps.ok_or(ModuleError::CapsMissing)?;
    let declared = parse_declared(caps)?;
    let manifest = manifest.map(Manifest::parse).transpose()?;
    declared
        .check(manifest.as_ref())
        .map_err(|e| ModuleError::BindingInvalid(e.to_string()))?;
    let caps = if declared.has_placeholders() {
        declared.static_caps()?
    } else {
        Capabilities::parse(caps)?
    };
    let sha256 = Sha256::digest(bytes)
        .iter()
        .fold(String::new(), |mut s, b| {
            use std::fmt::Write;
            let _ = write!(s, "{b:02x}");
            s
        });
    Ok(ModuleInfo {
        abi,
        caps,
        declared,
        manifest,
        sha256,
    })
}

impl ModuleInfo {
    /// What this module asks for under the install `config`: the config is checked against
    /// the manifest's options (when there is one) and its values are substituted into the
    /// capability placeholders. A module without placeholders asks for [`Self::caps`].
    pub fn effective_caps(&self, config: &serde_json::Value) -> Result<Capabilities, ModuleError> {
        let values = match &self.manifest {
            Some(m) => m
                .option_values(config)
                .map_err(ModuleError::OptionsInvalid)?,
            None => serde_json::Map::default(),
        };
        if !self.declared.has_placeholders() {
            return Ok(self.caps.clone());
        }
        resolve_caps(&self.declared, self.manifest.as_ref(), &values)
            .map_err(|e| ModuleError::BindingInvalid(e.to_string()))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A WAT module with the given abi bytes (escaped) and caps JSON sections.
    pub(crate) fn wat_with(abi: Option<&str>, caps: &[&str], body: &str) -> Vec<u8> {
        wat_with_manifest(abi, caps, &[], body)
    }

    /// As [`wat_with`], plus one `wayhouse.plugin-manifest` section per entry.
    pub(crate) fn wat_with_manifest(
        abi: Option<&str>,
        caps: &[&str],
        manifests: &[&str],
        body: &str,
    ) -> Vec<u8> {
        let mut src = String::from("(module\n");
        if let Some(a) = abi {
            src += &format!("(@custom \"wayhouse.plugin-abi\" \"{a}\")\n");
        }
        for c in caps {
            let esc = c.replace('\\', "\\\\").replace('"', "\\\"");
            src += &format!("(@custom \"wayhouse.plugin-caps\" \"{esc}\")\n");
        }
        for m in manifests {
            let esc = m
                .replace('\n', " ")
                .replace('\\', "\\\\")
                .replace('"', "\\\"");
            src += &format!("(@custom \"wayhouse.plugin-manifest\" \"{esc}\")\n");
        }
        src += body;
        src += ")";
        wat::parse_str(src).unwrap()
    }

    pub(crate) const GOOD_ABI: &str = "\\00\\00\\02\\00";
    const GOOD_CAPS: &str = r#"{"triggers":{"on_timer":true},"tick_interval_secs":30,"log":true}"#;

    #[test]
    fn accepts_a_valid_module() {
        let m = wat_with(Some(GOOD_ABI), &[GOOD_CAPS], "");
        let info = inspect(&m).unwrap();
        assert_eq!(info.abi, HOST_ABI);
        assert!(info.caps.log && info.caps.triggers.on_timer);
        assert_eq!(info.caps.tick_interval_secs, 30);
        assert_eq!(info.sha256.len(), 64);
    }

    #[test]
    fn rejects_empty_and_oversize() {
        assert!(matches!(inspect(&[]), Err(ModuleError::Empty)));
        let big = vec![0u8; MAX_MODULE_BYTES + 1];
        assert!(matches!(inspect(&big), Err(ModuleError::TooLarge { .. })));
    }

    #[test]
    fn rejects_garbage() {
        assert!(matches!(
            inspect(b"not wasm at all"),
            Err(ModuleError::Compile(_))
        ));
    }

    #[test]
    fn rejects_missing_or_wrong_abi() {
        let m = wat_with(None, &[GOOD_CAPS], "");
        assert!(matches!(inspect(&m), Err(ModuleError::AbiMissing)));
        let m = wat_with(Some("\\00\\00\\01"), &[GOOD_CAPS], "");
        assert!(matches!(inspect(&m), Err(ModuleError::AbiMalformed(3))));
    }

    #[test]
    fn a_module_with_another_abi_minor_is_refused_either_way() {
        // 0.1 on a 0.2 host, and 0.3 (a future module) on this host.
        for (abi, minor) in [("\\00\\00\\01\\00", 1), ("\\00\\00\\03\\00", 3)] {
            let m = wat_with(Some(abi), &[GOOD_CAPS], "");
            match inspect(&m) {
                Err(ModuleError::AbiIncompatible { plugin, host }) => {
                    assert_eq!((plugin.major, plugin.minor), (0, minor));
                    assert_eq!(host, HOST_ABI);
                    assert_eq!(HOST_ABI.to_string(), "0.2");
                }
                other => panic!("{other:?}"),
            }
        }
    }

    #[test]
    fn rejects_missing_duplicate_and_invalid_caps() {
        let m = wat_with(Some(GOOD_ABI), &[], "");
        assert!(matches!(inspect(&m), Err(ModuleError::CapsMissing)));
        let m = wat_with(Some(GOOD_ABI), &[GOOD_CAPS, GOOD_CAPS], "");
        assert!(matches!(inspect(&m), Err(ModuleError::CapsDuplicate)));
        let m = wat_with(Some(GOOD_ABI), &["{not json"], "");
        assert!(matches!(inspect(&m), Err(ModuleError::CapsInvalid(_))));
    }

    #[test]
    fn rejects_unknown_capability_fields() {
        let m = wat_with(Some(GOOD_ABI), &[r#"{"log":true,"filesystem":true}"#], "");
        assert!(matches!(inspect(&m), Err(ModuleError::CapsInvalid(_))));
    }

    #[test]
    fn enforces_the_tick_interval_minimum() {
        let m = wat_with(
            Some(GOOD_ABI),
            &[r#"{"triggers":{"on_timer":true},"tick_interval_secs":9}"#],
            "",
        );
        assert!(matches!(
            inspect(&m),
            Err(ModuleError::TickIntervalTooShort(9))
        ));
        let m = wat_with(Some(GOOD_ABI), &[r#"{"triggers":{"on_timer":true}}"#], "");
        assert!(matches!(
            inspect(&m),
            Err(ModuleError::UndeclaredTickInterval)
        ));
    }

    #[test]
    fn sha256_is_of_the_module_bytes() {
        let m = wat_with(Some(GOOD_ABI), &[GOOD_CAPS], "");
        let expected = Sha256::digest(&m)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        assert_eq!(inspect(&m).unwrap().sha256, expected);
    }

    const MANIFEST: &str = r#"{"name":"demo","options":{"type":"object","required":["panel"],
        "properties":{"panel":{"type":"string"}}},"bindings":[{"option":"panel","kind":"hostname"}]}"#;
    const BOUND_CAPS: &str = r#"{"http":{"hosts":[{"host":"${opt:panel}"}]}}"#;

    #[test]
    fn the_manifest_section_is_optional_and_read_without_running_the_module() {
        let m = wat_with(Some(GOOD_ABI), &[GOOD_CAPS], "");
        assert!(inspect(&m).unwrap().manifest.is_none());
        let m = wat_with_manifest(Some(GOOD_ABI), &[BOUND_CAPS], &[MANIFEST], "");
        let info = inspect(&m).unwrap();
        assert_eq!(info.manifest.unwrap().name, "demo");
        assert!(info.declared.has_placeholders());
    }

    #[test]
    fn a_manifest_declared_twice_or_malformed_is_refused() {
        let m = wat_with_manifest(Some(GOOD_ABI), &[GOOD_CAPS], &[MANIFEST, MANIFEST], "");
        assert!(matches!(inspect(&m), Err(ModuleError::ManifestDuplicate)));
        let m = wat_with_manifest(Some(GOOD_ABI), &[GOOD_CAPS], &["{not json"], "");
        assert!(matches!(inspect(&m), Err(ModuleError::ManifestInvalid(_))));
    }

    #[test]
    fn a_placeholder_the_manifest_does_not_bind_is_refused_at_inspect() {
        let m = wat_with(Some(GOOD_ABI), &[BOUND_CAPS], "");
        assert!(matches!(inspect(&m), Err(ModuleError::BindingInvalid(_))));
        let unbound = MANIFEST.replace(r#","bindings":[{"option":"panel","kind":"hostname"}]"#, "");
        let m = wat_with_manifest(Some(GOOD_ABI), &[BOUND_CAPS], &[&unbound], "");
        assert!(matches!(inspect(&m), Err(ModuleError::BindingInvalid(_))));
    }

    #[test]
    fn the_static_caps_of_a_bound_module_exclude_the_bound_sections() {
        let caps = r#"{"log":true,"http":{"hosts":[{"host":"${opt:panel}"}]}}"#;
        let m = wat_with_manifest(Some(GOOD_ABI), &[caps], &[MANIFEST], "");
        let info = inspect(&m).unwrap();
        assert!(info.caps.log && info.caps.http.is_none());
    }

    #[test]
    fn effective_caps_come_from_the_install_config() {
        let m = wat_with_manifest(Some(GOOD_ABI), &[BOUND_CAPS], &[MANIFEST], "");
        let info = inspect(&m).unwrap();
        let eff = info
            .effective_caps(&serde_json::json!({"panel":"p.example"}))
            .unwrap();
        assert_eq!(eff.http.unwrap().hosts[0].host, "p.example");
        for bad in [
            serde_json::json!({}),
            serde_json::json!(null),
            serde_json::json!({"panel":1}),
        ] {
            assert!(
                matches!(
                    info.effective_caps(&bad),
                    Err(ModuleError::OptionsInvalid(_))
                ),
                "{bad}"
            );
        }
        let wild = serde_json::json!({"panel":"*.example.com"});
        assert!(matches!(
            info.effective_caps(&wild),
            Err(ModuleError::BindingInvalid(_))
        ));
    }

    #[test]
    fn a_module_without_placeholders_ignores_the_config() {
        let m = wat_with(Some(GOOD_ABI), &[GOOD_CAPS], "");
        let info = inspect(&m).unwrap();
        let eff = info
            .effective_caps(&serde_json::json!({"anything": [1, 2]}))
            .unwrap();
        assert_eq!(eff, info.caps);
    }
}
