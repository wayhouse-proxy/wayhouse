//! `install --systemd`: after enrolling, write a unit that runs the node with `LoadCredential=`
//! for every secret file, so no token is ever on a command line or in the environment.
//!
//! The origin agent is the exception: it keeps writing into its data directory (its WireGuard
//! key), so its unit uses `StateDirectory=` and the agent reads its enrolled files from there.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::files::{self, Meta};
use crate::Role;

type BoxErr = Box<dyn std::error::Error + Send + Sync>;

pub struct Install {
    role: Role,
    bin: &'static str,
    bin_path: PathBuf,
    unit_dir: PathBuf,
    /// Extra `LoadCredential=` pairs (name, source path).
    extra_creds: Vec<(String, PathBuf)>,
}

impl Install {
    pub fn new(
        role: Role,
        bin: &'static str,
        bin_path: Option<PathBuf>,
        unit_dir: &Path,
        credentials: &[String],
    ) -> Result<Self, BoxErr> {
        let mut extra_creds = Vec::new();
        for c in credentials {
            let (name, path) = c
                .split_once('=')
                .ok_or_else(|| format!("--credential {c:?}: expected NAME=PATH"))?;
            if name.is_empty()
                || !name
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            {
                return Err(format!(
                    "--credential name {name:?} must be lowercase letters, digits and -"
                )
                .into());
            }
            extra_creds.push((name.to_owned(), check_word(path)?.into()));
        }
        let bin_path = match bin_path {
            Some(p) => p,
            None => std::env::current_exe()?,
        };
        check_word(&bin_path.to_string_lossy())?;
        Ok(Self {
            role,
            bin,
            bin_path,
            unit_dir: unit_dir.to_owned(),
            extra_creds,
        })
    }

    pub fn unit_name(&self) -> String {
        format!("{}.service", self.bin)
    }

    pub fn unit_path(&self) -> PathBuf {
        self.unit_dir.join(self.unit_name())
    }

    /// Where the enrolled files live. The agent's is its state directory; the others keep
    /// their secrets in a root-only directory that systemd turns into credentials.
    pub fn data_dir(&self) -> PathBuf {
        match self.role {
            Role::Agent => "/var/lib/wayhouse-agent".into(),
            r => format!("/etc/wayhouse/{}", r.as_str()).into(),
        }
    }

    /// Refuses to go on when it would replace a unit or an earlier enrollment.
    pub fn check_free(&self, data_dir: &Path) -> Result<(), BoxErr> {
        if self.unit_path().exists() {
            return Err(format!(
                "{} already exists; remove it first",
                self.unit_path().display()
            )
            .into());
        }
        if data_dir.join(files::META_FILE).exists() {
            return Err(format!(
                "{} is already enrolled; remove it first",
                data_dir.display()
            )
            .into());
        }
        Ok(())
    }
}

/// A value safe inside a unit line: no whitespace, quotes, backslashes or control characters.
fn check_word(s: &str) -> Result<&str, BoxErr> {
    if s.is_empty()
        || s.chars()
            .any(|c| c.is_whitespace() || c.is_control() || c == '"' || c == '\\' || c == '\'')
    {
        return Err(format!("{s:?} cannot be written into a systemd unit").into());
    }
    Ok(s)
}

/// For `ExecStart=`: `%` and `$` are doubled so systemd does not expand them.
fn unit_arg(s: &str) -> Result<String, BoxErr> {
    Ok(check_word(s)?.replace('%', "%%").replace('$', "$$"))
}

/// For `LoadCredential=`: specifiers (`%`) expand there, `$` does not.
fn cred_path(p: &Path) -> Result<String, BoxErr> {
    Ok(check_word(&p.to_string_lossy())?.replace('%', "%%"))
}

