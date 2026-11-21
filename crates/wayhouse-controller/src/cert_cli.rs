//! `wayhouse-controller cert`: issue or renew a leaf certificate under the CA that
//! `wayhouse-controller init` left in the TLS directory (`ca.pem`, `ca.key`).
//!
//! A leaf that already exists keeps its names (the new one is valid for the same names plus any
//! `--san`); a missing one is issued for `localhost`, the loopback addresses and the `--san`s,
//! like `init`. The key is always new. Nothing is written until every certificate has been made
//! and checked against the CA, and each file is replaced atomically, so a failed run changes
//! nothing and a reader never sees half a file. The servers re-read the pair on their own (every
//! 30 s); compose copies it again on the next `docker compose up`.

use std::io::Write;
use std::path::{Path, PathBuf};

use clap::{Parser, ValueEnum};
use rcgen::{Issuer, KeyPair};
use x509_parser::prelude::*;

use crate::init::{io, leaf, names, InitError, Leaf};

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum Which {
    Controller,
    Aggregator,
    /// The one certificate of `wayhouse-control` (controller, aggregator and UI in one process).
    Control,
}

impl Which {
    fn file(self) -> &'static str {
        match self {
            Which::Controller => "controller",
            Which::Aggregator => "aggregator",
            Which::Control => "control",
        }
    }
}

#[derive(Parser, Debug)]
#[command(
    name = "wayhouse-controller cert",
    about = "Issue or renew a leaf certificate under the CA made by `init` (the key is always new; existing names are kept)"
)]
pub struct CertArgs {
    /// Which certificate: `controller`, `aggregator` or `control` (repeatable).
    #[arg(value_enum, required = true)]
    pub which: Vec<Which>,

    /// The directory holding `ca.pem` and `ca.key` (what `init` wrote).
    #[arg(long, value_name = "DIR", default_value = "/etc/wayhouse/tls")]
    pub tls_dir: PathBuf,

    /// An extra DNS name or IP the certificate must be valid for (repeatable), added to the names
    /// an existing certificate already has.
    #[arg(long = "san", value_name = "NAME")]
    pub sans: Vec<String>,
}

fn pem_der(pem: &str) -> Result<Vec<u8>, String> {
    let (_, p) = parse_x509_pem(pem.as_bytes()).map_err(|e| e.to_string())?;
    Ok(p.contents)
}

/// The DNS names and IP addresses an existing certificate is valid for.
fn existing_names(cert_pem: &str) -> Result<Vec<String>, String> {
    let der = pem_der(cert_pem)?;
    let (_, cert) = parse_x509_certificate(&der).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    if let Ok(Some(san)) = cert.subject_alternative_name() {
        for n in &san.value.general_names {
            match n {
                GeneralName::DNSName(d) => out.push((*d).to_owned()),
                GeneralName::IPAddress(b) => {
                    if let Ok(a) = <[u8; 4]>::try_from(*b) {
                        out.push(std::net::Ipv4Addr::from(a).to_string());
                    } else if let Ok(a) = <[u8; 16]>::try_from(*b) {
                        out.push(std::net::Ipv6Addr::from(a).to_string());
                    }
                }
                _ => {}
            }
        }
    }
    Ok(out)
}

struct Made {
    which: Which,
    leaf: Leaf,
    names: Vec<String>,
    renewed: bool,
    expires: ::time::OffsetDateTime,
}

fn read(path: &Path) -> Result<String, InitError> {
    std::fs::read_to_string(path).map_err(io(path))
}

fn bad(path: &Path, what: impl std::fmt::Display) -> InitError {
    InitError::Io {
        path: path.to_owned(),
        source: std::io::Error::new(std::io::ErrorKind::InvalidData, what.to_string()),
    }
}

