#!/usr/bin/env python3
"""Checks the Deep dive inventory (docs/deep-dive/topics.tsv) against the source tree.

Fails when a source file (Rust under crates/, the UI frontend, CI, deploy) belongs to no topic,
when a topic's globs match nothing, or when a `written` topic has no page. Prints how many
topics are written. `--list` prints the todo topics. Stdlib only.
"""
import fnmatch
import os
import subprocess
import sys

REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
TOPICS = os.path.join(REPO, "docs", "deep-dive", "topics.tsv")
TRACKED = ("crates/", ".github/", "deploy/", "site/")
SKIP = ("/tests/", "/testing/", "test_support", "_test.py", "/test/", ".test.", "/fixtures/", "web/node_modules")
EXTRA = {"release-please-config.json", "RELEASING.md"}


def source_files():
    out = subprocess.run(["git", "ls-files"], cwd=REPO, capture_output=True, text=True, check=True).stdout.split("\n")
    keep = []
    for f in out:
        if not f or any(s in f for s in SKIP) or f.endswith(("tests.rs", "Cargo.toml", ".md", ".lock", ".json", ".svg", ".png", ".woff2", ".txt")):
            if f not in EXTRA:
                continue
        if f.startswith(TRACKED) or f in EXTRA:
            if f.startswith("crates/") and not (f.endswith(".rs") or "/web/src/" in f or "/fuzz/" in f):
                continue
            if f.startswith("site/") and "node_modules" in f:
                continue
            keep.append(f)
    return keep


def load():
    topics = []
    with open(TOPICS, encoding="utf-8") as f:
        for n, line in enumerate(f, 1):
            if not line.strip() or line.startswith("#"):
                continue
            cols = line.rstrip("\n").split("\t")
            if len(cols) != 4 or cols[2] not in ("todo", "written"):
                sys.exit("%s:%d: expected slug, title, todo|written, globs" % (TOPICS, n))
            topics.append({"slug": cols[0], "title": cols[1], "status": cols[2], "globs": cols[3].split(",")})
    return topics


def main(argv):
    topics, files, errors = load(), source_files(), []
    covered = set()
    for t in topics:
        hits = {f for f in files if any(fnmatch.fnmatch(f, g) for g in t["globs"])}
        if not hits:
            errors.append("topic %s matches no file (globs: %s)" % (t["slug"], ", ".join(t["globs"])))
        covered |= hits
        if t["status"] == "written" and not os.path.exists(os.path.join(REPO, "docs", "deep-dive", "topics", t["slug"] + ".md")):
            errors.append("topic %s is written but docs/deep-dive/topics/%s.md does not exist" % (t["slug"], t["slug"]))
    config = open(os.path.join(REPO, "site", "astro.config.mjs"), encoding="utf-8").read()
    for t in topics:
        if t["status"] == "written" and 'slug: "deep-dive/topics/%s"' % t["slug"] not in config:
            errors.append("topic %s is not in a Deep dive section of site/astro.config.mjs" % t["slug"])
    for f in sorted(set(files) - covered):
        errors.append("no Deep dive topic covers %s (add it to docs/deep-dive/topics.tsv)" % f)
    done = sum(t["status"] == "written" for t in topics)
    print("Deep dive: %d of %d topics written, %d source files mapped" % (done, len(topics), len(covered)))
    if "--list" in argv:
        for t in topics:
            if t["status"] == "todo":
                print("  todo: %s (%s)" % (t["slug"], t["title"]))
    print("\n".join(errors))
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
