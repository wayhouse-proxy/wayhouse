//! The operator's list of sniffer registries, persisted as a JSON file.
//!
//! One registry is the official one (a constant, removable); the rest are
//! external and installed from at the operator's own risk. Nothing here touches
//! the network: [`crate::registry_client`] fetches, this module only remembers
//! which URLs to fetch.

use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Where the official registry's `index.json` lives. One constant so a later
/// move of the index is a one-line change.
pub const DEFAULT_REGISTRY_URL: &str =
    "https://raw.githubusercontent.com/wayhouse-proxy/sniffers/main/index.json";

/// Where the official plugin registry's `index.json` lives.
pub const DEFAULT_PLUGIN_REGISTRY_URL: &str =
    "https://raw.githubusercontent.com/wayhouse-proxy/plugins/main/index.json";

/// Most registries kept at once.
pub const MAX_REGISTRIES: usize = 32;

/// What a registry serves. A file written before plugin registries existed has no kind,
/// and its entries are sniffer registries.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RegistryKind {
    #[default]
    Sniffer,
    Plugin,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryRef {
    /// First 12 hex characters of the sha256 of the normalised URL.
    pub id: String,
    pub name: String,
    pub url: String,
    pub official: bool,
    #[serde(default)]
    pub kind: RegistryKind,
}

#[derive(Debug, thiserror::Error)]
pub enum RegistriesError {
    #[error("registry URL must be a valid https:// URL with a host")]
    BadUrl,
    #[error("registry URL must not contain credentials")]
    Credentials,
    #[error("at most {MAX_REGISTRIES} registries")]
    TooMany,
    #[error("registries file: {0}")]
    File(String),
}

/// What the file holds: only what the operator changed. The official registry is
/// a constant, so it is never written; removing it is remembered as a flag.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Stored {
    #[serde(default)]
    registries: Vec<RegistryRef>,
    #[serde(default)]
    default_removed: bool,
    #[serde(default)]
    plugin_default_removed: bool,
}

struct Inner {
    added: Vec<RegistryRef>,
    default_removed: bool,
    plugin_default_removed: bool,
}

pub struct Registries {
    path: Option<PathBuf>,
    include_default: bool,
    inner: Mutex<Inner>,
}

fn id_for(url: &str) -> String {
    let digest = Sha256::digest(url.as_bytes());
    digest.iter().take(6).map(|b| format!("{b:02x}")).collect()
}

fn default_ref() -> RegistryRef {
    RegistryRef {
        id: id_for(DEFAULT_REGISTRY_URL),
        name: "Official wayhouse sniffers".into(),
        url: DEFAULT_REGISTRY_URL.into(),
        official: true,
        kind: RegistryKind::Sniffer,
    }
}

/// Id of the official plugin registry (tests).
#[cfg(test)]
pub(crate) fn default_plugin_ref_id() -> String {
    default_plugin_ref().id
}

fn default_plugin_ref() -> RegistryRef {
    RegistryRef {
        id: id_for(DEFAULT_PLUGIN_REGISTRY_URL),
        name: "Official wayhouse plugins".into(),
        url: DEFAULT_PLUGIN_REGISTRY_URL.into(),
        official: true,
        kind: RegistryKind::Plugin,
    }
}

/// The normalised form of a registry URL, or why it is refused.
fn normalise(url: &str) -> Result<String, RegistriesError> {
    let parsed = reqwest::Url::parse(url).map_err(|_| RegistriesError::BadUrl)?;
    if parsed.scheme() != "https" || parsed.host_str().is_none_or(str::is_empty) {
        return Err(RegistriesError::BadUrl);
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(RegistriesError::Credentials);
    }
    Ok(parsed.to_string())
}

impl Registries {
    pub fn load(path: Option<PathBuf>, include_default: bool) -> Result<Self, RegistriesError> {
        let stored = match &path {
            Some(p) if p.exists() => {
                let bytes = std::fs::read(p).map_err(|e| RegistriesError::File(e.to_string()))?;
                serde_json::from_slice::<Stored>(&bytes)
                    .map_err(|e| RegistriesError::File(format!("{}: {e}", p.display())))?
            }
            _ => Stored::default(),
        };
        Ok(Self {
            path,
            include_default,
            inner: Mutex::new(Inner {
                added: stored.registries,
                default_removed: stored.default_removed,
                plugin_default_removed: stored.plugin_default_removed,
            }),
        })
    }

