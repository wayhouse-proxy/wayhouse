//! Secrets at rest (design: `docs/deep-dive/topics/secrets.md`,
//! extended by `k1-design-2026-11-05.md`).
//!
//! One cluster keyring, provisioned identically to every node out of band
//! (`--plugin-secret-key-file`, or `WAYHOUSE_PLUGIN_SECRET_KEY`), encrypts each secret with
//! XChaCha20-Poly1305 and a random 192-bit nonce. The associated data binds a ciphertext
//! to `scope || 0 || name || 0 || key id` (a plugin secret's scope is its install id and its
//! name the slot), so one copied to another name or scope fails to open. The log, snapshots and the database hold only the [`Sealed`] form; the
//! keyring never leaves the process. The file is a keyring (one base64 key per line, the
//! last line active) polled for changes, so a rotation needs no restart.

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime};

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use chacha20poly1305::aead::{Aead, Payload};
use chacha20poly1305::{KeyInit, XChaCha20Poly1305};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

/// The shortest secret the store accepts: scrubbing a shorter value would mangle ordinary
/// text and protect nothing.
pub const MIN_SECRET_BYTES: usize = 8;

/// A secret's plaintext, zeroed when dropped, never `Debug`- or `Display`-printed.
pub struct SecretValue(Zeroizing<Vec<u8>>);

impl SecretValue {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(Zeroizing::new(bytes))
    }

    /// The plaintext. Call only where it is written into a request.
    pub fn expose(&self) -> &[u8] {
        &self.0
    }
}

impl std::fmt::Debug for SecretValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("<redacted>")
    }
}

impl std::fmt::Display for SecretValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("<redacted>")
    }
}

/// The environment variable that carries a keyring when no file is given.
pub const KEY_ENV: &str = "WAYHOUSE_PLUGIN_SECRET_KEY";
/// Largest secret value, in bytes.
pub const MAX_SECRET_BYTES: usize = 4096;
/// How often the key file is checked for a change.
const RELOAD_EVERY: Duration = Duration::from_secs(5);

/// A secret as stored and replicated: ciphertext only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sealed {
    pub key_id: String,
    /// Base64 of the 24-byte nonce.
    pub nonce: String,
    /// Base64 of the ciphertext with its tag.
    pub ciphertext: String,
    /// The revision this was sealed as (format 3). It is part of what the tag covers, so a blob
    /// cannot be relabelled as a newer one. Absent: the legacy format, which carries no revision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rev: Option<u64>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum KeyringError {
    #[error("the key file is readable by group or others; chmod 600 it")]
    Permissions,
    #[error("could not read the key file: {0}")]
    Read(String),
    #[error("line {0} is not a base64 key of exactly 32 bytes (generate one with `openssl rand -base64 32`)")]
    BadKey(usize),
    #[error("the keyring holds no key")]
    Empty,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SealError {
    #[error("no secret key is configured")]
    NoKey,
    #[error("a secret must be {MIN_SECRET_BYTES} to {MAX_SECRET_BYTES} bytes")]
    Size,
    #[error("a secret name is 1 to 63 of a-z, 0-9 and '-'")]
    Name,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum OpenError {
    /// The keyring lacks the key this secret was sealed with.
    #[error("key {0} is missing")]
    KeyMissing(String),
    #[error("the secret does not authenticate (wrong key, or moved to another slot)")]
    Auth,
    #[error("the stored secret is malformed")]
    Malformed,
    #[error("a secret name is 1 to 63 of a-z, 0-9 and '-'")]
    Name,
}

struct Key {
    id: String,
    bytes: Zeroizing<[u8; 32]>,
}

/// The keys this node holds; the last is active.
#[derive(Default)]
pub struct Keyring {
    keys: Vec<Key>,
}

fn key_id(key: &[u8; 32]) -> String {
    let mut h = Sha256::new();
    h.update(b"wayhouse-plugin-secret-key-id");
    h.update(key);
    h.finalize()[..8].iter().fold(String::new(), |mut s, b| {
        s.push_str(&format!("{b:02x}"));
        s
    })
}

fn aad(scope: &str, name: &str, key_id: &str, rev: Option<u64>) -> Vec<u8> {
    let mut a = Vec::new();
    // Format 3 starts with a tag no legacy AAD can have (a scope is never this text), so the two
    // formats never authenticate as each other.
    if rev.is_some() {
        a.extend_from_slice(b"wayhouse-secret/3");
        a.push(0);
    }
    for part in [scope, name, key_id] {
        a.extend_from_slice(part.as_bytes());
        a.push(0);
    }
    if let Some(r) = rev {
        a.extend_from_slice(r.to_string().as_bytes());
    }
    a
}

impl std::fmt::Debug for Keyring {
    /// Key ids only, never key material.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Keyring")
            .field("keys", &self.key_ids())
            .finish()
    }
}

