//! The registry `index.json` format and every rule on it.

use crate::catalog::{Catalog, CatalogProblem, Newest};
use serde::{Deserialize, Serialize};

/// The only index schema this crate understands.
pub const INDEX_SCHEMA: u32 = 2;
/// Largest index file accepted (1 MiB).
pub const MAX_INDEX_BYTES: usize = 1024 * 1024;
/// Largest per-sniffer detail file accepted (1 MiB).
pub const MAX_DETAIL_BYTES: usize = 1024 * 1024;
/// Largest module a registry may list. Equal to `MAX_MODULE_BYTES` in the proxy's
/// `sniffer_loader.rs` (the source of truth); keep the two in step.
pub const MAX_MODULE_BYTES: u64 = 8 * 1024 * 1024;
/// Largest `limits.max_memory_bytes` an index may declare (the proxy's own cap is lower or equal).
pub const MAX_LIMIT_MEMORY_BYTES: u64 = 256 * 1024 * 1024;
/// Largest `limits.call_timeout_ms` an index may declare.
pub const MAX_LIMIT_CALL_TIMEOUT_MS: u64 = 10_000;

/// What a registry serves. Plugins are reserved for the plugin wave.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Sniffer,
    Plugin,
}

/// `name` is a display name (the official index is "wayhouse official
/// sniffers"), not an identifier, so the lowercase rule that applies to
/// [`SnifferEntry`] names is not applied to it (#336).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Index {
    pub schema: u32,
    pub kind: Kind,
    pub name: String,
    /// Where the registry publishes download counts (`stats.json`); absent means no
    /// "popular" sort. Fetched by the UI, never trusted for anything but ordering.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stats_url: Option<String>,
    pub sniffers: Vec<SnifferEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnifferEntry {
    pub name: String,
    pub description: String,
    pub license: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Summary of the newest version, so the catalog can be filtered without the details.
    pub latest: semver::Version,
    pub abi: String,
    /// Whether the newest version lists a signature.
    pub signed: bool,
    /// `YYYY-MM-DD` of the newest publish.
    pub updated: String,
    /// Where the full version list lives; exclusive with inline `versions`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail_url: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub versions: Vec<VersionEntry>,
}

