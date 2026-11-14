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
/// A controller replica's Raft node id, for `--ha-node-id-file`; written beside `enrollment.json`.
pub const NODE_ID_FILE: &str = "ha-node-id";

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

/// Creates `dir` (mode 0700) if missing; an existing directory is tightened to 0700. A
/// directory that is a symbolic link, or that another user owns, is refused: its owner could
/// read what is written into it (a shared `/tmp` with a pre-created directory).
fn private_dir(dir: &Path) -> Result<(), FileError> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(io(dir))?;
    let m = std::fs::symlink_metadata(dir).map_err(io(dir))?;
    let refuse = |why: &str| FileError::Io {
        path: dir.to_owned(),
        source: std::io::Error::new(std::io::ErrorKind::PermissionDenied, why.to_owned()),
    };
    if m.file_type().is_symlink() {
        return Err(refuse("is a symbolic link; use the real directory"));
    }
    if m.uid() != nix::unistd::geteuid().as_raw() {
        return Err(refuse(
            "is owned by another user; pick a directory of your own",
        ));
    }
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).map_err(io(dir))
}

/// Makes `dir` ready (see [`private_dir`]) and proves a file can be written there, so a
/// problem shows before the one-time code is spent.
pub fn prepare(dir: &Path) -> Result<(), FileError> {
    private_dir(dir)?;
    let probe = dir.join(".enroll-probe");
    write_private(&probe, b"")?;
    std::fs::remove_file(&probe).map_err(io(&probe))
}

/// Writes `data` to `path` with mode 0600 from the first byte (no window at the umask). The
/// bytes go to a new file beside it (never following a link) that then replaces `path`, so a
/// symbolic link or a loose file planted at `path` is replaced, not written through or reused.
fn write_private(path: &Path, data: &[u8]) -> Result<(), FileError> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    // A leftover from an earlier run; `remove_file` removes a link, not what it points at.
    match std::fs::remove_file(&tmp) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(io(&tmp)(e)),
        _ => {}
    }
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .custom_flags(nix::libc::O_NOFOLLOW)
        .mode(0o600)
        .open(&tmp)
        .map_err(io(&tmp))?;
    f.write_all(data).map_err(io(&tmp))?;
    std::fs::rename(&tmp, path).map_err(io(path))
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
    if let Some(id) = meta.node_id {
        write_private(&dir.join(NODE_ID_FILE), format!("{id}\n").as_bytes())?;
    }
    // Last: `enrollment.json` is what marks the directory enrolled (`check_fresh`,
    // `--skip-if-enrolled`), so a write that dies half way leaves a directory that enrolls again.
    let meta_json = serde_json::to_vec_pretty(meta).expect("meta serializes");
    write_private(&dir.join(META_FILE), &meta_json)
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
    fn a_controller_replica_gets_its_node_id_as_a_plain_file_and_others_get_none() {
        let t = tempfile::tempdir().unwrap();
        let mut meta = Meta {
            controller_url: "https://c".into(),
            name: "ctl-1".into(),
            role: Role::Controller,
            parent_url: None,
            node_id: Some(7),
            tunnel_network: None,
            advertise: None,
        };
        write(t.path(), &meta, &std::collections::BTreeMap::new()).unwrap();
        let p = t.path().join(NODE_ID_FILE);
        assert_eq!(std::fs::read_to_string(&p).unwrap().trim(), "7");
        assert_eq!(mode(&p), 0o600);
        let t = tempfile::tempdir().unwrap();
        meta.node_id = None;
        write(t.path(), &meta, &std::collections::BTreeMap::new()).unwrap();
        assert!(!t.path().join(NODE_ID_FILE).exists());
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
    fn a_write_that_fails_half_way_leaves_the_directory_unenrolled() {
        let t = tempfile::tempdir().unwrap();
        // A directory where a file must go makes the write of that file fail.
        std::fs::create_dir(t.path().join(TOKEN_FILE)).unwrap();
        let meta = Meta {
            controller_url: "https://c".into(),
            name: "n".into(),
            role: Role::Agent,
            parent_url: None,
            node_id: None,
            tunnel_network: None,
            advertise: None,
        };
        let granted = [(TOKEN_FILE.to_owned(), "t".to_owned())].into();
        assert!(write(t.path(), &meta, &granted).is_err());
        assert!(!t.path().join(META_FILE).exists());
        assert!(check_fresh(t.path()).is_ok());
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

    fn meta() -> Meta {
        Meta {
            controller_url: "https://c".into(),
            name: "n".into(),
            role: Role::Agent,
            parent_url: None,
            node_id: None,
            tunnel_network: None,
            advertise: None,
        }
    }

    #[test]
    fn a_planted_symlink_or_loose_file_is_replaced_not_followed_or_reused() {
        let t = tempfile::tempdir().unwrap();
        let victim = t.path().join("victim");
        std::fs::write(&victim, "keep me").unwrap();
        let dir = t.path().join("data");
        std::fs::create_dir(&dir).unwrap();
        std::os::unix::fs::symlink(&victim, dir.join("secret.keys")).unwrap();
        std::fs::write(dir.join("auth-token"), "old").unwrap();
        std::fs::set_permissions(
            dir.join("auth-token"),
            std::fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        let granted = [
            ("secret.keys".to_owned(), "k".to_owned()),
            ("auth-token".to_owned(), "t".to_owned()),
        ]
        .into();
        write(&dir, &meta(), &granted).unwrap();
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep me");
        for f in ["secret.keys", "auth-token"] {
            let m = std::fs::symlink_metadata(dir.join(f)).unwrap();
            assert!(m.file_type().is_file(), "{f} is a regular file again");
            assert_eq!(m.permissions().mode() & 0o777, 0o600, "{f}");
        }
    }

    #[test]
    fn a_data_directory_that_is_a_symlink_is_refused() {
        let t = tempfile::tempdir().unwrap();
        let real = t.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let link = t.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let e = write(&link, &meta(), &std::collections::BTreeMap::new()).unwrap_err();
        assert!(e.to_string().contains("symbolic link"), "{e}");
        assert!(!real.join(META_FILE).exists());
    }

    #[test]
    fn a_data_directory_owned_by_someone_else_is_refused() {
        if !nix::unistd::geteuid().is_root() {
            return; // chown to another user needs root
        }
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("data");
        std::fs::create_dir(&dir).unwrap();
        nix::unistd::chown(&dir, Some(nix::unistd::Uid::from_raw(54321)), None).unwrap();
        let e = write(&dir, &meta(), &std::collections::BTreeMap::new()).unwrap_err();
        assert!(e.to_string().contains("owned by"), "{e}");
    }

    #[test]
    fn prepare_proves_the_directory_is_writable_before_a_code_is_spent() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("fresh");
        prepare(&dir).unwrap();
        assert!(dir.is_dir());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0, "probe removed");
        // A file where the directory should be cannot be enrolled into.
        let file = t.path().join("file");
        std::fs::write(&file, "x").unwrap();
        assert!(prepare(&file.join("sub")).is_err());
    }
}
