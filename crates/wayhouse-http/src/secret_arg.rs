//! Where a binary gets a token or password from, so it never has to sit on the command line
//! (visible in `ps`, `/proc/<pid>/cmdline` and `docker inspect`).
//!
//! The lookup order for a secret called `name` (for example `auth-token`) is:
//! 1. the file named by `--<name>-file`,
//! 2. `$CREDENTIALS_DIRECTORY/<name>` (what systemd's `LoadCredential=` provides),
//! 3. the environment variable `WAYHOUSE_<NAME>` with `-` as `_` (for example
//!    `WAYHOUSE_AUTH_TOKEN`).
//!
//! A file's trailing newline is dropped; an empty secret is an error, never "no secret", so a
//! mis-mounted file cannot silently turn authentication off.

use std::path::Path;

/// The secret `name`, or `None` when no source provides one.
pub fn resolve(name: &str, file: Option<&Path>) -> Result<Option<String>, String> {
    resolve_with(
        name,
        file,
        std::env::var_os("CREDENTIALS_DIRECTORY")
            .as_deref()
            .map(Path::new),
        |k| std::env::var(k).ok(),
    )
}

fn resolve_with(
    name: &str,
    file: Option<&Path>,
    creds_dir: Option<&Path>,
    env: impl Fn(&str) -> Option<String>,
) -> Result<Option<String>, String> {
    if let Some(path) = file {
        return read(name, path).map(Some);
    }
    if let Some(dir) = creds_dir {
        let path = dir.join(name);
        if path.is_file() {
            return read(name, &path).map(Some);
        }
    }
    let var = format!("WAYHOUSE_{}", name.to_uppercase().replace('-', "_"));
    match env(&var) {
        Some(v) => clean(name, &var, v).map(Some),
        None => Ok(None),
    }
}

fn read(name: &str, path: &Path) -> Result<String, String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("--{name}-file {}: {e}", path.display()))?;
    clean(name, &path.display().to_string(), raw)
}

fn clean(name: &str, source: &str, mut v: String) -> Result<String, String> {
    while v.ends_with('\n') || v.ends_with('\r') {
        v.pop();
    }
    if v.is_empty() {
        return Err(format!("--{name} from {source} is empty"));
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn a_file_wins_and_loses_its_trailing_newline() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("t");
        std::fs::write(&f, "from-file-0123456789\n").unwrap();
        let got = resolve_with("auth-token", Some(&f), Some(d.path()), |_| {
            Some("env".into())
        });
        assert_eq!(got.unwrap().as_deref(), Some("from-file-0123456789"));
    }

    #[test]
    fn the_credentials_directory_comes_before_the_environment() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("auth-token"), "from-creds-0123456789").unwrap();
        let got = resolve_with("auth-token", None, Some(d.path()), |_| Some("env".into()));
        assert_eq!(got.unwrap().as_deref(), Some("from-creds-0123456789"));
        // A credentials directory without this secret falls through to the environment.
        let other = tempfile::tempdir().unwrap();
        let got = resolve_with("auth-token", None, Some(other.path()), |k| {
            (k == "WAYHOUSE_AUTH_TOKEN").then(|| "from-env-0123456789".into())
        });
        assert_eq!(got.unwrap().as_deref(), Some("from-env-0123456789"));
    }

    #[test]
    fn nothing_set_is_none_but_empty_and_missing_files_are_errors() {
        assert_eq!(resolve_with("auth-token", None, None, no_env), Ok(None));
        let d = tempfile::tempdir().unwrap();
        let empty = d.path().join("e");
        std::fs::write(&empty, "\n").unwrap();
        assert!(resolve_with("auth-token", Some(&empty), None, no_env).is_err());
        assert!(resolve_with("auth-token", Some(&d.path().join("missing")), None, no_env).is_err());
        assert!(resolve_with("auth-token", None, None, |_| Some(String::new())).is_err());
    }
}
