//! What an enrolled node keeps on disk: a 0700 directory with `enrollment.json` (no secret),
//! `agent-token` and `tunnel-psk`, each 0600. The secrets are separate files so a service can
//! be given exactly one of them (`LoadCredential=`, a Docker secret).

use std::io::Write;
use std::path::{Path, PathBuf};

use base64::Engine;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::Role;

pub const META_FILE: &str = "enrollment.json";
pub const TOKEN_FILE: &str = "agent-token";
pub const PSK_FILE: &str = "tunnel-psk";
pub const KEYRING_FILE: &str = "secret.keys";
/// A controller replica's proof that the cluster may add it (`<id>.<secret>`; single purpose).
pub const JOIN_PROOF_FILE: &str = "ha-join-proof";

/// `enrollment.json`: where this node enrolled and as whom. Holds no secret.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Meta {
    pub controller_url: String,
    pub name: String,
    pub role: Role,
    /// An aggregator tier's parent URL, when the admin set one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_url: Option<String>,
    /// A controller replica's Raft node id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<u64>,
    /// A controller replica's `--tunnel-network` (the cluster's; they must match).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tunnel_network: Option<String>,
    /// How the other replicas reach a controller replica (`--ha-advertise`); not a secret.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub advertise: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum FileError {
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{} is not a base64 32-byte key", path.display())]
    BadPsk { path: PathBuf },
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> FileError + '_ {
    move |source| FileError::Io {
        path: path.to_owned(),
        source,
    }
}

/// Creates `dir` (mode 0700) if missing; an existing directory is tightened to 0700.
fn private_dir(dir: &Path) -> Result<(), FileError> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(io(dir))?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).map_err(io(dir))
}

/// Writes `data` to `path` with mode 0600 from the first byte (no window at the umask).
fn write_private(path: &Path, data: &[u8]) -> Result<(), FileError> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(io(path))?;
    f.write_all(data).map_err(io(path))
}

/// Refuses a directory that already holds an enrollment: a second `enroll` would overwrite its
/// tokens and tunnel key with truncating writes.
pub fn check_fresh(dir: &Path) -> Result<(), FileError> {
    let p = dir.join(META_FILE);
    if p.exists() {
        return Err(FileError::Io {
            path: p,
            source: std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "this directory is already enrolled; remove it or pick another --data-dir",
            ),
        });
    }
    Ok(())
}

/// Writes `enrollment.json` and every granted file (name -> content). A name is one of the
/// role's fixed grant names, never a path.
pub fn write(
    dir: &Path,
    meta: &Meta,
    files: &std::collections::BTreeMap<String, String>,
) -> Result<(), FileError> {
    private_dir(dir)?;
    let meta_json = serde_json::to_vec_pretty(meta).expect("meta serializes");
    write_private(&dir.join(META_FILE), &meta_json)?;
    for (name, content) in files {
        debug_assert!(!name.contains('/') && name != "..");
        write_private(&dir.join(name), content.as_bytes())?;
        if name == PSK_FILE {
            // A fresh key is generation 0; a generation left by an earlier life would make the
            // agent register a secret the controller no longer has.
            match std::fs::remove_file(psk_gen_path(&dir.join(name))) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                    return Err(io(&psk_gen_path(&dir.join(name)))(e))
                }
                _ => {}
            }
        }
    }
    Ok(())
}

pub fn read_meta(dir: &Path) -> Result<Meta, FileError> {
    let p = dir.join(META_FILE);
    let b = std::fs::read(&p).map_err(io(&p))?;
    serde_json::from_slice(&b).map_err(|e| FileError::Io {
        path: p,
        source: std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()),
    })
}

/// Reads a PSK file and checks it is base64 of exactly 32 bytes (what WireGuard takes).
pub fn read_psk(path: &Path) -> Result<Zeroizing<String>, FileError> {
    let text = Zeroizing::new(std::fs::read_to_string(path).map_err(io(path))?);
    let key = text.trim();
    let ok = base64::engine::general_purpose::STANDARD
        .decode(key)
        .is_ok_and(|b| b.len() == 32);
    if !ok {
        return Err(FileError::BadPsk {
            path: path.to_owned(),
        });
    }
    Ok(Zeroizing::new(key.to_owned()))
}

/// Where the generation of the key in `psk` is kept (`<psk file>.gen`), so a restart tells the
/// controller which key the agent holds.
pub fn psk_gen_path(psk: &Path) -> PathBuf {
    let mut p = psk.as_os_str().to_owned();
    p.push(".gen");
    p.into()
}

/// The generation recorded beside `psk`; 0 (the key enrollment made) when there is none.
pub fn read_psk_gen(psk: &Path) -> u64 {
    std::fs::read_to_string(psk_gen_path(psk))
        .ok()
        .and_then(|t| t.trim().parse().ok())
        .unwrap_or(0)
}

