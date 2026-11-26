#!/usr/bin/env python3
"""Extracts the things the docs must cover from the source tree, without compiling anything.

  config_keys()    every key of the config schema (crates/wayhouse-config/schema/config.schema.json)
  cli_flags()      every clap argument of every command (derive structs under crates/)
  metrics()        every `pub const X: &str = "wayhouse_..."` with its doc comment
  cargo_features() every `[features]` entry of every crate

Used by sync_docs.py (generated Reference pages) and check_coverage.py. Stdlib only.
"""
import glob
import json
import os
import re

REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))


def _read(path):
    with open(path, encoding="utf-8") as f:
        return f.read()


def clean(text: str) -> str:
    """Rust doc comment -> one line of Markdown that is safe on a generated page."""
    text = re.sub(r"\[(`[^`]+`)\]\([^)]*\)", r"\1", text)  # [`x`](crate::y) -> `x`
    text = re.sub(r"\[(`[^`]+`)\]", r"\1", text)  # [`x`] -> `x`
    text = re.sub(r"\s+", " ", text).strip()
    parts = re.split(r"(`[^`]*`)", text)  # raw `<` and `{` outside code would parse as HTML/JSX
    for i in range(0, len(parts), 2):
        parts[i] = parts[i].replace("<", "&lt;").replace(">", "&gt;").replace("|", "\\|")
    parts[1::2] = [p.replace("|", "\\|") for p in parts[1::2]]
    return "".join(parts)


# ---------------------------------------------------------------- config keys

def config_keys():
    """Returns [{path, type, description, default, since}] in schema order."""
    schema = json.loads(_read(os.path.join(REPO, "crates/wayhouse-config/schema/config.schema.json")))
    defs = schema.get("$defs", {})
    out = []

    def resolve(node):
        while "$ref" in node:
            node = defs[node["$ref"].rsplit("/", 1)[1]]
        return node

    def variants(node):
        node = resolve(node)
        for key in ("allOf", "anyOf", "oneOf"):
            if key in node:
                return [resolve(n) for n in node[key]]
        return [node]

    def type_of(node):
        node = resolve(node)
        if "enum" in node:
            return " \\| ".join("`%s`" % v for v in node["enum"])
        t = node.get("type")
        if isinstance(t, list):
            t = " or ".join(x for x in t if x != "null")
        if t == "array" and "items" in node:
            return "list of " + type_of(node["items"])
        return t or ""

    def walk(node, prefix, seen):
        for v in variants(node):
            if v.get("type") == "array" and "items" in v:
                walk(v["items"], prefix + "[]", seen)
                continue
            for name, prop in sorted(v.get("properties", {}).items()) if False else v.get("properties", {}).items():
                path = (prefix + "." if prefix else "") + name
                p = resolve(prop)
                desc = prop.get("description") or p.get("description") or ""
                if not desc:
                    for alt in variants(prop):
                        desc = alt.get("description", "") or desc
                out.append({
                    "path": path,
                    "type": type_of(prop),
                    "description": desc,
                    "default": prop.get("default", p.get("default")),
                    "since": prop.get("x-wayhouse-since", ""),
                })
                ref = prop.get("$ref") or next((n.get("$ref") for n in prop.get("allOf", prop.get("anyOf", [])) if "$ref" in n), None)
                if ref in seen:
                    continue  # recursive type
                walk(prop, path, seen | ({ref} if ref else set()))

    walk(schema, "", set())
    return out


# ------------------------------------------------------------------ CLI flags

STRUCT = re.compile(r"^[ \t]*(?:pub(?:\([a-z]+\))? )?(struct|enum) (\w+)[^{;]*\{", re.M)


def _block(text, start):
    """Text of the {...} block whose opening brace is at text[start - 1]."""
    depth, i = 1, start
    while depth and i < len(text):
        if text[i] == "{":
            depth += 1
        elif text[i] == "}":
            depth -= 1
        i += 1
    return text[start : i - 1]


def _doc(lines):
    return " ".join(l.strip()[3:].strip() for l in lines if l.strip().startswith("///"))


def _attr_args(attr):
    """`#[arg(long, env = "X", default_value = "y")]` -> {'long': True, 'env': 'X', ...}."""
    inner = re.search(r"\((.*)\)\s*\]", attr, re.S)
    args = {}
    if not inner:
        return args
    for key, val in re.findall(r'(\w+)\s*(?:=\s*("(?:[^"\\]|\\.)*"|[^,]+))?', inner.group(1)):
        args[key] = val.strip().strip('"') if val else True
    return args


