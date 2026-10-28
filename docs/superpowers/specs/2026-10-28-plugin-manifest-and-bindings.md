# Plugin manifest, options and capability bindings (ABI 0.2)

Status: design note for 0.4.0 wave 1 (slice P2). Refs #386. Builds on the [plugin system design](2026-10-07-plugin-system-design.md) and its [automation hooks addendum](2026-10-07-plugin-automation-hooks-design.md). Wueschli decided (2026-10-23, 4A) that the plugin ABI moves to 0.2 with install options and capability bindings.

## Problem

A plugin's `wayhouse.plugin-caps` section is fixed in the module, so `http.hosts`, `secrets[].hosts`, `routes.hosts`, `routes.backends` and `port_routes` are fixed too. The panel host, the domain and the node network differ per operator, so an official prebuilt plugin cannot be installed against an arbitrary panel (#386). Registry install needs one module that every operator can configure.

## Shape

Two additions, both read from the module bytes without running it:

- A **manifest** in a new custom section `wayhouse.plugin-manifest` (JSON, covered by the module sha256): a display name, an `options` schema for the install `config`, and a list of `bindings`.
- **Placeholders** in the caps JSON: a string value that is exactly `"${opt:NAME}"` stands for the operator's value of option `NAME`.

At install the host validates `config` against `options`, substitutes the option values into the caps, and validates the result with the same code that parses a literal declaration. The result is the **effective capability set**. The operator approves that concrete set; the approval record is the resolved `Capabilities` plus the module sha256, exactly as today, so the store format does not change.

## Placeholder grammar

- A placeholder is a whole JSON string value, `"${opt:NAME}"`, with `NAME` of `a-z`, `0-9`, `_`, starting with a letter, up to 32 characters.
- No partial interpolation (`"x${opt:a}"` is refused, never substituted), no nesting, no placeholders in object keys.
- A placeholder is allowed only at the positions below. Each position has one kind, and the manifest binding for that option must declare the same kind.

| Kind                 | Allowed positions                                          | Value checked with                                                            |
| -------------------- | ---------------------------------------------------------- | ----------------------------------------------------------------------------- |
| `hostname`           | `http.hosts[].host`, `secrets[].hosts[]`                   | the `http` host rule (`caps.rs`: lowercase DNS name or IP literal, no `*`)    |
| `route_host_pattern` | `routes.hosts[]`                                           | `routes::valid_pattern` (an exact name or `*.suffix`)                         |
| `cidr`               | `routes.backends[]`, `port_routes.backends[]`              | `routes::Net::parse` (`ip` or `ip/prefix`)                                    |
| `port_range`         | `port_routes.ports[]`                                      | `port_routes::parse_range` (`port` or `lo-hi`, 1 to 65535)                    |
| `int`                | `http.hosts[].port`, `routes.max_entries`, `port_routes.max_entries` | a JSON integer, then the same bounds as a literal (`1` to the host ceiling) |

Everything else stays literal in the module: triggers, `tick_interval_secs`, `log`, `state`, `secrets[].name`, `http.hosts[].allow_private`, `port_routes.proto`. The module author decides which values are operator-supplied; the operator never gets to change what kind of access a capability is, only where it points.

## Two types

- `DeclaredCaps` is the caps JSON as the module wrote it, placeholders included. It is opaque: nothing reads a field from it.
- `Capabilities` is the resolved set. It is what the operator approves, what `check_within` compares and what the runtime enforces. It is only ever produced by `Capabilities::parse`, so a resolved set is always one a literal declaration could have been.

Resolution is `resolve_caps(declared, manifest, values)`: substitute at the allowed positions, then call `Capabilities::parse` on the result. Validation is not duplicated.

## Options

`options` is a restricted JSON Schema (draft 2020-12 subset): an object schema with `properties`, `required`, `type` (`string`, `integer`, `number`, `boolean`, `array`, `object`), `enum`, `default`, `minimum`, `maximum`, `items`, `additionalProperties` (a boolean), and the display hints `title`, `description`, `group`, `order`, `x-wayhouse-secret`. Any other keyword is refused when the manifest is read. `pattern` and `format` are not in the first cut: the bound kinds already validate the values that matter for capabilities, and a regex engine in the host is attack surface for no capability gain. Hard limits: manifest 16 KiB, nesting depth 6, 64 properties per object, 64 `enum` entries.

The install `config` is the option values: option `NAME` is the top-level `config.NAME`, falling back to the schema `default`. `config` is still handed to `init` unchanged. A schema violation answers `422` with the JSON path.

## Failure modes

| Case                                                        | Result                                                          |
| ----------------------------------------------------------- | --------------------------------------------------------------- |
| Placeholder with no binding in the manifest                 | module refused (`unbound placeholder`)                          |
| Binding for an option the schema does not declare           | module refused                                                  |
| Binding kind does not match the position, or the option type does not fit the kind | module refused                          |
| Placeholder in a non-bindable field, a key, or inside a longer string | module refused                                        |
| Bound option has no value and no default at install         | install `422`                                                   |
| Value of the wrong type or kind (`*` in a `hostname`, `10.0.0.0/33`) | install `422`, same message the literal parser gives   |
| `approved` differs from the effective set                   | install `400`, answer shows both sets                           |
| Option edited after approval so the effective set grows     | the plugin does not load (`CapsNotApproved`); the editing route (P3) requires re-approval with a before and after diff |
| Option edited without changing the effective set            | no re-approval                                                  |
| Manifest declared twice, over 16 KiB, too deep, unknown field | module refused                                                |

A module without placeholders needs no manifest and behaves as before. `approved` for such a module only has to cover the declaration, as today.

## ABI and compatibility

- `ABI_MINOR` 1 becomes 2; the host still requires an exact `major.minor` match while the major is 0. A 0.1 module on a 0.2 host, and a 0.2 module on a 0.1 host, fail with `AbiIncompatible` before the caps are read, so a placeholder can never be parsed as a literal host.
- `PLUGIN_SUPPORT` 4 becomes 5, so an HA leader proposes installs only when every member can read the manifest.
- The registry index `abi` value for plugins moves to `0.2`; the options schema in the index is a later slice (E).

### Migration from 0.1

Nothing is in production, so this is a rebuild, not a compatibility layer. A plugin author updates the `wayhouse-plugin-abi` dependency and rebuilds; a plugin with literal caps changes nothing else. A plugin that wants operator-supplied values adds a manifest section and placeholders. The operator re-uploads the rebuilt module and approves it again. The three official plugins are rebuilt in the plugins repository after the host change merges (it pins the ABI crate by commit).

## Security properties to test

1. The effective set always passes `Capabilities::parse`.
2. A plugin cannot widen its own approval: the loader compares the effective set with `approved` on every load, so editing stored options cannot grant more.
3. `allow_private` is never bindable, so an option cannot make a public-only host reach a private address.
4. A wildcard never reaches `http.hosts`.
5. An option value reaches only the positions of its declared kind.
