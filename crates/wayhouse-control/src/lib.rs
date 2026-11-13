//! `wayhouse-control`: controller, aggregator and UI in one process, for a small install.
//!
//! The three programs are the standalone ones (`wayhouse_controller::serve::run` and friends),
//! each with its own listener, its own tokens and its own settings file; nothing is merged. This
//! crate only routes one command line to them ([`plan`]) and points the UI at the other two over
//! loopback with the same tokens a split deployment would use, so the code path is the same.
//!
//! Flags: `--settings-dir`, `--tls-cert`, `--tls-key` and `--ca-file` are this program's. The
//! settings directory applies to all three, the certificate pair to the controller and
//! aggregator (the UI's own TLS is `--ui-tls-cert`/`--ui-tls-key`), and the CA file is trusted by
//! the process. Everything else is namespaced: `--controller-X`, `--aggregator-X` and `--ui-X`
//! reach flag `--X` of that program; a UI flag that is already named `ui-...` keeps its name
//! (`--ui-ui-password-file`). The UI's `--controller-url`, `--aggregator-url` and their token
//! files are not flags here; they are derived.
//!
//! Notes:
//! - TLS is decided here, from the command line only (a `tls-cert` in a settings file would not
//!   change the scheme the UI dials), so give `--tls-cert` there.
//! - With TLS the UI reaches the others at `https://127.0.0.1:<port>`, so the shared certificate
//!   must be valid for `127.0.0.1` (and `::1` when listening there).
//! - The three programs share one metrics recorder, so every `/metrics` lists the series of all
//!   three.
//! - The token split (controller, proxy, agent, ingest) is enforced at the HTTP layer between
//!   the listeners; inside one process it is nominal, because one compromised process holds
//!   every token. Fleets keep the split binaries.

use std::ffi::OsString;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};

use clap::Command;

/// One command line per program (each starts with the program name), and the CA file this
/// process trusts (initialised once here, because the programs would each try to).
#[derive(Debug, PartialEq, Eq)]
pub struct Plan {
    pub controller: Vec<OsString>,
    pub aggregator: Vec<OsString>,
    pub ui: Vec<OsString>,
    pub ca_file: Option<PathBuf>,
}

/// The flags of the three programs, to route by.
pub struct Programs {
    pub controller: Command,
    pub aggregator: Command,
    pub ui: Command,
}

impl Programs {
    pub fn linked() -> Self {
        Self {
            controller: wayhouse_controller::serve::command(),
            aggregator: wayhouse_aggregator::serve::command(),
            ui: wayhouse_ui::serve::command(),
        }
    }
}

const OWN: [&str; 4] = ["settings-dir", "tls-cert", "tls-key", "ca-file"];

/// The flag name of `arg` without its dashes, and its value when it is written `--x=v`.
fn split_flag(arg: &OsString) -> Option<(String, Option<OsString>)> {
    let s = arg.to_str()?.strip_prefix("--")?;
    Some(match s.split_once('=') {
        Some((n, v)) => (n.to_owned(), Some(v.into())),
        None => (s.to_owned(), None),
    })
}

/// The value `flag` has in `args`, written `--flag v` or `--flag=v`.
fn value_of(args: &[OsString], flag: &str) -> Option<String> {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match split_flag(a) {
            Some((n, Some(v))) if n == flag => return Some(v.to_string_lossy().into_owned()),
            Some((n, None)) if n == flag => {
                return it.next().map(|v| v.to_string_lossy().into_owned())
            }
            _ => {}
        }
    }
    None
}

fn has_flag(cmd: &Command, name: &str) -> bool {
    cmd.get_arguments().any(|a| {
        a.get_long() == Some(name) || a.get_all_aliases().is_some_and(|al| al.contains(&name))
    })
}

