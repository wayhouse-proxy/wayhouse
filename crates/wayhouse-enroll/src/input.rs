//! Where a node binary gets its code from. Order: `--code` (visible in `ps`, so the least
//! preferred), `--code-file`, `$WAYHOUSE_ENROLL_CODE_FILE`, `$WAYHOUSE_ENROLL_CODE`, then a
//! prompt on a terminal.

use std::io::{BufRead, IsTerminal, Write};
use std::path::Path;

use crate::{Code, CodeError};

#[derive(Debug, thiserror::Error)]
pub enum InputError {
    #[error("no enrollment code: pass --code-file, set WAYHOUSE_ENROLL_CODE_FILE or WAYHOUSE_ENROLL_CODE, or run on a terminal to be asked")]
    Missing,
    #[error("cannot read the code file {}: {}", .0.display(), .1)]
    File(std::path::PathBuf, std::io::Error),
    #[error(transparent)]
    Code(#[from] CodeError),
}

pub fn resolve(
    code: Option<&str>,
    code_file: Option<&Path>,
    env: impl Fn(&str) -> Option<String>,
    prompt: impl FnOnce() -> Option<String>,
) -> Result<Code, InputError> {
    let from_file =
        |p: &Path| std::fs::read_to_string(p).map_err(|e| InputError::File(p.to_owned(), e));
    let text = if let Some(c) = code {
        c.to_owned()
    } else if let Some(p) = code_file {
        from_file(p)?
    } else if let Some(p) = env("WAYHOUSE_ENROLL_CODE_FILE") {
        from_file(Path::new(&p))?
    } else if let Some(c) = env("WAYHOUSE_ENROLL_CODE") {
        c
    } else {
        prompt().ok_or(InputError::Missing)?
    };
    Ok(Code::parse(&text)?)
}

/// Asks on stderr when stdin is a terminal. shortcut: the code is echoed (a single-use code
/// that expires in minutes), upgrade to a no-echo read if codes ever live longer.
pub fn tty_prompt() -> Option<String> {
    let stdin = std::io::stdin();
    if !stdin.is_terminal() {
        return None;
    }
    eprint!("Enrollment code: ");
    std::io::stderr().flush().ok()?;
    let mut line = String::new();
    stdin.lock().read_line(&mut line).ok()?;
    Some(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn good() -> String {
        Code::generate().expose().to_string()
    }

    #[test]
    fn the_sources_are_tried_in_order() {
        let (a, b, c) = (good(), good(), good());
        let t = tempfile::tempdir().unwrap();
        let f = t.path().join("code");
        std::fs::write(&f, format!("{b}\n")).unwrap();
        let env_file = f.display().to_string();
        let env = |k: &str| match k {
            "WAYHOUSE_ENROLL_CODE_FILE" => Some(env_file.clone()),
            "WAYHOUSE_ENROLL_CODE" => Some(c.clone()),
            _ => None,
        };
        let id = |c: Code| c.id().to_owned();
        let pre = |s: &str| Code::parse(s).unwrap().id().to_owned();
        assert_eq!(
            id(resolve(Some(&a), Some(&f), env, || None).unwrap()),
            pre(&a)
        );
        assert_eq!(id(resolve(None, Some(&f), env, || None).unwrap()), pre(&b));
        assert_eq!(
            id(resolve(None, None, env, || None).unwrap()),
            pre(&b),
            "the file env wins over the plain env"
        );
        assert_eq!(
            id(resolve(
                None,
                None,
                |k| (k == "WAYHOUSE_ENROLL_CODE").then(|| c.clone()),
                || None
            )
            .unwrap()),
            pre(&c)
        );
        assert_eq!(
            id(resolve(None, None, |_| None, || Some(a.clone())).unwrap()),
            pre(&a)
        );
        assert!(matches!(
            resolve(None, None, |_| None, || None),
            Err(InputError::Missing)
        ));
        assert!(matches!(
            resolve(Some("nope"), None, |_| None, || None),
            Err(InputError::Code(_))
        ));
    }
}
