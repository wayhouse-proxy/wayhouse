//! The proxy-local sealed secret store (S2a): for a proxy on a file config, which has no
//! controller to fetch from. A JSON file of `name -> sealed value`, sealed under the proxy's own
//! key file with the same name-bound scope the controller's store uses, so a value moved to
//! another name or opened with another key fails to open. The file holds ciphertext only and is
//! owner-only; `--secret-set` reads the value from stdin so it never touches argv or history.

use std::collections::BTreeMap;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

use wayhouse_secrets::{valid_cluster_name, Keyring, Sealed};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("the secrets file is readable by group or others; chmod 600 it")]
    Permissions,
    #[error("could not read the secrets file: {0}")]
    Read(String),
    #[error("the secrets file is not valid: {0}")]
    Parse(String),
    #[error("could not write the secrets file: {0}")]
    Write(String),
    #[error("a secret name is 1 to 63 of a-z, 0-9 and '-', not starting or ending with '-'")]
    Name,
    #[error("{0}")]
    Seal(String),
}

pub type Store = BTreeMap<String, Sealed>;

/// The store at `path`; a file that does not exist is an empty store.
pub fn read(path: &Path) -> Result<Store, StoreError> {
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Store::new()),
        Err(e) => return Err(StoreError::Read(e.to_string())),
    };
    if meta.permissions().mode() & 0o077 != 0 {
        return Err(StoreError::Permissions);
    }
    let text = std::fs::read_to_string(path).map_err(|e| StoreError::Read(e.to_string()))?;
    serde_json::from_str(&text).map_err(|e| StoreError::Parse(e.to_string()))
}

/// Replaces the file in one rename, owner-only from the first byte.
fn write(path: &Path, store: &Store) -> Result<(), StoreError> {
    let tmp = path.with_extension("tmp");
    let fail = |e: std::io::Error| StoreError::Write(e.to_string());
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)
        .map_err(fail)?;
    f.write_all(
        serde_json::to_string_pretty(store)
            .expect("sealed values serialize")
            .as_bytes(),
    )
    .map_err(fail)?;
    f.sync_all().map_err(fail)?;
    std::fs::rename(&tmp, path).map_err(fail)
}

/// Seals `value` under `name` (replacing any earlier value) and stores it.
pub fn set(path: &Path, keyring: &Keyring, name: &str, value: &[u8]) -> Result<(), StoreError> {
    if !valid_cluster_name(name) {
        return Err(StoreError::Name);
    }
    let sealed = keyring
        .seal_cluster(name, value)
        .map_err(|e| StoreError::Seal(e.to_string()))?;
    let mut store = read(path)?;
    store.insert(name.to_owned(), sealed);
    write(path, &store)
}

/// Removes `name`; `false` when it was not there.
pub fn remove(path: &Path, name: &str) -> Result<bool, StoreError> {
    let mut store = read(path)?;
    let had = store.remove(name).is_some();
    if had {
        write(path, &store)?;
    }
    Ok(had)
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY_A: &str = "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=";
    const KEY_B: &str = "AgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgI=";
    const VALUE: &[u8] = b"correct-horse-battery-staple";

    fn dir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn a_missing_file_is_an_empty_store() {
        let d = dir();
        assert!(read(&d.path().join("none.json")).unwrap().is_empty());
    }

    #[test]
    fn a_stored_value_opens_only_under_its_own_name_and_key() {
        let d = dir();
        let p = d.path().join("s.json");
        let a = Keyring::parse(KEY_A).unwrap();
        set(&p, &a, "gossip-psk", VALUE).unwrap();
        let store = read(&p).unwrap();
        let sealed = &store["gossip-psk"];
        assert_eq!(
            a.open_cluster("gossip-psk", sealed).unwrap().expose(),
            VALUE
        );
        assert!(a.open_cluster("other", sealed).is_err(), "name-bound");
        assert!(Keyring::parse(KEY_B)
            .unwrap()
            .open_cluster("gossip-psk", sealed)
            .is_err());
    }

    #[test]
    fn the_file_is_owner_only_and_holds_no_plaintext() {
        let d = dir();
        let p = d.path().join("s.json");
        set(&p, &Keyring::parse(KEY_A).unwrap(), "gossip-psk", VALUE).unwrap();
        assert_eq!(
            std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(!text.contains("correct-horse"), "{text}");
        assert!(
            !d.path().join("s.tmp").exists(),
            "the temp file is renamed away"
        );
    }

    #[test]
    fn setting_again_replaces_and_removing_deletes() {
        let d = dir();
        let p = d.path().join("s.json");
        let a = Keyring::parse(KEY_A).unwrap();
        set(&p, &a, "x1", b"first-value-1").unwrap();
        set(&p, &a, "x1", b"second-value-2").unwrap();
        let s = read(&p).unwrap();
        assert_eq!(s.len(), 1);
        assert_eq!(
            a.open_cluster("x1", &s["x1"]).unwrap().expose(),
            b"second-value-2"
        );
        assert!(remove(&p, "x1").unwrap());
        assert!(!remove(&p, "x1").unwrap());
        assert!(read(&p).unwrap().is_empty());
    }

    #[test]
    fn a_bad_name_or_a_short_value_is_refused_before_anything_is_written() {
        let d = dir();
        let p = d.path().join("s.json");
        let a = Keyring::parse(KEY_A).unwrap();
        assert!(matches!(
            set(&p, &a, "Bad_Name", VALUE),
            Err(StoreError::Name)
        ));
        assert!(matches!(
            set(&p, &a, "ok-name", b"x"),
            Err(StoreError::Seal(_))
        ));
        assert!(!p.exists());
    }

    #[test]
    fn a_group_readable_file_or_garbage_is_refused() {
        let d = dir();
        let p = d.path().join("s.json");
        std::fs::write(&p, "{}").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o640)).unwrap();
        assert!(matches!(read(&p), Err(StoreError::Permissions)));
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::write(&p, "not json").unwrap();
        assert!(matches!(read(&p), Err(StoreError::Parse(_))));
    }
}
