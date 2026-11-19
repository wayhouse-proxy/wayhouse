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
    /// The directory or file cannot be written (a read-only mount, a permission).
    NotWritable,
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

/// Replaces the file with `text` if it still has the content `base` names. `text` may carry the
/// masks [`redact`] put in a read (the file's own values come back), and is checked the way a
/// reload parses it: with the plugin routes merged in. Returns the new etag and revision.
pub fn write(path: &Path, base: Base<'_>, text: &str) -> Result<(String, u64), WriteError> {
    let _guard = WRITE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let current =
        std::fs::read(path).map_err(|e| WriteError::Io(format!("reading the config file: {e}")))?;
    if !base.is(&current) {
        return Err(WriteError::Stale);
    }
    let text = restore(text, &String::from_utf8_lossy(&current))?;
    let cfg = wayhouse_config::parse_str(&crate::plugin_overlay::merge(&text))
        .map_err(|e| WriteError::Invalid(e.to_string()))?;
    cfg.check_secret_strength()
        .map_err(|e| WriteError::Invalid(e.to_string()))?;
    // `--allow-config-write` needs the admin API closed to strangers (the start refuses without a
    // token); a save that removes the token would lock the proxy out at its next start.
    if cfg.admin_auth_token.is_none() {
        return Err(WriteError::Invalid(
            "settings.admin.auth_token is required while --allow-config-write is on".into(),
        ));
    }
    swap(path, &current, &text).map_err(|e| {
        if matches!(
            e.kind(),
            std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::ReadOnlyFilesystem
        ) {
            WriteError::NotWritable
        } else {
            WriteError::Io(e.to_string())
        }
    })?;
    Ok((etag(text.as_bytes()), revision(text.as_bytes())))
}

