#!/usr/bin/env python3
"""Fails when something users can see is not mentioned in the hand-written docs.

Checks that every config key, command-line flag, metric, runtime feature flag and Cargo feature
appears in the Markdown under docs/ (the generated Reference pages do not count, they list
everything by construction). Known gaps are listed in docs/coverage-allowlist.txt, one
`kind name` per line; an entry that is now documented is an error too, so the list only shrinks.

Usage: check_coverage.py [--write-allowlist] [--report]. Stdlib only.
"""
import glob
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import reference as ref  # noqa: E402

DOCS = os.path.join(ref.REPO, "docs")
ALLOWLIST = os.path.join(DOCS, "coverage-allowlist.txt")
# Dev tooling and test-only switches: not something an operator runs or sets.
DEV_CRATES = {"wayhouse-bench", "wayhouse-settings"}
USER_FEATURE_CRATES = {"wayhouse", "wayhouse-controller"}
SKIP = ("superpowers", "design-system", "reference")


def corpus():
    parts = []
    for path in glob.glob(os.path.join(DOCS, "**", "*.md"), recursive=True):
        if os.path.relpath(path, DOCS).split(os.sep)[0] in SKIP:
            continue
        with open(path, encoding="utf-8") as f:
            parts.append(f.read())
    return "\n".join(parts)


def items():
    """Yields (kind, name, regex that proves it is documented)."""
    for k in ref.config_keys():
        leaf = k["path"].rsplit(".", 1)[-1].replace("[]", "")
        yield "config", k["path"], re.compile(r"(?<![\w-])%s(?::|`)" % re.escape(leaf))
    seen = set()
    for c in ref.cli_flags():
        if c["crate"] in DEV_CRATES:
            continue
        for i in c["items"]:
            if i["flag"].startswith("--") and i["flag"] not in seen:
                seen.add(i["flag"])
                yield "flag", i["flag"], re.compile(r"(?<![\w-])%s(?![\w-])" % re.escape(i["flag"]))
    for m in ref.metrics():
        yield "metric", m["name"], re.compile(r"\b%s\b" % m["name"])
    for f in ref.runtime_flags():
        yield "feature", f["name"], re.compile(r"(?<![\w.])%s(?![\w])" % re.escape(f["name"]))
    done = set()
    for f in ref.cargo_features():
        if f["name"] not in done and f["crate"] in USER_FEATURE_CRATES:
            done.add(f["name"])
            yield "cargo-feature", f["name"], re.compile(r"`%s`" % re.escape(f["name"]))


def main(argv):
    text = corpus()
    missing = sorted("%s %s" % (k, n) for k, n, rx in items() if not rx.search(text))
    total = sum(1 for _ in items())
    if "--write-allowlist" in argv:
        with open(ALLOWLIST, "w", encoding="utf-8") as f:
            f.write("# Documentation gaps that are known. Shrink this list by documenting the item and\n"
                    "# deleting its line; check_coverage.py fails on a documented item that is still listed.\n"
                    "# Format: <kind> <name>\n")
            f.write("\n".join(missing) + "\n")
        print("wrote %d entries" % len(missing))
        return 0
    allowed = set()
    if os.path.exists(ALLOWLIST):
        with open(ALLOWLIST, encoding="utf-8") as f:
            allowed = {l.strip() for l in f if l.strip() and not l.startswith("#")}
    new = [m for m in missing if m not in allowed]
    stale = sorted(allowed - set(missing))
    print("%d of %d items documented, %d known gaps" % (total - len(missing), total, len(missing)))
    if "--report" in argv:
        by_kind = {}
        for m in missing:
            by_kind.setdefault(m.split()[0], []).append(m.split()[1])
        for k, v in sorted(by_kind.items()):
            print("%s (%d): %s" % (k, len(v), ", ".join(v)))
    for m in new:
        print("undocumented: %s  (document it in docs/, or add it to docs/coverage-allowlist.txt)" % m)
    for m in stale:
        print("now documented, remove from docs/coverage-allowlist.txt: %s" % m)
    return 1 if new or stale else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
