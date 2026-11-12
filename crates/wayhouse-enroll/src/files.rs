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

/// `enrollment.json`: where this node enrolled and as whom. Holds no secret.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Meta {
    pub controller_url: String,
    pub name: String,
    pub role: Role,
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

pub fn write(
    dir: &Path,
    meta: &Meta,
    agent_token: Option<&str>,
    psk: &str,
) -> Result<(), FileError> {
    private_dir(dir)?;
    let meta_json = serde_json::to_vec_pretty(meta).expect("meta serializes");
    write_private(&dir.join(META_FILE), &meta_json)?;
    if let Some(t) = agent_token {
        write_private(&dir.join(TOKEN_FILE), t.as_bytes())?;
    }
    write_private(&dir.join(PSK_FILE), psk.as_bytes())
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

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn mode(p: &Path) -> u32 {
        std::fs::metadata(p).unwrap().permissions().mode() & 0o777
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
        };
        write(&dir, &meta, Some("tok-tok-tok-tok-tok"), &psk).unwrap();
        assert_eq!(mode(&dir), 0o700);
        for f in [META_FILE, TOKEN_FILE, PSK_FILE] {
            assert_eq!(mode(&dir.join(f)), 0o600, "{f}");
        }
        assert_eq!(*read_psk(&dir.join(PSK_FILE)).unwrap(), *psk);
        assert_eq!(read_meta(&dir).unwrap(), meta);
    }

    #[test]
    fn a_loose_existing_directory_is_tightened_and_a_bad_psk_refused() {
        let t = tempfile::tempdir().unwrap();
        std::fs::set_permissions(t.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        let meta = Meta {
            controller_url: "https://c".into(),
            name: "n".into(),
            role: Role::Agent,
        };
        write(t.path(), &meta, Some("tok-tok-tok-tok-tok"), "not-a-key").unwrap();
        assert_eq!(mode(t.path()), 0o700);
        assert!(matches!(
            read_psk(&t.path().join(PSK_FILE)),
            Err(FileError::BadPsk { .. })
        ));
    }
}