/// The address another program reaches `cmd`'s listener at: its `--listen` (or default), with a
/// wildcard address replaced by loopback.
fn loopback_addr(cmd: &Command, forwarded: &[OsString]) -> Result<SocketAddr, String> {
    let listen = value_of(forwarded, "listen")
        .or_else(|| {
            let arg = cmd
                .get_arguments()
                .find(|a| a.get_long() == Some("listen"))?;
            Some(
                arg.get_default_values()
                    .first()?
                    .to_string_lossy()
                    .into_owned(),
            )
        })
        .ok_or("no --listen")?;
    let mut addr: SocketAddr = listen
        .parse()
        .map_err(|e| format!("--listen {listen}: {e}"))?;
    if addr.ip().is_unspecified() {
        addr.set_ip(match addr.ip() {
            IpAddr::V4(_) => IpAddr::from([127, 0, 0, 1]),
            IpAddr::V6(_) => IpAddr::from(std::net::Ipv6Addr::LOCALHOST),
        });
    }
    Ok(addr)
}

fn addr_flag(cmd: &Command) -> Result<String, String> {
    let arg = cmd
        .get_arguments()
        .find(|a| a.get_long() == Some("listen"))
        .and_then(|a| a.get_default_values().first())
        .ok_or("no default --listen")?;
    Ok(arg.to_string_lossy().into_owned())
}

/// The `--...-file` flags of `cmd` that carry a secret (a token, a password, a proof). The
/// programs also read these from `$CREDENTIALS_DIRECTORY/<name>` and `WAYHOUSE_<NAME>`, names the
/// three share, so here they are given per program instead (see [`plan`]).
pub fn secret_file_flags(cmd: &Command) -> Vec<&str> {
    cmd.get_arguments()
        .filter_map(|a| a.get_long())
        .filter(|n| {
            n.ends_with("-file")
                && ["token", "password", "step-up", "proof"]
                    .iter()
                    .any(|w| n.contains(w))
        })
        .collect()
}

/// A secret in the environment (`WAYHOUSE_AUTH_TOKEN`) has a name all three programs share, so
/// one value would reach all three: refused, naming the variable.
pub fn refuse_shared_secret_env(
    mut env: impl Iterator<Item = (String, String)>,
) -> Result<(), String> {
    match env.find(|(k, _)| {
        k.starts_with("WAYHOUSE_")
            && ["TOKEN", "PASSWORD", "STEP_UP", "PROOF"]
                .iter()
                .any(|w| k.contains(w))
    }) {
        Some((k, _)) => Err(format!(
            "{k} is set, but its name is shared by all three programs; give the secret as \
             --<program>-<name>-file or a credential named <program>-<name>"
        )),
        None => Ok(()),
    }
}

