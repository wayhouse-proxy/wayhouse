---
title: "The config schema: parsing, validation and versioning"
description: How one YAML document becomes a validated, resolved configuration, why the raw and the resolved types are separate, how the schema version stops a new field from reaching an old proxy, and how the JSON Schema is kept honest.
---

Everything a proxy does starts from one YAML document: listeners, pools, backend sources, resolvers, settings, web sites and feature flags. This page is about what happens between the text and the running proxy: how it is parsed, how it is checked, how the schema evolves without breaking a mixed fleet, and how the same definition feeds a JSON Schema for editors. The options themselves are in the [Manual](../../manual/index.md) and the generated [Reference](../../reference/index.md).

## The problem

1. **A bad config must never half apply.** A typo at three in the morning must leave the running configuration alone, not produce a proxy that listens on some of its ports.
2. **Errors must point at the cause.** "Invalid configuration" is useless. The message has to name the listener, the route and the field.
3. **The schema grows, and the fleet is not upgraded at once.** A controller that hands out a document with a new field must not reach a proxy that does not know the field and would ignore it.
4. **Hostile input is possible.** The controller parses documents sent by operators and by plugins. A document that is small on the wire and huge in memory must be refused fast.
5. **Several consumers need the same shape.** The proxy, the controller, the editor in the UI and a person with a text editor all need to agree on what a valid document is.

## Two layers of types: raw and resolved

The crate has two sets of types, and the split is the main design decision.

- **Raw types** (`schema.rs`) are a one to one mapping of the YAML. Every field is optional or defaulted, strings are still strings, and a route's matcher is a bag of optional fields next to a `type` name. They exist only to be deserialised.
- **Resolved types** (`resolved.rs`) are what the runtime consumes: durations are `Duration`, addresses are `SocketAddr`, a matcher is an enum whose variants hold only the fields that variant uses, and references between objects are already checked.

`validate` is the one function that turns the first into the second. After it succeeds, the data path never asks "is this field set?" or "does this pool exist?". It receives a `Config` that cannot be inconsistent. The cost is a second copy of most types and a function of about a thousand lines. The gain is that every rule lives in one place, and a rule that is missing shows up as a failing test in that one place.

The raw structs use `deny_unknown_fields`. A misspelled key is an error that names the key, not a setting that is silently ignored. This is the same rule that makes the version mechanism below work.

## One entry point

`parse_str` is the entry point for text, and `load` is the same thing after reading a file. The steps are fixed:

1. Deserialise into the raw types with `serde_norway`.
2. If that fails, look at the document again as a plain value. A document from a newer schema usually fails on its new fields too, and "your config needs schema 5, this build supports 4" is the useful thing to say, so the version error is reported first.
3. Parse the document a second time as a plain value, only when the version table has entries, so the version check can ask which keys are present.
4. Run `validate`.

The result is either a `Config` or a `ConfigError` with one of four kinds: the file could not be read, the YAML did not parse, the document is invalid (with a message), or the schema is too new. There is no third outcome that applies part of the document.

### Limits that exist because documents are hostile

A YAML alias can expand a small document into a very large one. The parser's repetition limit refuses such a document, and a test builds nine levels of nine fold aliases and requires an error in under five seconds. The object counts are also capped, because several validation loops are quadratic: at most 1,000 pools, 1,000 listeners and 1,000 routes per listener, far above any real deployment. The timeout and interval values are capped at one day, because a larger value overflows the millisecond arithmetic downstream and is never sensible.

## What validation checks

Validation is long because it is the place where every cross reference is checked. In outline:

- **Names are unique** across pools, listeners, resolvers and backend sources, and every reference resolves: a route's `pool` and a listener's `pool` must exist, and the message says which listener and which route.
- **Matchers are strict.** A match type accepts only its own fields: `cidrs` belongs to `client_cidr` and `dst`, `ports` to `port`, `prefix` and `length` to `first_bytes`, `host` to `sni` and `sniffer`. A `first_bytes` matcher needs a prefix or a length, and its prefix is capped at the peek budget described in [routing and matchers](routing-and-matchers.md).
- **Targets are well formed.** An address that appears twice in one pool is an error.
- **Discovery URLs cannot leak a credential.** A URL that carries a token must be `https://`, or `http://` to a loopback address, and must not contain user info.
- **Secrets are strong enough.** `check_secret_strength` rejects a plaintext `settings.admin.auth_token` or `settings.gossip.psk` shorter than 16 bytes and suggests `openssl rand -hex 32`. A reference to a sealed secret is skipped here, since its value is checked where it resolves (see [secrets](secrets.md)). The controller runs the same function on every submission, so a document it stores can always boot.
- **Keys are keys.** WireGuard keys must be standard base64 that decodes to exactly 32 bytes, through one shared function, so the controller's peer registry applies the identical check.

Findings that are not errors come back from `Config::warnings`, for the caller to log at startup and reload or to print in `--check`. A plaintext secret in the file is the usual one: allowed, but flagged.

## Versioning: the document says what it needs

`schema_version` is **the minimum schema the document needs**. Absent means 1. This build understands up to `CONFIG_SCHEMA_VERSION`, currently 4, and a document asking for more is refused outright.

The rule that makes this useful is the bump rule, written at the top of `version.rs`: any change that adds a config field, even an optional one, bumps the version and adds the field's dotted path to the `FIELD_SINCE` table with the new number. The parser then refuses a document that uses the field without declaring a `schema_version` that covers it. The error says which field, which version it needs, and when to set it: once every proxy that will receive the document supports it.

