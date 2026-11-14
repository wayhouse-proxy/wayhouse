# Sniffers: modules, manifests and the registry format

A **sniffer** is a WASM module that peeks at the first bytes of a connection or
datagram and returns a routing hint (a hostname or a tag). This page is the format
reference for authors and registry operators. The ABI itself is documented in
[`crates/wayhouse-sniffer-abi/README.md`](../crates/wayhouse-sniffer-abi/README.md) and the module doc of
`crates/wayhouse/src/sniffer_loader.rs`; the design and its reasons are in the
[registry spec](superpowers/specs/2026-10-05-plugin-registry-design.md).

Sniffers are not **plugins**: plugins are integrations with other systems (for
example the Pelican panel) and have their own design, see the
[plugin system design](superpowers/specs/2026-10-07-plugin-system-design.md).

## What the crate does

[`crates/wayhouse-registry`](../crates/wayhouse-registry) holds every rule below as pure
functions (no network, no I/O): parsing and validating an index, picking a compatible
version, verifying a download, and generating an index from manifests. The web UI backend
uses it to browse and install, and the sniffers repository CI uses its
`wayhouse-registry-gen` binary to publish.

## Module requirements

A module must be a core WebAssembly module with no imports, at most 8 MiB, and carry a
`wayhouse.abi` custom section (`major` then `minor`, both `u16` little endian). While the
ABI major is 0 the host requires an exact match of the minor. Depending on
`wayhouse-sniffer-abi` adds the section for you.

## Manifest (`manifest.toml`)

One per sniffer, written by the author. Unknown keys are an error.

