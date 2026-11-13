//! `wayhouse-control`: controller, aggregator and UI in one process, for a small install.
//!
//! The three programs are the standalone ones (`wayhouse_controller::serve::run` and friends),
//! each with its own listener, its own tokens and its own settings file; nothing is merged. This
//! crate only routes one command line to them ([`plan`]) and points the UI at the other two over
//! loopback with the same tokens a split deployment would use, so the code path is the same.
//!
//! Flags: `--settings-dir`, `--tls-cert`, `--tls-key` and `--ca-file` are this program's and
//! apply to all three. Everything else is namespaced: `--controller-X`, `--aggregator-X` and
//! `--ui-X` reach flag `--X` of that program (`--ui-ui-password-file` is not needed: a flag
//! already named `ui-...` is passed as it is). The UI's `--controller-url`, `--aggregator-url`
//! and their token files are not flags here; they are derived.

use std::ffi::OsString;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;

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
    cmd.get_arguments().any(|a| a.get_long() == Some(name))
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

pub fn plan(argv: &[OsString], programs: &Programs) -> Result<Plan, String> {
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
            let v = values
                .pop()
                .ok_or_else(|| format!("--{name} needs a value"))?;
            own.push((name, v));
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
        for n in ["settings-dir", "tls-cert", "tls-key"] {
            if let Some(v) = own_value(n) {
                forwarded[target].push(format!("--{n}").into());
                forwarded[target].push(v);
            }
        }
        forwarded[target].insert(0, format!("wayhouse-{prefix}").into());
    }
    // The UI reaches the other two over loopback with the tokens that gate them.
    let scheme = if tls { "https" } else { "http" };
    let mut derived = Vec::new();
    for (target, flag) in [(0, "controller"), (1, "aggregator")] {
        let addr = loopback_addr(comps[target].1, &forwarded[target])?;
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
        plan(&argv, &Programs::linked())
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
    fn shared_flags_go_to_all_three_and_the_ca_file_to_none() {
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
        for v in [&p.controller, &p.aggregator, &p.ui] {
            let v = strs(v);
            for (f, x) in [
                ("--settings-dir", "/s"),
                ("--tls-cert", "/c"),
                ("--tls-key", "/k"),
            ] {
                assert!(v.windows(2).any(|w| w == [f, x]), "{f} missing in {v:?}");
            }
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
