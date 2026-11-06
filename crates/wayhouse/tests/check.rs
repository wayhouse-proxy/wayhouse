//! `wayhouse --check` is the pre-deploy gate: it must refuse every setting this build
//! cannot run, not only the ones `main` happens to construct before it returns
//! (issue #127). Each test runs the real binary against a config that needs one
//! optional cargo feature and asserts on the build's feature set: a build
//! without the feature refuses with a message naming it, a build with it accepts.

use std::process::{Command, Output};

const BASE: &str = "pools:\n  - name: p\n    targets: [\"127.0.0.1:1\"]\n\
                    listeners:\n  - name: l\n    bind: \"127.0.0.1:0\"\n    pool: p\n";

fn check(yaml: &str, extra: &[&str]) -> Output {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("wayhouse.yaml");
    std::fs::write(&path, yaml).unwrap();
    Command::new(env!("CARGO_BIN_EXE_wayhouse"))
        .arg("--check")
        .arg("--config")
        .arg(&path)
        .args(extra)
        .output()
        .unwrap()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn a_plain_config_passes() {
    let out = check(BASE, &[]);
    assert!(out.status.success(), "{}", stderr(&out));
}

#[test]
fn a_grpc_resolver_needs_the_grpc_resolver_feature() {
    let yaml = format!(
        "resolvers:\n  - name: mm\n    type: grpc\n    endpoint: \"http://127.0.0.1:1\"\n{BASE}"
    );
    let out = check(&yaml, &[]);
    if cfg!(feature = "grpc-resolver") {
        assert!(out.status.success(), "{}", stderr(&out));
    } else {
        assert!(!out.status.success());
        assert!(
            stderr(&out).contains("`grpc-resolver` cargo feature"),
            "{}",
            stderr(&out)
        );
    }
}

#[test]
fn a_dns_srv_source_needs_the_dns_srv_feature() {
    let yaml =
        "backend_sources:\n  - name: s\n    type: dns_srv\n    record: \"_g._udp.example.com\"\n\
                pools:\n  - name: p\n    source: s\n\
                listeners:\n  - name: l\n    bind: \"127.0.0.1:0\"\n    pool: p\n";
    let out = check(yaml, &[]);
    if cfg!(feature = "dns-srv") {
        assert!(out.status.success(), "{}", stderr(&out));
    } else {
        assert!(!out.status.success());
        assert!(
            stderr(&out).contains("`dns-srv` cargo feature"),
            "{}",
            stderr(&out)
        );
    }
}

#[cfg(not(feature = "tunnel"))]
mod without_tunnel {
    use super::*;

    #[test]
    fn the_tunnel_flags_are_refused_naming_the_feature() {
        let out = check(
            BASE,
            &[
                "--tunnel-iface",
                "wayhouse0",
                "--tunnel-controller-url",
                "http://127.0.0.1:1",
                "--tunnel-name",
                "p1",
                "--tunnel-endpoint",
                "203.0.113.1:51820",
            ],
        );
        assert!(!out.status.success());
        assert!(
            stderr(&out).contains("`tunnel` cargo feature"),
            "{}",
            stderr(&out)
        );
    }

    #[test]
    fn a_tunnel_source_is_refused_naming_the_feature() {
        let yaml = "backend_sources:\n  - name: s\n    type: tunnel\n    \
                    pubkey: \"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=\"\n\
                    pools:\n  - name: p\n    source: s\n\
                    listeners:\n  - name: l\n    bind: \"127.0.0.1:0\"\n    pool: p\n";
        let out = check(yaml, &[]);
        assert!(!out.status.success());
        assert!(
            stderr(&out).contains("`tunnel` cargo feature"),
            "{}",
            stderr(&out)
        );
    }
}

#[test]
fn a_plugin_overlay_url_goes_with_a_config_file_and_its_token_needs_it() {
    // A file config may take plugin routes from a controller.
    let out = check(BASE, &["--plugin-overlay-url", "http://127.0.0.1:9"]);
    assert!(out.status.success(), "{}", stderr(&out));
    // A token alone is a mistake, and so is a second config source.
    let out = check(BASE, &["--plugin-overlay-token", "x"]);
    assert!(!out.status.success());
    let out = check(
        BASE,
        &[
            "--plugin-overlay-url",
            "http://127.0.0.1:9",
            "--controller",
            "http://127.0.0.1:9",
        ],
    );
    assert!(!out.status.success());
}

#[test]
fn a_plugin_overlay_url_is_checked_at_startup() {
    // No scheme (the typo that would otherwise only show up as "no plugin routes").
    let out = check(BASE, &["--plugin-overlay-url", "controller:9901"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("--plugin-overlay-url"),
        "{}",
        stderr(&out)
    );
    // A trailing slash is trimmed, a query is refused.
    let out = check(BASE, &["--plugin-overlay-url", "http://127.0.0.1:9/"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let out = check(BASE, &["--plugin-overlay-url", "http://127.0.0.1:9/?x=1"]);
    assert!(!out.status.success());
}

#[test]
fn flags_come_from_the_settings_file_and_a_check_does_not_use_up_a_try() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config.yaml"), BASE).unwrap();
    let file = serde_json::json!({
        "rev": 1,
        // An editable flag the file supplies; it must be a number, so a bad value shows that the
        // file's value reached the parser.
        "values": { "aggregator-interval-sec": "soon" },
        "previous": {},
        "pending": { "applied_at": wayhouse_settings::now(), "attempts": 1 },
    });
    let settings = dir.path().join("wayhouse.json");
    std::fs::write(&settings, file.to_string()).unwrap();
    let run = |extra: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_wayhouse"))
            .current_dir(dir.path())
            .args(["--check", "--settings-dir"])
            .arg(dir.path())
            .args(extra)
            .output()
            .unwrap()
    };
    // Not on the command line: the file supplies it, and the parser refuses it.
    let out = run(&[]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("aggregator-interval-sec"),
        "{}",
        stderr(&out)
    );
    // The command line wins over the file.
    let out = run(&["--aggregator-interval-sec", "5"]);
    assert!(out.status.success(), "{}", stderr(&out));
    // A check is not a start: the try counter is where it was.
    let after: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    assert_eq!(after["pending"]["attempts"], 1);
}