| Key                                                 | Meaning                                                                                                  |
| --------------------------------------------------- | -------------------------------------------------------------------------------------------------------- |
| `name`                                              | Letters, digits, `-` and `_` only (the proxy's module name rule)                                         |
| `description`, `license`                            | Shown in the UI                                                                                          |
| `version`                                           | SemVer of this sniffer                                                                                   |
| `abi`                                               | `major.minor` the module was built for; must equal the section in the built module                       |
| `min_proxy`                                         | Oldest proxy version it runs on                                                                          |
| `limits.max_memory_bytes`, `limits.call_timeout_ms` | Resource limits the sniffer is designed for; the client rejects 0 and anything above 256 MiB / 10 000 ms |
| `categories` (1 to 3), `tags` (0 to 8)              | Labels the UI filters by; see "Catalog and detail files"                                                 |
| `config` (optional), `homepage` (optional)          | Documentation of the per-module config string; a link                                                    |

## Index (`index.json`)

The same file format for the official registry and for external ones, served over HTTPS.

```json
{
    "schema": 2,
    "kind": "sniffer",
    "name": "wayhouse official sniffers",
    "stats_url": "https://.../stats.json",
    "sniffers": [
        {
            "name": "minecraft",
            "description": "...",
            "license": "MIT OR Apache-2.0",
            "categories": ["game"],
            "tags": ["minecraft", "java-edition"],
            "latest": "0.1.0",
            "abi": "0.1",
            "signed": true,
            "updated": "2026-11-12",
            "versions": [
                {
                    "version": "0.1.0",
                    "abi": "0.1",
                    "min_proxy": "0.1.0",
                    "url": "https://.../minecraft.wasm",
                    "sha256": "<64 lowercase hex>",
                    "size": 24576,
                    "signature_url": "https://.../minecraft.wasm.minisig",
                    "limits": { "max_memory_bytes": 1048576, "call_timeout_ms": 50 }
                }
            ]
        }
    ]
}
```

### Catalog and detail files (schema 2)

Every entry is a _catalog entry_: enough to search and filter (`categories`, `tags`, `description`,
`latest`, `abi`, `signed`, `updated`) without the version list. The versions live either **inline**
(`versions`, the form above; right for a small registry in one file) or in a **detail file**:
the entry then has `"detail_url": "https://.../minecraft.json"` instead of `versions`, and that file is

```json
{
    "schema": 2,
    "kind": "sniffer",
    "name": "minecraft",
    "versions": [{ "version": "0.1.0", "...": "..." }]
}
```

`latest`, `abi` and `signed` must describe the newest listed version (`signed` means it lists a
`signature_url`); the client refuses a detail file that disagrees, so a catalog cannot advertise
a signature or an ABI the versions do not have. `signed` is a display hint only: an install still
checks the version entry itself. A catalog stays near 450 bytes per sniffer (about 450 KB for
1000), inside the 1 MiB cap; a detail file has its own 1 MiB cap and is fetched only when an
operator opens a sniffer, installs it or checks for updates. `stats_url` (optional) points to a
registry-published `stats.json` used only to sort by popularity; leave it out and that sort is
hidden. The plugin registry (`kind = plugin`, `plugins` instead of `sniffers`) uses the same fields.

`categories` (at most 3) and `tags` (at most 8) are 2 to 24 characters of `a-z`, `0-9` and `-`. A
client checks only that syntax, so a registry may use its own categories. The official registry
takes categories from a fixed list (`categories.toml` in its repository, enforced by the generator
with `--categories`), adding one is a maintainer change. Tags are free-form; the generator trims,
lowercases and de-duplicates them.

Rules a client enforces: `schema` is 2 (schema 1 is refused: regenerate the index); `kind` is `sniffer` (`plugin` is reserved for the
plugin registry and rejected for now); names are unique and valid; versions are unique
and sorted newest first; every URL is `https://` with a host and no user info; `sha256` is 64 lowercase hex; `size`
is at most 8 MiB; the whole file is at most 1 MiB. Unknown extra fields are ignored so a
newer registry can still be read.

## Choosing a version

For a fleet, the newest version whose `abi` matches the host and whose `min_proxy` is met
by every proxy. When none qualifies the reason is reported (for example "built for sniffer
ABI 0.2, but this proxy speaks ABI 0.1"). If proxy versions are unknown the proxy check is
skipped.

## Verifying a download

In this order: size equals the index, sha256 equals the index, the signature (when one is
published and the official key is known) verifies, and the module itself passes the
requirements above and declares the ABI the index says. A signature that is present but
invalid is an error, never a quiet downgrade to "unsigned". Signatures (minisign) are
optional for now, and an unsigned sniffer is shown as unsigned. The official key (minisign key id
`C44491B8C8BF6083`, shared with the official plugins) is built in; a release signed with it shows as signed. The signature must also say what it signs: its minisign _trusted comment_ has to be exactly
`wayhouse-v1 <kind> <name> <version> <abi> <sha256>` (`kind` is `sniffer` or `plugin`, `abi` is `major.minor`). A
signature made for other bytes, another name or another version is refused, so an index host
cannot replay an old signed module under a new version or someone else's name. Sign with
`minisign -S -s <key> -t "wayhouse-v1 sniffer <name> <version> <abi> <sha256>" -m <name>.wasm`. Signatures
made before this format (no claim) no longer verify; publish a new version instead. The sha256 comes from the
same index as the URL, so for an unsigned sniffer it only protects against corruption.

## Generating an index

```sh
wayhouse-registry-gen generate --name "my registry" --base-url https://example.com/dl \
  --previous index.json --out index.json sniffers/minecraft sniffers/a2s
wayhouse-registry-gen verify index.json
```

`wayhouse-registry-gen verify-plugin index.json` checks a plugin index (`kind: plugin`)
with the same validator the UI uses, for the plugins repository's CI and release.

Each directory holds `manifest.toml` and `<name>.wasm` (and optionally
`<name>.wasm.minisig`). Artifacts are expected at
`<base-url>/<name>-v<version>/<name>.wasm`. Output is deterministic (sorted, fixed field
order), older versions are kept, and re-running with identical bytes changes nothing. A
published version is immutable: the same version with a different sha256 is refused, so
bump the version instead. A failed run leaves the existing file untouched.

Options: `--categories categories.toml` (every manifest category must be on the list),
`--date YYYY-MM-DD` (the `updated` value for what is published now; default today, UTC) and
`--stats-url`. By default every version is inline. With `--details-dir <dir> --details-base-url <url>`
the output is a catalog plus `<dir>/<name>.json` per sniffer (served at `<url>/<name>.json`), and the next
run reads the old versions back from those files, so keep the directory next to the index.
`verify index.json --details-dir <dir>` checks the detail files too.

If `--previous` is left out and `--out` already exists, `--out` is read as the previous
index, so a plain regenerate cannot replace a published version. If that file exists but is
not a valid index, the run fails instead of overwriting it; fix or remove the file first.
A schema 1 previous index is the exception: it is upgraded in place (versions kept, `updated` set to
the run date, no categories or tags until the next published version brings them).

## Installing from a registry (UI backend)

`wayhouse-ui` fetches indexes and artifacts itself, runs every check above, and only then hands
the verified bytes to the fleet upload, so a proxy needs no internet access and still re-validates
what it receives. The routes are `GET /api/registries` and `GET /api/registries/{id}/sniffers`
(viewer), `POST /api/registries/{id}/install` (operator, the same level as uploading a module by
hand), and `POST /api/registries` and `DELETE /api/registries/{id}` (admin: the registry list is
the trust list). Once the official signing key is configured, the official registry must list a
signature for every version it serves; an index that drops it is refused rather than installed
unsigned. Re-adding the official URL after removing it restores the official entry.

- **Flags.** `--registries-file <path>` keeps the list across restarts (JSON, written atomically,
  mode 0600; without it changes live in memory only). `--no-default-registry` hides the official
  registry. The official index is `https://raw.githubusercontent.com/wayhouse-proxy/sniffers/main/index.json`.
- **Risk.** Every response says whether a registry is `official` or `external`. External registries
  are installed from at the operator's own risk; the page shows that before every install.
- **Network guards.** Only `https://` is fetched, a redirect is held to the same rules, and a URL or
  hostname that resolves to a private, loopback, link-local or unique-local address is refused.
  Bodies are cut off at the size limits while streaming, and requests time out. The client does not
  use `HTTPS_PROXY`/`HTTP_PROXY`: a forward proxy resolves the destination itself and would bypass
  the address check, so the UI host needs direct outbound HTTPS to the registry.
- **ABI and `min_proxy`.** The ABI is this release's. The aggregator does not report proxy versions
  yet, so `min_proxy` cannot be enforced and the listing says `min_proxy_checked: false`.
- **Signatures.** The official public key is not set yet, so signatures are not checked and installs
  are reported `signed: false`. See the maintainer to-do below.
- **Partial installs.** The reply lists every proxy. A proxy that rejects the module because it pins
  its sniffers (`settings.sniffers.modules`) answers `409` with a reply starting `pinned:`; it is
  reported as `pinned` together with the `name` and `sha256` to add to its pin list. Any other
  refusal (including the `409` of a proxy with `settings.sniffers` unset, which needs a restart) is
  reported with its `detail` text, not as pinned. `200` means every proxy accepted, `207` some did
  or some are pinned, `502` none did and none is pinned.

**The Sniffers page.** Under "Install from a registry" the page lists the registries (a badge says
`official registry` or `external, at your own risk`), the sniffers of the selected one with the
version this release can run (or the reason none can), and an Install button. The install dialog shows
the version, size, memory and time limits, signature and config notes before anything happens; for an
external registry it also shows the risk text and needs a tick before it enables the button. Adding a
registry needs the same tick. After an install the dialog lists every proxy (installed, pinned,
failed with the reason); for pinned proxies it shows the `name`/`sha256` entry to add to
`settings.sniffers.modules`, with a Copy button. When wayhouse-ui runs without `--registries-file` the
page warns that added registries are lost on restart.

## Updates and rollback

Updating a sniffer is installing a newer version over the old one; the proxy keeps the old one so an
operator can go back, and falls back to it on its own when a new file turns out to be unusable.

- **The kept version.** An upload over an existing module saves the replaced file as the dotfile
  `.<name>.wasm.prev` in `settings.sniffers.dir`. Exactly one previous version is kept: a second
  update replaces it, so two updates followed by a rollback give the version before the last update
  only. The replaced file is not kept when it no longer loads (it would push out a working
  previous version) or when the upload is the same build. The
  `*.wasm` scan and `GET /admin/sniffers` skip dotfiles; the listing reports `has_previous` and
  `fallback` for each module. Removing a module removes its previous version too.
- **Rollback.** `POST /admin/sniffers/{name}/rollback` on a proxy swaps `<name>.wasm` with
  `.<name>.wasm.prev` and requests a reload, so a second rollback undoes the first. It answers `404`
  when there is nothing to go back to. The fleet verb is `POST /fleet/sniffers/{name}/rollback` on the
  aggregator (`/api/fleet/sniffers/{name}/rollback` in the UI, operator role); the reply lists every
  proxy, and one without a previous version shows its `404` without failing the others.
- **Pinned instances.** A proxy with `settings.sniffers.modules` only runs a version whose sha256 is
  pinned, so an update upload is refused (`409 pinned:`) until the new hash is in the pin list, and a
  rollback is `409 pinned:` and changes nothing when the previous version's hash is not pinned. An
  update never changes a pin. A rolled-back file never leaves a state the next scan would reject.
- **Automatic fallback.** When `<name>.wasm` fails validation at a rescan (a bad file copied in by
  hand, for instance) the loader runs `.<name>.wasm.prev` instead, if it validates and, on a pinned
  proxy, matches the pin. It logs an error naming both files and the reason on every rescan, and the
  listing shows `fallback: true`. Without a usable previous version a bad file is skipped as before
  (or fails the scan, with pins). A call already running when a rescan swaps the module finishes on
  the old instance.
- **Checking for updates.** `POST /api/registries/updates/check` (operator) refetches every
  registry's index (a cached one would hide a release) and asks each proxy what it runs. There is no
  timer and no background fetch: it runs when someone clicks "Check for updates". Each installed
  build's sha256 is looked up in the indexes. One row comes back per sniffer and build, with the
  instances running it, so a half-updated fleet shows two rows. A row carries `installed_version`,
  and `update` when a newer version exists: `compatible: true`, or `compatible: false` with a
  `reason` (for example another sniffer ABI). A build whose hash is in no index is an **unknown
  build**, typically a module uploaded by hand or built locally. It has `known: false`, no update
  is offered, and the page says so rather than guessing from the name. An unreachable registry or
  proxy is listed in the reply (`registries`, `instance_errors`) and the rest still answers.
- **Updating.** The Updates section's "Update to X" button opens the install dialog for exactly that
  version: same limits, signature notes and risk confirmation as an install. The update itself is the
  fleet upload from "Installing from a registry".
- **The page.** The module table shows a `fallback active` badge and a "roll back" button (with a
  confirmation that it acts on every instance) for modules that keep a previous version.

**Maintainer to-do.** Generate the minisign key pair offline, put the public half in
`crates/wayhouse-ui/src/registry_keys.rs`, and store the secret in the sniffers repository (see the
bootstrap plan). Until then everything installs unsigned.

## Installing by hand

Download `<name>.wasm` from a [sniffers release](https://github.com/wayhouse-proxy/sniffers/releases)
(or build your own) and copy it into the directory named by `settings.sniffers.dir`, named
`<sniffer-name>.wasm`: the proxy names a sniffer after its file stem (so the regex sniffer is
`regex_firstbytes`). Then use the name in a route:

```yaml
settings:
    sniffers:
        dir: "/etc/wayhouse/sniffers"

listeners:
    - name: mc
      bind: "0.0.0.0:25565"
      routes:
          - match: { type: sniffer, sniffer: minecraft, host: ["survival.example.net"] }
            action: { pool: survival }
          - match: { type: always }
            action: { pool: lobby }
```

A config reload rescans `dir` live, so an added, removed or replaced module needs no restart.
`settings.sniffers.modules: [{ name, sha256 }]` optionally pins each file's hash (`sha256sum
<file>.wasm`).

An instance with `settings.sniffers` configured also accepts modules over its admin API, so
no filesystem access is needed:

```sh
curl -X POST --data-binary @a2s.wasm "http://<admin-listen>/admin/sniffers?name=a2s"
curl "http://<admin-listen>/admin/sniffers"                # list, with sha256 + loaded state
curl -X DELETE "http://<admin-listen>/admin/sniffers/a2s"
```

This writes into the same `dir` and triggers the same rescan, so it is interchangeable with
copying files. `wayhouse-aggregator` fans the upload and delete out to every known instance
(`POST`/`DELETE /fleet/sniffers[/{name}]`), and the UI's Sniffers page uses that fan-out. All three
routes answer `409` on an instance with no `settings.sniffers` block (turning sniffing on from
nothing is startup-only, see `docs/05-configuration.md`). If `modules` pins hashes, an upload whose
name and hash are not listed is refused with `409` (`pinned:`) and writes nothing; add the matching
name and SHA-256 to `modules` first.

## What this repository tests against the official sniffers

Main builds no sniffer. The loader's own tests use small WAT modules (accept, reject, trap,
timeout, memory cap, wrong ABI version). On top of that, `sniffers.lock` pins the official releases
by tag and sha256, and the `sniffers-e2e` CI job (`make sniffers-fetch`, then the `#[ignore]`d
`wayhouse` tests) loads exactly those bytes through the real loader on amd64 and arm64: the ABI
check, every sniffer's recognition, and the latency gate. It is part of the required `ci-ok`
gate (#296), so a PR that changes the loader or `sniffers.lock` cannot merge with it red; it is
skipped, and counts as passed, on PRs that touch neither. The price is that an outage of the
sniffers repository blocks the PRs that do run it (re-run once, like any flake). It runs when
`wayhouse` or `sniffers.lock` changes, on the merge queue and in the nightly run. Community sniffers are tested by the CI of the
repository that hosts them, not here.

When the ABI changes (`HOST_ABI`, the wire format): update `crates/wayhouse-sniffer-abi`, bump the revision
the sniffers repo pins, release the rebuilt sniffers there (its **Release** workflow), then run
`python3 .github/scripts/update_sniffers_lock.py` here to pin the new tags and hashes from the
published `index.json`, and the e2e job is green again. The same command is how a newer official
sniffer release gets picked up.