/// A per-sniffer detail file (`detail_url`): the full version list of one sniffer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnifferDetail {
    pub schema: u32,
    pub kind: Kind,
    pub name: String,
    pub versions: Vec<VersionEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionEntry {
    pub version: semver::Version,
    pub abi: String,
    pub min_proxy: semver::Version,
    pub url: String,
    pub sha256: String,
    pub size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature_url: Option<String>,
    pub limits: Limits,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limits {
    pub max_memory_bytes: u64,
    pub call_timeout_ms: u64,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
/// Index `name` is a display name and is not validated like a sniffer name
/// ([`IndexError::BadName`] covers sniffer entries only).
pub enum IndexError {
    #[error("index is {0} bytes, over the {MAX_INDEX_BYTES} byte limit")]
    TooLarge(usize),
    #[error("index is not valid JSON for this format: {0}")]
    Json(String),
    #[error("unsupported index schema {0} (this client understands schema {INDEX_SCHEMA}); regenerate the index with a current wayhouse-registry-gen")]
    UnsupportedSchema(u32),
    #[error("registry kind {0:?} is not supported yet (only sniffer registries)")]
    UnsupportedKind(Kind),
    #[error("invalid sniffer name {0:?}: use only lowercase letters, digits, '-' and '_'")]
    BadName(String),
    #[error("sniffer {0:?} is listed twice")]
    DuplicateSniffer(String),
    #[error("sniffer {sniffer:?} lists version {version} twice")]
    DuplicateVersion { sniffer: String, version: String },
    #[error("sniffer {sniffer:?}: versions must be sorted newest first ({later} is listed after {earlier})")]
    UnsortedVersions {
        sniffer: String,
        earlier: String,
        later: String,
    },
    #[error("sniffer {sniffer:?} {version}: {field} must be an https:// URL, got {url:?}")]
    NotHttps {
        sniffer: String,
        version: String,
        field: &'static str,
        url: String,
    },
    #[error(
        "sniffer {sniffer:?} {version}: {field} must have a host and no user info, got {url:?}"
    )]
    BadUrlHost {
        sniffer: String,
        version: String,
        field: &'static str,
        url: String,
    },
    #[error("sniffer {sniffer:?} {version}: limits.max_memory_bytes must be 1..={MAX_LIMIT_MEMORY_BYTES} and limits.call_timeout_ms 1..={MAX_LIMIT_CALL_TIMEOUT_MS}")]
    BadLimits { sniffer: String, version: String },
    #[error("sniffer {sniffer:?} {version}: sha256 must be 64 lowercase hex characters")]
    BadSha256 { sniffer: String, version: String },
    #[error("sniffer {sniffer:?} {version}: size {size} is over the {MAX_MODULE_BYTES} byte module limit")]
    OversizeModule {
        sniffer: String,
        version: String,
        size: u64,
    },
    #[error("sniffer {sniffer:?}: {problem}")]
    Catalog {
        sniffer: String,
        problem: CatalogProblem,
    },
    #[error("stats_url must be an https:// URL with a host and no user info, got {0:?}")]
    BadStatsUrl(String),
    #[error("detail file is {0} bytes, over the {MAX_DETAIL_BYTES} byte limit")]
    DetailTooLarge(usize),
    #[error("detail file is for {found:?}, not {expected:?}")]
    DetailName { expected: String, found: String },
    #[error("sniffer {sniffer:?} {version}: abi {abi:?} is not of the form major.minor")]
    BadAbi {
        sniffer: String,
        version: String,
        abi: String,
    },
}

/// Just the `kind`, read before the rest of the index (see `parse_index`).
#[derive(Deserialize)]
struct Head {
    kind: Option<Kind>,
    schema: Option<u32>,
}

/// Parse and validate an index. Never panics on hostile input.
pub fn parse_index(bytes: &[u8]) -> Result<Index, IndexError> {
    if bytes.len() > MAX_INDEX_BYTES {
        return Err(IndexError::TooLarge(bytes.len()));
    }
    // Look at `kind` and `schema` first: another kind of registry has a different payload, and an
    // older schema lacks fields, so "missing field `latest`" would not say what is wrong.
    if let Ok(head) = serde_json::from_slice::<Head>(bytes) {
        if let Some(kind @ Kind::Plugin) = head.kind {
            return Err(IndexError::UnsupportedKind(kind));
        }
        if let Some(schema) = head.schema.filter(|s| *s != INDEX_SCHEMA) {
            return Err(IndexError::UnsupportedSchema(schema));
        }
    }
    let index: Index =
        serde_json::from_slice(bytes).map_err(|e| IndexError::Json(e.to_string()))?;
    index.validate()?;
    Ok(index)
}

impl Index {
    pub fn validate(&self) -> Result<(), IndexError> {
        if self.schema != INDEX_SCHEMA {
            return Err(IndexError::UnsupportedSchema(self.schema));
        }
        if self.kind != Kind::Sniffer {
            return Err(IndexError::UnsupportedKind(self.kind));
        }
        if let Some(url) = &self.stats_url {
            check_https_url(url).map_err(|_| IndexError::BadStatsUrl(url.clone()))?;
        }
        let mut seen = std::collections::HashSet::new();
        for sniffer in &self.sniffers {
            if !valid_name(&sniffer.name) {
                return Err(IndexError::BadName(sniffer.name.clone()));
            }
            if !seen.insert(sniffer.name.as_str()) {
                return Err(IndexError::DuplicateSniffer(sniffer.name.clone()));
            }
            sniffer.validate()?;
        }
        Ok(())
    }
}

