//! The `enroll` and `install` subcommands every node binary shares. A binary calls
//! [`is_node_cmd`] on its raw arguments before it parses its own flags; when it matches, [`run`]
//! does the work and the binary exits.

use std::ffi::OsString;
use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use crate::client::{parse_fingerprint, Pin};
use crate::install::{install_systemd, Install};
use crate::Role;

type BoxErr = Box<dyn std::error::Error + Send + Sync>;

#[derive(Args, Debug)]
pub struct EnrollArgs {
    /// Directory to write the node's files into (mode 0700). Default `wayhouse-<role>-data`.
    #[arg(long)]
    pub data_dir: Option<PathBuf>,
    /// Base URL of the controller. Must be `https://` (plain `http://` only to loopback).
    #[arg(long)]
    pub controller_url: String,
    /// The controller's TLS leaf certificate as `sha256:<hex>` (what the minting UI shows).
    #[arg(long, value_name = "SHA256", conflicts_with = "controller_ca")]
    pub pin: Option<String>,
    /// PEM file of the CA that signed the controller's certificate, trusted instead of the
    /// built-in roots.
    #[arg(long, value_name = "PATH")]
    pub controller_ca: Option<PathBuf>,
    /// The code itself. Visible in `ps`; prefer `--code-file` or the prompt.
    #[arg(long)]
    pub code: Option<String>,
    /// File holding the code (or set `WAYHOUSE_ENROLL_CODE_FILE` / `WAYHOUSE_ENROLL_CODE`).
    #[arg(long, value_name = "PATH")]
    pub code_file: Option<PathBuf>,
    /// Controller replica only: the address the other replicas reach this one on, as
    /// `--ha-peers` takes it (`https://host:port`). The cluster dials it to add this node.
    #[arg(long, value_name = "URL")]
    pub advertise: Option<String>,
    /// This node's name, for a code minted with a name prefix (one code for many pods, such as a
    /// DaemonSet's init container). The controller refuses a name outside the prefix.
    #[arg(long)]
    pub name: Option<String>,
    /// `enroll` only: exit 0 without redeeming when `--data-dir` is already enrolled, so an init
    /// container that runs again (the pod's sandbox restarted) does not spend another use.
    #[arg(long)]
    pub skip_if_enrolled: bool,
}

#[derive(Args, Debug)]
pub struct InstallArgs {
    /// Install a systemd unit for this node (the only target so far).
    #[arg(long, required = true)]
    pub systemd: bool,
    #[command(flatten)]
    pub enroll: EnrollArgs,
    /// Where to write the unit file.
    #[arg(long, default_value = "/etc/systemd/system")]
    pub unit_dir: PathBuf,
    /// Write the files but do not run `systemctl`.
    #[arg(long)]
    pub no_start: bool,
    /// An extra `LoadCredential=` as `NAME=PATH`, for a secret this node needs that enrollment
    /// does not deliver (the UI's `ui-password`). Repeatable.
    #[arg(long, value_name = "NAME=PATH")]
    pub credential: Vec<String>,
    /// Path of the binary the unit runs. Default: the running executable.
    #[arg(long)]
    pub bin_path: Option<PathBuf>,
    /// Further flags for the binary itself, after `--` (for example `--backends :25565`).
    #[arg(last = true)]
    pub extra: Vec<String>,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Redeem a one-time enrollment code and save this node's files.
    Enroll(EnrollArgs),
    /// Enroll, then install a unit that runs this node (needs root).
    Install(InstallArgs),
}

#[derive(Parser, Debug)]
#[command(version = wayhouse_http::LONG_VERSION)]
struct NodeCli {
    #[command(subcommand)]
    cmd: Cmd,
}

/// True when the first argument is one of the shared subcommands.
pub fn is_node_cmd(argv: &[OsString]) -> bool {
    argv.get(1).is_some_and(|a| a == "enroll" || a == "install")
}

fn pin(e: &EnrollArgs) -> Result<Option<Pin>, BoxErr> {
    Ok(match (&e.pin, &e.controller_ca) {
        (Some(fp), None) => Some(Pin::Fingerprint(parse_fingerprint(fp)?)),
        (None, Some(ca)) => Some(Pin::CaFile(ca.clone())),
        // Only a loopback http:// URL may go without; the client refuses anything else.
        _ => None,
    })
}

/// `--advertise` is for a controller replica and nothing else; it ends up in a unit line, so it
/// must be a plain URL or `host:port`.
fn check_advertise(role: Role, e: &EnrollArgs) -> Result<(), BoxErr> {
    match (role, e.advertise.as_deref()) {
        (Role::Controller, None) => Err(
            "a controller replica needs --advertise URL (how the other replicas reach it)".into(),
        ),
        (Role::Controller, Some(a))
            if a.is_empty()
                || a.len() > 256
                || a.chars().any(|c| {
                    c.is_whitespace()
                        || c.is_control()
                        || matches!(c, '"' | '\\' | '\'' | '%' | '$')
                }) =>
        {
            Err("--advertise must be a URL such as https://ctl-2.example:9901".into())
        }
        (r, Some(_)) if r != Role::Controller => {
            Err("--advertise is only for a controller replica".into())
        }
        _ => Ok(()),
    }
}

async fn enroll_into(role: Role, e: &EnrollArgs, dir: &std::path::Path) -> Result<String, BoxErr> {
    check_advertise(role, e)?;
    let code = crate::input::resolve(
        e.code.as_deref(),
        e.code_file.as_deref(),
        |k| std::env::var(k).ok(),
        crate::input::tty_prompt,
    )?;
    crate::enroll(
        &e.controller_url,
        pin(e)?.as_ref(),
        &code,
        role,
        dir,
        e.advertise.as_deref(),
        e.name.as_deref(),
    )
    .await
}