def _fields(body, is_enum):
    """Yields (variant or '', doc lines, attribute text, declaration line) for each field."""
    depth, variant, pend_doc, pend_attr, attr_open = 0, "", [], [], False
    for line in body.split("\n"):
        s = line.strip()
        d0 = depth
        depth += sum(line.count(c) for c in "{(") - sum(line.count(c) for c in "})")
        if attr_open:
            pend_attr[-1] += " " + s
            attr_open = s.count("[") + pend_attr[-1].count("[") > pend_attr[-1].count("]") and not s.endswith("]")
            continue
        if s.startswith("///"):
            pend_doc.append(s)
        elif s.startswith("#["):
            pend_attr.append(s)
            attr_open = s.count("[") > s.count("]")
        elif s and not s.startswith("//") and s not in ("}", "},", ")", "),"):
            if is_enum and d0 == 0:
                variant = re.match(r"\w+", s).group(0) if re.match(r"\w+", s) else ""
                yield variant, "", pend_doc, " ".join(pend_attr), s, True
            elif (not is_enum and d0 == 0) or (is_enum and d0 == 1):
                yield variant, "", pend_doc, " ".join(pend_attr), s, False
            pend_doc, pend_attr = [], []
        elif not s:
            pend_doc, pend_attr = [], []


def cli_flags():
    """Returns [{crate, source, struct, kind, doc, items: [{flag, short, env, default, doc, cmd}]}]."""
    out = []
    for path in sorted(glob.glob(os.path.join(REPO, "crates/*/src/**/*.rs"), recursive=True)):
        text = _read(path)
        for m in STRUCT.finditer(text):
            pre = text[: m.start()].rsplit("\n\n", 1)[-1]
            if not re.search(r"derive\([^)]*\b(Parser|Args|Subcommand)\b", pre):
                continue
            rel = os.path.relpath(path, REPO).replace(os.sep, "/")
            entry = {"crate": rel.split("/")[1], "source": rel, "struct": m.group(2), "kind": m.group(1),
                     "doc": _doc(pre.split("\n")), "items": []}
            for variant, _, doc, attrs, decl, is_variant in _fields(_block(text, m.end()), m.group(1) == "enum"):
                name = re.match(r"(?:pub(?:\([a-z]+\))? )?(\w+)", decl)
                if not name:
                    continue
                if is_variant:
                    entry["items"].append({"flag": re.sub(r"(?<!^)(?=[A-Z])", "-", name.group(1)).lower(),
                                           "short": "", "env": "", "default": "", "doc": _doc(doc), "cmd": "", "sub": True})
                    continue
                if "arg(skip" in attrs.replace(" ", "") or "command(flatten" in attrs.replace(" ", "") or ":" not in decl:
                    continue
                a = _attr_args(next((x for x in attrs.split("#[") if x.startswith("arg")), ""))
                if a.get("long") is True:
                    flag = "--" + name.group(1).replace("_", "-")
                elif a.get("long"):
                    flag = "--" + a["long"]
                else:
                    flag = name.group(1) if "short" not in a else ""
                if not flag and "short" not in a:
                    continue
                entry["items"].append({
                    "flag": flag, "short": ("-" + a["short"]) if isinstance(a.get("short"), str) else "",
                    "env": a["env"] if isinstance(a.get("env"), str) else "",
                    "default": next((a[k] for k in ("default_value", "default_value_t") if isinstance(a.get(k), str)), ""),
                    "doc": _doc(doc), "cmd": variant, "sub": False,
                })
            if entry["items"]:
                out.append(entry)
    return out


# -------------------------------------------------------------------- metrics

METRIC = re.compile(r'((?:[ \t]*///[^\n]*\n)+)[ \t]*pub const (\w+): &str = "(wayhouse_[a-z0-9_]+)";')


def metrics():
    """Returns [{name, kind, doc, source}] sorted by name."""
    out = {}
    for path in sorted(glob.glob(os.path.join(REPO, "crates/*/src/**/*.rs"), recursive=True)):
        for m in METRIC.finditer(_read(path)):
            doc = _doc(m.group(1).split("\n"))
            kind = re.match(r"(Counter|Gauge|Histogram|Summary)", doc)
            out[m.group(3)] = {
                "name": m.group(3), "kind": kind.group(1).lower() if kind else "",
                "doc": doc, "source": os.path.relpath(path, REPO).replace(os.sep, "/"),
            }
    # A metric's doc comment starts with its type; other `wayhouse_*` strings (cookie names) are not metrics.
    return [out[k] for k in sorted(out) if out[k]["kind"]]


