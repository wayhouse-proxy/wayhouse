//! The `wayhouse-registry-gen` binary, end to end.

use std::path::Path;
use std::process::{Command, Output};

const BIN: &str = env!("CARGO_BIN_EXE_wayhouse-registry-gen");

fn manifest(name: &str, version: &str) -> String {
    format!(
        r#"name = "{name}"
description = "{name} sniffer"
license = "MIT"
version = "{version}"
abi = "0.1"
min_proxy = "0.1.0"
categories = ["game"]
[limits]
max_memory_bytes = 1048576
call_timeout_ms = 50
"#
    )
}

/// `<root>/<name>/{manifest.toml, <name>.wasm}` with a module distinct per `marker`.
fn sniffer_dir(
    root: &Path,
    name: &str,
    version: &str,
    marker: &str,
    sig: bool,
) -> std::path::PathBuf {
    let dir = root.join(format!("{name}-{version}"));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("manifest.toml"), manifest(name, version)).unwrap();
    let wasm = wat::parse_str(format!(
        r#"(module (memory 1) (func (export "{marker}")) (@custom "wayhouse.abi" "\00\00\01\00"))"#
    ))
    .unwrap();
    std::fs::write(dir.join(format!("{name}.wasm")), wasm).unwrap();
    if sig {
        std::fs::write(dir.join(format!("{name}.wasm.minisig")), "sig").unwrap();
    }
    dir
}

fn run(args: &[&std::ffi::OsStr]) -> Output {
    Command::new(BIN).args(args).output().unwrap()
}

fn gen_args<'a>(
    out: &'a Path,
    previous: Option<&'a Path>,
    dirs: &'a [&'a Path],
) -> Vec<&'a std::ffi::OsStr> {
    let mut a: Vec<&std::ffi::OsStr> = vec![
        "generate".as_ref(),
        "--name".as_ref(),
        "test registry".as_ref(),
    ];
    a.extend([
        "--base-url".as_ref(),
        "https://example.com/dl".as_ref(),
        "--out".as_ref(),
        out.as_os_str(),
    ]);
    if let Some(p) = previous {
        a.extend(["--previous".as_ref(), p.as_os_str()]);
    }
    a.extend(dirs.iter().map(|d| d.as_os_str()));
    a
}

#[test]
fn generate_writes_a_parseable_index_and_prints_what_changed() {
    let t = tempfile::tempdir().unwrap();
    let d = sniffer_dir(t.path(), "a2s", "0.1.0", "x", true);
    let out = t.path().join("index.json");
    let r = run(&gen_args(&out, None, &[&d]));
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
    assert!(String::from_utf8_lossy(&r.stdout).contains("added a2s 0.1.0"));
    let index = wayhouse_registry::parse_index(&std::fs::read(&out).unwrap()).unwrap();
    let v = &index.sniffers[0].versions[0];
    assert_eq!(v.url, "https://example.com/dl/a2s-v0.1.0/a2s.wasm");
    assert_eq!(
        v.signature_url.as_deref(),
        Some("https://example.com/dl/a2s-v0.1.0/a2s.wasm.minisig")
    );
}

#[test]
fn generate_with_previous_adds_a_version_and_can_overwrite_the_same_file() {
    let t = tempfile::tempdir().unwrap();
    let out = t.path().join("index.json");
    let d1 = sniffer_dir(t.path(), "a2s", "0.1.0", "x", false);
    assert!(run(&gen_args(&out, None, &[&d1])).status.success());
    let d2 = sniffer_dir(t.path(), "a2s", "0.2.0", "y", false);
    let r = run(&gen_args(&out, Some(&out), &[&d2]));
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
    let index = wayhouse_registry::parse_index(&std::fs::read(&out).unwrap()).unwrap();
    assert_eq!(index.sniffers[0].versions.len(), 2);
}

#[test]
fn generate_refuses_to_change_a_published_version() {
    let t = tempfile::tempdir().unwrap();
    let out = t.path().join("index.json");
    let d1 = sniffer_dir(t.path(), "a2s", "0.1.0", "x", false);
    assert!(run(&gen_args(&out, None, &[&d1])).status.success());
    let before = std::fs::read(&out).unwrap();
    let rebuilt = t.path().join("rebuilt");
    let d2 = sniffer_dir(&rebuilt, "a2s", "0.1.0", "other", false);
    let r = run(&gen_args(&out, Some(&out), &[&d2]));
    assert!(!r.status.success());
    assert!(String::from_utf8_lossy(&r.stderr).contains("immutable"));
    assert_eq!(
        std::fs::read(&out).unwrap(),
        before,
        "a failed run must not touch the index"
    );
}