fn default_dir(role: Role) -> PathBuf {
    PathBuf::from(format!("wayhouse-{}-data", role.as_str()))
}

/// Runs `argv` (the binary's own arguments, program name first) as `role`. `bin` is the
/// binary's name, for help text and the unit it installs.
pub async fn run(role: Role, bin: &'static str, argv: Vec<OsString>) -> Result<(), BoxErr> {
    let mut argv = argv;
    if let Some(a0) = argv.first_mut() {
        *a0 = bin.into();
    }
    match NodeCli::parse_from(argv).cmd {
        Cmd::Enroll(e) => {
            let dir = e.data_dir.clone().unwrap_or_else(|| default_dir(role));
            // Only "already enrolled" skips; any other problem with the directory is an error.
            let enrolled = matches!(
                crate::files::check_fresh(&dir),
                Err(crate::files::FileError::Io { ref source, .. })
                    if source.kind() == std::io::ErrorKind::AlreadyExists
            );
            if e.skip_if_enrolled && enrolled {
                eprintln!("{} is already enrolled; nothing to do", dir.display());
                return Ok(());
            }
            crate::files::check_fresh(&dir)?;
            let name = enroll_into(role, &e, &dir).await?;
            eprintln!(
                "enrolled as {name:?}; files written to {}\n{}",
                dir.display(),
                crate::install::run_hint(role, bin, &dir, &e.controller_url)
            );
        }
        Cmd::Install(i) => {
            let spec = Install::new(role, bin, i.bin_path.clone(), &i.unit_dir, &i.credential)?;
            let dir = i.enroll.data_dir.clone().unwrap_or_else(|| spec.data_dir());
            // Check before spending the code: a unit that already exists is never replaced.
            spec.check_free(&dir)?;
            spec.preflight(
                i.enroll.data_dir.as_deref(),
                &i.enroll.controller_url,
                i.enroll.advertise.as_deref(),
                &i.extra,
            )?;
            // Everything the unit needs is checked before the one-time code is spent.
            if role == Role::Controller
                && !i
                    .extra
                    .iter()
                    .any(|a| a == "--listen" || a.starts_with("--listen="))
            {
                return Err("a controller replica needs --listen ADDR after `--`".into());
            }
            let name = enroll_into(role, &i.enroll, &dir).await?;
            install_systemd(&spec, &dir, &i.enroll.controller_url, &i.extra, !i.no_start)?;
            eprintln!(
                "enrolled as {name:?}; unit {} installed{}",
                spec.unit_path().display(),
                if i.no_start { "" } else { " and started" }
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn skip_if_enrolled_leaves_an_enrolled_directory_alone_without_asking_the_controller() {
        let t = tempfile::tempdir().unwrap();
        std::fs::write(t.path().join(crate::files::META_FILE), "{}").unwrap();
        let argv = |skip: bool| -> Vec<OsString> {
            let mut a: Vec<OsString> = ["x", "enroll", "--controller-url=http://127.0.0.1:1"]
                .map(Into::into)
                .into();
            a.push(format!("--data-dir={}", t.path().display()).into());
            a.push("--code=aaaaaaaa.bbbb".into());
            if skip {
                a.push("--skip-if-enrolled".into());
            }
            a
        };
        assert!(run(Role::Agent, "x", argv(true)).await.is_ok());
        assert!(run(Role::Agent, "x", argv(false)).await.is_err());
    }

    /// An `install` argv whose unit could never work; a controller nobody listens on proves the
    /// refusal came before the code was spent (a spent code would be a connection error).
    fn install_argv(role: &str, extra: &[&str]) -> Vec<OsString> {
        let t = tempfile::tempdir().unwrap().keep();
        let mut a: Vec<OsString> = [
            "x",
            "install",
            "--systemd",
            "--no-start",
            "--controller-url=http://127.0.0.1:1",
            "--code=aaaaaaaa.bbbb",
            "--bin-path=/usr/bin/x",
        ]
        .map(Into::into)
        .into();
        a.push(format!("--unit-dir={}", t.display()).into());
        a.push(format!("--data-dir={}", t.join(role).display()).into());
        a.extend(extra.iter().map(OsString::from));
        a
    }

    async fn refusal(role: Role, argv: Vec<OsString>) -> String {
        run(role, "x", argv).await.unwrap_err().to_string()
    }

    #[tokio::test]
    async fn a_ui_unit_without_a_login_is_refused_before_the_code_is_spent() {
        let e = refusal(Role::Ui, install_argv("ui", &[])).await;
        assert!(e.contains("login"), "{e}");
        // A login by credential or users file passes the check (and then fails to connect).
        for ok in [
            vec!["--credential=ui-password=/etc/ui-pw"],
            vec!["--", "--users-file=/etc/users.json"],
        ] {
            let e = refusal(Role::Ui, install_argv("ui", &ok)).await;
            assert!(!e.contains("login"), "{ok:?}: {e}");
        }
    }

    #[tokio::test]
    async fn an_agent_unit_cannot_move_its_data_dir() {
        let e = refusal(Role::Agent, install_argv("agent", &[])).await;
        assert!(
            e.contains("--data-dir") && e.contains("/var/lib/wayhouse-agent"),
            "{e}"
        );
    }

    #[tokio::test]
    async fn a_flag_the_unit_cannot_hold_is_refused_before_the_code_is_spent() {
        let e = refusal(
            Role::Proxy,
            install_argv("proxy", &["--", "--backends=a b"]),
        )
        .await;
        assert!(e.contains("cannot be written into a systemd unit"), "{e}");
    }
}