    fn snapshot(&self, inner: &Inner) -> Vec<RegistryRef> {
        let mut all = Vec::new();
        if self.include_default && !inner.default_removed {
            all.push(default_ref());
        }
        if self.include_default && !inner.plugin_default_removed {
            all.push(default_plugin_ref());
        }
        all.extend(inner.added.iter().cloned());
        all
    }

    /// The sniffer registries.
    pub fn list(&self) -> Vec<RegistryRef> {
        self.list_kind(RegistryKind::Sniffer)
    }

    /// The plugin registries.
    pub fn list_plugins(&self) -> Vec<RegistryRef> {
        self.list_kind(RegistryKind::Plugin)
    }

    fn list_kind(&self, kind: RegistryKind) -> Vec<RegistryRef> {
        let inner = self.inner.lock().unwrap();
        self.snapshot(&inner)
            .into_iter()
            .filter(|r| r.kind == kind)
            .collect()
    }

    /// A registry of either kind by id; callers check `kind` for the one they serve.
    pub fn get(&self, id: &str) -> Option<RegistryRef> {
        let inner = self.inner.lock().unwrap();
        self.snapshot(&inner).into_iter().find(|r| r.id == id)
    }

    pub fn add(&self, url: &str, name: Option<&str>) -> Result<RegistryRef, RegistriesError> {
        self.add_kind(url, name, RegistryKind::Sniffer)
    }

    pub fn add_kind(
        &self,
        url: &str,
        name: Option<&str>,
        kind: RegistryKind,
    ) -> Result<RegistryRef, RegistriesError> {
        let url = normalise(url)?;
        let id = id_for(&url);
        let mut inner = self.inner.lock().unwrap();
        if let Some(existing) = self.snapshot(&inner).into_iter().find(|r| r.id == id) {
            return Ok(existing);
        }
        // Re-adding the official URL brings the official entry back; an unofficial copy would
        // never get the signing key applied.
        let official = match kind {
            RegistryKind::Sniffer => default_ref(),
            RegistryKind::Plugin => default_plugin_ref(),
        };
        if self.include_default && id == official.id {
            let (sniffers_removed, plugins_removed) = match kind {
                RegistryKind::Sniffer => (false, inner.plugin_default_removed),
                RegistryKind::Plugin => (inner.default_removed, false),
            };
            self.persist(&inner.added, sniffers_removed, plugins_removed)?;
            (inner.default_removed, inner.plugin_default_removed) =
                (sniffers_removed, plugins_removed);
            return Ok(official);
        }
        if self.snapshot(&inner).len() >= MAX_REGISTRIES {
            return Err(RegistriesError::TooMany);
        }
        let host = reqwest::Url::parse(&url)
            .ok()
            .and_then(|u| u.host_str().map(str::to_owned))
            .unwrap_or_default();
        let entry = RegistryRef {
            id,
            name: name
                .map(str::trim)
                .filter(|n| !n.is_empty())
                .map_or(host, |n| n.chars().take(80).collect()),
            url,
            official: false,
            kind,
        };
        // Write first, change memory after: a failed write must not leave the list
        // showing something that would be gone after a restart.
        let mut candidate = inner.added.clone();
        candidate.push(entry.clone());
        self.persist(
            &candidate,
            inner.default_removed,
            inner.plugin_default_removed,
        )?;
        inner.added = candidate;
        Ok(entry)
    }

    /// Tests only: register a registry without the https check, so a local plain-http
    /// server can stand in for one.
    #[cfg(test)]
    pub(crate) fn add_unchecked(&self, url: &str, official: bool) -> RegistryRef {
        self.add_unchecked_kind(url, official, RegistryKind::Sniffer)
    }

    #[cfg(test)]
    pub(crate) fn add_unchecked_kind(
        &self,
        url: &str,
        official: bool,
        kind: RegistryKind,
    ) -> RegistryRef {
        let entry = RegistryRef {
            id: id_for(url),
            name: "test registry".into(),
            url: url.into(),
            official,
            kind,
        };
        self.inner.lock().unwrap().added.push(entry.clone());
        entry
    }

