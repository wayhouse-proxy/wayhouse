#!/usr/bin/env python3
"""Build the official plugins pinned in plugins.lock and run the host's conformance check on them.

    python3 .github/scripts/check_plugins.py [--lock plugins.lock] [--work target/plugins-src]

Clones `repo` at `rev` (the commit must be exactly the pinned one), builds every plugin for
wasm32-unknown-unknown, and runs `wayhouse-plugin-check` (built from this checkout) on each
module, with the plugin's `check-config.json` when it has one. A plugin built against another
ABI minor than this checkout's cannot load while the major is 0, so that case is a warning,
not a failure: it is what an ABI bump looks like until plugins.lock moves to the re-pinned
plugins commit. Any other failure fails the job.
"""
import argparse
import os
import pathlib
import re
import subprocess
import sys
import tomllib

HEX40 = re.compile(r"^[0-9a-f]{40}$")
REPO = re.compile(r"^wayhouse-proxy/[A-Za-z0-9_-][A-Za-z0-9_.-]*$")
SECTION = b"wayhouse.plugin-abi"


def load_lock(path):
    lock = tomllib.loads(pathlib.Path(path).read_text())
    repo, rev = lock.get("repo", ""), lock.get("rev", "")
    if not REPO.fullmatch(repo):
        raise ValueError(f"lock: bad repo {repo!r}")
    if not HEX40.fullmatch(rev):
        raise ValueError("lock: rev must be a 40 character lowercase commit sha")
    return repo, rev


def host_abi_minor(abi_source):
    m = re.search(r"pub const ABI_MINOR: u16 = (\d+);", abi_source)
    if not m:
        raise ValueError("ABI_MINOR not found in the plugin ABI crate")
    return int(m.group(1))


def _leb(data, i):
    n = shift = 0
    while True:
        if i >= len(data):
            raise ValueError("truncated module")
        b = data[i]
        i += 1
        n |= (b & 0x7F) << shift
        shift += 7
        if not b & 0x80:
            return n, i


def wasm_abi_minor(data):
    """The ABI minor a module declares in its `wayhouse.plugin-abi` custom section (walks the sections)."""
    if data[:8] != b"\0asm\1\0\0\0":
        raise ValueError("not a wasm module")
    i = 8
    while i < len(data):
        sid = data[i]
        size, i = _leb(data, i + 1)
        end = i + size
        if end > len(data):
            raise ValueError("truncated module")
        if sid == 0:
            nlen, j = _leb(data, i)
            if data[j : j + nlen] == SECTION:
                payload = data[j + nlen : end]
                if len(payload) != 4:
                    raise ValueError("bad wayhouse.plugin-abi section")
                major, minor = int.from_bytes(payload[:2], "little"), int.from_bytes(payload[2:], "little")
                if major != 0:
                    raise ValueError(f"unexpected ABI major {major}")
                return minor
        i = end
    raise ValueError("no wayhouse.plugin-abi section")


def run(cmd, **kw):
    print("+", " ".join(map(str, cmd)), flush=True)
    return subprocess.run(cmd, check=True, **kw)


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--lock", default="plugins.lock")
    ap.add_argument("--work", default="target/plugins-src")
    args = ap.parse_args()
    try:
        repo, rev = load_lock(args.lock)
        want = host_abi_minor(pathlib.Path("crates/wayhouse-plugin-abi/src/lib.rs").read_text())
    except (OSError, ValueError, tomllib.TOMLDecodeError) as e:
        print(f"{e}", file=sys.stderr)
        return 2
    work = pathlib.Path(args.work)
    if not work.is_dir():
        run(["git", "init", "-q", str(work)])
        run(["git", "-C", str(work), "remote", "add", "origin", f"https://github.com/{repo}"])
    run(["git", "-C", str(work), "fetch", "-q", "--depth", "1", "origin", rev])
    run(["git", "-C", str(work), "checkout", "-q", "--force", "--detach", "FETCH_HEAD"])
    run(["git", "-C", str(work), "clean", "-q", "-fdx", "-e", "target"])
    got = subprocess.run(["git", "-C", str(work), "rev-parse", "HEAD"], check=True, capture_output=True, text=True).stdout.strip()
    if got != rev:
        print(f"checked out {got}, plugins.lock pins {rev}", file=sys.stderr)
        return 1
    run(["cargo", "build", "--locked", "--release", "--target", "wasm32-unknown-unknown", "--workspace"], cwd=work)
    run(["cargo", "build", "-p", "wayhouse-plugin-host", "--bin", "wayhouse-plugin-check"])
    checker = pathlib.Path("target/debug/wayhouse-plugin-check").resolve()
    out = work / "target/wasm32-unknown-unknown/release"
    names = sorted(d.name for d in (work / "plugins").iterdir() if d.is_dir())
    wasms = [out / (n.replace("-", "_") + ".wasm") for n in names]
    missing = [w.name for w in wasms if not w.is_file()]
    if not wasms or missing:
        print("the plugins build produced no module for: " + ", ".join(missing or ["any plugin"]), file=sys.stderr)
        return 1
    failed = []
    checked = 0
    for w in wasms:
        name = w.stem.replace("_", "-")
        minor = wasm_abi_minor(w.read_bytes())
        if minor != want:
            print(f"::warning::{name} is built for ABI 0.{minor}, this checkout is 0.{want}: move plugins.lock to a plugins commit pinned to this ABI")
            continue
        cfg = work / "plugins" / name / "check-config.json"
        cmd = [str(checker), str(w)] + ([str(cfg)] if cfg.is_file() else [])
        print(f"== {name}", flush=True)
        checked += 1
        if subprocess.run(cmd).returncode != 0:
            failed.append(name)
    print(f"checked {checked} of {len(wasms)} modules")
    if checked == 0 and os.environ.get("GITHUB_EVENT_NAME") in ("schedule", "merge_group"):
        print("no module was checked: plugins.lock is behind this checkout's ABI", file=sys.stderr)
        return 1
    if failed:
        print("conformance failed: " + ", ".join(failed), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