/// Writes `bytes` to a new temp file next to `path`, with `path`'s permissions, and syncs it.
fn stage(path: &Path, tag: &str, bytes: &[u8]) -> std::io::Result<PathBuf> {
    use std::io::Write;
    let dir = path
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let tmp = dir.join(format!(".{name}.{}.{tag}.tmp", std::process::id()));
    // A stale temp file of an earlier crash would make `create_new` fail for ever.
    let _ = std::fs::remove_file(&tmp);
    let result = (|| {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        f.set_permissions(std::fs::metadata(path)?.permissions())?;
        f.write_all(bytes)?;
        f.sync_all()
    })();
    match result {
        Ok(()) => Ok(tmp),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// New file and previous file are both staged before either name changes, so a full disk or a
/// permission error leaves the config and its `.prev` exactly as they were.
fn swap(path: &Path, current: &[u8], text: &str) -> std::io::Result<()> {
    let new = stage(path, "new", text.as_bytes())?;
    let prev = match stage(path, "prev", current) {
        Ok(p) => p,
        Err(e) => {
            let _ = std::fs::remove_file(&new);
            return Err(e);
        }
    };
    let result = std::fs::rename(&prev, prev_path(path)).and_then(|()| std::fs::rename(&new, path));
    for tmp in [&new, &prev] {
        let _ = std::fs::remove_file(tmp);
    }
    result
}

const TOKEN_MASK: &str = "__wayhouse_redacted_admin_token__";
const PSK_MASK: &str = "__wayhouse_redacted_gossip_psk__";

/// The inline secrets of a config text, each with its mask. A reference (`{ secret: name }`) is not
/// a secret in the file and is left alone; text that does not parse has none to find.
fn inline_secrets(text: &str) -> Vec<(&'static str, String)> {
    let Ok(cfg) = wayhouse_config::parse_str(text) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    if let Some(wayhouse_config::SecretOr::Plain(v)) = &cfg.admin_auth_token {
        found.push((TOKEN_MASK, v.clone()));
    }
    if let Some(g) = &cfg.gossip {
        if let wayhouse_config::SecretOr::Plain(v) = &g.psk {
            found.push((PSK_MASK, v.clone()));
        }
    }
    found.retain(|(_, v)| !v.is_empty());
    found
}

/// `text` with its inline secrets replaced by masks, for a read. Text that does not parse is
/// masked line by line (`auth_token:` and `psk:` values that are not a reference), so a broken
/// file never reads back with a secret in it.
pub fn redact(text: &str) -> String {
    if wayhouse_config::parse_str(text).is_err() {
        return text
            .lines()
            .map(|l| redact_line(l).unwrap_or_else(|| l.to_string()))
            .collect::<Vec<_>>()
            .join("\n")
            + if text.ends_with('\n') { "\n" } else { "" };
    }
    inline_secrets(text)
        .into_iter()
        .fold(text.to_string(), |t, (mask, v)| t.replace(&v, mask))
}

fn redact_line(line: &str) -> Option<String> {
    let (key, mask) = [("auth_token", TOKEN_MASK), ("psk", PSK_MASK)]
        .into_iter()
        .find(|(k, _)| {
            line.trim_start()
                .trim_start_matches("- ")
                .starts_with(&format!("{k}:"))
        })?;
    let at = line.find(&format!("{key}:"))? + key.len() + 1;
    let value = line[at..].trim();
    if value.is_empty() || value.starts_with('{') || value.starts_with('#') {
        return None;
    }
    Some(format!("{} {mask}", &line[..at]))
}

/// A YAML alias (`*name`) anywhere in `text`.
fn has_alias(text: &str) -> bool {
    text.as_bytes().windows(2).enumerate().any(|(i, w)| {
        w[0] == b'*'
            && w[1].is_ascii_alphanumeric()
            && (i == 0 || b" \t[{,:-".contains(&text.as_bytes()[i - 1]))
    })
}

/// `text` (an edit of a redacted read) with each mask replaced by the value `current` holds. A
/// mask counts only as the whole value of its own field (`admin.auth_token`, `gossip.psk`), once:
/// anywhere else it would copy a secret into a field the caller can read back. A mask that has no
/// secret to stand for (the file holds a reference, or does not parse) is refused too, since it
/// would otherwise be saved as the secret itself.
fn restore(text: &str, current: &str) -> Result<String, WriteError> {
    let bad = |m: &str| WriteError::Invalid(m.to_string());
    let masks = [TOKEN_MASK, PSK_MASK];
    if !masks.iter().any(|m| text.contains(m)) {
        return Ok(text.to_string());
    }
    let cfg = wayhouse_config::parse_str(&crate::plugin_overlay::merge(text))
        .map_err(|e| WriteError::Invalid(e.to_string()))?;
    let is_mask = |f: Option<&wayhouse_config::SecretOr>, m: &str| matches!(f, Some(wayhouse_config::SecretOr::Plain(v)) if v == m);
    let psk = cfg.gossip.as_ref().map(|g| &g.psk);
    let fields = [
        is_mask(cfg.admin_auth_token.as_ref(), TOKEN_MASK),
        is_mask(psk, PSK_MASK),
    ];
    for (mask, in_field) in masks.iter().zip(fields) {
        let n = text.matches(mask).count();
        if n != usize::from(in_field) || (n > 0 && has_alias(text)) {
            return Err(bad(
                "a redacted-secret placeholder may only be the whole value of its own field (admin.auth_token or gossip.psk)",
            ));
        }
    }
    let known = inline_secrets(current);
    let mut out = text.to_string();
    for (mask, v) in &known {
        out = out.replace(mask, v);
    }
    if masks.iter().any(|m| out.contains(m)) {
        return Err(bad(
            "a redacted-secret placeholder has no secret to stand for in the current file; write the value in",
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = "settings:\n  admin:\n    auth_token: \"0123456789abcdef0123\"\npools:\n  - name: p\n    targets: [\"127.0.0.1:1\"]\nlisteners:\n  - name: l\n    bind: \"127.0.0.1:0\"\n    pool: p\n";

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

    #[test]
    fn a_read_hides_inline_secrets_and_a_save_of_that_text_keeps_them() {
        let (_d, path) = file(GOOD);
        let shown = redact(GOOD);
        assert!(!shown.contains("0123456789abcdef0123"), "{shown}");
        assert!(shown.contains(TOKEN_MASK));
        // The editor saves what it was shown, with an unrelated edit.
        let edited = shown.replace("127.0.0.1:1", "127.0.0.1:2");
        write(&path, Base::Etag(&etag(GOOD.as_bytes())), &edited).unwrap();
        let on_disk = std::fs::read_to_string(&path).unwrap();
        assert!(on_disk.contains("0123456789abcdef0123"), "{on_disk}");
        assert!(!on_disk.contains(TOKEN_MASK));
        assert!(on_disk.contains("127.0.0.1:2"));
        // A reference is not a secret in the file: nothing to hide.
        let by_ref = GOOD.replace("\"0123456789abcdef0123\"", "{ secret: admin-token }");
        assert_eq!(redact(&by_ref), by_ref);
    }

    const PSK_FILE: &str = "settings:\n  admin:\n    auth_token: \"0123456789abcdef0123\"\n  gossip:\n    bind: \"127.0.0.1:7946\"\n    psk: \"fedcba9876543210fedcba\"\npools:\n  - name: p\n    targets: [\"127.0.0.1:1\"]\nlisteners:\n  - name: l\n    bind: \"127.0.0.1:0\"\n    pool: p\n";

    #[test]
    fn a_mask_outside_its_own_field_is_refused_and_leaks_nothing() {
        let (_d, path) = file(PSK_FILE);
        let base = etag(PSK_FILE.as_bytes());
        let shown = redact(PSK_FILE);
        assert!(
            shown.contains(PSK_MASK) && shown.contains(TOKEN_MASK),
            "{shown}"
        );
        for bad in [
            shown
                .replace("name: p\n", &format!("name: {PSK_MASK}\n"))
                .replace("pool: p", &format!("pool: {PSK_MASK}")),
            shown
                .replace("name: p\n", &format!("name: {PSK_MASK}\n"))
                .replace("pool: p", &format!("pool: {PSK_MASK}"))
                .replace(
                    &format!("psk: \"{PSK_MASK}\""),
                    &format!("psk: \"{TOKEN_MASK}\""),
                ),
        ] {
            assert!(matches!(
                write(&path, Base::Etag(&base), &bad),
                Err(WriteError::Invalid(_))
            ));
        }
        let swapped = shown
            .replace(&format!("psk: \"{PSK_MASK}\""), "psk: __T__")
            .replace(
                &format!("auth_token: \"{TOKEN_MASK}\""),
                &format!("auth_token: \"{PSK_MASK}\""),
            )
            .replace("psk: __T__", &format!("psk: \"{TOKEN_MASK}\""));
        assert!(matches!(
            write(&path, Base::Etag(&base), &swapped),
            Err(WriteError::Invalid(_))
        ));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), PSK_FILE);
    }

    #[test]
    fn a_mask_with_no_secret_behind_it_is_never_saved_as_the_secret() {
        let by_ref = GOOD.replace("\"0123456789abcdef0123\"", "{ secret: admin-token }");
        let (_d, path) = file(&by_ref);
        let masked = GOOD.replace("0123456789abcdef0123", TOKEN_MASK);
        let err = write(&path, Base::Etag(&etag(by_ref.as_bytes())), &masked).unwrap_err();
        assert!(matches!(err, WriteError::Invalid(_)), "{err:?}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), by_ref);
    }

    #[test]
    fn a_file_that_does_not_parse_still_reads_without_its_secrets() {
        let broken = format!("{GOOD}\nlisteners: [\n");
        let shown = redact(&broken);
        assert!(!shown.contains("0123456789abcdef0123"), "{shown}");
        assert!(shown.contains(TOKEN_MASK));
        let by_ref = "settings:\n  admin:\n    auth_token: { secret: t }\nx: [\n";
        assert_eq!(redact(by_ref), by_ref);
    }

    #[test]
    fn a_save_that_drops_the_admin_token_is_refused() {
        let (_d, path) = file(GOOD);
        let no_token = "pools:\n  - name: p\n    targets: [\"127.0.0.1:1\"]\nlisteners:\n  - name: l\n    bind: \"127.0.0.1:0\"\n    pool: p\n";
        let err = write(&path, Base::Etag(&etag(GOOD.as_bytes())), no_token).unwrap_err();
        assert!(
            matches!(&err, WriteError::Invalid(m) if m.contains("auth_token")),
            "{err:?}"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), GOOD);
    }

    #[cfg(unix)]
    #[test]
    fn the_previous_file_has_the_mode_of_the_file_it_copies() {
        use std::os::unix::fs::PermissionsExt;
        let (_d, path) = file(GOOD);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        write(
            &path,
            Base::Etag(&etag(GOOD.as_bytes())),
            &GOOD.replace("127.0.0.1:1", "127.0.0.1:2"),
        )
        .unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&prev_path(&path)), 0o600);
        assert_eq!(mode(&path), 0o600);
    }
}