impl Keyring {
    /// Parses a keyring: one base64 key per line; blank lines and `#` comments are skipped.
    pub fn parse(text: &str) -> Result<Self, KeyringError> {
        let mut keys = Vec::new();
        for (i, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let raw = Zeroizing::new(
                STANDARD
                    .decode(line)
                    .map_err(|_| KeyringError::BadKey(i + 1))?,
            );
            let bytes: [u8; 32] = raw
                .as_slice()
                .try_into()
                .map_err(|_| KeyringError::BadKey(i + 1))?;
            let id = key_id(&bytes);
            // The last occurrence of a repeated key decides its place, so the active
            // key (the last line) is the one the file ends with.
            keys.retain(|k: &Key| k.id != id);
            keys.push(Key {
                id,
                bytes: Zeroizing::new(bytes),
            });
        }
        if keys.is_empty() {
            return Err(KeyringError::Empty);
        }
        Ok(Self { keys })
    }

    /// Reads a key file, refusing one other users can read.
    pub fn from_file(path: &Path) -> Result<Self, KeyringError> {
        use std::io::Read;
        // Open once and check the permissions of the open file, so the file that was checked
        // is the file that is read.
        let mut file = std::fs::File::open(path).map_err(|e| KeyringError::Read(e.to_string()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let meta = file
                .metadata()
                .map_err(|e| KeyringError::Read(e.to_string()))?;
            if meta.permissions().mode() & 0o077 != 0 {
                return Err(KeyringError::Permissions);
            }
        }
        let mut text = Zeroizing::new(String::new());
        file.read_to_string(&mut text)
            .map_err(|e| KeyringError::Read(e.to_string()))?;
        Self::parse(&text)
    }

    /// The keyring from `WAYHOUSE_PLUGIN_SECRET_KEY` (one or more keys, whitespace
    /// separated), if set.
    pub fn from_env() -> Result<Option<Self>, KeyringError> {
        match std::env::var(KEY_ENV) {
            Ok(v) if !v.trim().is_empty() => {
                let v = Zeroizing::new(v);
                let joined = Zeroizing::new(v.split_whitespace().collect::<Vec<_>>().join("\n"));
                Self::parse(&joined).map(Some)
            }
            _ => Ok(None),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// The key file text `parse` reads back: one base64 key per line, the active key last.
    /// This is key material; only enrollment hands it to a node.
    pub fn export(&self) -> Zeroizing<String> {
        let mut out = Zeroizing::new(String::new());
        for k in &self.keys {
            out.push_str(&STANDARD.encode(*k.bytes));
            out.push('\n');
        }
        out
    }

    /// Ids of every key held, oldest first.
    pub fn key_ids(&self) -> Vec<String> {
        self.keys.iter().map(|k| k.id.clone()).collect()
    }

    pub fn active_id(&self) -> Option<&str> {
        self.keys.last().map(|k| k.id.as_str())
    }

    fn cipher(key: &Key) -> XChaCha20Poly1305 {
        XChaCha20Poly1305::new((&*key.bytes).into())
    }

    /// Encrypts `plaintext` for `(scope, name)` under the active key.
    pub fn seal(&self, scope: &str, name: &str, plaintext: &[u8]) -> Result<Sealed, SealError> {
        self.seal_rev(scope, name, plaintext, None)
    }

    /// As [`Keyring::seal`], binding the revision `rev` into the tag (format 3).
    pub fn seal_rev(
        &self,
        scope: &str,
        name: &str,
        plaintext: &[u8],
        rev: Option<u64>,
    ) -> Result<Sealed, SealError> {
        if !(MIN_SECRET_BYTES..=MAX_SECRET_BYTES).contains(&plaintext.len()) {
            return Err(SealError::Size);
        }
        let key = self.keys.last().ok_or(SealError::NoKey)?;
        let nonce: [u8; 24] = rand::random();
        let ciphertext = Self::cipher(key)
            .encrypt(
                (&nonce).into(),
                Payload {
                    msg: plaintext,
                    aad: &aad(scope, name, &key.id, rev),
                },
            )
            .map_err(|_| SealError::Size)?;
        Ok(Sealed {
            key_id: key.id.clone(),
            nonce: STANDARD.encode(nonce),
            ciphertext: STANDARD.encode(ciphertext),
            rev,
        })
    }

    /// Decrypts a secret sealed for `(scope, name)`.
    pub fn open(&self, scope: &str, name: &str, sealed: &Sealed) -> Result<SecretValue, OpenError> {
        let key = self
            .keys
            .iter()
            .find(|k| k.id == sealed.key_id)
            .ok_or_else(|| OpenError::KeyMissing(sealed.key_id.clone()))?;
        let nonce: [u8; 24] = STANDARD
            .decode(&sealed.nonce)
            .ok()
            .and_then(|n| n.try_into().ok())
            .ok_or(OpenError::Malformed)?;
        let ciphertext = STANDARD
            .decode(&sealed.ciphertext)
            .map_err(|_| OpenError::Malformed)?;
        let plain = Self::cipher(key)
            .decrypt(
                (&nonce).into(),
                Payload {
                    msg: &ciphertext,
                    aad: &aad(scope, name, &key.id, sealed.rev),
                },
            )
            .map_err(|_| OpenError::Auth)?;
        Ok(SecretValue::new(plain))
    }
}

/// The scope of a cluster secret (admin token, gossip PSK). An install id is hex, so no plugin
/// secret can be sealed under this scope, and the two kinds never open as each other.
pub const CLUSTER_SCOPE: &str = "@cluster";

/// A cluster secret's name: 1 to 63 of `a-z`, `0-9` and `-`, not starting or ending with `-`.
pub fn valid_cluster_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 63
        && !name.starts_with('-')
        && !name.ends_with('-')
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

impl Keyring {
    /// Seals a cluster secret under its name.
    pub fn seal_cluster(&self, name: &str, plaintext: &[u8]) -> Result<Sealed, SealError> {
        if !valid_cluster_name(name) {
            return Err(SealError::Name);
        }
        self.seal(CLUSTER_SCOPE, name, plaintext)
    }

    /// Seals a cluster secret as revision `rev` (format 3).
    pub fn seal_cluster_rev(
        &self,
        name: &str,
        plaintext: &[u8],
        rev: u64,
    ) -> Result<Sealed, SealError> {
        if !valid_cluster_name(name) {
            return Err(SealError::Name);
        }
        self.seal_rev(CLUSTER_SCOPE, name, plaintext, Some(rev))
    }

    /// Opens a cluster secret sealed under `name`.
    pub fn open_cluster(&self, name: &str, sealed: &Sealed) -> Result<SecretValue, OpenError> {
        if !valid_cluster_name(name) {
            return Err(OpenError::Name);
        }
        self.open(CLUSTER_SCOPE, name, sealed)
    }
}

/// The highest revision of each cluster secret a process has opened. A reader that opens every
/// blob it is served through it cannot be walked back to an older secret (a rotated token or
/// PSK). It lives in memory: after a restart it starts empty, so a hostile server can roll a
/// reader back once per restart.
#[derive(Default)]
pub struct RevFloor(std::sync::Mutex<std::collections::HashMap<String, u64>>);

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum FloorError {
    #[error("the served secret is older than one already accepted (a rollback)")]
    Rollback,
    /// A blob from before revisions: it cannot be told apart from an old one.
    #[error("the served secret carries no revision; have the controller rewrap it (POST /secrets/rewrap)")]
    NoRevision,
    #[error(transparent)]
    Open(#[from] OpenError),
}

impl RevFloor {
    /// Opens the cluster secret `name`. A blob with no revision, or with one below what this
    /// process has already opened, is refused. The floor rises only once the blob has
    /// authenticated, so a forged blob with a huge revision cannot lock the real one out.
    pub fn open_cluster(
        &self,
        ring: &Keyring,
        name: &str,
        sealed: &Sealed,
    ) -> Result<SecretValue, FloorError> {
        let rev = sealed.rev.ok_or(FloorError::NoRevision)?;
        let mut seen = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if seen.get(name).is_some_and(|floor| rev < *floor) {
            return Err(FloorError::Rollback);
        }
        let value = ring.open_cluster(name, sealed)?;
        seen.insert(name.to_owned(), rev);
        Ok(value)
    }
}

/// The floor of this process, for the readers (proxy, agent) that fetch sealed secrets.
pub fn process_floor() -> &'static RevFloor {
    static FLOOR: std::sync::OnceLock<RevFloor> = std::sync::OnceLock::new();
    FLOOR.get_or_init(RevFloor::default)
}

/// Seconds since the Unix epoch, as the clock the revisions follow.
pub fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The next revision of a secret that is at `current` (0: none yet). It follows the clock when
/// that is ahead, so a name that is deleted and set again never goes back to a revision a reader
/// has already accepted. That relies on the controller's clock not being set back past a revision
/// it has issued while a reader still remembers it; such a reader is refused (a rollback) until
/// the clock catches up or it restarts.
pub fn next_rev(current: u64, now_unix: u64) -> u64 {
    current.saturating_add(1).max(now_unix)
}

/// The node's keyring, swapped whole when the key file changes.
#[derive(Clone, Default)]
pub struct KeyringHandle(Arc<RwLock<Arc<Keyring>>>);

impl KeyringHandle {
    pub fn new(keyring: Keyring) -> Self {
        Self(Arc::new(RwLock::new(Arc::new(keyring))))
    }

    pub fn current(&self) -> Arc<Keyring> {
        self.0
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn set(&self, keyring: Keyring) {
        *self
            .0
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Arc::new(keyring);
    }
}

/// Loads the keyring from `file` or the environment. `None` when neither is configured.
pub fn load(file: Option<&Path>) -> Result<Option<KeyringHandle>, KeyringError> {
    match file {
        Some(path) => Keyring::from_file(path).map(|k| Some(KeyringHandle::new(k))),
        None => Ok(Keyring::from_env()?.map(KeyringHandle::new)),
    }
}

/// What identifies a version of the key file.
type Stamp = Option<(SystemTime, u64)>;

fn stamp(path: &Path) -> Stamp {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

/// Which version of the key file the keyring holds, and which one last failed to load.
pub struct Seen {
    loaded: Stamp,
    /// `Some(stamp)` once that version failed to load (a missing file is `Some(None)`).
    failed: Option<Stamp>,
}

impl Seen {
    pub fn of(path: &Path) -> Self {
        Self {
            loaded: stamp(path),
            failed: None,
        }
    }
}

/// Polls `path`; a changed file that parses replaces the keyring, one that does not is
/// logged and ignored (the old keys stay).
pub fn spawn_reload(handle: KeyringHandle, path: PathBuf) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut seen = Seen::of(&path);
        let mut every = tokio::time::interval(RELOAD_EVERY);
        loop {
            every.tick().await;
            reload_if_changed(&handle, &path, &mut seen);
        }
    })
}

/// Reloads when the file differs from the version the keyring holds. A version that fails to
/// load is retried on every poll (a `chmod` after the copy changes neither mtime nor size) but
/// logged once.
pub fn reload_if_changed(handle: &KeyringHandle, path: &Path, seen: &mut Seen) {
    let now = stamp(path);
    if now == seen.loaded {
        return;
    }
    match Keyring::from_file(path) {
        Ok(k) => {
            tracing::info!(keys = ?k.key_ids(), "reloaded the secret keyring");
            handle.set(k);
            seen.loaded = now;
            seen.failed = None;
        }
        Err(e) => {
            if seen.failed != Some(now) {
                tracing::warn!(error = %e, "the secret key file changed but did not load; keeping the old keys and retrying");
            }
            seen.failed = Some(now);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(n: u8) -> String {
        STANDARD.encode([n; 32])
    }

    fn ring(keys: &[u8]) -> Keyring {
        Keyring::parse(&keys.iter().map(|n| key(*n)).collect::<Vec<_>>().join("\n")).unwrap()
    }

    #[test]
    fn an_exported_keyring_parses_back_with_the_same_keys_and_active_key() {
        let r = ring(&[1, 2, 3]);
        let back = Keyring::parse(&r.export()).unwrap();
        assert_eq!(back.key_ids(), r.key_ids());
        assert_eq!(back.active_id(), r.active_id());
    }

    #[test]
    fn a_sealed_secret_opens_only_where_it_was_sealed() {
        let r = ring(&[1]);
        let sealed = r.seal("inst", "TOKEN", b"hunter2-hunter2").unwrap();
        assert_eq!(
            r.open("inst", "TOKEN", &sealed).unwrap().expose(),
            b"hunter2-hunter2"
        );
        // Moved to another slot or install: the associated data no longer matches.
        assert_eq!(
            r.open("inst", "OTHER", &sealed).unwrap_err(),
            OpenError::Auth
        );
        assert_eq!(
            r.open("other", "TOKEN", &sealed).unwrap_err(),
            OpenError::Auth
        );
        // And the ciphertext is not the plaintext.
        assert!(!sealed.ciphertext.contains("hunter"));
        assert!(!STANDARD
            .decode(&sealed.ciphertext)
            .unwrap()
            .windows(7)
            .any(|w| w == b"hunter2"));
    }

    #[test]
    fn a_cluster_secret_and_a_plugin_secret_cannot_stand_in_for_each_other() {
        let r = ring(&[1]);
        let cluster = r
            .seal_cluster("admin-token", b"cluster-secret-value")
            .unwrap();
        assert_eq!(
            r.open_cluster("admin-token", &cluster).unwrap().expose(),
            b"cluster-secret-value"
        );
        // Another cluster name fails; so does opening it as a plugin slot of any install id.
        assert_eq!(
            r.open_cluster("gossip-psk", &cluster).unwrap_err(),
            OpenError::Auth
        );
        assert_eq!(
            r.open("0123abcd", "admin-token", &cluster).unwrap_err(),
            OpenError::Auth
        );
        // A plugin secret never opens as a cluster secret, even under the same name.
        let plugin = r
            .seal("0123abcd", "admin-token", b"plugin-secret-value")
            .unwrap();
        assert_eq!(
            r.open_cluster("admin-token", &plugin).unwrap_err(),
            OpenError::Auth
        );
        // The reserved scope cannot be an install id (those are hex).
        assert!(CLUSTER_SCOPE.starts_with('@'));
    }

    /// Made by the code before the extraction (key `[7; 32]`, scope `0123abcd`, name `API_TOKEN`), so a
    /// change to the associated data, the key id or the cipher cannot slip through a seal-then-open test.
    #[test]
    fn a_secret_sealed_by_the_old_code_still_opens() {
        let r = ring(&[7]);
        let sealed = Sealed {
            key_id: "4559e316c2d041bb".into(),
            nonce: "6qSK5zM1/dc9IASHmYjCZOGJSU/tT8AB".into(),
            ciphertext: "Ozj/AXqO3XulpUCPVKVo1R31jNEC7T0DuA8CYLyLwK00wbRd/w==".into(),
            rev: None,
        };
        assert_eq!(r.active_id(), Some("4559e316c2d041bb"));
        assert_eq!(
            r.open("0123abcd", "API_TOKEN", &sealed).unwrap().expose(),
            b"old-code-secret-value"
        );
        assert_eq!(
            r.open("0123abcd", "OTHER", &sealed).unwrap_err(),
            OpenError::Auth
        );
    }

    #[test]
    fn a_cluster_name_that_is_not_valid_is_refused_before_it_reaches_the_associated_data() {
        let r = ring(&[1]);
        for bad in ["", "a\0b", "Admin", "a/b"] {
            assert_eq!(
                r.seal_cluster(bad, b"long-enough-value").unwrap_err(),
                SealError::Name,
                "{bad:?}"
            );
        }
        let ok = r.seal_cluster("admin-token", b"long-enough-value").unwrap();
        assert_eq!(r.open_cluster("a\0b", &ok).unwrap_err(), OpenError::Name);
    }

    #[test]
    fn cluster_secret_names_are_short_lowercase_words() {
        for ok in ["admin-token", "gossip-psk", "a", "x1-2"] {
            assert!(valid_cluster_name(ok), "{ok}");
        }
        for bad in [
            "",
            "Admin",
            "a b",
            "a/b",
            "..",
            "-lead",
            "trail-",
            &"a".repeat(64),
        ] {
            assert!(!valid_cluster_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn two_seals_of_the_same_secret_differ() {
        let r = ring(&[1]);
        let a = r.seal("i", "T", b"same-secret-value").unwrap();
        let b = r.seal("i", "T", b"same-secret-value").unwrap();
        assert_ne!(a.nonce, b.nonce);
        assert_ne!(a.ciphertext, b.ciphertext);
    }

    #[test]
    fn the_last_key_is_active_and_older_ones_still_open() {
        let old = ring(&[1]);
        let sealed_old = old.seal("i", "T", b"old-secret-value").unwrap();
        let both = ring(&[1, 2]);
        assert_eq!(both.active_id().unwrap(), key_id(&[2; 32]));
        assert_eq!(
            both.open("i", "T", &sealed_old).unwrap().expose(),
            b"old-secret-value"
        );
        assert_eq!(
            both.seal("i", "T", b"new-secret-value").unwrap().key_id,
            key_id(&[2; 32])
        );
        // Retiring early: a secret under a removed key is missing its key, not corrupt.
        let only_new = ring(&[2]);
        assert_eq!(
            only_new.open("i", "T", &sealed_old).unwrap_err(),
            OpenError::KeyMissing(key_id(&[1; 32]))
        );
    }

    #[test]
    fn no_key_and_bad_sizes_are_refused() {
        assert_eq!(
            Keyring::default()
                .seal("i", "T", b"long-enough-value")
                .unwrap_err(),
            SealError::NoKey
        );
        let r = ring(&[1]);
        assert_eq!(r.seal("i", "T", b"short").unwrap_err(), SealError::Size);
        assert_eq!(
            r.seal("i", "T", &vec![b'x'; MAX_SECRET_BYTES + 1])
                .unwrap_err(),
            SealError::Size
        );
    }

    #[test]
    fn the_keyring_text_must_be_32_byte_base64_keys() {
        assert_eq!(Keyring::parse("").unwrap_err(), KeyringError::Empty);
        assert_eq!(
            Keyring::parse("# only a comment\n").unwrap_err(),
            KeyringError::Empty
        );
        assert_eq!(
            Keyring::parse("not base64!!").unwrap_err(),
            KeyringError::BadKey(1)
        );
        assert_eq!(
            Keyring::parse(&format!("{}\n{}", key(1), STANDARD.encode([0u8; 16]))).unwrap_err(),
            KeyringError::BadKey(2)
        );
        let r = Keyring::parse(&format!("# old\n{}\n\n{}\n{}\n", key(1), key(2), key(2))).unwrap();
        assert_eq!(r.key_ids().len(), 2, "a repeated key counts once");
        let r = Keyring::parse(&format!("{}\n{}\n{}\n", key(1), key(2), key(1))).unwrap();
        assert_eq!(r.key_ids().len(), 2);
        let want = Keyring::parse(&key(1)).unwrap().key_ids();
        assert_eq!(
            r.active_id(),
            Some(want[0].as_str()),
            "the last line is active"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_key_file_others_can_read_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keys");
        std::fs::write(&path, key(1)).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(
            Keyring::from_file(&path).unwrap_err(),
            KeyringError::Permissions
        );
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(Keyring::from_file(&path).unwrap().key_ids().len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn a_changed_key_file_is_picked_up_and_a_broken_one_is_ignored() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keys");
        let write = |text: &str| {
            std::fs::write(&path, text).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        };
        write(&key(1));
        let handle = load(Some(&path)).unwrap().unwrap();
        let mut seen = Seen::of(&path);
        assert_eq!(handle.current().key_ids().len(), 1);
        write(&format!("{}\n{}", key(1), key(2)));
        reload_if_changed(&handle, &path, &mut seen);
        assert_eq!(handle.current().key_ids().len(), 2);
        write("garbage");
        reload_if_changed(&handle, &path, &mut seen);
        assert_eq!(handle.current().key_ids().len(), 2, "the old keys stay");
    }

    #[cfg(unix)]
    #[test]
    fn a_key_file_that_failed_to_load_is_retried_until_it_loads() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keys");
        let write = |text: &str, mode: u32| {
            std::fs::write(&path, text).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        };
        write(&key(1), 0o600);
        let handle = load(Some(&path)).unwrap().unwrap();
        let mut seen = Seen::of(&path);
        // `cp newkeys keyfile; chmod 600 keyfile`: the copy lands readable by others first.
        write(&format!("{}\n{}", key(1), key(2)), 0o644);
        reload_if_changed(&handle, &path, &mut seen);
        assert_eq!(handle.current().key_ids().len(), 1, "refused for its mode");
        // The chmod changes neither mtime nor size, so only a retry can pick the file up.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        reload_if_changed(&handle, &path, &mut seen);
        assert_eq!(handle.current().key_ids().len(), 2);
    }

    #[test]
    fn a_deleted_key_file_is_a_failure_to_report_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keys");
        std::fs::write(&path, key(1)).unwrap();
        let mut seen = Seen::of(&path);
        std::fs::remove_file(&path).unwrap();
        let handle = KeyringHandle::new(ring(&[1]));
        reload_if_changed(&handle, &path, &mut seen);
        assert_eq!(
            seen.failed,
            Some(None),
            "reported as a failure, not as 'nothing failed'"
        );
        assert_eq!(handle.current().key_ids().len(), 1, "the old keys stay");
    }

    #[test]
    fn a_revision_is_part_of_the_tag_and_legacy_blobs_still_open() {
        let r = ring(&[1]);
        let a = r.seal_cluster_rev("psk", b"first-secret-value", 7).unwrap();
        assert_eq!(a.rev, Some(7));
        assert_eq!(
            r.open_cluster("psk", &a).unwrap().expose(),
            b"first-secret-value"
        );

        // Relabelled as another revision, or stripped of it: does not authenticate.
        for rev in [Some(8), Some(6), None] {
            let relabelled = Sealed { rev, ..a.clone() };
            assert!(matches!(
                r.open_cluster("psk", &relabelled),
                Err(OpenError::Auth)
            ));
        }
        // A legacy blob (no revision) opens as before, and its JSON has no `rev`.
        let old = r.seal_cluster("psk", b"legacy-secret-value").unwrap();
        assert_eq!(old.rev, None);
        assert_eq!(
            r.open_cluster("psk", &old).unwrap().expose(),
            b"legacy-secret-value"
        );
        // A revision cannot be added to a legacy blob either.
        let upgraded = Sealed {
            rev: Some(1),
            ..old
        };
        assert!(r.open_cluster("psk", &upgraded).is_err());
    }

    #[test]
    fn the_floor_refuses_older_revisions_and_blobs_without_one() {
        let r = ring(&[1]);
        let floor = RevFloor::default();
        let at = |n| r.seal_cluster_rev("psk", b"some-secret-value", n).unwrap();
        let legacy = r.seal_cluster("psk", b"some-secret-value").unwrap();
        assert!(matches!(
            floor.open_cluster(&r, "psk", &legacy),
            Err(FloorError::NoRevision)
        ));
        floor.open_cluster(&r, "psk", &at(5)).unwrap();
        floor.open_cluster(&r, "psk", &at(5)).unwrap();
        floor.open_cluster(&r, "psk", &at(9)).unwrap();
        assert!(matches!(
            floor.open_cluster(&r, "psk", &at(8)),
            Err(FloorError::Rollback)
        ));
        assert!(matches!(
            floor.open_cluster(&r, "psk", &legacy),
            Err(FloorError::NoRevision)
        ));
        floor
            .open_cluster(
                &r,
                "other",
                &r.seal_cluster_rev("other", b"some-secret-value", 1)
                    .unwrap(),
            )
            .unwrap();
    }

    #[test]
    fn a_blob_that_does_not_authenticate_never_raises_the_floor() {
        let r = ring(&[1]);
        let floor = RevFloor::default();
        let real = r.seal_cluster_rev("psk", b"some-secret-value", 5).unwrap();
        // Garbage with a huge revision arrives first (a hostile or broken controller).
        let junk = Sealed {
            rev: Some(u64::MAX),
            ..real.clone()
        };
        assert!(matches!(
            floor.open_cluster(&r, "psk", &junk),
            Err(FloorError::Open(OpenError::Auth))
        ));
        // The real one still opens.
        assert_eq!(
            floor.open_cluster(&r, "psk", &real).unwrap().expose(),
            b"some-secret-value"
        );
    }

    #[test]
    fn the_next_revision_never_goes_back() {
        assert_eq!(next_rev(0, 1_000), 1_000);
        assert_eq!(next_rev(1_000, 1_000), 1_001);
        assert_eq!(
            next_rev(5_000, 1_000),
            5_001,
            "a clock set back changes nothing"
        );
    }
}