/// Replaces the key file with `key` (checked) in one rename, 0600, so a crash leaves the old
/// key or the new one and never half of either.
pub fn replace_psk(psk: &Path, key: &str) -> Result<(), FileError> {
    let ok = base64::engine::general_purpose::STANDARD
        .decode(key.trim())
        .is_ok_and(|b| b.len() == 32);
    if !ok {
        return Err(FileError::BadPsk {
            path: psk.to_owned(),
        });
    }
    replace_private(psk, key.trim().as_bytes())
}

/// Records the generation of the key now in `psk`. Written after the key itself.
pub fn write_psk_gen(psk: &Path, gen: u64) -> Result<(), FileError> {
    replace_private(&psk_gen_path(psk), gen.to_string().as_bytes())
}

fn replace_private(path: &Path, data: &[u8]) -> Result<(), FileError> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".new");
    let tmp = PathBuf::from(tmp);
    let written = write_private(&tmp, data)
        .and_then(|()| {
            std::fs::File::open(&tmp)
                .and_then(|f| f.sync_all())
                .map_err(io(&tmp))
        })
        .and_then(|()| std::fs::rename(&tmp, path).map_err(io(path)));
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn mode(p: &Path) -> u32 {
        std::fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn a_rotated_key_replaces_the_old_one_privately_and_its_generation_is_kept_beside_it() {
        let t = tempfile::tempdir().unwrap();
        let path = t.path().join(PSK_FILE);
        let (old, new) = (crate::generate_psk(), crate::generate_psk());
        replace_psk(&path, &old).unwrap();
        assert_eq!(read_psk_gen(&path), 0, "no record means the enrolled key");
        replace_psk(&path, &new).unwrap();
        write_psk_gen(&path, 3).unwrap();
        assert_eq!(*read_psk(&path).unwrap(), *new);
        // Enrolling again writes generation 0 over whatever was there.
        let meta = Meta {
            controller_url: "https://c".into(),
            name: "home".into(),
            role: Role::Agent,
            parent_url: None,
            node_id: None,
            tunnel_network: None,
            advertise: None,
        };
        let files = [(PSK_FILE.to_owned(), new.to_string())]
            .into_iter()
            .collect();
        write(t.path(), &meta, &files).unwrap();
        assert_eq!(read_psk_gen(&path), 0);
        write_psk_gen(&path, 3).unwrap();
        assert_eq!(read_psk_gen(&path), 3);
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(&psk_gen_path(&path)), 0o600);
        // A key that is not 32 base64 bytes never reaches the file, and nothing is left over.
        assert!(replace_psk(&path, "short").is_err());
        assert_eq!(*read_psk(&path).unwrap(), *new);
        let left: Vec<_> = std::fs::read_dir(t.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(left.len(), 3, "{left:?}");
    }

    #[test]
    fn files_are_private_and_the_psk_reads_back() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("node");
        let psk = crate::generate_psk();
        let meta = Meta {
            controller_url: "https://c".into(),
            name: "home".into(),
            role: Role::Agent,
            parent_url: None,
            node_id: None,
            tunnel_network: None,
            advertise: None,
        };
        let granted = [
            (TOKEN_FILE.to_owned(), "tok-tok-tok-tok-tok".to_owned()),
            (PSK_FILE.to_owned(), (*psk).clone()),
        ]
        .into();
        write(&dir, &meta, &granted).unwrap();
        assert_eq!(mode(&dir), 0o700);
        for f in [META_FILE, TOKEN_FILE, PSK_FILE] {
            assert_eq!(mode(&dir.join(f)), 0o600, "{f}");
        }
        assert_eq!(*read_psk(&dir.join(PSK_FILE)).unwrap(), *psk);
        assert_eq!(read_meta(&dir).unwrap(), meta);
    }

    #[test]
    fn an_enrolled_directory_is_not_enrolled_again() {
        let t = tempfile::tempdir().unwrap();
        assert!(check_fresh(t.path()).is_ok());
        std::fs::write(t.path().join(META_FILE), "{}").unwrap();
        assert!(check_fresh(t.path()).is_err());
    }

    #[test]
    fn a_loose_existing_directory_is_tightened_and_a_bad_psk_refused() {
        let t = tempfile::tempdir().unwrap();
        std::fs::set_permissions(t.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        let meta = Meta {
            controller_url: "https://c".into(),
            name: "n".into(),
            role: Role::Agent,
            parent_url: None,
            node_id: None,
            tunnel_network: None,
            advertise: None,
        };
        let granted = [(PSK_FILE.to_owned(), "not-a-key".to_owned())].into();
        write(t.path(), &meta, &granted).unwrap();
        assert_eq!(mode(t.path()), 0o700);
        assert!(matches!(
            read_psk(&t.path().join(PSK_FILE)),
            Err(FileError::BadPsk { .. })
        ));
    }
}