/// The unit text. Pure, so a golden test pins it. `files` are the credential names enrollment
/// wrote into the data directory.
pub fn render_unit(
    spec: &Install,
    dir: &Path,
    meta: &Meta,
    files: &[String],
    extra: &[String],
) -> Result<String, BoxErr> {
    let url = unit_arg(&meta.controller_url)?;
    let name = unit_arg(&meta.name)?;
    let mut flags: Vec<String> = match spec.role {
        Role::Agent => vec!["--data-dir=/var/lib/wayhouse-agent".into()],
        Role::Proxy => vec![
            format!("--controller={url}"),
            format!("--aggregator-instance={name}"),
            "--secret-key-file=%d/secret.keys".into(),
        ],
        Role::Aggregator => meta
            .parent_url
            .iter()
            .map(|u| unit_arg(u).map(|u| format!("--parent-url={u}")))
            .collect::<Result<_, _>>()?,
        Role::Ui => vec![format!("--controller-url={url}")],
    };
    for a in extra {
        flags.push(unit_arg(a)?);
    }
    let mut u = String::new();
    let _ = writeln!(
        u,
        "[Unit]\nDescription=wayhouse {} node {name}\nAfter=network-online.target\nWants=network-online.target\n\n[Service]",
        spec.role.as_str()
    );
    if spec.role != Role::Agent {
        u.push_str("DynamicUser=yes\n");
        for f in files {
            let _ = writeln!(u, "LoadCredential={f}:{}/{f}", cred_path(dir)?);
        }
        for (n, p) in &spec.extra_creds {
            let _ = writeln!(u, "LoadCredential={n}:{}", cred_path(p)?);
        }
    } else {
        u.push_str("StateDirectory=wayhouse-agent\nStateDirectoryMode=0700\n");
    }
    let _ = write!(
        u,
        "ExecStart={}",
        unit_arg(&spec.bin_path.to_string_lossy())?
    );
    for f in &flags {
        let _ = write!(u, " \\\n  {f}");
    }
    u.push_str("\nRestart=on-failure\nRestartSec=2\nNoNewPrivileges=yes\nProtectSystem=strict\nProtectHome=yes\nPrivateTmp=yes\nProtectKernelTunables=yes\nProtectControlGroups=yes\n");
    match spec.role {
        // Creates the WireGuard interface: needs the tun device and NET_ADMIN.
        Role::Agent => u.push_str("AmbientCapabilities=CAP_NET_ADMIN\nCapabilityBoundingSet=CAP_NET_ADMIN\nRestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX AF_NETLINK\n"),
        Role::Proxy => u.push_str("PrivateDevices=yes\nAmbientCapabilities=CAP_NET_BIND_SERVICE\nCapabilityBoundingSet=CAP_NET_BIND_SERVICE\nRestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX AF_NETLINK\n"),
        Role::Aggregator | Role::Ui => u.push_str("PrivateDevices=yes\nCapabilityBoundingSet=\nRestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX\n"),
    }
    u.push_str("\n[Install]\nWantedBy=multi-user.target\n");
    Ok(u)
}

/// Writes the unit (never over an existing file) and, with `start`, enables and starts it.
pub fn install_systemd(
    spec: &Install,
    dir: &Path,
    _controller_url: &str,
    extra: &[String],
    start: bool,
) -> Result<(), BoxErr> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let meta = files::read_meta(dir)?;
    let granted: Vec<String> = spec
        .role
        .grant_files()
        .iter()
        .chain(&["auth-token"])
        .filter(|f| dir.join(f).is_file() && spec.role != Role::Agent)
        .map(|f| (*f).to_owned())
        .collect();
    let text = render_unit(spec, dir, &meta, &granted, extra)?;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o644)
        .open(spec.unit_path())
        .map_err(|e| format!("{}: {e}", spec.unit_path().display()))?;
    f.write_all(text.as_bytes())?;
    if start {
        for args in [
            vec!["daemon-reload".to_owned()],
            vec!["enable".into(), "--now".into(), spec.unit_name()],
        ] {
            let ok = std::process::Command::new("systemctl")
                .args(&args)
                .status()?
                .success();
            if !ok {
                return Err(format!("systemctl {} failed", args.join(" ")).into());
            }
        }
    }
    Ok(())
}

