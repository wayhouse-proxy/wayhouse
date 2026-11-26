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


def api_endpoints():
    rows = ["| Program | Path | Methods |", "| --- | --- | --- |"]
    for r in ref.api_routes():
        rows.append("| %s | `%s` | %s |" % (r["crate"], r["path"], ", ".join("`%s`" % m for m in r["methods"]) or "any"))
    return _page(
        "HTTP API endpoints", "Every HTTP route of every program.",
        "A compact index of every route registered by the proxy's admin API, the controller, the aggregator and the UI backend, "
        "extracted from the router definitions. A `{name}` segment is a path parameter; `{*rest}` matches "
        "the remaining path. Parameters, bodies, answers and status codes are on the full pages: "
        "[Proxy admin API](/reference/proxy-admin-api/), [Controller API](/reference/controller-api/), "
        "[Aggregator API](/reference/aggregator-api/) and [Console API](/reference/console-api/).", "\n".join(rows))


def ui_screens():
    rows = ["| Path | Screen component |", "| --- | --- |"]
    for s in ref.ui_screens():
        rows.append("| `%s` | `%s` |" % (s["path"], s["component"]))
    return _page("UI screens", "Every screen of the admin UI.",
                 "Every route of the admin UI frontend, from its router.", "\n".join(rows))


def env_vars():
    rows = ["| Variable | Read by |", "| --- | --- |"]
    for e in ref.env_vars():
        rows.append("| `%s` | [`%s`](%s%s) |" % (e["name"], e["source"], SRC, e["source"]))
    return _page("Environment variables", "Every environment variable the programs read.",
                 "Every environment variable named in the source. Secrets are normally passed as files "
                 "(`--...-file` flags or `$CREDENTIALS_DIRECTORY`); the variables listed here are fallbacks.",
                 "\n".join(rows))


def proxy_admin_api():
    import os
    import openapi_pages
    return openapi_pages.render(
        os.path.join(openapi_pages.DOCS, "reference", "openapi", "proxy-admin.json"),
        "Proxy admin API",
        "Every route of the HTTP API a wayhouse proxy serves: what it does, who may call it, its parameters, bodies and answers.",
        "The API every `wayhouse` proxy serves on `settings.admin.listen` (default `http://127.0.0.1:9900`). "
        "It reads the live state, changes backends at runtime, drains the instance, and reads or replaces the config "
        "file. How tokens, errors and versions work is on [HTTP API basics](/manual/operate/http-apis/).\n\n"
        "These pages are generated from the OpenAPI document in `docs/reference/openapi/proxy-admin.json`, which is "
        "itself generated from the handlers' own annotations and checked by a test that fails when the "
        "checked-in document no longer matches the annotations of the routes that carry one. A running proxy serves the same document at `GET /openapi.json`; "
        "use it to generate a client. The examples assume `TOKEN` holds `settings.admin.auth_token`; leave the "
        "header out when the API has no token.",
        "http://127.0.0.1:9900")


def controller_api():
    import os
    import openapi_pages
    return openapi_pages.render(
        os.path.join(openapi_pages.DOCS, "reference", "openapi", "controller.json"),
        "Controller API",
        "Every route of the HTTP API wayhouse-controller serves: what it does, which tokens may call it, its parameters, bodies and answers.",
        "The API `wayhouse-controller` serves (default `http://127.0.0.1:9901`). The admin token works on every route; "
        "each route also lists the scoped tokens (proxy, agent, UI, origin) that may call it. How tokens, errors and "
        "versions work is on [HTTP API basics](/manual/operate/http-apis/). With no `--auth-token` set the API is open, "
        "which is only meant for local trials. The `/raft/*` routes are the controllers' peer-to-peer channel inside an HA "
        "cluster; they are not an operator API and are not listed here. Routes of an optional part answer when that part is "
        "off: the plugin and ACME routes with `501` and a reason (a few with `404`), the HA routes (`/admin/ha/*`) with `404` on a controller "
        "that does not run HA.\n\n"
        "These pages are generated from the OpenAPI document in `docs/reference/openapi/controller.json`, which is "
        "itself generated from the handlers' own annotations and checked by a test that fails when the "
        "checked-in document no longer matches the annotations of the routes that carry one. The examples assume `TOKEN` holds the admin token.",
        "http://127.0.0.1:9901")


def aggregator_api():
    import os
    import openapi_pages
    return openapi_pages.render(
        os.path.join(openapi_pages.DOCS, "reference", "openapi", "aggregator.json"),
        "Aggregator API",
        "Every route of the HTTP API wayhouse-aggregator serves: the push endpoint, the fleet read views and the verbs that act on every instance.",
        "The API `wayhouse-aggregator` serves (default `http://127.0.0.1:9902`). Proxies push their state to `POST /ingest`; "
        "everything under `/fleet` reads that state or fans a verb out to the instances' own admin APIs. The admin token "
        "(`--auth-token`) unlocks `/fleet`; `/ingest` takes the separate ingest token. With no `--auth-token` the API is open, "
        "which is only accepted on a loopback listener. How tokens, errors and versions work is on "
        "[HTTP API basics](/manual/operate/http-apis/). The console reaches these routes through its own `/api/fleet` routes.\n\n"
        "These pages are generated from the OpenAPI document in `docs/reference/openapi/aggregator.json`, which is "
        "itself generated from the handlers' own annotations and checked by a test that fails when the "
        "checked-in document no longer matches the annotations of the routes that carry one. The examples assume `TOKEN` "
        "holds the admin token.",
        "http://127.0.0.1:9902")


def console_api():
    import os
    import openapi_pages
    return openapi_pages.render(
        os.path.join(openapi_pages.DOCS, "reference", "openapi", "console.json"),
        "Console API",
        "Every route of the HTTP API wayhouse-ui serves: sign-in, the relays to the controller and the aggregator, registries, process settings and the console's own records.",
        "The API `wayhouse-ui` serves (default `http://127.0.0.1:9903`), the backend of the web console. A person signs in with "
        "`POST /ui/login`, the browser holds the session cookie `wayhouse_ui_session`, and the session's role (viewer, operator or admin) "
        "decides which routes answer. Most routes relay to the controller or the aggregator with the console's own tokens and "
        "return their answer unchanged, so they point to the [Controller API](/reference/controller-api/) or the "
        "[Aggregator API](/reference/aggregator-api/) for the request and the answer. The console is meant for browsers; "
        "scripts should call the controller and the aggregator directly. How errors and versions work is on "
        "[HTTP API basics](/manual/operate/http-apis/).\n\n"
        "These pages are generated from the OpenAPI document in `docs/reference/openapi/console.json`, which is "
        "itself generated from the route descriptions in `crates/wayhouse-ui/src/openapi_paths.rs` and checked by tests that fail "
        "when the checked-in document is stale, or when a route exists that the document does not describe, or the other way round, "
        "or when a route's role differs from the one stated. The examples assume `COOKIE` holds the session cookie.",
        "http://127.0.0.1:9903")


PAGES = {
    "reference/proxy-admin-api": proxy_admin_api,
    "reference/controller-api": controller_api,
    "reference/aggregator-api": aggregator_api,
    "reference/console-api": console_api,
    "reference/api-endpoints": api_endpoints,
    "reference/ui-screens": ui_screens,
    "reference/env-vars": env_vars,
    "reference/config-keys": config_keys,
    "reference/cli-flags": cli_flags,
    "reference/metrics": metrics,
    "reference/feature-flags": feature_flags,
}
