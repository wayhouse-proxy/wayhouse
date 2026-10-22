//! Writing a bundle to `<dir>/<site>/`: temp file, flush, rename, so a reader sees the old file
//! or the new one and never half of one.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

use crate::client::Bundle;

pub const CHAIN_FILE: &str = "fullchain.pem";
pub const KEY_FILE: &str = "privkey.pem";
pub const META_FILE: &str = "meta.json";

/// A site name as the controller allows it. The name becomes a directory, so anything else
/// (`..`, a slash) is refused rather than trusted.
pub fn valid_site(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 63
        && !name.starts_with('-')
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// What `<dir>/<site>/` holds: the version and expiry recorded in `meta.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Local {
    pub version: u64,
    pub not_after: u64,
}

/// The installed copy, if `meta.json` is readable and both PEM files are present and look like
/// PEM. A missing or truncated file reads as "nothing installed", so the next pass rewrites it.
pub fn local_state(dir: &Path, site: &str) -> Option<Local> {
    let site_dir = dir.join(site);
    let text = fs::read(site_dir.join(META_FILE)).ok()?;
    let meta = serde_json::from_slice::<serde_json::Value>(&text).ok()?;
    let ends_with = |file: &str, marker: &str| pem_ends_with(&site_dir.join(file), marker);
    if !ends_with(CHAIN_FILE, "END CERTIFICATE") || !ends_with(KEY_FILE, "END ") {
        return None;
    }
    Some(Local {
        version: meta.get("version")?.as_u64()?,
        not_after: meta
            .get("not_after")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0),
    })
}

/// Whether the last bytes of the PEM file at `path` hold `marker`. Only the tail is read, so a
/// key is not copied into memory on every pass; a truncated file has no closing line there.
/// A read error, or trailing data past the last 256 bytes, counts as "not intact".
fn pem_ends_with(path: &Path, marker: &str) -> bool {
    use std::io::{Read, Seek, SeekFrom};
    const TAIL: u64 = 256;
    let tail = (|| {
        let mut file = File::open(path)?;
        let len = file.metadata()?.len();
        file.seek(SeekFrom::Start(len.saturating_sub(TAIL)))?;
        let mut buf = Vec::with_capacity(TAIL as usize);
        file.take(TAIL).read_to_end(&mut buf)?;
        Ok::<_, io::Error>(buf)
    })();
    tail.is_ok_and(|b| String::from_utf8_lossy(&b).contains(marker))
}

/// The version recorded in `<dir>/<site>/meta.json`, if the installed copy is intact.
pub fn local_version(dir: &Path, site: &str) -> Option<u64> {
    local_state(dir, site).map(|l| l.version)
}

fn invalid(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}