/// Makes every requested leaf without touching the directory.
fn make(args: &CertArgs) -> Result<Vec<Made>, InitError> {
    let ca_path = args.tls_dir.join("ca.pem");
    let key_path = args.tls_dir.join("ca.key");
    let ca_pem = read(&ca_path)?;
    let ca_key = KeyPair::from_pem(&read(&key_path)?).map_err(|e| bad(&key_path, e))?;
    let issuer = Issuer::from_ca_cert_pem(&ca_pem, &ca_key).map_err(|e| bad(&ca_path, e))?;
    let ca_der = pem_der(&ca_pem).map_err(|e| bad(&ca_path, e))?;
    let (_, ca) = parse_x509_certificate(&ca_der).map_err(|e| bad(&ca_path, e))?;
    let until = ca.validity().not_after.to_datetime();
    if until <= ::time::OffsetDateTime::now_utc() {
        return Err(bad(&ca_path, "the CA has expired; start over with `init`"));
    }
    let mut made = Vec::new();
    for &which in &args.which {
        if made.iter().any(|m: &Made| m.which == which) {
            continue;
        }
        let cert_path = args.tls_dir.join(format!("{}.pem", which.file()));
        let old = match std::fs::read_to_string(&cert_path) {
            Ok(p) => Some(existing_names(&p).map_err(|e| bad(&cert_path, e))?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(io(&cert_path)(e)),
        };
        let renewed = old.is_some();
        let own = old.unwrap_or_default();
        let sans = names(&own, &args.sans);
        let l = leaf(
            &format!("wayhouse-{}", which.file()),
            &sans,
            &issuer,
            "--san",
            Some(until),
        )?;
        // The CA key must really be the one behind ca.pem, or the result would not verify.
        let der = pem_der(&l.cert_pem).map_err(|e| bad(&cert_path, e))?;
        let (_, c) = parse_x509_certificate(&der).map_err(|e| bad(&cert_path, e))?;
        c.verify_signature(Some(ca.public_key()))
            .map_err(|_| bad(&key_path, "ca.key is not the key of ca.pem"))?;
        let expires = c.validity().not_after.to_datetime();
        made.push(Made {
            which,
            leaf: l,
            names: sans,
            renewed,
            expires,
        });
    }
    Ok(made)
}

/// Writes `data` to `path` through a sibling file and a rename, with `mode`.
fn replace(path: &Path, mode: u32, data: &[u8]) -> Result<(), InitError> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".new");
    let tmp = PathBuf::from(tmp);
    let _ = std::fs::remove_file(&tmp);
    let result = (|| {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(&tmp)
            .map_err(io(&tmp))?;
        f.write_all(data).map_err(io(&tmp))?;
        f.sync_all().map_err(io(&tmp))?;
        std::fs::rename(&tmp, path).map_err(io(path))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Runs the command; `summary` gets the result, which never contains a secret value.
pub fn run(args: &CertArgs, summary: &mut dyn Write) -> Result<(), InitError> {
    let made = make(args)?;
    for m in &made {
        let f = m.which.file();
        // Key first: a reader that sees the new certificate always finds its key already there.
        replace(
            &args.tls_dir.join(format!("{f}.key")),
            0o600,
            m.leaf.key_pem.as_bytes(),
        )?;
        replace(
            &args.tls_dir.join(format!("{f}.pem")),
            0o644,
            m.leaf.cert_pem.as_bytes(),
        )?;
        let _ = writeln!(
            summary,
            "{} {f}.pem for {} (valid until {}); fingerprint (public; `wayhouse-agent enroll --pin`): {}",
            if m.renewed { "renewed" } else { "issued" },
            m.names.join(", "),
            m.expires.date(),
            m.leaf.fingerprint,
        );
    }
    let _ = writeln!(
        summary,
        "the servers pick the new pair up within 30 s; anything pinned to the old fingerprint needs the new one"
    );
    Ok(())
}

pub fn main(args: impl IntoIterator<Item = std::ffi::OsString>) -> anyhow::Result<()> {
    let parsed = CertArgs::parse_from(
        std::iter::once(std::ffi::OsString::from("wayhouse-controller cert")).chain(args),
    );
    run(&parsed, &mut std::io::stdout().lock()).map_err(anyhow::Error::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::init::{self, InitArgs, Target};
    use std::os::unix::fs::PermissionsExt;

    fn init_into(dir: &Path, control: bool, sans: &[&str]) {
        let args = InitArgs {
            target: Target::Systemd,
            dir: Some(dir.to_owned()),
            env_file: None,
            credentials_dir: None,
            tls_dir: None,
            sans: sans.iter().map(|s| (*s).to_owned()).collect(),
            namespace: "wayhouse".into(),
            output: None,
            control,
        };
        init::run(&args, &mut Vec::new(), false, &mut Vec::new()).unwrap();
    }

    fn args(dir: &Path, which: &[Which], sans: &[&str]) -> CertArgs {
        CertArgs {
            which: which.to_vec(),
            tls_dir: dir.join("tls"),
            sans: sans.iter().map(|s| (*s).to_owned()).collect(),
        }
    }

    fn names_of(path: &Path) -> Vec<String> {
        existing_names(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn renewing_replaces_the_pair_keeps_the_names_and_chains_to_the_same_ca() {
        let d = tempfile::tempdir().unwrap();
        init_into(d.path(), false, &["ctl.example.org"]);
        let tls = d.path().join("tls");
        let (old_cert, old_key) = (
            std::fs::read(tls.join("controller.pem")).unwrap(),
            std::fs::read(tls.join("controller.key")).unwrap(),
        );
        let ca = std::fs::read(tls.join("ca.pem")).unwrap();

        let mut out = Vec::new();
        run(
            &args(d.path(), &[Which::Controller], &["10.9.8.7"]),
            &mut out,
        )
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("renewed controller.pem") && text.contains("sha256:"),
            "{text}"
        );

        assert_ne!(std::fs::read(tls.join("controller.pem")).unwrap(), old_cert);
        assert_ne!(std::fs::read(tls.join("controller.key")).unwrap(), old_key);
        assert_eq!(
            std::fs::read(tls.join("ca.pem")).unwrap(),
            ca,
            "the CA stays"
        );
        let n = names_of(&tls.join("controller.pem"));
        for want in [
            "localhost",
            "127.0.0.1",
            "::1",
            "ctl.example.org",
            "10.9.8.7",
        ] {
            assert!(n.iter().any(|x| x == want), "{want} missing from {n:?}");
        }
        let mode = |f: &str| std::fs::metadata(tls.join(f)).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            (mode("controller.key"), mode("controller.pem")),
            (0o600, 0o644)
        );
        assert!(!tls.join("controller.key.new").exists());

        // It verifies against the CA, which `make` already checks; run it again to be sure a
        // second renewal over the renewed file works too.
        run(&args(d.path(), &[Which::Controller], &[]), &mut Vec::new()).unwrap();
        assert!(names_of(&tls.join("controller.pem")).contains(&"10.9.8.7".to_owned()));
    }

    #[test]
    fn a_missing_leaf_is_issued_and_a_bad_ca_key_changes_nothing() {
        let d = tempfile::tempdir().unwrap();
        init_into(d.path(), true, &[]);
        let tls = d.path().join("tls");
        assert!(!tls.join("aggregator.pem").exists());
        run(
            &args(d.path(), &[Which::Aggregator], &["aggregator"]),
            &mut Vec::new(),
        )
        .unwrap();
        assert!(names_of(&tls.join("aggregator.pem")).contains(&"aggregator".to_owned()));
        assert!(tls.join("aggregator.key").exists());

        // A key that is not the CA's: refused before anything is written.
        let other = KeyPair::generate().unwrap().serialize_pem();
        std::fs::write(tls.join("ca.key"), other).unwrap();
        let before = std::fs::read(tls.join("control.pem")).unwrap();
        let err = run(&args(d.path(), &[Which::Control], &[]), &mut Vec::new()).unwrap_err();
        assert!(err.to_string().contains("ca.key"), "{err}");
        assert_eq!(std::fs::read(tls.join("control.pem")).unwrap(), before);
    }

    #[test]
    fn an_invalid_name_or_a_missing_ca_is_refused_without_writing() {
        let d = tempfile::tempdir().unwrap();
        let err = run(&args(d.path(), &[Which::Controller], &[]), &mut Vec::new()).unwrap_err();
        assert!(err.to_string().contains("ca.pem"), "{err}");
        init_into(d.path(), false, &[]);
        let before = std::fs::read(d.path().join("tls/controller.pem")).unwrap();
        let err = run(
            &args(d.path(), &[Which::Controller], &["bad name"]),
            &mut Vec::new(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("bad name"), "{err}");
        assert_eq!(
            std::fs::read(d.path().join("tls/controller.pem")).unwrap(),
            before
        );
    }
}
