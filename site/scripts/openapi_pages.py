"""Renders an OpenAPI 3.1 document (generated from the Rust handlers) as a Markdown reference page.

The spec is the checked-in rendering of the code's own `#[utoipa::path]` annotations, so the page
cannot say something the handler does not: a CI test fails when the file is stale.
"""
import json
import os

DOCS = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "docs")
METHODS = ["get", "post", "put", "patch", "delete"]


def _ref(spec, node):
    """Follow `$ref` (only local `#/components/schemas/X` ones occur)."""
    while isinstance(node, dict) and "$ref" in node:
        node = spec["components"]["schemas"][node["$ref"].rsplit("/", 1)[1]]
    if isinstance(node, dict) and "allOf" in node:
        # a flattened struct: one object holding the properties of every part
        merged = {"type": "object", "properties": {}, "required": []}
        if "description" in node:
            merged["description"] = node["description"]
        for part in node["allOf"]:
            part = _ref(spec, part)
            merged["properties"].update(part.get("properties", {}))
            merged["required"] += part.get("required", [])
        return merged
    return node


def _type(spec, node):
    if "$ref" in node:
        return "`%s`" % node["$ref"].rsplit("/", 1)[1]
    if "enum" in node:
        return " \\| ".join("`%s`" % v for v in node["enum"])
    if "oneOf" in node or "anyOf" in node:
        alts = [a for a in node.get("oneOf", node.get("anyOf", [])) if a.get("type") != "null"]
        return " or ".join(_type(spec, a) for a in alts) or "any"
    t = node.get("type")
    if isinstance(t, list):
        t = " or ".join(x for x in t if x != "null") or "any"
    if t == "array":
        return "array of " + _type(spec, node.get("items", {}))
    if t == "integer" and node.get("format"):
        return "integer (%s)" % node["format"]
    if t == "string" and node.get("format") == "binary":
        return "bytes"
    return "`%s`" % t if t else "any"


def _one_line(text):
    return " ".join((text or "").split()).replace("|", "\\|")


def _example(spec, node, depth=0):
    """A sample value for a schema: its own example, else built from its properties."""
    node = _ref(spec, node)
    if "example" in node:
        return node["example"]
    if "enum" in node:
        return node["enum"][0]
    for key in ("oneOf", "anyOf"):
        if key in node:
            alts = [a for a in node[key] if a.get("type") != "null"]
            return _example(spec, alts[0], depth) if alts else None
    t = node.get("type")
    t = [x for x in t if x != "null"][0] if isinstance(t, list) else t
    if t == "object" and depth < 4:
        return {k: _example(spec, v, depth + 1) for k, v in node.get("properties", {}).items()
                if k in node.get("required", []) or "example" in _ref(spec, v)}
    if t == "array":
        return [_example(spec, node.get("items", {}), depth + 1)]
    return {"string": "string", "integer": 0, "number": 0, "boolean": True}.get(t)


def _props(spec, node):
    node = _ref(spec, node)
    if node.get("type") == "array":
        items = node.get("items", {})
        if "$ref" not in items:
            return None
        node = _ref(spec, items)
    props = node.get("properties")
    if not props:
        return None
    req = set(node.get("required", []))
    rows = ["| Field | Type | Required | Description |", "| --- | --- | --- | --- |"]
    for name, p in props.items():
        r = _ref(spec, p) if "$ref" in p else p
        desc = _one_line(p.get("description") or r.get("description"))
        extra = []
        if "default" in p:
            extra.append("default `%s`" % json.dumps(p["default"]))
        if "maximum" in p:
            extra.append("%s to %s" % (p.get("minimum", 0), p["maximum"]))
        if extra:
            desc = (desc + " " if desc else "") + "(" + ", ".join(extra) + ")"
        rows.append("| `%s` | %s | %s | %s |" % (name, _type(spec, p), "yes" if name in req else "no", desc))
    return "\n".join(rows)


def _params_table(params):
    rows = ["| Name | In | Type | Required | Description |", "| --- | --- | --- | --- | --- |"]
    for p in params:
        rows.append("| `%s` | %s | %s | %s | %s |" % (
            p["name"], p["in"], _type({}, p.get("schema", {})) if p.get("schema") else "",
            "yes" if p.get("required") else "no", _one_line(p.get("description"))))
    return "\n".join(rows)


def _json(v):
    return json.dumps(v, indent=2)


def _scheme(spec, op):
    """The name of the security scheme an operation asks for: `bearer` unless it names another."""
    sec = op.get("security")
    if sec and sec != [{}] and sec[0]:
        return next(iter(sec[0]))
    return "bearer"