/// `credentials` is systemd's `$CREDENTIALS_DIRECTORY`: a file `<program>-<flag without -file>`
/// in it (`controller-auth-token`, `aggregator-ingest-token`, `ui-ui-password`) stands in for
/// that flag when the command line does not give it.
pub fn plan(
    argv: &[OsString],
    programs: &Programs,
    credentials: Option<&Path>,
) -> Result<Plan, String> {
    let comps = [
        ("controller", &programs.controller),
        ("aggregator", &programs.aggregator),
        ("ui", &programs.ui),
    ];
    let mut forwarded: [Vec<OsString>; 3] = Default::default();
    let mut own: Vec<(String, OsString)> = Vec::new();
    let mut it = argv.iter().skip(1).peekable();
    while let Some(arg) = it.next() {
        let (name, inline) =
            split_flag(arg).ok_or_else(|| format!("unexpected argument {arg:?}"))?;
        // A flag takes the tokens after it that are not flags themselves (some take several).
        let mut values: Vec<OsString> = inline.into_iter().collect();
        let inline_value = !values.is_empty();
        while !inline_value
            && it
                .peek()
                .is_some_and(|n| !n.to_string_lossy().starts_with("--"))
        {
            values.extend(it.next().cloned());
        }
        if OWN.contains(&name.as_str()) {
            if values.len() != 1 {
                return Err(format!("--{name} takes exactly one value"));
            }
            if own.iter().any(|(k, _)| *k == name) {
                return Err(format!("--{name} given twice"));
            }
            own.push((name, values.remove(0)));
            continue;
        }
        let target = comps
            .iter()
            .position(|(p, _)| name.strip_prefix(p).is_some_and(|r| r.starts_with('-')))
            .ok_or_else(|| format!("unknown flag --{name}"))?;
        let (prefix, cmd) = comps[target];
        let stripped = &name[prefix.len() + 1..];
        // `--ui-password-file` is the UI's own flag name, not `--password-file` of the UI.
        let real = [stripped, name.as_str()]
            .into_iter()
            .find(|n| has_flag(cmd, n))
            .ok_or_else(|| {
                format!("unknown flag --{name} (a program's flags are written --{prefix}-<flag>)")
            })?;
        if real == "hash-password" {
            return Err(format!(
                "--{name}: that is a one-shot of wayhouse-ui; run `wayhouse-ui --hash-password`"
            ));
        }
        // A shared flag has one value for the process; the UI's own TLS pair is the exception.
        if OWN.contains(&real) && !(prefix == "ui" && real.starts_with("tls-")) {
            return Err(format!("--{name}: give the shared --{real} instead"));
        }
        if prefix == "ui" && (real.starts_with("controller-") || real.starts_with("aggregator-")) {
            return Err(format!("--{name} is set by wayhouse-control"));
        }
        forwarded[target].push(format!("--{real}").into());
        forwarded[target].extend(values);
    }
    let own_value = |n: &str| {
        own.iter()
            .rev()
            .find(|(k, _)| k == n)
            .map(|(_, v)| v.clone())
    };
    let tls = own_value("tls-cert").is_some();
    for (target, (prefix, _)) in comps.iter().enumerate() {
        // The UI is what an operator's browser meets: its TLS is its own choice (`--ui-tls-cert`).
        let shared: &[&str] = if *prefix == "ui" {
            &["settings-dir"]
        } else {
            &["settings-dir", "tls-cert", "tls-key"]
        };
        for n in shared {
            if let Some(v) = own_value(n) {
                forwarded[target].push(format!("--{n}").into());
                forwarded[target].push(v);
            }
        }
        forwarded[target].insert(0, format!("wayhouse-{prefix}").into());
    }
    if let Some(dir) = credentials {
        for (target, (prefix, cmd)) in comps.iter().enumerate() {
            for flag in secret_file_flags(cmd) {
                let file = dir.join(format!("{prefix}-{}", flag.trim_end_matches("-file")));
                if value_of(&forwarded[target], flag).is_none() && file.is_file() {
                    forwarded[target].push(format!("--{flag}").into());
                    forwarded[target].push(file.into());
                }
            }
        }
    }
    // The UI is what a browser meets: pinned like the others, and off loopback only with its own
    // certificate (a login over plain http would send the password in the clear).
    let ui_addr: SocketAddr = match value_of(&forwarded[2], "listen") {
        Some(l) => l.parse().map_err(|e| format!("--ui-listen {l}: {e}"))?,
        None => {
            let d = addr_flag(&programs.ui)?;
            forwarded[2].extend(["--listen".into(), d.clone().into()]);
            d.parse().map_err(|e| format!("--ui-listen {d}: {e}"))?
        }
    };
    if !ui_addr.ip().is_loopback() && value_of(&forwarded[2], "tls-cert").is_none() {
        return Err(format!(
            "--ui-listen {ui_addr} is not loopback: give the UI its own certificate \
             (--ui-tls-cert and --ui-tls-key), or keep it on loopback"
        ));
    }
    // The UI reaches the other two over loopback with the tokens that gate them.
    let scheme = if tls { "https" } else { "http" };
    let mut derived = Vec::new();
    for (target, flag) in [(0, "controller"), (1, "aggregator")] {
        let addr = loopback_addr(comps[target].1, &forwarded[target])?;
        // The UI is told this address, so the program must serve it: the command line (and this
        // default) beats a `listen` in the settings file, which is applied after this plan.
        if value_of(&forwarded[target], "listen").is_none() {
            forwarded[target].extend(["--listen".into(), addr_flag(comps[target].1)?.into()]);
        }
        derived.extend([
            format!("--{flag}-url").into(),
            format!("{scheme}://{addr}").into(),
        ]);
        if let Some(t) = value_of(&forwarded[target], "auth-token-file") {
            derived.extend([format!("--{flag}-token-file").into(), t.into()]);
        }
    }
    forwarded[2].extend(derived);
    let [controller, aggregator, ui] = forwarded;
    Ok(Plan {
        controller,
        aggregator,
        ui,
        ca_file: own_value("ca-file").map(Into::into),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(args: &[&str]) -> Result<Plan, String> {
        let argv: Vec<OsString> = std::iter::once("wayhouse-control")
            .chain(args.iter().copied())
            .map(Into::into)
            .collect();
        plan(&argv, &Programs::linked(), None)
    }

    fn strs(v: &[OsString]) -> Vec<String> {
        v.iter().map(|s| s.to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn a_namespaced_flag_reaches_only_its_program() {
        let p = run(&[
            "--controller-data-dir=/d",
            "--aggregator-listen",
            "127.0.0.1:1",
            "--ui-listen",
            "127.0.0.1:2",
            "--ui-ui-password-file",
            "/pw",
        ])
        .unwrap();
        let c = strs(&p.controller);
        assert_eq!(c[0], "wayhouse-controller");
        assert!(c.windows(2).any(|w| w == ["--data-dir", "/d"]));
        let a = strs(&p.aggregator);
        assert!(a.windows(2).any(|w| w == ["--listen", "127.0.0.1:1"]));
        assert!(!a.iter().any(|x| x.contains("data-dir")));
        let u = strs(&p.ui);
        assert!(u.windows(2).any(|w| w == ["--listen", "127.0.0.1:2"]));
        assert!(u.windows(2).any(|w| w == ["--ui-password-file", "/pw"]));
    }

    #[test]
    fn shared_flags_go_where_they_apply_and_the_ca_file_to_none() {
        let p = run(&[
            "--settings-dir",
            "/s",
            "--tls-cert",
            "/c",
            "--tls-key",
            "/k",
            "--ca-file",
            "/ca",
        ])
        .unwrap();
        for (v, tls) in [(&p.controller, true), (&p.aggregator, true), (&p.ui, false)] {
            let v = strs(v);
            assert!(v.windows(2).any(|w| w == ["--settings-dir", "/s"]), "{v:?}");
            assert_eq!(v.contains(&"--tls-cert".to_string()), tls, "{v:?}");
            assert_eq!(v.contains(&"--tls-key".to_string()), tls, "{v:?}");
            assert!(!v.iter().any(|a| a.contains("ca-file")), "{v:?}");
        }
        assert_eq!(p.ca_file, Some("/ca".into()));
    }

    #[test]
    fn the_ui_is_pointed_at_the_others_with_their_own_tokens() {
        let p = run(&[
            "--controller-listen",
            "0.0.0.0:7001",
            "--controller-auth-token-file",
            "/ct",
            "--aggregator-auth-token-file",
            "/at",
        ])
        .unwrap();
        let u = strs(&p.ui);
        let has = |f: &str, v: &str| u.windows(2).any(|w| w == [f, v]);
        assert!(has("--controller-url", "http://127.0.0.1:7001"), "{u:?}");
        assert!(has("--aggregator-url", "http://127.0.0.1:9902"), "{u:?}");
        assert!(has("--controller-token-file", "/ct"), "{u:?}");
        assert!(has("--aggregator-token-file", "/at"), "{u:?}");
    }

    #[test]
    fn tls_makes_the_loopback_urls_https() {
        let p = run(&["--tls-cert", "/c", "--tls-key", "/k"]).unwrap();
        let u = strs(&p.ui);
        assert!(u.contains(&"https://127.0.0.1:9901".into()), "{u:?}");
        assert!(u.contains(&"https://127.0.0.1:9902".into()), "{u:?}");
    }

    #[test]
    fn a_per_program_credential_stands_in_for_its_token_file_and_reaches_the_ui() {
        let d = std::env::temp_dir().join(format!("wh-control-creds-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        for n in [
            "controller-auth-token",
            "aggregator-auth-token",
            "ui-ui-password",
        ] {
            std::fs::write(d.join(n), "x").unwrap();
        }
        let argv: Vec<OsString> = ["wayhouse-control", "--aggregator-auth-token-file", "/given"]
            .map(Into::into)
            .into();
        let p = plan(&argv, &Programs::linked(), Some(&d)).unwrap();
        let has = |v: &[OsString], f: &str, x: &Path| {
            v.windows(2).any(|w| w[0] == f && Path::new(&w[1]) == x)
        };
        assert!(has(
            &p.controller,
            "--auth-token-file",
            &d.join("controller-auth-token")
        ));
        // The command line wins over a credential of the same flag.
        assert!(has(&p.aggregator, "--auth-token-file", Path::new("/given")));
        assert!(has(&p.ui, "--ui-password-file", &d.join("ui-ui-password")));
        // The UI presents each program's own token to it.
        assert!(has(
            &p.ui,
            "--controller-token-file",
            &d.join("controller-auth-token")
        ));
        assert!(has(&p.ui, "--aggregator-token-file", Path::new("/given")));
        // No credential for a flag means no flag.
        assert!(!p.controller.iter().any(|a| a == "--proxy-token-file"));
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn the_listen_address_the_ui_is_told_is_pinned_so_a_settings_file_cannot_move_it() {
        let p = run(&["--aggregator-listen", "127.0.0.1:7002"]).unwrap();
        let v = strs(&p.controller);
        assert!(
            v.windows(2).any(|w| w == ["--listen", "127.0.0.1:9901"]),
            "{v:?}"
        );
        let v = strs(&p.aggregator);
        assert!(
            v.windows(2).any(|w| w == ["--listen", "127.0.0.1:7002"]),
            "{v:?}"
        );
        assert_eq!(v.iter().filter(|a| *a == "--listen").count(), 1, "{v:?}");
    }

    #[test]
    fn flags_that_would_be_duplicated_or_never_return_are_refused_with_a_pointer() {
        for (bad, hint) in [
            (&["--controller-tls-cert", "/c"][..], "--tls-cert"),
            (&["--aggregator-tls-key", "/k"], "--tls-key"),
            (&["--ui-settings-dir", "/s"], "--settings-dir"),
            (&["--controller-ca-file", "/ca"], "--ca-file"),
            (&["--tls-cert", "/a", "--tls-cert", "/b"], "twice"),
            (&["--settings-dir"], "exactly one"),
            (&["--ui-hash-password"], "wayhouse-ui --hash-password"),
        ] {
            let e = run(bad).unwrap_err();
            assert!(e.contains(hint), "{bad:?}: {e}");
        }
        // The UI's own certificate pair is allowed.
        let p = run(&["--ui-tls-cert", "/c", "--ui-tls-key", "/k"]).unwrap();
        assert!(strs(&p.ui).contains(&"--tls-cert".to_string()));
        assert!(!strs(&p.controller).contains(&"--tls-cert".to_string()));
    }

    #[test]
    fn an_environment_secret_shared_by_name_is_refused() {
        let env = |pairs: &[(&str, &str)]| {
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect::<Vec<_>>()
                .into_iter()
        };
        assert!(
            refuse_shared_secret_env(env(&[("PATH", "/bin"), ("WAYHOUSE_LOG", "info")])).is_ok()
        );
        let e = refuse_shared_secret_env(env(&[("WAYHOUSE_AUTH_TOKEN", "x")])).unwrap_err();
        assert!(e.contains("WAYHOUSE_AUTH_TOKEN"), "{e}");
        assert!(refuse_shared_secret_env(env(&[("WAYHOUSE_UI_PASSWORD", "x")])).is_err());
    }

    #[test]
    fn a_ui_reachable_from_other_hosts_needs_its_own_certificate() {
        let e = run(&["--ui-listen", "0.0.0.0:9903"]).unwrap_err();
        assert!(e.contains("--ui-tls-cert"), "{e}");
        // The shared certificate does not cover it, and a loopback UI needs none.
        assert!(run(&[
            "--ui-listen",
            "0.0.0.0:9903",
            "--tls-cert",
            "/c",
            "--tls-key",
            "/k"
        ])
        .is_err());
        assert!(run(&["--ui-listen", "127.0.0.1:9903"]).is_ok());
        assert!(run(&[
            "--ui-listen",
            "0.0.0.0:9903",
            "--ui-tls-cert",
            "/c",
            "--ui-tls-key",
            "/k"
        ])
        .is_ok());
    }

    #[test]
    fn the_ui_listen_address_is_pinned_too() {
        let p = run(&[]).unwrap();
        let v = strs(&p.ui);
        assert!(
            v.windows(2).any(|w| w == ["--listen", "127.0.0.1:9903"]),
            "{v:?}"
        );
    }

    /// The shipped unit must be a command line and credentials this program accepts, and the
    /// controller, aggregator and UI must each end up with their own secret files.
    /// Every credential source the unit loads must be a file `init --target systemd --control`
    /// writes, or the unit would not start on a fresh box.
    #[test]
    fn the_unit_loads_exactly_what_init_control_writes() {
        use wayhouse_controller::init::{run, InitArgs, Target};
        let d = std::env::temp_dir().join(format!("wh-control-init-{}", std::process::id()));
        let args = InitArgs {
            target: Target::Systemd,
            dir: Some(d.clone()),
            env_file: None,
            credentials_dir: None,
            tls_dir: None,
            sans: vec![],
            namespace: "wayhouse".into(),
            output: None,
            control: true,
        };
        run(&args, &mut Vec::new(), false, &mut Vec::new()).unwrap();
        let unit = include_str!("../../../deploy/systemd/wayhouse-control.service");
        let sources: Vec<&str> = unit
            .lines()
            .filter_map(|l| l.strip_prefix("LoadCredential="))
            .filter_map(|l| l.split_once(':').map(|(_, src)| src))
            .collect();
        assert_eq!(sources.len(), 7, "{sources:?}");
        for src in sources {
            let file = src.strip_prefix("/etc/wayhouse/").expect(src);
            assert!(d.join(file).is_file(), "init wrote no {file}");
        }
        for cert in [
            "--tls-cert=/etc/wayhouse/tls/control.pem",
            "--ca-file=/etc/wayhouse/tls/ca.pem",
        ] {
            let file = cert.split_once("/etc/wayhouse/").unwrap().1;
            assert!(unit.contains(cert) && d.join(file).is_file(), "{cert}");
        }
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn the_systemd_unit_is_accepted_and_wires_each_secret_to_its_program() {
        let unit = include_str!("../../../deploy/systemd/wayhouse-control.service");
        let d = std::env::temp_dir().join(format!("wh-control-unit-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        for id in unit
            .lines()
            .filter_map(|l| l.strip_prefix("LoadCredential="))
            .filter_map(|l| l.split(':').next())
        {
            std::fs::write(d.join(id), "x").unwrap();
        }
        let exec: String = unit
            .lines()
            .skip_while(|l| !l.starts_with("ExecStart="))
            .take_while(|l| l.ends_with('\\') || l.starts_with("ExecStart="))
            .map(|l| l.trim_end_matches('\\'))
            .collect::<Vec<_>>()
            .join(" ");
        let mut argv: Vec<OsString> = vec!["wayhouse-control".into()];
        argv.extend(
            exec.split_whitespace()
                .skip(1)
                .map(|a| a.replace("%d", &d.display().to_string()).into()),
        );
        let p = plan(&argv, &Programs::linked(), Some(&d)).unwrap();
        let file = |v: &[OsString], flag: &str, id: &str| {
            v.windows(2)
                .any(|w| w[0] == flag && Path::new(&w[1]) == d.join(id))
        };
        assert!(file(
            &p.controller,
            "--auth-token-file",
            "controller-auth-token"
        ));
        assert!(file(
            &p.controller,
            "--proxy-token-file",
            "controller-proxy-token"
        ));
        assert!(file(
            &p.controller,
            "--agent-token-file",
            "controller-agent-token"
        ));
        assert!(file(
            &p.aggregator,
            "--auth-token-file",
            "aggregator-auth-token"
        ));
        assert!(file(
            &p.aggregator,
            "--ingest-token-file",
            "aggregator-ingest-token"
        ));
        assert!(file(&p.ui, "--ui-password-file", "ui-ui-password"));
        assert!(file(
            &p.ui,
            "--controller-token-file",
            "controller-auth-token"
        ));
        assert!(strs(&p.ui).contains(&"https://127.0.0.1:9902".to_string()));
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn an_unknown_or_derived_flag_is_refused_by_name() {
        for bad in [
            "--bogus",
            "--controller-nope",
            "--ui-controller-url",
            "--controller-url",
        ] {
            let e = run(&[bad, "x"]).unwrap_err();
            assert!(e.contains(bad), "{bad}: {e}");
        }
        assert!(run(&["stray"]).is_err());
    }
}