/// What `enroll` (without `install`) tells the operator to run next.
pub fn run_hint(role: Role, bin: &str, dir: &Path, url: &str) -> String {
    let d = dir.display();
    match role {
        Role::Agent => format!("run: {bin} --data-dir {d} --backends :PORT"),
        Role::Proxy => format!(
            "run: CREDENTIALS_DIRECTORY={d} {bin} --controller {url} --secret-key-file {d}/secret.keys ..."
        ),
        Role::Aggregator | Role::Ui => {
            format!("run: CREDENTIALS_DIRECTORY={d} {bin} --controller-url {url} ...")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(role: Role, bin: &'static str, creds: &[String]) -> Install {
        Install::new(
            role,
            bin,
            Some("/usr/local/bin/x".into()),
            Path::new("/u"),
            creds,
        )
        .unwrap()
    }

    fn meta(role: Role, parent: Option<&str>) -> Meta {
        Meta {
            controller_url: "https://ctl.example:9901".into(),
            name: "edge-1".into(),
            role,
            parent_url: parent.map(str::to_owned),
        }
    }

    #[test]
    fn the_proxy_unit_loads_every_secret_as_a_credential_and_has_none_on_the_command_line() {
        let s = spec(Role::Proxy, "wayhouse", &[]);
        let files = ["controller-token", "aggregator-token", "secret.keys"].map(String::from);
        let u = render_unit(
            &s,
            Path::new("/etc/wayhouse/proxy"),
            &meta(Role::Proxy, None),
            &files,
            &["--aggregator".into(), "https://agg.example".into()],
        )
        .unwrap();
        assert_eq!(
            u,
            "[Unit]\nDescription=wayhouse proxy node edge-1\nAfter=network-online.target\nWants=network-online.target\n\n[Service]\nDynamicUser=yes\nLoadCredential=controller-token:/etc/wayhouse/proxy/controller-token\nLoadCredential=aggregator-token:/etc/wayhouse/proxy/aggregator-token\nLoadCredential=secret.keys:/etc/wayhouse/proxy/secret.keys\nExecStart=/usr/local/bin/x \\\n  --controller=https://ctl.example:9901 \\\n  --aggregator-instance=edge-1 \\\n  --secret-key-file=%d/secret.keys \\\n  --aggregator \\\n  https://agg.example\nRestart=on-failure\nRestartSec=2\nNoNewPrivileges=yes\nProtectSystem=strict\nProtectHome=yes\nPrivateTmp=yes\nProtectKernelTunables=yes\nProtectControlGroups=yes\nPrivateDevices=yes\nAmbientCapabilities=CAP_NET_BIND_SERVICE\nCapabilityBoundingSet=CAP_NET_BIND_SERVICE\nRestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX AF_NETLINK\n\n[Install]\nWantedBy=multi-user.target\n"
        );
    }

    #[test]
    fn the_other_roles_render_their_own_flags() {
        let a = render_unit(
            &spec(Role::Aggregator, "wayhouse-aggregator", &[]),
            Path::new("/d"),
            &meta(Role::Aggregator, Some("https://parent.example")),
            &["ingest-token".into(), "auth-token".into()],
            &[],
        )
        .unwrap();
        assert!(
            a.contains("LoadCredential=auth-token:/d/auth-token\n")
                && a.contains("--parent-url=https://parent.example"),
            "{a}"
        );
        let ui = render_unit(
            &spec(
                Role::Ui,
                "wayhouse-ui",
                &["ui-password=/etc/wayhouse/ui-password".into()],
            ),
            Path::new("/d"),
            &meta(Role::Ui, None),
            &["controller-token".into()],
            &[],
        )
        .unwrap();
        assert!(
            ui.contains("LoadCredential=ui-password:/etc/wayhouse/ui-password\n")
                && ui.contains("--controller-url=https://ctl.example:9901"),
            "{ui}"
        );
        let ag = render_unit(
            &spec(Role::Agent, "wayhouse-agent", &[]),
            Path::new("/var/lib/wayhouse-agent"),
            &meta(Role::Agent, None),
            &[],
            &["--backends".into(), ":25565".into()],
        )
        .unwrap();
        assert!(
            ag.contains("StateDirectory=wayhouse-agent")
                && ag.contains("CAP_NET_ADMIN")
                && !ag.contains("LoadCredential")
                && ag.contains("--data-dir=/var/lib/wayhouse-agent"),
            "{ag}"
        );
    }

    #[test]
    fn anything_that_could_break_out_of_a_unit_line_is_refused_or_escaped() {
        let s = spec(Role::Ui, "wayhouse-ui", &[]);
        for bad in ["a b", "a\nExecStartPre=x", "a\"b", "a\\b"] {
            assert!(
                render_unit(
                    &s,
                    Path::new("/d"),
                    &meta(Role::Ui, None),
                    &[],
                    &[bad.into()]
                )
                .is_err(),
                "{bad:?}"
            );
        }
        let u = render_unit(
            &s,
            Path::new("/d"),
            &meta(Role::Ui, None),
            &[],
            &["--x=$HOME%h".into()],
        )
        .unwrap();
        assert!(u.contains("--x=$$HOME%%h"));
        // A data dir that would break the line is refused, and `$` is not doubled in a credential path.
        assert!(render_unit(
            &spec(Role::Ui, "wayhouse-ui", &[]),
            Path::new("/d x"),
            &meta(Role::Ui, None),
            &["controller-token".into()],
            &[]
        )
        .is_err());
        let c = render_unit(
            &spec(Role::Ui, "wayhouse-ui", &["p=/etc/a$b".into()]),
            Path::new("/d"),
            &meta(Role::Ui, None),
            &[],
            &[],
        )
        .unwrap();
        assert!(c.contains("LoadCredential=p:/etc/a$b\n"), "{c}");
        assert!(Install::new(
            Role::Ui,
            "wayhouse-ui",
            Some("/bin dir/x".into()),
            Path::new("/u"),
            &[]
        )
        .is_err());
        assert!(Install::new(
            Role::Ui,
            "wayhouse-ui",
            Some("/b".into()),
            Path::new("/u"),
            &["Bad_Name=/p".into()]
        )
        .is_err());
        assert!(Install::new(
            Role::Ui,
            "wayhouse-ui",
            Some("/b".into()),
            Path::new("/u"),
            &["nopath".into()]
        )
        .is_err());
    }

    #[test]
    fn an_existing_unit_or_enrollment_is_never_replaced() {
        let t = tempfile::tempdir().unwrap();
        let s = Install::new(Role::Ui, "wayhouse-ui", Some("/b".into()), t.path(), &[]).unwrap();
        let data = t.path().join("data");
        assert!(s.check_free(&data).is_ok());
        std::fs::write(s.unit_path(), "x").unwrap();
        assert!(s.check_free(&data).is_err());
        std::fs::remove_file(s.unit_path()).unwrap();
        std::fs::create_dir(&data).unwrap();
        std::fs::write(data.join(files::META_FILE), "{}").unwrap();
        assert!(s.check_free(&data).is_err());
    }

    #[test]
    fn install_writes_the_unit_with_mode_0644_and_refuses_a_second_write() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        let s = Install::new(Role::Ui, "wayhouse-ui", Some("/b".into()), t.path(), &[]).unwrap();
        let data = t.path().join("data");
        let granted = [("controller-token".to_owned(), "tok".to_owned())].into();
        files::write(&data, &meta(Role::Ui, None), &granted).unwrap();
        install_systemd(&s, &data, "", &[], false).unwrap();
        assert_eq!(
            std::fs::metadata(s.unit_path())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o644
        );
        assert!(std::fs::read_to_string(s.unit_path())
            .unwrap()
            .contains("LoadCredential=controller-token:"));
        assert!(install_systemd(&s, &data, "", &[], false).is_err());
    }
}