# ------------------------------------------------------------- cargo features

def cargo_features():
    """Returns [{crate, name, default, enables}]."""
    out = []
    for path in sorted(glob.glob(os.path.join(REPO, "crates/*/Cargo.toml"))):
        text = _read(path)
        m = re.search(r"^\[features\]\n(.*?)(?=^\[|\Z)", text, re.M | re.S)
        if not m:
            continue
        body = m.group(1)
        entries = {}
        for fm in re.finditer(r"^([\w-]+)\s*=\s*(\[.*?\])", body, re.M | re.S):
            entries[fm.group(1)] = re.findall(r'"([^"]+)"', fm.group(2))
        default = entries.pop("default", [])
        for name, enables in entries.items():
            out.append({
                "crate": os.path.basename(os.path.dirname(path)), "name": name,
                "default": name in default, "enables": [e for e in enables if not e.startswith("dep:")],
            })
    return out


def api_routes():
    """Returns [{crate, path, methods}]: every `.route("/path", get(..).post(..))` of the axum routers."""
    out = {}
    for path in sorted(glob.glob(os.path.join(REPO, "crates/*/src/**/*.rs"), recursive=True)):
        if "test" in os.path.basename(path):
            continue
        text = _read(path)
        crate = os.path.relpath(path, REPO).split(os.sep)[1]
        for m in re.finditer(r"\.route\(\s*\"(/[^\"]*)\"\s*,", text):
            depth, i = 1, m.end()
            while depth and i < len(text):
                depth += (text[i] == "(") - (text[i] == ")")
                i += 1
            methods = sorted(set(x.upper() for x in re.findall(r"\b(get|post|put|patch|delete)\(", text[m.end():i])))
            key = (crate, m.group(1))
            out.setdefault(key, set()).update(methods)
    return [{"crate": c, "path": p, "methods": sorted(v)} for (c, p), v in sorted(out.items())]


def ui_screens():
    """Returns [{path, component}] from the router of the UI frontend."""
    text = _read(os.path.join(REPO, "crates/wayhouse-ui/web/src/App.tsx"))
    return [{"path": p, "component": c} for p, c in re.findall(r'<Route path="([^"]+)" element=\{<(\w+)', text)]


def env_vars():
    """Returns [{name, source}]: environment variables the programs read (clap `env =` and `env::var`)."""
    out = {}
    for path in sorted(glob.glob(os.path.join(REPO, "crates/*/src/**/*.rs"), recursive=True)):
        if "test" in os.path.basename(path):
            continue
        text = _read(path)
        for name in re.findall(r'env\s*=\s*"([A-Z][A-Z0-9_]+)"', text) + \
                re.findall(r'env::var(?:_os)?\(\s*"([A-Z][A-Z0-9_]+)"', text) + \
                re.findall(r'"(WAYHOUSE_[A-Z][A-Z0-9_]+)"', text):
            if "_TEST_" not in name and "fleet-tests" not in path and name not in ("WAYHOUSE_GIT_SHA", "WAYHOUSE_LONG_VERSION"):
                out.setdefault(name, os.path.relpath(path, REPO).replace(os.sep, "/"))
    return [{"name": n, "source": s} for n, s in sorted(out.items())]


def runtime_flags():
    """The feature-flag registry (`features:` config section): [{name, scope, override, wired, protection, risk, impact}]."""
    text = _read(os.path.join(REPO, "crates/wayhouse-config/src/features.rs"))
    text = text[text.index("pub const REGISTRY"):]
    q = r'"((?:[^"\\]|\\.)*)"'
    out = []
    for m in re.finditer(r"flag\(\s*" + q + r",\s*(\w+),\s*(\w+),\s*" + q + r",\s*" + q + r"\s*,?\s*\)", text):
        pre = text[max(0, m.start() - 40): m.start()]
        out.append({
            "name": m.group(1), "scope": m.group(2).lower(), "override": "off only" if m.group(3) == "OffOnly" else "both",
            "wired": "wired(" in pre or "wired(\n" in pre, "protection": "protection(" in pre,
            "risk": m.group(4), "impact": m.group(5),
        })
    return out


if __name__ == "__main__":
    print(len(config_keys()), "config keys;", sum(len(c["items"]) for c in cli_flags()), "cli args in",
          len(cli_flags()), "commands;", len(metrics()), "metrics;", len(cargo_features()), "cargo features;", len(runtime_flags()), "runtime flags;", len(api_routes()), "api routes;",
          len(ui_screens()), "ui screens;", len(env_vars()), "env vars")