/// Writes `contents` to `<site_dir>/<name>` through a temp file in the same directory.
/// A private file is created `0600` from the start, so its content is never readable by others.
fn write_atomic(site_dir: &Path, name: &str, contents: &[u8], private: bool) -> io::Result<()> {
    let tmp = site_dir.join(format!(".{name}.tmp"));
    // A leftover from a crash may have another mode; start clean.
    match fs::remove_file(&tmp) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(if private { 0o600 } else { 0o644 });
    }
    #[cfg(not(unix))]
    let _ = private;
    let result = (|| {
        let mut file = options.open(&tmp)?;
        file.write_all(contents)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, site_dir.join(name))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// [`install_with`] without a hook.
pub fn install(dir: &Path, site: &str, bundle: &Bundle) -> io::Result<()> {
    install_with(dir, site, bundle, |_| {})
}

/// Writes the chain, then the key, then `meta.json`; `meta.json` is written last, so a copy
/// whose `meta.json` names a version has both files of that version. `renamed` is called with
/// each file name right after it replaced the old one.
pub fn install_with(
    dir: &Path,
    site: &str,
    bundle: &Bundle,
    mut renamed: impl FnMut(&str),
) -> io::Result<()> {
    if !valid_site(site) {
        return Err(invalid("not a valid site name"));
    }
    if !bundle.chain_pem.contains("BEGIN CERTIFICATE") {
        return Err(invalid("the chain is not a PEM certificate"));
    }
    if !bundle.key_pem.contains("PRIVATE KEY") {
        return Err(invalid("the key is not a PEM private key"));
    }
    let site_dir = dir.join(site);
    fs::create_dir_all(&site_dir)?;
    write_atomic(&site_dir, CHAIN_FILE, bundle.chain_pem.as_bytes(), false)?;
    renamed(CHAIN_FILE);
    write_atomic(&site_dir, KEY_FILE, bundle.key_pem.as_bytes(), true)?;
    renamed(KEY_FILE);
    let meta = serde_json::json!({
        "version": bundle.version,
        "not_after": bundle.not_after,
        "serial": bundle.serial,
        "untrusted": bundle.untrusted,
    });
    write_atomic(&site_dir, META_FILE, meta.to_string().as_bytes(), false)?;
    renamed(META_FILE);
    // Make the renames durable; some filesystems cannot sync a directory, which is fine.
    if let Ok(d) = File::open(&site_dir) {
        let _ = d.sync_all();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn bundle(version: u64) -> Bundle {
        Bundle {
            version,
            chain_pem: format!(
                "-----BEGIN CERTIFICATE-----\nchain{version}\n-----END CERTIFICATE-----\n"
            ),
            key_pem: format!(
                "-----BEGIN PRIVATE KEY-----\nkey{version}\n-----END PRIVATE KEY-----\n"
            )
            .into(),
            not_after: 2_000_000_000,
            serial: "0a".into(),
            untrusted: false,
        }
    }

    #[test]
    fn writes_the_layout_with_a_private_key() {
        let dir = tempfile::tempdir().unwrap();
        install(dir.path(), "web", &bundle(3)).unwrap();
        let site = dir.path().join("web");
        assert!(fs::read_to_string(site.join(CHAIN_FILE))
            .unwrap()
            .contains("chain3"));
        assert!(fs::read_to_string(site.join(KEY_FILE))
            .unwrap()
            .contains("key3"));
        let meta: serde_json::Value =
            serde_json::from_slice(&fs::read(site.join(META_FILE)).unwrap()).unwrap();
        assert_eq!(meta["version"], 3);
        assert_eq!(meta["not_after"], 2_000_000_000u64);
        assert_eq!(meta["serial"], "0a");
        assert_eq!(local_version(dir.path(), "web"), Some(3));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |f: &str| fs::metadata(site.join(f)).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(KEY_FILE), 0o600);
            assert_eq!(mode(CHAIN_FILE), 0o644);
        }
        let leftovers: Vec<_> = fs::read_dir(&site)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .filter(|n| n.starts_with('.'))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    #[test]
    fn replaces_chain_then_key_then_meta_and_keeps_old_files_until_each_rename() {
        let dir = tempfile::tempdir().unwrap();
        install(dir.path(), "web", &bundle(1)).unwrap();
        let site = dir.path().join("web");
        let read = |f: &str| fs::read_to_string(site.join(f)).unwrap();
        let mut order = Vec::new();
        install_with(dir.path(), "web", &bundle(2), |name| {
            order.push(name.to_string());
            match name {
                CHAIN_FILE => {
                    assert!(read(CHAIN_FILE).contains("chain2"));
                    assert!(
                        read(KEY_FILE).contains("key1"),
                        "the key is still the old one"
                    );
                    assert_eq!(local_version(dir.path(), "web"), Some(1));
                }
                KEY_FILE => {
                    assert!(read(KEY_FILE).contains("key2"));
                    assert_eq!(
                        local_version(dir.path(), "web"),
                        Some(1),
                        "meta.json is the commit marker and moves last"
                    );
                }
                _ => assert!(read(META_FILE).contains("\"version\":2")),
            }
        })
        .unwrap();
        assert_eq!(order, [CHAIN_FILE, KEY_FILE, META_FILE]);
    }

    #[test]
    fn a_stale_temp_file_with_a_loose_mode_does_not_leak_the_key() {
        let dir = tempfile::tempdir().unwrap();
        let site = dir.path().join("web");
        fs::create_dir_all(&site).unwrap();
        fs::write(site.join(".privkey.pem.tmp"), "old").unwrap();
        install(dir.path(), "web", &bundle(1)).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(site.join(KEY_FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn bad_site_names_and_non_pem_bundles_are_refused_before_anything_is_written() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["", "..", "a/b", "../x", "Web", "-x", &"a".repeat(64)] {
            assert!(install(dir.path(), name, &bundle(1)).is_err(), "{name}");
        }
        let mut no_chain = bundle(1);
        no_chain.chain_pem = "hello".into();
        let mut no_key = bundle(1);
        no_key.key_pem = "hello".to_string().into();
        assert!(install(dir.path(), "web", &no_chain).is_err());
        assert!(install(dir.path(), "web", &no_key).is_err());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn only_the_tail_of_a_pem_file_decides_whether_it_is_intact() {
        let dir = tempfile::tempdir().unwrap();
        install(dir.path(), "web", &bundle(1)).unwrap();
        let site = dir.path().join("web");
        // A long chain with its closing line at the end is intact.
        let long = format!(
            "-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----\n",
            "A".repeat(50_000)
        );
        fs::write(site.join(CHAIN_FILE), long).unwrap();
        assert_eq!(local_version(dir.path(), "web"), Some(1));
        // The closing line early in the file does not count once the end is cut off.
        let cut = format!(
            "-----BEGIN CERTIFICATE-----\nx\n-----END CERTIFICATE-----\n{}",
            "A".repeat(1_000)
        );
        fs::write(site.join(CHAIN_FILE), cut).unwrap();
        assert_eq!(local_version(dir.path(), "web"), None);
        // A key cut off mid-body reads as not installed.
        install(dir.path(), "web", &bundle(1)).unwrap();
        fs::write(site.join(KEY_FILE), "-----BEGIN PRIVATE KEY-----\nkey").unwrap();
        assert_eq!(local_version(dir.path(), "web"), None);
    }
}
