//! The config file of a proxy that runs from `--config`, read and replaced through the admin API
//! (`GET` and `PUT /config/file`, #476). Reading is always on; replacing needs
//! `--allow-config-write`.
//!
//! A write is a compare-and-swap on the file's content: the caller names the etag it edited from
//! (`If-Match`), a changed file answers stale. The new text is validated like a config reload
//! would, written to a temp file in the same directory and renamed over the file, so a crash or a
//! full disk never leaves half a config; the previous file is kept as `<file>.prev`. Applying is
//! the ordinary reload (the admin handler asks for it); nothing here touches the running proxy.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use sha2::{Digest, Sha256};

/// The config file this proxy runs from, and whether the admin API may replace it.
#[derive(Clone)]
pub struct ConfigFile {
    pub path: PathBuf,
    pub allow_write: bool,
}

/// One write at a time: the compare, the backup and the rename are one step.
static WRITE_LOCK: Mutex<()> = Mutex::new(());

/// Why a write was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum WriteError {
    /// The file changed since the caller read it.
    Stale,
    /// The text is not a config this build runs.
    Invalid(String),
    /// The disk refused.
    Io(String),
}

/// The strong etag of some file content.
pub fn etag(bytes: &[u8]) -> String {
    format!("\"{}\"", hex(&Sha256::digest(bytes)))
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// The file's revision as the web editor knows it (`X-Config-Revision`, `?base=`): the first six
/// bytes of the content hash as a number a browser holds exactly. Same content, same revision.
pub fn revision(bytes: &[u8]) -> u64 {
    Sha256::digest(bytes)[..6]
        .iter()
        .fold(0u64, |n, b| (n << 8) | u64::from(*b))
}

/// What a write was edited from: the etag a read returned, or its revision number.
#[derive(Clone, Copy)]
pub enum Base<'a> {
    Etag(&'a str),
    Revision(u64),
}

impl Base<'_> {
    fn is(self, content: &[u8]) -> bool {
        match self {
            Base::Etag(tag) => normalise(tag).as_deref() == Some(etag(content).as_str()),
            Base::Revision(n) => n == revision(content),
        }
    }
}

/// `<file>.prev`, the copy a write keeps of the file it replaced.
pub fn prev_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".prev");
    PathBuf::from(name)
}

/// An `If-Match` value as an etag to compare: the quotes are optional, a weak tag never matches.
fn normalise(tag: &str) -> Option<String> {
    let t = tag.trim();
    if t.starts_with("W/") {
        return None;
    }
    Some(format!("\"{}\"", t.trim_matches('"')))
}

/// Replaces the file with `text` if it still has the content `base` names. Returns the new etag and revision.
pub fn write(path: &Path, base: Base<'_>, text: &str) -> Result<(String, u64), WriteError> {
    let _guard = WRITE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let current =
        std::fs::read(path).map_err(|e| WriteError::Io(format!("reading the config file: {e}")))?;
    if !base.is(&current) {
        return Err(WriteError::Stale);
    }
    let cfg = wayhouse_config::parse_str(text).map_err(|e| WriteError::Invalid(e.to_string()))?;
    cfg.check_secret_strength()
        .map_err(|e| WriteError::Invalid(e.to_string()))?;
    swap(path, &current, text).map_err(|e| WriteError::Io(e.to_string()))?;
    Ok((etag(text.as_bytes()), revision(text.as_bytes())))
}

fn swap(path: &Path, current: &[u8], text: &str) -> std::io::Result<()> {
    use std::io::Write;
    std::fs::write(prev_path(path), current)?;
    let dir = path
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    // A stale temp file of an earlier crash would make `create_new` fail for ever.
    let _ = std::fs::remove_file(&tmp);
    let result = (|| {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        f.set_permissions(std::fs::metadata(path)?.permissions())?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = "pools:\n  - name: p\n    targets: [\"127.0.0.1:1\"]\nlisteners:\n  - name: l\n    bind: \"127.0.0.1:0\"\n    pool: p\n";

    fn file(text: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wayhouse.yaml");
        std::fs::write(&path, text).unwrap();
        (dir, path)
    }

    #[test]
    fn a_write_replaces_the_file_keeps_the_old_one_and_names_the_new_etag() {
        let (_d, path) = file(GOOD);
        let newer = GOOD.replace("127.0.0.1:1", "127.0.0.1:2");
        let (new_tag, _) = write(&path, Base::Etag(&etag(GOOD.as_bytes())), &newer).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), newer);
        assert_eq!(std::fs::read_to_string(prev_path(&path)).unwrap(), GOOD);
        assert_eq!(new_tag, etag(newer.as_bytes()));
        // No temp file is left behind.
        let leftovers: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    #[test]
    fn a_write_from_an_old_read_is_stale_and_changes_nothing() {
        let (_d, path) = file(GOOD);
        let other = GOOD.replace("127.0.0.1:1", "127.0.0.1:3");
        std::fs::write(&path, &other).unwrap();
        let err = write(&path, Base::Etag(&etag(GOOD.as_bytes())), GOOD).unwrap_err();
        assert_eq!(err, WriteError::Stale);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), other);
        assert!(!prev_path(&path).exists());
        // A weak tag never matches, an unquoted one does.
        assert_eq!(
            write(
                &path,
                Base::Etag(&format!("W/{}", etag(other.as_bytes()))),
                GOOD
            ),
            Err(WriteError::Stale)
        );
        let bare = etag(other.as_bytes()).trim_matches('"').to_string();
        assert!(write(&path, Base::Etag(&bare), GOOD).is_ok());
    }

    #[test]
    fn a_config_this_build_refuses_is_never_written() {
        let (_d, path) = file(GOOD);
        let err = write(&path, Base::Etag(&etag(GOOD.as_bytes())), "listeners: nope").unwrap_err();
        assert!(matches!(err, WriteError::Invalid(_)), "{err:?}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), GOOD);
        assert!(!prev_path(&path).exists());
    }

    #[test]
    fn a_missing_file_is_an_io_error_not_a_create() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("none.yaml");
        assert!(matches!(
            write(&path, Base::Etag("\"x\""), GOOD),
            Err(WriteError::Io(_))
        ));
        assert!(!path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn the_file_keeps_its_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let (_d, path) = file(GOOD);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
        write(
            &path,
            Base::Etag(&etag(GOOD.as_bytes())),
            &GOOD.replace("127.0.0.1:1", "127.0.0.1:2"),
        )
        .unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o640
        );
    }

    #[test]
    fn a_revision_number_works_as_the_base_and_stays_within_what_a_browser_holds() {
        let (_d, path) = file(GOOD);
        let rev = revision(GOOD.as_bytes());
        assert!(rev < (1u64 << 53));
        assert_eq!(
            revision(GOOD.as_bytes()),
            rev,
            "same content, same revision"
        );
        assert_ne!(revision(b"other"), rev);
        assert_eq!(
            write(&path, Base::Revision(rev + 1), GOOD),
            Err(WriteError::Stale)
        );
        assert!(write(
            &path,
            Base::Revision(rev),
            &GOOD.replace("127.0.0.1:1", "127.0.0.1:2")
        )
        .is_ok());
    }
}