    pub fn remove(&self, id: &str) -> Result<bool, RegistriesError> {
        let mut inner = self.inner.lock().unwrap();
        let candidate: Vec<RegistryRef> =
            inner.added.iter().filter(|r| r.id != id).cloned().collect();
        let removed_default =
            self.include_default && !inner.default_removed && id == default_ref().id;
        let removed_plugin_default =
            self.include_default && !inner.plugin_default_removed && id == default_plugin_ref().id;
        let changed =
            candidate.len() != inner.added.len() || removed_default || removed_plugin_default;
        if changed {
            let default_removed = inner.default_removed || removed_default;
            let plugin_default_removed = inner.plugin_default_removed || removed_plugin_default;
            self.persist(&candidate, default_removed, plugin_default_removed)?;
            inner.added = candidate;
            inner.default_removed = default_removed;
            inner.plugin_default_removed = plugin_default_removed;
        }
        Ok(changed)
    }

    /// Write the file atomically (temp file in the same directory, then rename),
    /// mode 0600. In-memory only when no path was configured.
    fn persist(
        &self,
        added: &[RegistryRef],
        default_removed: bool,
        plugin_default_removed: bool,
    ) -> Result<(), RegistriesError> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let stored = Stored {
            registries: added.to_vec(),
            default_removed,
            plugin_default_removed,
        };
        let json =
            serde_json::to_vec_pretty(&stored).map_err(|e| RegistriesError::File(e.to_string()))?;
        let tmp = path.with_extension("json.tmp");
        write_private(&tmp, &json).map_err(|e| RegistriesError::File(e.to_string()))?;
        std::fs::rename(&tmp, path).map_err(|e| RegistriesError::File(e.to_string()))
    }
}

