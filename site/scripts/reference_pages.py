"""Renders the generated Reference pages (Markdown) from reference.py's extracts."""
import json

import reference as ref

SRC = "https://github.com/wayhouse-proxy/wayhouse/blob/main/"


def _page(title, description, intro, body):
    return '---\ntitle: %s\ndescription: %s\n---\n\n%s\n\n%s\n' % (json.dumps(title), json.dumps(description), intro, body)


def _cell(x):
    return ref.clean(str(x)) if x not in (None, "") else ""


def config_keys():
    rows = ["| Key | Type | Default | Since | Description |", "| --- | --- | --- | --- | --- |"]
    for k in ref.config_keys():
        default = "" if k["default"] is None else "`%s`" % json.dumps(k["default"]).replace("|", "\\|")
        desc = ref.clean(k["description"].split("\n\n")[0])
        rows.append("| `%s` | %s | %s | %s | %s |" % (k["path"], k["type"], default, _cell(k["since"]), desc))
    return _page(
        "Config keys", "Every key of the config file.",
        "Every key of the YAML config, generated from the schema in `crates/wayhouse-config/schema/`. "
        "`[]` marks a list entry. For what the sections mean and how they combine, see "
        "[Configuration](/deep-dive/design/configuration/).", "\n".join(rows))


def cli_flags():
    body = []
    for c in ref.cli_flags():
        title = "%s: %s" % (c["crate"], c["struct"])
        body.append("## %s\n" % title)
        body.append("Defined in [`%s`](%s%s)." % (c["source"], SRC, c["source"]) + (" " + ref.clean(c["doc"]) if c["doc"] else ""))
        has_env = any(i["env"] for i in c["items"])
        head = ["Argument", "Env", "Default", "Description"] if has_env else ["Argument", "Default", "Description"]
        body.append("\n| " + " | ".join(head) + " |\n| " + " | ".join("---" for _ in head) + " |")
        for i in c["items"]:
            name = i["flag"] + (" (%s)" % i["short"] if i["short"] else "")
            cells = ["`%s`" % name] + (["`%s`" % i["env"] if i["env"] else ""] if has_env else []) + \
                ["`%s`" % _cell(i["default"]) if i["default"] else "", ref.clean(i["doc"])]
            body.append("| " + " | ".join(cells) + " |")
        body.append("")
    return _page(
        "Command-line flags", "Every command-line argument of every binary.",
        "Every argument of every command, extracted from the clap definitions in the source. "
        "Subcommands are listed by name; their own arguments follow in their own table.", "\n".join(body))


def metrics():
    rows = ["| Metric | Type | Description |", "| --- | --- | --- |"]
    for m in ref.metrics():
        doc = m["doc"]
        for kind in ("Counter", "Gauge", "Histogram"):
            if doc.startswith(kind):
                doc = doc[len(kind):].lstrip(" .")
        rows.append("| `%s` | %s | %s |" % (m["name"], m["kind"], ref.clean(doc)))
    return _page(
        "Metrics", "Every Prometheus metric wayhouse exports.",
        "Every metric name, generated from the doc comments next to the name constants. "
        "For how to scrape and alert on them, see "
        "[Operations and observability](/deep-dive/design/operations-observability/).", "\n".join(rows))


def feature_flags():
    rows = ["| Flag | Scope | Override | Turning it off |", "| --- | --- | --- | --- |"]
    for f in ref.runtime_flags():
        notes = []
        if f["protection"]:
            notes.append("removes protection, so an override needs an expiry")
        if not f["wired"]:
            notes.append("accepted but not consulted yet")
        rows.append("| `%s` | %s | %s | %s %s %s |" % (
            f["name"], f["scope"], f["override"], ref.clean(f["risk"]), ref.clean(f["impact"]),
            ("*(%s)*" % ", ".join(notes)) if notes else ""))
    cargo = ["| Crate | Feature | On by default | Also enables |", "| --- | --- | --- | --- |"]
    for f in ref.cargo_features():
        cargo.append("| `%s` | `%s` | %s | %s |" % (f["crate"], f["name"], "yes" if f["default"] else "no",
                                                  ", ".join("`%s`" % e for e in f["enables"])))
    body = "## Runtime flags\n\nSwitched in the `features:` config section or by a controller override.\n\n" + \
        "\n".join(rows) + "\n\n## Cargo features\n\nChosen when the binary is built.\n\n" + "\n".join(cargo)
    return _page("Feature flags", "Runtime feature flags and Cargo features.",
                 "Two kinds of switch exist: runtime flags an operator flips, and Cargo features chosen at build time.", body)


PAGES = {
    "reference/config-keys": config_keys,
    "reference/cli-flags": cli_flags,
    "reference/metrics": metrics,
    "reference/feature-flags": feature_flags,
}