/// Parse a detail file and merge it into its catalog `entry`; the result is a fully
/// inline entry that passes the same checks as one from a one-file registry.
pub fn parse_detail(bytes: &[u8], entry: &SnifferEntry) -> Result<SnifferEntry, IndexError> {
    if bytes.len() > MAX_DETAIL_BYTES {
        return Err(IndexError::DetailTooLarge(bytes.len()));
    }
    let detail: SnifferDetail =
        serde_json::from_slice(bytes).map_err(|e| IndexError::Json(e.to_string()))?;
    if detail.schema != INDEX_SCHEMA {
        return Err(IndexError::UnsupportedSchema(detail.schema));
    }
    if detail.name != entry.name {
        return Err(IndexError::DetailName {
            expected: entry.name.clone(),
            found: detail.name,
        });
    }
    let mut merged = entry.clone();
    merged.detail_url = None;
    merged.versions = detail.versions;
    merged.validate()?;
    Ok(merged)
}

impl SnifferEntry {
    /// Split into the catalog entry (pointing at `detail_url`) and its detail file.
    pub fn split(&self, detail_url: String) -> (SnifferEntry, SnifferDetail) {
        let mut catalog = self.clone();
        catalog.versions = Vec::new();
        catalog.detail_url = Some(detail_url);
        let detail = SnifferDetail {
            schema: INDEX_SCHEMA,
            kind: Kind::Sniffer,
            name: self.name.clone(),
            versions: self.versions.clone(),
        };
        (catalog, detail)
    }

    fn validate(&self) -> Result<(), IndexError> {
        let newest = self.versions.first().map(|v| Newest {
            version: &v.version,
            abi: &v.abi,
            signed: v.signature_url.is_some(),
        });
        let catalog = Catalog {
            categories: &self.categories,
            tags: &self.tags,
            updated: &self.updated,
            homepage: self.homepage.as_deref(),
            detail_url: self.detail_url.as_deref(),
            latest: &self.latest,
            abi: &self.abi,
            signed: self.signed,
            newest,
        }
        .check()
        .map_err(|problem| IndexError::Catalog {
            sniffer: self.name.clone(),
            problem,
        });
        for pair in self.versions.windows(2) {
            let (earlier, later) = (&pair[0].version, &pair[1].version);
            if earlier == later {
                return Err(IndexError::DuplicateVersion {
                    sniffer: self.name.clone(),
                    version: earlier.to_string(),
                });
            }
            if earlier < later {
                return Err(IndexError::UnsortedVersions {
                    sniffer: self.name.clone(),
                    earlier: earlier.to_string(),
                    later: later.to_string(),
                });
            }
        }
        for v in &self.versions {
            v.validate(&self.name)?;
        }
        catalog
    }
}

impl VersionEntry {
    fn validate(&self, sniffer: &str) -> Result<(), IndexError> {
        let version = self.version.to_string();
        let https = |field: &'static str, url: &str| match check_https_url(url) {
            Ok(()) => Ok(()),
            Err(UrlProblem::NotHttps) => Err(IndexError::NotHttps {
                sniffer: sniffer.to_owned(),
                version: version.clone(),
                field,
                url: url.to_owned(),
            }),
            Err(UrlProblem::BadHost) => Err(IndexError::BadUrlHost {
                sniffer: sniffer.to_owned(),
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
            return Err(IndexError::BadSha256 {
                sniffer: sniffer.to_owned(),
                version,
            });
        }
        if !(1..=MAX_LIMIT_MEMORY_BYTES).contains(&self.limits.max_memory_bytes)
            || !(1..=MAX_LIMIT_CALL_TIMEOUT_MS).contains(&self.limits.call_timeout_ms)
        {
            return Err(IndexError::BadLimits {
                sniffer: sniffer.to_owned(),
                version,
            });
        }
        if self.size > MAX_MODULE_BYTES {
            return Err(IndexError::OversizeModule {
                sniffer: sniffer.to_owned(),
                version,
                size: self.size,
            });
        }
        if parse_abi(&self.abi).is_none() {
            return Err(IndexError::BadAbi {
                sniffer: sniffer.to_owned(),
                version,
                abi: self.abi.clone(),
            });
        }
        Ok(())
    }
}

/// Why an index URL is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UrlProblem {
    NotHttps,
    BadHost,
}

