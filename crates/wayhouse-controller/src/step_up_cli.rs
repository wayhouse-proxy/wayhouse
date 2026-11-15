//! `wayhouse-controller step-up`: the operator's side of the signed step-up (#638).
//!
//! - `step-up init`: derives the key from a passphrase and writes only its public half (the salt
//!   and the verifying key) for `--enroll-step-up-pubkey-file`. The passphrase is never stored.
//! - `step-up sign`: prints the assertion for one mint request, for `curl` (the UI does the same
//!   in the browser flow). The passphrase and the salt it needs come from files, never from
//!   the command line.
//!
//! The passphrase is read from `--passphrase-file` (`-` is stdin); nothing secret is printed.

use std::io::Read;
use std::path::{Path, PathBuf};

use clap::{Args, Parser, Subcommand};
use wayhouse_enroll::step_up::{self, StepUpKey};
use zeroize::Zeroizing;

#[derive(Parser, Debug)]
#[command(
    name = "wayhouse-controller step-up",
    about = "Make the step-up key for --enroll-step-up-pubkey-file, or sign one enrollment mint"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Derive the key from a passphrase and write its public half.
    Init(InitArgs),
    /// Print the signed assertion for one `POST /enrollments` body.
    Sign(SignArgs),
}

#[derive(Args, Debug)]
struct Pass {
    /// File holding the step-up passphrase (at least 16 characters); `-` reads stdin.
    #[arg(long, value_name = "PATH")]
    passphrase_file: PathBuf,
}

#[derive(Args, Debug)]
struct InitArgs {
    #[command(flatten)]
    pass: Pass,
    /// Where to write the public key file. Refused if it exists.
    #[arg(long, value_name = "PATH")]
    out: PathBuf,
    /// Make a random passphrase and write it to `--passphrase-file` (0600; refused if it exists)
    /// instead of reading one. Prefer this: the key file allows offline guessing of a chosen one.
    #[arg(long)]
    generate: bool,
}

#[derive(Args, Debug)]
struct SignArgs {
    #[command(flatten)]
    pass: Pass,
    /// The public key file the controller was started with (it holds the salt).
    #[arg(long, value_name = "PATH")]
    pubkey_file: PathBuf,
    #[arg(long, default_value = "agent")]
    role: String,
    #[arg(long)]
    name: String,
    #[arg(long)]
    ttl_secs: Option<u64>,
    #[arg(long)]
    uses: Option<u64>,
    #[arg(long)]
    name_prefix: Option<String>,
    #[arg(long)]
    parent_url: Option<String>,
}

fn read_passphrase(path: &Path) -> Result<Zeroizing<String>, String> {
    let mut text = Zeroizing::new(String::new());
    if path == Path::new("-") {
        std::io::stdin()
            .read_to_string(&mut text)
            .map_err(|e| format!("stdin: {e}"))?;
    } else {
        text.push_str(
            &std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?,
        );
    }
    // A trailing newline from `echo` or an editor is not part of the passphrase.
    Ok(Zeroizing::new(
        text.trim_end_matches(['\n', '\r']).to_owned(),
    ))
}

/// Writes a random passphrase to a new 0600 file (never over an existing one) and returns it.
fn write_generated(path: &Path) -> Result<Zeroizing<String>, String> {
    use std::os::unix::fs::OpenOptionsExt;
    let pass = Zeroizing::new(wayhouse_enroll::generate_token());
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    std::io::Write::write_all(&mut f, pass.as_bytes())
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(pass)
}

/// Runs the subcommand and returns what to print on stdout.
fn run(args: impl IntoIterator<Item = std::ffi::OsString>) -> Result<String, String> {
    let cli = Cli::try_parse_from(
        std::iter::once(std::ffi::OsString::from("wayhouse-controller step-up")).chain(args),
    )
    .map_err(|e| e.to_string())?;
    match cli.cmd {
        Cmd::Init(a) => {
            let pass = if a.generate {
                // A refused `--out` must not leave a stray passphrase file behind.
                if a.out.exists() {
                    return Err(format!("{}: already exists", a.out.display()));
                }
                write_generated(&a.pass.passphrase_file)?
            } else {
                read_passphrase(&a.pass.passphrase_file)?
            };
            let key = StepUpKey::generate(&pass)?;
            // Created exclusively: an existing key file is never replaced under a running controller.
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&a.out)
                .map_err(|e| {
                    if a.generate {
                        let _ = std::fs::remove_file(&a.pass.passphrase_file);
                    }
                    format!("{}: {e}", a.out.display())
                })?;
            std::io::Write::write_all(&mut f, key.render().as_bytes())
                .map_err(|e| format!("{}: {e}", a.out.display()))?;
            Ok(format!(
                "wrote {} (the public key and salt; the passphrase is not stored). Start the \
                 controller with --enroll-step-up-pubkey-file {}.\n",
                a.out.display(),
                a.out.display()
            ))
        }
        Cmd::Sign(a) => {
            let pass = read_passphrase(&a.pass.passphrase_file)?;
            let text = std::fs::read_to_string(&a.pubkey_file)
                .map_err(|e| format!("{}: {e}", a.pubkey_file.display()))?;
            let key = StepUpKey::parse(&text)?;
            let signing = step_up::derive(&pass, &key.salt)?;
            if signing.verifying_key().to_bytes() != key.public {
                return Err("that passphrase does not match the controller's step-up key".into());
            }
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            let req = step_up::Request {
                role: &a.role,
                name: &a.name,
                ttl_secs: a.ttl_secs,
                uses: a.uses,
                name_prefix: a.name_prefix.as_deref(),
                parent_url: a.parent_url.as_deref(),
            };
            let assertion = step_up::sign(&signing, &req, now);
            serde_json::to_string(&assertion)
                .map(|j| format!("{j}\n"))
                .map_err(|e| e.to_string())
        }
    }
}