#[test]
fn an_existing_out_file_is_the_previous_index_when_none_is_given() {
    let t = tempfile::tempdir().unwrap();
    let out = t.path().join("index.json");
    let d1 = sniffer_dir(t.path(), "a2s", "0.1.0", "x", false);
    assert!(run(&gen_args(&out, None, &[&d1])).status.success());
    let before = std::fs::read(&out).unwrap();
    // Same version, different bytes, and no --previous: must still be refused.
    let rebuilt = t.path().join("rebuilt");
    let d2 = sniffer_dir(&rebuilt, "a2s", "0.1.0", "other", false);
    let r = run(&gen_args(&out, None, &[&d2]));
    assert!(!r.status.success());
    assert!(String::from_utf8_lossy(&r.stderr).contains("immutable"));
    assert_eq!(std::fs::read(&out).unwrap(), before);
    // A new version keeps the old one instead of replacing the index.
    let d3 = sniffer_dir(t.path(), "a2s", "0.2.0", "y", false);
    assert!(run(&gen_args(&out, None, &[&d3])).status.success());
    let index = wayhouse_registry::parse_index(&std::fs::read(&out).unwrap()).unwrap();
    assert_eq!(index.sniffers[0].versions.len(), 2);
}

#[test]
fn verify_accepts_a_generated_index_and_rejects_garbage() {
    let t = tempfile::tempdir().unwrap();
    let out = t.path().join("index.json");
    let d = sniffer_dir(t.path(), "a2s", "0.1.0", "x", false);
    assert!(run(&gen_args(&out, None, &[&d])).status.success());
    assert!(run(&["verify".as_ref(), out.as_os_str()]).status.success());
    let bad = t.path().join("bad.json");
    std::fs::write(&bad, "{}").unwrap();
    let r = run(&["verify".as_ref(), bad.as_os_str()]);
    assert!(!r.status.success());
    assert!(!String::from_utf8_lossy(&r.stderr).is_empty());
}

fn with_extra<'a>(
    out: &'a Path,
    extra: &[&'a std::ffi::OsStr],
    dir: &'a Path,
) -> Vec<&'a std::ffi::OsStr> {
    let mut a = gen_args(out, None, &[]);
    a.extend(extra.iter().copied());
    a.push(dir.as_os_str());
    a
}

#[test]
fn catalog_mode_writes_detail_files_and_a_rerun_keeps_old_versions() {
    let t = tempfile::tempdir().unwrap();
    let out = t.path().join("index.json");
    let details = t.path().join("details");
    let cats = t.path().join("categories.toml");
    std::fs::write(&cats, "categories = [\"game\", \"voip\"]\n").unwrap();
    let extra: Vec<&std::ffi::OsStr> = vec![
        "--details-dir".as_ref(),
        details.as_os_str(),
        "--details-base-url".as_ref(),
        "https://example.com/d".as_ref(),
        "--categories".as_ref(),
        cats.as_os_str(),
        "--date".as_ref(),
        "2026-11-12".as_ref(),
    ];
    let d1 = sniffer_dir(t.path(), "a2s", "0.1.0", "x", false);
    let r = run(&with_extra(&out, &extra, &d1));
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
    let index = wayhouse_registry::parse_index(&std::fs::read(&out).unwrap()).unwrap();
    let entry = &index.sniffers[0];
    assert!(entry.versions.is_empty());
    assert_eq!(
        entry.detail_url.as_deref(),
        Some("https://example.com/d/a2s.json")
    );
    assert_eq!(
        (entry.latest.to_string(), entry.updated.as_str()),
        ("0.1.0".into(), "2026-11-12")
    );
    // The second run reads the old versions back through the detail file.
    let d2 = sniffer_dir(t.path(), "a2s", "0.2.0", "y", false);
    assert!(run(&with_extra(&out, &extra, &d2)).status.success());
    let detail = std::fs::read(details.join("a2s.json")).unwrap();
    let index = wayhouse_registry::parse_index(&std::fs::read(&out).unwrap()).unwrap();
    let merged = wayhouse_registry::parse_detail(&detail, &index.sniffers[0]).unwrap();
    assert_eq!(merged.versions.len(), 2);
    let ok = run(&[
        "verify".as_ref(),
        out.as_os_str(),
        "--details-dir".as_ref(),
        details.as_os_str(),
    ]);
    assert!(
        ok.status.success(),
        "{}",
        String::from_utf8_lossy(&ok.stderr)
    );
    assert!(!run(&["verify".as_ref(), out.as_os_str()]).status.success());
}