The failure mode this prevents is quiet. A controller is upgraded, an operator adds a new optional field, and an old proxy that tolerated unknown fields would drop the field and run without it. Instead, two things hold together. `deny_unknown_fields` makes an old node reject a newer field loudly, and `FIELD_SINCE` makes a new node refuse to let an operator use the field by accident. The operator opts in by raising the number, which is a deliberate act that can wait until the fleet is ready.

Paths in the table walk mappings by key and pass through sequences, so `listeners.rate_limit.enabled` matches `enabled` under `rate_limit` in every listener. The table today records the introduction of `plugin_routes` (2), `web` (3) and the `features` section with the `enabled` switches on listeners, limits, sources, resolvers and sites (4).

A second reader of the version, `declared_schema_version`, reads it without validating the rest and falls back to 1 for anything it cannot read. The controller uses it to route stored documents to the proxies that can accept them. A document it cannot read is passed on and refused by the receiver as before.

## A JSON Schema from the same types

With the `json-schema` cargo feature, the raw types derive a JSON Schema (draft 2020-12) through `schemars`. A small function walks the `FIELD_SINCE` table and writes `x-wayhouse-since` onto each field it names, so an editor can tell the operator which schema a field needs. The path walk follows references, array items and the non null arm of an optional, and a table entry that is not in the schema panics, so the table cannot drift away from the types.

The schema is checked in, at `schema/config.schema.json` (about 43 KB), and embedded in the binary as `CONFIG_JSON_SCHEMA`. A test compares the checked in file with a freshly derived one and fails with the exact command to regenerate it. Because the schema is built from the same types the parser reads, a field cannot exist in the parser and be missing from the editor. The proxy's `/config/schema` endpoint and the controller serve this text, which is how the UI builds forms without a copy of the types (see the [admin API](admin-api.md)).

## Redaction

A config revision is stored as the exact text that was submitted. Once a newer revision replaces it, nothing needs the plaintext `settings.admin.auth_token` or `settings.gossip.psk` in it, so the controller rewrites them to `[redacted]`. The rewrite is a line edit that keeps comments and layout when the value sits on the key's own line, which is every form an operator writes in practice. Any other layout falls back to re serialising the document, and a last pass replaces whatever copy of the value is still left, in a comment or behind an alias. A value never survives.

The marker `[redacted]` is shorter than the 16 byte minimum on purpose, so a document that still carries it is refused as a new submission rather than deployed with a placeholder for a password. A bare number or boolean is also treated as a secret if it is long enough to be one, and a short token is not swept at all, because it would match unrelated settings such as `true` or `8080`. `has_redacted_secret` tells a caller whether a stored document has been scrubbed.

## Secrets as references

A secret value can be plaintext or a reference, `psk: { secret: gossip-psk }`. The config crate only parses the reference into `SecretOr::Ref`. Turning it into text is the caller's job, through a `SecretResolver` that the binary implements, and a reference that cannot be resolved to a non empty value is an error and never an empty token. Every error names the secret and what is missing, and none contains a value. See [secrets](secrets.md).

## Two sections that are merged in

- **`web`** (schema 3) declares sites published through the packaged Caddy. Validation resolves the section and appends one loopback listener per pool used with `via: wayhouse`, so the data path sees ordinary listeners and needs no knowledge of the section.
- **Plugin routes.** The controller publishes the routes that enabled plugins declare, and a proxy merges them into the operator's document as text before parsing. Only listeners that opted in with `plugin_routes:` receive them, they are appended after the listener's own routes so an operator route, including an `always` fallback, always wins, and each distinct backend becomes a one target pool named from a hash of its address. A route is keyed by hostname for `sni` and `sniffer` listeners and by entry port for `type: port` listeners, and a listener takes only the routes of its own kind.

Both follow the same idea as the rest of the crate: extend the document, then let the one validation pass see the result.

## What was traded away

- **Duplicated types.** Raw and resolved structs mirror each other, and a new field is added in both. A test and the JSON Schema drift check catch a missed one, but the cost is real.
- **A minimum, not an exact version.** `schema_version` says what the document needs, not what it was written for. A document that declares 1 but uses nothing new is valid on every build, which is the point and also means the number does not tell you when a file was last edited.
- **Strictness over leniency.** An unknown field is an error, not a warning. A config copied from a newer version fails fast on an older one.
- **Revalidated on every submission.** The controller runs the same parse and strength check each time, so a document it stores can always boot, at the price of doing the work again.

## Where to look

- `crates/wayhouse-config/src/lib.rs`: the entry points and `ConfigError`.
- `crates/wayhouse-config/src/schema.rs`: the raw types.
- `crates/wayhouse-config/src/resolved.rs`: the resolved types and the warnings.
- `crates/wayhouse-config/src/validate.rs` and `parse.rs`: every rule and the value parsers.
- `crates/wayhouse-config/src/version.rs`: the version, `FIELD_SINCE` and the bump rule.
- `crates/wayhouse-config/src/json_schema.rs` and `schema/config.schema.json`: the JSON Schema and its drift test.
- `crates/wayhouse-config/src/redact.rs` and `secret.rs`: scrubbing and references.
- `crates/wayhouse-config/src/web.rs` and `plugin_routes.rs`: the merged in sections.