pub fn main(args: impl IntoIterator<Item = std::ffi::OsString>) -> anyhow::Result<()> {
    match run(args) {
        Ok(out) => {
            print!("{out}");
            Ok(())
        }
        Err(e) => Err(anyhow::anyhow!(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(v: &[&str]) -> Vec<std::ffi::OsString> {
        v.iter().map(Into::into).collect()
    }

    #[test]
    fn init_writes_only_the_public_half_and_sign_matches_it() {
        let d = tempfile::tempdir().unwrap();
        let pass = d.path().join("pass");
        std::fs::write(&pass, "a long enough step-up passphrase\n").unwrap();
        let out = d.path().join("pub");
        let pf = pass.to_str().unwrap();
        let o = out.to_str().unwrap();
        let said = run(os(&["init", "--passphrase-file", pf, "--out", o])).unwrap();
        assert!(!said.contains("passphrase-"), "{said}");
        let file = std::fs::read_to_string(&out).unwrap();
        assert!(file.starts_with("v1 "), "{file}");
        assert!(
            !file.contains("long enough"),
            "the passphrase is in the key file"
        );
        // A second init refuses to replace it.
        assert!(run(os(&["init", "--passphrase-file", pf, "--out", o])).is_err());
        let json = run(os(&[
            "sign",
            "--passphrase-file",
            pf,
            "--pubkey-file",
            o,
            "--role",
            "proxy",
            "--name",
            "edge-1",
        ]))
        .unwrap();
        let a: step_up::Assertion = serde_json::from_str(&json).unwrap();
        let key = StepUpKey::parse(&file).unwrap();
        let req = step_up::Request {
            role: "proxy",
            name: "edge-1",
            ttl_secs: None,
            uses: None,
            name_prefix: None,
            parent_url: None,
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        step_up::verify(&key.public, &req, &a, now).unwrap();
        // A different passphrase is named, not signed with.
        std::fs::write(&pass, "another passphrase entirely").unwrap();
        let e = run(os(&[
            "sign",
            "--passphrase-file",
            pf,
            "--pubkey-file",
            o,
            "--name",
            "edge-1",
        ]))
        .unwrap_err();
        assert!(e.contains("does not match"), "{e}");
    }

    #[test]
    fn generate_makes_a_private_passphrase_file_and_a_matching_key() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let pass = d.path().join("pass");
        let out = d.path().join("pub");
        let (pf, o) = (pass.to_str().unwrap(), out.to_str().unwrap());
        run(os(&[
            "init",
            "--generate",
            "--passphrase-file",
            pf,
            "--out",
            o,
        ]))
        .unwrap();
        assert_eq!(
            std::fs::metadata(&pass).unwrap().permissions().mode() & 0o777,
            0o600
        );
        // An existing `--out` is refused before any passphrase file is made.
        let kept = d.path().join("kept");
        std::fs::write(&kept, "x").unwrap();
        let stray = d.path().join("stray");
        assert!(run(os(&[
            "init",
            "--generate",
            "--passphrase-file",
            stray.to_str().unwrap(),
            "--out",
            kept.to_str().unwrap()
        ]))
        .is_err());
        assert!(!stray.exists());
        // It never replaces an existing passphrase file.
        let out2 = d.path().join("pub2");
        assert!(run(os(&[
            "init",
            "--generate",
            "--passphrase-file",
            pf,
            "--out",
            out2.to_str().unwrap()
        ]))
        .is_err());
        // The generated passphrase signs for the key that was written.
        run(os(&[
            "sign",
            "--passphrase-file",
            pf,
            "--pubkey-file",
            o,
            "--name",
            "n",
        ]))
        .unwrap();
    }

    #[test]
    fn a_short_passphrase_is_refused() {
        let d = tempfile::tempdir().unwrap();
        let pass = d.path().join("pass");
        std::fs::write(&pass, "short").unwrap();
        let e = run(os(&[
            "init",
            "--passphrase-file",
            pass.to_str().unwrap(),
            "--out",
            d.path().join("pub").to_str().unwrap(),
        ]))
        .unwrap_err();
        assert!(e.contains("at least 16"), "{e}");
    }
}