def _curl(spec, method, path, op, base):
    url = base + path
    for p in op.get("parameters", []):
        if p["in"] == "path":
            ex = p.get("example") or (p.get("schema") or {}).get("example") or p["name"]
            url = url.replace("{%s}" % p["name"], str(ex))
    queries = []
    for p in op.get("parameters", []):
        if p["in"] == "query" and p.get("required"):
            ex = p.get("example") or (p.get("schema") or {}).get("example") or "value"
            queries.append("%s=%s" % (p["name"], ex))
    if queries:
        url += "?" + "&".join(queries)
    secured = op.get("security", spec.get("security", [])) != [{}] and op.get("security", spec.get("security"))
    parts = ["curl -sS"]
    if method != "get":
        parts.append("-X %s" % method.upper())
    if secured:
        var = "TOKEN" if _scheme(spec, op) == "bearer" else _scheme(spec, op).upper() + "_TOKEN"
        parts.append('-H "Authorization: Bearer $%s"' % var)
    for p in op.get("parameters", []):
        if p["in"] == "header" and p.get("required"):
            parts.append('-H "%s: %s"' % (p["name"], p.get("example") or (p.get("schema") or {}).get("example") or "value"))
    body = ""
    rb = op.get("requestBody")
    if rb:
        ctype, content = next(iter(rb["content"].items()))
        if ctype == "application/json":
            parts.append('-H "Content-Type: application/json"')
            body = _json(_example(spec, content["schema"]))
            parts.append("-d '%s'" % json.dumps(json.loads(body)))
        elif ctype == "application/octet-stream":
            parts.append("--data-binary @module.wasm")
        else:
            parts.append('-H "Content-Type: %s"' % ctype)
            parts.append("--data-binary @config.yaml")
    parts.append('"%s"' % url)
    return " ".join(parts[:-1]) + " " + parts[-1] if len(" ".join(parts)) < 100 else " \\\n  ".join(parts)


def _responses(spec, op):
    rows = ["| Status | Meaning |", "| --- | --- |"]
    examples = []
    for code, r in op["responses"].items():
        desc = _one_line(r.get("description"))
        hdrs = r.get("headers")
        if hdrs:
            desc += " Headers: " + ", ".join("`%s`" % h for h in hdrs) + "."
        content = r.get("content") or {}
        if content:
            ctype, c = next(iter(content.items()))
            schema = c.get("schema", {})
            kind = _type(spec, schema)
            desc += (" Body: %s `%s`." % (kind, ctype)) if ctype != "text/plain" and kind != "any" else ""
            ex = c.get("example", None)
            if ex is None and code.startswith("2") and (
                    "$ref" in schema or (schema.get("type") == "array" and "$ref" in schema.get("items", {}))):
                ex = _example(spec, schema)
            if ex is not None and (code.startswith("2") or "example" in c):
                examples.append((code, ctype, ex))
        rows.append("| `%s` | %s |" % (code, desc))
    return "\n".join(rows), examples


def _operation(spec, path, method, op, base):
    out = ["### `%s %s`\n" % (method.upper(), path)]
    out.append("**%s.**" % op.get("summary", "").rstrip("."))
    if op.get("description"):
        out.append("\n" + op["description"].strip())
    secured = op.get("security") != [{}]
    auth = "bearer token (when one is configured)" if secured else "none"
    if secured and "x-wayhouse-tokens" in op:
        extra = op["x-wayhouse-tokens"]
        auth = "the admin token" + (
            ", and the %s token%s" % (" and ".join(extra) if len(extra) < 3 else ", ".join(extra[:-1]) + " and " + extra[-1],
                                      "s" if len(extra) > 1 else "") if extra else " only")
    if secured and _scheme(spec, op) != "bearer":
        scheme = spec.get("components", {}).get("securitySchemes", {}).get(_scheme(spec, op), {})
        auth = _one_line(scheme.get("description", auth))
    out.append("\n**Auth:** %s" % auth)
    params = op.get("parameters", [])
    if params:
        out.append("\n**Parameters**\n\n" + _params_table(params))
    rb = op.get("requestBody")
    if rb:
        ctype, content = next(iter(rb["content"].items()))
        schema = content.get("schema", {})
        out.append("\n**Request body** (`%s`)%s" % (ctype, ": " + _one_line(rb.get("description")) if rb.get("description") else ""))
        table = _props(spec, schema)
        if table:
            out.append("\n" + table)
            ex = _example(spec, schema)
            if ex:
                out.append("\n```json\n%s\n```" % _json(ex))
    table, examples = _responses(spec, op)
    out.append("\n**Responses**\n\n" + table)
    for n, (code, ctype, ex) in enumerate(examples[:2]):
        out.append("\nExample `%s` response:\n" % code)
        if isinstance(ex, (dict, list)):
            out.append("```json\n%s\n```" % _json(ex))
            rs = op["responses"][code]["content"][ctype]["schema"]
            t2 = _props(spec, rs)
            if t2 and n == 0:
                out.append("\nResponse fields:\n\n" + t2)
        else:
            out.append("```text\n%s\n```" % str(ex).rstrip("\n"))
    out.append("\n```sh\n%s\n```\n" % _curl(spec, method, path, op, base))
    return "\n".join(out)


def render(spec_file, title, description, intro, base):
    with open(spec_file, encoding="utf-8") as f:
        spec = json.load(f)
    tags = [t["name"] for t in spec.get("tags", [])]
    by_tag = {t: [] for t in tags}
    for path, item in spec["paths"].items():
        for m in METHODS:
            if m in item:
                op = item[m]
                by_tag.setdefault((op.get("tags") or ["Other"])[0], []).append((path, m, op))
    body = [intro.strip(), ""]
    descr = {t["name"]: t.get("description", "") for t in spec.get("tags", [])}
    for tag, ops in by_tag.items():
        if not ops:
            continue
        body.append("## %s\n" % tag)
        if descr.get(tag):
            body.append(descr[tag] + "\n")
        for path, m, op in ops:
            body.append(_operation(spec, path, m, op, base))
    if spec.get("components", {}).get("schemas"):
        pass
    return '---\ntitle: %s\ndescription: %s\n---\n\n%s\n' % (json.dumps(title), json.dumps(description), "\n".join(body))