#[test]
fn a_category_off_the_list_fails_and_writes_nothing() {
    let t = tempfile::tempdir().unwrap();
    let out = t.path().join("index.json");
    let cats = t.path().join("categories.toml");
    std::fs::write(&cats, "categories = [\"voip\"]\n").unwrap();
    let d = sniffer_dir(t.path(), "a2s", "0.1.0", "x", false);
    let dirs = [d.as_path()];
    let mut a = gen_args(&out, None, &dirs);
    a.extend(["--categories".as_ref(), cats.as_os_str()]);
    let r = run(&a);
    assert!(!r.status.success());
    assert!(String::from_utf8_lossy(&r.stderr).contains("unknown category"));
    assert!(!out.exists());
}

#[test]
fn a_schema_1_index_is_upgraded_and_keeps_its_published_versions() {
    let t = tempfile::tempdir().unwrap();
    let out = t.path().join("index.json");
    let d1 = sniffer_dir(t.path(), "a2s", "0.1.0", "x", false);
    assert!(run(&gen_args(&out, None, &[&d1])).status.success());
    // Rewrite what was just generated as a schema 1 file: no catalog fields.
    let mut v: serde_json::Value = serde_json::from_slice(&std::fs::read(&out).unwrap()).unwrap();
    v["schema"] = 1.into();
    let e = v["sniffers"][0].as_object_mut().unwrap();
    for k in ["categories", "tags", "latest", "abi", "signed", "updated"] {
        e.remove(k);
    }
    std::fs::write(&out, v.to_string()).unwrap();
    let before = v["sniffers"][0]["versions"].clone();
    // Same bytes under the same version: still immutable, nothing added.
    let r = run(&gen_args(&out, None, &[&d1]));
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
    assert!(String::from_utf8_lossy(&r.stderr).contains("upgrading"));
    let index = wayhouse_registry::parse_index(&std::fs::read(&out).unwrap()).unwrap();
    assert_eq!(
        serde_json::to_value(&index.sniffers[0].versions).unwrap(),
        before
    );
    // A rebuilt module under the old version is still refused.
    let rebuilt = t.path().join("rebuilt");
    let d2 = sniffer_dir(&rebuilt, "a2s", "0.1.0", "other", false);
    std::fs::write(&out, v.to_string()).unwrap();
    let r = run(&gen_args(&out, None, &[&d2]));
    assert!(!r.status.success());
    assert!(String::from_utf8_lossy(&r.stderr).contains("immutable"));
}

fn plugin_index(url: &str) -> String {
    format!(
        r#"{{"schema":2,"kind":"plugin","name":"p","plugins":[{{"name":"demo","description":"d","license":"MIT","categories":["integration"],"tags":[],"latest":"0.1.0","abi":"0.2","signed":false,"updated":"2026-11-12","versions":[{{"version":"0.1.0","abi":"0.2","url":"{url}","sha256":"{}","size":10}}]}}]}}"#,
        "a".repeat(64)
    )
}

#[test]
fn verify_plugin_runs_the_client_validator() {
    let t = tempfile::tempdir().unwrap();
    let good = t.path().join("good.json");
    std::fs::write(&good, plugin_index("https://example.com/p.wasm")).unwrap();
    let r = run(&["verify-plugin".as_ref(), good.as_os_str()]);
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
    assert!(String::from_utf8_lossy(&r.stdout).contains("1 plugins, 1 versions"));
    for bad_url in [
        "https://@example.com/p",
        "http://example.com/p",
        "https://x.test:99999/p",
    ] {
        let bad = t.path().join("bad.json");
        std::fs::write(&bad, plugin_index(bad_url)).unwrap();
        assert!(
            !run(&["verify-plugin".as_ref(), bad.as_os_str()])
                .status
                .success(),
            "{bad_url}"
        );
    }
    // A sniffer index is not a plugin index.
    let sniffer = t.path().join("sniffer.json");
    let d = sniffer_dir(t.path(), "a2s", "0.1.0", "x", false);
    assert!(run(&gen_args(&sniffer, None, &[&d])).status.success());
    assert!(!run(&["verify-plugin".as_ref(), sniffer.as_os_str()])
        .status
        .success());
}