fn write_private(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(path)?;
    f.write_all(bytes)?;
    f.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> Registries {
        Registries::load(None, true).unwrap()
    }

    #[test]
    fn plugin_registries_are_a_separate_list_with_their_own_official_default() {
        let r = fresh();
        let plugins = r.list_plugins();
        assert_eq!(plugins.len(), 1);
        assert!(plugins[0].official && plugins[0].kind == RegistryKind::Plugin);
        assert_eq!(plugins[0].url, DEFAULT_PLUGIN_REGISTRY_URL);
        // The sniffer list is unchanged by it.
        assert_eq!(r.list().len(), 1);
        assert!(r.list().iter().all(|x| x.kind == RegistryKind::Sniffer));
        let added = r
            .add_kind("https://example.com/p.json", None, RegistryKind::Plugin)
            .unwrap();
        assert_eq!(r.list_plugins().len(), 2);
        assert_eq!(r.list().len(), 1);
        assert_eq!(r.get(&added.id).unwrap().kind, RegistryKind::Plugin);
    }

    #[test]
    fn removing_the_plugin_default_is_remembered_and_old_files_load_as_sniffer_registries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("registries.json");
        std::fs::write(
            &path,
            r#"{"registries":[{"id":"abc","name":"old","url":"https://old.example/i.json","official":false}],"default_removed":false}"#,
        )
        .unwrap();
        let r = Registries::load(Some(path.clone()), true).unwrap();
        let old = r.get("abc").unwrap();
        assert_eq!(old.kind, RegistryKind::Sniffer);
        assert_eq!(r.list().len(), 2);
        let id = r.list_plugins()[0].id.clone();
        assert!(r.remove(&id).unwrap());
        assert!(r.list_plugins().is_empty());
        let again = Registries::load(Some(path), true).unwrap();
        assert!(again.list_plugins().is_empty(), "the removal persists");
        assert_eq!(again.list().len(), 2, "sniffer registries are untouched");
    }

    #[test]
    fn default_registry_is_listed_and_official() {
        let l = fresh().list();
        assert_eq!(l.len(), 1);
        assert!(l[0].official);
        assert_eq!(l[0].url, DEFAULT_REGISTRY_URL);
        assert_eq!(l[0].id.len(), 12);
    }

    #[test]
    fn no_default_flag_hides_it() {
        assert!(Registries::load(None, false).unwrap().list().is_empty());
    }

    #[test]
    fn re_adding_the_official_url_after_removing_it_restores_the_official_entry() {
        let r = fresh();
        let id = default_ref().id;
        assert!(r.remove(&id).unwrap());
        assert!(r.list().is_empty());
        let back = r.add(DEFAULT_REGISTRY_URL, Some("mine")).unwrap();
        assert!(back.official, "the official key must apply again");
        let l = r.list();
        assert_eq!(l.len(), 1);
        assert!(l[0].official && l[0].id == id);
    }

    #[test]
    fn re_adding_the_official_plugin_url_restores_the_official_plugin_registry() {
        let r = fresh();
        let id = default_plugin_ref().id;
        assert!(r.remove(&id).unwrap());
        assert!(r.list_plugins().is_empty());
        let back = r
            .add_kind(DEFAULT_PLUGIN_REGISTRY_URL, None, RegistryKind::Plugin)
            .unwrap();
        assert!(back.official && back.kind == RegistryKind::Plugin);
        assert_eq!(r.list_plugins().len(), 1);
        assert_eq!(r.list().len(), 1, "the sniffer default is untouched");
    }

    #[test]
    fn add_rejects_http() {
        let r = fresh().add("http://example.com/index.json", None);
        assert!(matches!(r, Err(RegistriesError::BadUrl)));
    }

    #[test]
    fn add_rejects_garbage_and_hostless_urls() {
        for u in ["not a url", "https://", "ftp://example.com/x", ""] {
            assert!(
                matches!(fresh().add(u, None), Err(RegistriesError::BadUrl)),
                "{u:?}"
            );
        }
    }

    #[test]
    fn add_rejects_userinfo_url() {
        let r = fresh().add("https://user:pw@example.com/index.json", None);
        assert!(matches!(r, Err(RegistriesError::Credentials)));
    }

    #[test]
    fn add_is_idempotent_same_id() {
        let r = fresh();
        let a = r
            .add("https://example.com/index.json", Some("mine"))
            .unwrap();
        let b = r
            .add("HTTPS://Example.com/index.json", Some("other"))
            .unwrap();
        assert_eq!(a.id, b.id);
        assert!(!a.official);
        assert_eq!(r.list().len(), 2, "default plus one");
    }

    #[test]
    fn add_caps_at_32() {
        let r = Registries::load(None, false).unwrap();
        for i in 0..MAX_REGISTRIES {
            r.add(&format!("https://example.com/{i}.json"), None)
                .unwrap();
        }
        let over = r.add("https://example.com/over.json", None);
        assert!(matches!(over, Err(RegistriesError::TooMany)));
        // Re-adding an existing one at the cap is still a no-op, not an error.
        r.add("https://example.com/0.json", None).unwrap();
    }

    #[test]
    fn remove_unknown_is_false_and_known_is_true() {
        let r = fresh();
        assert!(!r.remove("nope").unwrap());
        let id = r.list()[0].id.clone();
        assert!(r.remove(&id).unwrap());
        assert!(r.get(&id).is_none());
    }

    #[test]
    fn persists_atomically_and_reloads() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("registries.json");
        let r = Registries::load(Some(file.clone()), false).unwrap();
        let added = r
            .add("https://example.com/index.json", Some("mine"))
            .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&file).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        let again = Registries::load(Some(file), false).unwrap();
        assert_eq!(again.list(), vec![added]);
    }

    #[test]
    fn default_is_not_persisted_but_removal_is_remembered() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("registries.json");
        let r = Registries::load(Some(file.clone()), true).unwrap();
        let id = r.list()[0].id.clone();
        r.remove(&id).unwrap();
        let again = Registries::load(Some(file), true).unwrap();
        assert!(again.get(&id).is_none(), "a removed default stays removed");
    }

    #[test]
    fn a_failed_write_leaves_the_list_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("registries.json");
        let r = Registries::load(Some(file.clone()), false).unwrap();
        let kept = r.add("https://example.com/a.json", None).unwrap();
        // The temp file's path is a directory, so every later write fails.
        std::fs::create_dir(dir.path().join("registries.json.tmp")).unwrap();

        assert!(matches!(
            r.add("https://example.com/b.json", None),
            Err(RegistriesError::File(_))
        ));
        assert_eq!(
            r.list(),
            vec![kept.clone()],
            "a failed add must not show up"
        );

        assert!(matches!(r.remove(&kept.id), Err(RegistriesError::File(_))));
        assert_eq!(r.list(), vec![kept], "a failed remove must not take effect");
    }

    #[test]
    fn a_failed_write_does_not_forget_the_default_either() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("registries.json");
        let r = Registries::load(Some(file), true).unwrap();
        std::fs::create_dir(dir.path().join("registries.json.tmp")).unwrap();
        let id = r.list()[0].id.clone();
        assert!(r.remove(&id).is_err());
        assert!(r.get(&id).is_some());
    }

    #[test]
    fn corrupt_file_is_an_error_not_silent_reset() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("registries.json");
        std::fs::write(&file, "{ not json").unwrap();
        assert!(matches!(
            Registries::load(Some(file), true),
            Err(RegistriesError::File(_))
        ));
    }
}
