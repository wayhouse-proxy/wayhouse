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

    /// Everything about the unit that can be known before the one-time code is spent: a unit
    /// that could never start, or that would be written wrong, must not cost the code (for an
    /// agent re-enroll, the old token and key too).
    pub fn preflight(
        &self,
        requested_dir: Option<&Path>,
        controller_url: &str,
        advertise: Option<&str>,
        extra: &[String],
    ) -> Result<(), BoxErr> {
        let flag = |name: &str| {
            extra
                .iter()
                .any(|a| a == name || a.strip_prefix(name).is_some_and(|r| r.starts_with('=')))
        };
        if self.role == Role::Agent && requested_dir.is_some_and(|d| d != self.data_dir()) {
            return Err(format!(
                "the agent's unit keeps its files in {}; drop --data-dir",
                self.data_dir().display()
            )
            .into());
        }
        // The UI starts on loopback holding the enrolled admin token; without a login every
        // local user would be a fleet admin.
        if self.role == Role::Ui
            && !self.extra_creds.iter().any(|(n, _)| n == "ui-password")
            && !flag("--users-file")
            && !flag("--ui-password-file")
        {
            return Err("a UI unit without a login would make every local user a fleet admin: pass --credential ui-password=PATH, or --users-file PATH after `--`".into());
        }
        // The name and id arrive with the redeem; a stand-in is enough to check the rest.
        let stand_in = Meta {
            controller_url: controller_url.to_owned(),
            name: "node".into(),
            role: self.role,
            parent_url: None,
            node_id: Some(1),
            tunnel_network: None,
            advertise: Some(advertise.unwrap_or("https://node.invalid").to_owned()),
        };
        render_unit(self, &self.data_dir(), &stand_in, &[], extra).map(drop)
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
    // Only a controller replica's unit turns this credential into a flag; for any other role it
    // would be loaded and never used.
    if spec.role != Role::Controller && spec.extra_creds.iter().any(|(n, _)| n == "tls-key") {
        return Err("--credential tls-key=PATH is only for controller replica installs".into());
    }
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
        Role::Controller => {
            // A replica must be reachable by the others: the operator gives its `--listen` (and
            // the certificate flags) after `--`, so a unit is never written that listens on
            // loopback only.
            if !extra
                .iter()
                .any(|a| a == "--listen" || a.starts_with("--listen="))
            {
                return Err("a controller replica needs --listen ADDR after `--` (the address the other replicas reach it on)".into());
            }
            let id = meta
                .node_id
                .ok_or("the enrollment holds no node id; re-enroll with a new code")?;
            let adv = meta
                .advertise
                .as_deref()
                .ok_or("the enrollment holds no --advertise address")?;
            let mut flags = vec![
                "--data-dir=/var/lib/wayhouse-controller".into(),
                "--ha-join".into(),
                format!("--ha-node-id={id}"),
                format!("--ha-advertise={}", unit_arg(adv)?),
                format!("--ha-join-via={url}"),
                "--secret-key-file=%d/secret.keys".into(),
            ];
            // The key goes in as a credential: the unit runs as a dynamic user, which could only
            // read a world-readable key file, so a hand-written --tls-key (which names a path
            // the unit cannot read) is refused whether or not the credential is given.
            if extra
                .iter()
                .any(|a| a == "--tls-key" || a.starts_with("--tls-key="))
            {
                return Err("a controller replica's unit cannot read a --tls-key path (it runs as a dynamic user); pass the key with --credential tls-key=PATH instead".into());
            }
            if spec.extra_creds.iter().any(|(n, _)| n == "tls-key") {
                flags.push("--tls-key=%d/tls-key".into());
            }
            // A replica with another network than the cluster's refuses tunnel writes.
            if let Some(n) = &meta.tunnel_network {
                flags.push(format!("--tunnel-network={}", unit_arg(n)?));
            }
            flags
        }
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
        if spec.role == Role::Controller {
            u.push_str("StateDirectory=wayhouse-controller\nStateDirectoryMode=0700\n");
        }
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
        Role::Aggregator | Role::Ui | Role::Controller => u.push_str("PrivateDevices=yes\nCapabilityBoundingSet=\nRestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX\n"),
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
        // The aggregator makes its own admin token; the controller's arrives in its grant.
        .chain(&["auth-token"][..usize::from(spec.role == Role::Aggregator)])
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
        Role::Controller => match files::read_meta(dir) {
            Ok(Meta {
                node_id: Some(id),
                advertise: Some(adv),
                ..
            }) => format!(
                "run: CREDENTIALS_DIRECTORY={d} {bin} --data-dir DIR --listen ADDR --ha-join --ha-node-id {id} --ha-advertise {adv} --ha-join-via {url} --secret-key-file {d}/secret.keys (plus --tls-cert/--tls-key and --ca-file)"
            ),
            _ => format!("run: CREDENTIALS_DIRECTORY={d} {bin} --ha-join ..."),
        },
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
            node_id: None,
            tunnel_network: None,
            advertise: None,
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

    #[test]
    fn the_controller_replica_unit_joins_the_group_and_keeps_every_secret_in_a_credential() {
        let mut m = meta(Role::Controller, None);
        m.node_id = Some(4);
        m.advertise = Some("https://ctl-4.example:9901".into());
        m.tunnel_network = Some("10.60.0.0/24".into());
        let files: Vec<String> = [
            "ha-token",
            "secret.keys",
            "auth-token",
            "proxy-token",
            "agent-token",
            "ingest-token",
            "enroll-step-up",
            "ha-join-proof",
        ]
        .map(String::from)
        .to_vec();
        let u = render_unit(
            &spec(Role::Controller, "wayhouse-controller", &[]),
            Path::new("/etc/wayhouse/controller"),
            &m,
            &files,
            &["--listen".into(), "0.0.0.0:9901".into()],
        )
        .unwrap();
        assert_eq!(
            u,
            "[Unit]\nDescription=wayhouse controller node edge-1\nAfter=network-online.target\nWants=network-online.target\n\n[Service]\nDynamicUser=yes\nStateDirectory=wayhouse-controller\nStateDirectoryMode=0700\nLoadCredential=ha-token:/etc/wayhouse/controller/ha-token\nLoadCredential=secret.keys:/etc/wayhouse/controller/secret.keys\nLoadCredential=auth-token:/etc/wayhouse/controller/auth-token\nLoadCredential=proxy-token:/etc/wayhouse/controller/proxy-token\nLoadCredential=agent-token:/etc/wayhouse/controller/agent-token\nLoadCredential=ingest-token:/etc/wayhouse/controller/ingest-token\nLoadCredential=enroll-step-up:/etc/wayhouse/controller/enroll-step-up\nLoadCredential=ha-join-proof:/etc/wayhouse/controller/ha-join-proof\nExecStart=/usr/local/bin/x \\\n  --data-dir=/var/lib/wayhouse-controller \\\n  --ha-join \\\n  --ha-node-id=4 \\\n  --ha-advertise=https://ctl-4.example:9901 \\\n  --ha-join-via=https://ctl.example:9901 \\\n  --secret-key-file=%d/secret.keys \\\n  --tunnel-network=10.60.0.0/24 \\\n  --listen \\\n  0.0.0.0:9901\nRestart=on-failure\nRestartSec=2\nNoNewPrivileges=yes\nProtectSystem=strict\nProtectHome=yes\nPrivateTmp=yes\nProtectKernelTunables=yes\nProtectControlGroups=yes\nPrivateDevices=yes\nCapabilityBoundingSet=\nRestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX\n\n[Install]\nWantedBy=multi-user.target\n"
        );
        // Not one secret value is in the unit, only names.
        assert!(!u.contains("Bearer") && !u.contains("--ha-token"));
    }

    #[test]
    fn a_tls_key_credential_reaches_the_replica_as_a_flag_not_a_world_readable_path() {
        let mut m = meta(Role::Controller, None);
        m.node_id = Some(4);
        m.advertise = Some("https://ctl-4.example:9901".into());
        let s = spec(
            Role::Controller,
            "wayhouse-controller",
            &["tls-key=/etc/wayhouse/ctl-4.key".into()],
        );
        let listen = ["--listen=0.0.0.0:9901".to_owned()];
        let u = render_unit(&s, Path::new("/d"), &m, &[], &listen).unwrap();
        assert!(
            u.contains("LoadCredential=tls-key:/etc/wayhouse/ctl-4.key\n"),
            "{u}"
        );
        assert!(u.contains("--tls-key=%d/tls-key"), "{u}");
        // The same flag by hand would point at a path the dynamic user cannot read.
        let both = [
            "--listen=0.0.0.0:9901".to_owned(),
            "--tls-key=/k".to_owned(),
        ];
        assert!(render_unit(&s, Path::new("/d"), &m, &[], &both).is_err());
        // ...and without the credential too: the dynamic user cannot read that path either.
        let plain = spec(Role::Controller, "wayhouse-controller", &[]);
        assert!(render_unit(&plain, Path::new("/d"), &m, &[], &both).is_err());
        // Other roles never turn the credential into a flag, so it is refused, not ignored.
        let proxy = spec(
            Role::Proxy,
            "wayhouse",
            &["tls-key=/etc/wayhouse/x.key".into()],
        );
        assert!(render_unit(&proxy, Path::new("/d"), &meta(Role::Proxy, None), &[], &[]).is_err());
        // Without the credential nothing is added.
        let plain = spec(Role::Controller, "wayhouse-controller", &[]);
        let u = render_unit(&plain, Path::new("/d"), &m, &[], &listen).unwrap();
        assert!(!u.contains("%d/tls-key"), "{u}");
    }

    #[test]
    fn a_controller_unit_needs_a_listen_address_a_node_id_and_an_advertise_address() {
        let s = spec(Role::Controller, "wayhouse-controller", &[]);
        let listen = ["--listen=0.0.0.0:9901".to_owned()];
        let mut m = meta(Role::Controller, None);
        let go = |m: &Meta, extra: &[String]| render_unit(&s, Path::new("/d"), m, &[], extra);
        assert!(go(&m, &listen).is_err(), "no node id");
        m.node_id = Some(2);
        assert!(go(&m, &listen).is_err(), "no advertise address");
        m.advertise = Some("https://c2:9901".into());
        assert!(
            go(&m, &[]).is_err(),
            "a replica that listens on loopback only is useless"
        );
        assert!(go(&m, &listen).unwrap().contains("--ha-node-id=2"));
        m.advertise = Some("https://c2:9901 --evil".into());
        assert!(go(&m, &listen).is_err(), "no way out of the unit line");
    }

    #[test]
    fn the_controller_role_has_a_grant_list_and_needs_the_step_up() {
        assert!(Role::Controller.needs_step_up());
        assert!(Role::Controller.grant_files().contains(&"ha-join-proof"));
        assert!(Role::Controller.grant_files().contains(&"ha-token"));
        // The unit for a controller never loads auth-token twice.
        let t = tempfile::tempdir().unwrap();
        let s = Install::new(
            Role::Controller,
            "wayhouse-controller",
            Some("/b".into()),
            t.path(),
            &[],
        )
        .unwrap();
        let data = t.path().join("data");
        let mut m = meta(Role::Controller, None);
        m.node_id = Some(2);
        m.advertise = Some("https://c2:9901".into());
        let granted = [("auth-token".to_owned(), "tok".to_owned())].into();
        files::write(&data, &m, &granted).unwrap();
        install_systemd(&s, &data, "", &["--listen=0.0.0.0:9901".into()], false).unwrap();
        let unit = std::fs::read_to_string(s.unit_path()).unwrap();
        assert_eq!(
            unit.matches("LoadCredential=auth-token:").count(),
            1,
            "{unit}"
        );
    }
}