/// An index URL must be `https://` with a host, an optional valid port and no user info.
/// Whether the host is a private address is the fetch-time guard's job.
pub(crate) fn check_https_url(url: &str) -> Result<(), UrlProblem> {
    let Some(rest) = url.strip_prefix("https://") else {
        return Err(UrlProblem::NotHttps);
    };
    // Authority is everything up to the first '/', '?' or '#'.
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let (host, port, host_ok) = match authority.strip_prefix('[') {
        Some(v6) => v6.split_once(']').map_or(("", None, false), |(h, r)| {
            (
                h,
                r.strip_prefix(':').or(Some(r).filter(|r| !r.is_empty())),
                h.parse::<std::net::Ipv6Addr>().is_ok(),
            )
        }),
        None => (
            authority.rsplit_once(':').map_or(authority, |(h, _)| h),
            authority.rsplit_once(':').map(|(_, p)| p),
            // An IPv6 literal needs brackets, so a bare authority holds at most one colon.
            authority.matches(':').count() <= 1,
        ),
    };
    let port_ok = port.is_none_or(|p| p.parse::<u16>().is_ok());
    if authority.contains('@') || host.is_empty() || !host_ok || !port_ok {
        return Err(UrlProblem::BadHost);
    }
    Ok(())
}

/// Registry names are lowercase only, so two names never collide as files on a
/// case-insensitive filesystem. Stricter than `valid_module_name` in the
/// proxy's `admin.rs`, which stays case-sensitive.
pub(crate) fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// Parse an ABI string `major.minor` (both `u16`).
pub(crate) fn parse_abi(abi: &str) -> Option<(u16, u16)> {
    let (major, minor) = abi.split_once('.')?;
    // Canonical decimal only: "0.02" would parse as 0.2 here but not equal "0.2" where the
    // strings are compared (the module's own ABI section against the index).
    let digits = |s: &str| {
        !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) && (s == "0" || !s.starts_with('0'))
    };
    if !digits(major) || !digits(minor) {
        return None;
    }
    Some((major.parse().ok()?, minor.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = include_str!("../tests/fixtures/example-index.json");

    fn example() -> serde_json::Value {
        serde_json::from_str(EXAMPLE).unwrap()
    }

    fn parse(v: &serde_json::Value) -> Result<Index, IndexError> {
        parse_index(v.to_string().as_bytes())
    }

    fn first_version(v: &mut serde_json::Value) -> &mut serde_json::Value {
        &mut v["sniffers"][0]["versions"][0]
    }

    #[test]
    fn parses_the_example_from_the_spec() {
        let index = parse_index(EXAMPLE.as_bytes()).unwrap();
        assert_eq!(index.kind, Kind::Sniffer);
        assert_eq!(index.sniffers[0].versions.len(), 2);
        assert_eq!(index.sniffers[0].versions[0].version.to_string(), "0.2.0");
    }

    #[test]
    fn rejects_other_schemas() {
        for other in [1, 3] {
            let mut v = example();
            v["schema"] = other.into();
            assert_eq!(parse(&v), Err(IndexError::UnsupportedSchema(other)));
        }
    }

    #[test]
    fn a_plugin_registry_gets_the_kind_error_not_a_missing_field() {
        // A plugin index lists `plugins`, not `sniffers`; the message must say what
        // the registry is, not that a field is missing.
        let plugin_index = br#"{"schema":1,"kind":"plugin","name":"x","plugins":[]}"#;
        assert_eq!(
            parse_index(plugin_index),
            Err(IndexError::UnsupportedKind(Kind::Plugin))
        );
    }

    #[test]
    fn rejects_missing_kind() {
        let mut v = example();
        v.as_object_mut().unwrap().remove("kind");
        assert!(matches!(parse(&v), Err(IndexError::Json(m)) if m.contains("kind")));
    }

    #[test]
    fn rejects_plugin_kind_until_supported() {
        let mut v = example();
        v["kind"] = "plugin".into();
        assert_eq!(parse(&v), Err(IndexError::UnsupportedKind(Kind::Plugin)));
    }

    #[test]
    fn rejects_duplicate_sniffer_name() {
        let mut v = example();
        let dup = v["sniffers"][0].clone();
        v["sniffers"].as_array_mut().unwrap().push(dup);
        assert_eq!(
            parse(&v),
            Err(IndexError::DuplicateSniffer("minecraft".into()))
        );
    }

    #[test]
    fn rejects_duplicate_version() {
        let mut v = example();
        v["sniffers"][0]["versions"][1]["version"] = "0.2.0".into();
        assert!(matches!(
            parse(&v),
            Err(IndexError::DuplicateVersion { .. })
        ));
    }

    #[test]
    fn rejects_unsorted_versions() {
        let mut v = example();
        v["sniffers"][0]["versions"]
            .as_array_mut()
            .unwrap()
            .reverse();
        assert!(matches!(
            parse(&v),
            Err(IndexError::UnsortedVersions { .. })
        ));
    }

    #[test]
    fn rejects_http_url() {
        let mut v = example();
        first_version(&mut v)["url"] = "http://example.com/a.wasm".into();
        assert!(matches!(
            parse(&v),
            Err(IndexError::NotHttps { field: "url", .. })
        ));
    }

    #[test]
    fn rejects_empty_host_and_user_info() {
        for url in [
            "https://",
            "https:///a.wasm",
            "https://:443/a.wasm",
            "https://user:pw@example.com/a.wasm",
            "https://example.com@evil.example/a.wasm",
            "https://?x=1",
            "https://example.com:not-a-port/a.wasm",
            "https://example.com:99999/a.wasm",
            "https://[::1]x/a.wasm",
            "https://[::1/a.wasm",
            "https://[abc]/a.wasm",
            "https://::1/a.wasm",
            "https://a:b:443/a.wasm",
        ] {
            let mut v = example();
            first_version(&mut v)["url"] = url.into();
            assert!(
                matches!(parse(&v), Err(IndexError::BadUrlHost { field: "url", .. })),
                "{url}"
            );
        }
        let mut v = example();
        first_version(&mut v)["signature_url"] = "https://u@example.com/a.minisig".into();
        assert!(matches!(
            parse(&v),
            Err(IndexError::BadUrlHost {
                field: "signature_url",
                ..
            })
        ));
    }

    #[test]
    fn accepts_a_port_and_an_ipv6_host() {
        for url in [
            "https://example.com:8443/a.wasm",
            "https://[::1]:8443/a.wasm",
        ] {
            let mut v = example();
            first_version(&mut v)["url"] = url.into();
            parse(&v).unwrap();
        }
    }

    #[test]
    fn rejects_out_of_range_limits() {
        for (mem, ms) in [
            (0u64, 50u64),
            (u64::MAX, 50),
            (MAX_LIMIT_MEMORY_BYTES + 1, 50),
            (1_048_576, 0),
            (1_048_576, u64::MAX),
            (1_048_576, MAX_LIMIT_CALL_TIMEOUT_MS + 1),
        ] {
            let mut v = example();
            first_version(&mut v)["limits"] =
                serde_json::json!({"max_memory_bytes": mem, "call_timeout_ms": ms});
            assert!(
                matches!(parse(&v), Err(IndexError::BadLimits { .. })),
                "{mem} {ms}"
            );
        }
    }

    #[test]
    fn accepts_limits_at_the_caps() {
        let mut v = example();
        first_version(&mut v)["limits"] = serde_json::json!({
            "max_memory_bytes": MAX_LIMIT_MEMORY_BYTES,
            "call_timeout_ms": MAX_LIMIT_CALL_TIMEOUT_MS
        });
        parse(&v).unwrap();
    }

    #[test]
    fn rejects_http_signature_url() {
        let mut v = example();
        first_version(&mut v)["signature_url"] = "http://example.com/a.minisig".into();
        assert!(matches!(
            parse(&v),
            Err(IndexError::NotHttps {
                field: "signature_url",
                ..
            })
        ));
    }

    #[test]
    fn rejects_uppercase_or_short_sha() {
        for bad in [
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "abc",
        ] {
            let mut v = example();
            first_version(&mut v)["sha256"] = bad.into();
            assert!(
                matches!(parse(&v), Err(IndexError::BadSha256 { .. })),
                "{bad}"
            );
        }
    }

    #[test]
    fn rejects_bad_name() {
        for bad in [
            "../evil",
            "",
            "has space",
            ".",
            "..",
            "Minecraft",
            "MINECRAFT",
        ] {
            let mut v = example();
            v["sniffers"][0]["name"] = bad.into();
            assert!(matches!(parse(&v), Err(IndexError::BadName(_))), "{bad:?}");
        }
    }

    #[test]
    fn the_bad_name_message_says_lowercase() {
        let msg = IndexError::BadName("Minecraft".into()).to_string();
        assert!(msg.contains("lowercase letters"), "{msg}");
    }

    #[test]
    fn rejects_oversize_module() {
        let mut v = example();
        first_version(&mut v)["size"] = (MAX_MODULE_BYTES + 1).into();
        assert!(matches!(parse(&v), Err(IndexError::OversizeModule { .. })));
    }

    #[test]
    fn accepts_a_module_of_exactly_the_limit() {
        let mut v = example();
        first_version(&mut v)["size"] = MAX_MODULE_BYTES.into();
        assert!(parse(&v).is_ok());
    }

    #[test]
    fn rejects_bad_abi_string() {
        for bad in ["1", "a.b", "1.2.3", "", "0.02", "01.2"] {
            let mut v = example();
            first_version(&mut v)["abi"] = bad.into();
            assert!(
                matches!(parse(&v), Err(IndexError::BadAbi { .. })),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn rejects_oversize_index_bytes() {
        let big = vec![b' '; MAX_INDEX_BYTES + 1];
        assert_eq!(
            parse_index(&big),
            Err(IndexError::TooLarge(MAX_INDEX_BYTES + 1))
        );
    }

    #[test]
    fn rejects_garbage_json_without_panic() {
        for garbage in [&b""[..], b"{", b"null", b"[]", b"\xff\xfe"] {
            assert!(matches!(parse_index(garbage), Err(IndexError::Json(_))));
        }
    }

    #[test]
    fn a_bare_schema_1_object_is_an_unsupported_schema() {
        assert!(matches!(
            parse_index(b"{\"schema\":1}"),
            Err(IndexError::UnsupportedSchema(1))
        ));
    }

    #[test]
    fn unknown_fields_are_ignored_for_forward_compatibility() {
        let mut v = example();
        v["future_field"] = true.into();
        first_version(&mut v)["future"] = 1.into();
        assert!(parse(&v).is_ok());
    }

    fn catalog_problem(v: &serde_json::Value) -> CatalogProblem {
        match parse(v) {
            Err(IndexError::Catalog { problem, .. }) => problem,
            other => panic!("expected a catalog problem, got {other:?}"),
        }
    }

    #[test]
    fn catalog_fields_are_checked() {
        let mut v = example();
        v["sniffers"][0]["categories"] = serde_json::json!(["Game"]);
        assert_eq!(catalog_problem(&v), CatalogProblem::BadLabel("Game".into()));
        let mut v = example();
        v["sniffers"][0]["updated"] = "yesterday".into();
        assert!(matches!(catalog_problem(&v), CatalogProblem::BadDate(_)));
        let mut v = example();
        v["sniffers"][0]["latest"] = "0.1.0".into();
        assert_eq!(catalog_problem(&v), CatalogProblem::LatestMismatch);
        let mut v = example();
        v["sniffers"][0]["signed"] = false.into();
        assert_eq!(catalog_problem(&v), CatalogProblem::LatestMismatch);
    }

    #[test]
    fn a_homepage_must_be_an_https_url() {
        for bad in [
            "javascript:alert(1)",
            "http://example.com",
            "https://u:p@example.com/",
            "example.com",
        ] {
            let mut v = example();
            v["sniffers"][0]["homepage"] = bad.into();
            assert_eq!(
                catalog_problem(&v),
                CatalogProblem::BadHomepage(bad.into()),
                "{bad}"
            );
        }
    }

    #[test]
    fn versions_and_detail_url_are_exclusive() {
        let mut v = example();
        v["sniffers"][0]["detail_url"] = "https://example.com/minecraft.json".into();
        assert_eq!(catalog_problem(&v), CatalogProblem::VersionsOrDetail);
        let mut v = example();
        v["sniffers"][0]["versions"] = serde_json::json!([]);
        assert_eq!(catalog_problem(&v), CatalogProblem::VersionsOrDetail);
        v["sniffers"][0]["detail_url"] = "http://example.com/m.json".into();
        assert!(matches!(
            catalog_problem(&v),
            CatalogProblem::BadDetailUrl(_)
        ));
    }

    #[test]
    fn stats_url_must_be_https() {
        let mut v = example();
        v["stats_url"] = "https://example.com/stats.json".into();
        assert!(parse(&v).is_ok());
        v["stats_url"] = "http://example.com/stats.json".into();
        assert!(matches!(parse(&v), Err(IndexError::BadStatsUrl(_))));
    }

    fn split_example() -> (SnifferEntry, Vec<u8>) {
        let index = parse_index(EXAMPLE.as_bytes()).unwrap();
        let (catalog, detail) = index.sniffers[0].split("https://example.com/m.json".into());
        (catalog, serde_json::to_vec(&detail).unwrap())
    }

    #[test]
    fn a_catalog_entry_plus_its_detail_equals_the_inline_entry() {
        let (catalog, detail) = split_example();
        assert!(catalog.versions.is_empty() && catalog.detail_url.is_some());
        let mut v = example();
        v["sniffers"][0] = serde_json::to_value(&catalog).unwrap();
        assert!(
            parse(&v).is_ok(),
            "a catalog-only entry is valid on its own"
        );
        let merged = parse_detail(&detail, &catalog).unwrap();
        assert_eq!(merged, parse_index(EXAMPLE.as_bytes()).unwrap().sniffers[0]);
    }

    #[test]
    fn a_detail_for_another_sniffer_or_another_latest_is_refused() {
        let (catalog, detail) = split_example();
        let mut other = catalog.clone();
        other.name = "other".into();
        assert!(matches!(
            parse_detail(&detail, &other),
            Err(IndexError::DetailName { .. })
        ));
        let mut stale = catalog.clone();
        stale.latest = "0.3.0".parse().unwrap();
        assert!(matches!(
            parse_detail(&detail, &stale),
            Err(IndexError::Catalog { .. })
        ));
        assert!(matches!(
            parse_detail(&vec![b' '; MAX_DETAIL_BYTES + 1], &catalog),
            Err(IndexError::DetailTooLarge(_))
        ));
        assert!(parse_detail(b"{}", &catalog).is_err());
    }

    #[test]
    fn a_real_schema_1_file_is_told_its_schema_not_a_missing_field() {
        let v1 = br#"{"schema":1,"kind":"sniffer","name":"o","sniffers":[{"name":"x","description":"","license":"MIT","versions":[]}]}"#;
        assert_eq!(parse_index(v1), Err(IndexError::UnsupportedSchema(1)));
    }
}
