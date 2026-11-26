---
title: "Registries, signing and the release feed"
description: How sniffers and plugins are published and installed from a registry whose index nobody has to trust, what a signature actually says, how a downloaded module is checked before it goes near the fleet, and how the console learns that a newer release exists.
---

A sniffer or a plugin is code that runs inside the proxy's sandbox. An operator should be able to install one from a catalogue the way they install a package, and a fleet that does so should not have to trust the host that serves the catalogue. This page explains how the registry formats, the checks and the signatures fit together, and how the same machinery carries the small signed file that tells the console a newer wayhouse exists. The screens are in the [Manual](../../manual/index.md) and the generated [Reference](../../reference/index.md). The modules themselves are covered in [sniffers](sniffers.md) and [the plugin host](plugin-host.md).

## The problem

1. **The index is just a file on someone's server.** If it is the only thing vouching for a module, whoever controls the host controls what runs in the proxy.
2. **A signature that is too general can be replayed.** A signed blob under another name, or an old signed version under a higher version number, would pass a naive check.
3. **A catalogue grows.** Hundreds of entries with full version lists make an index too big to fetch whole, and a hostile one must not exhaust the reader.
4. **Operators add their own registries,** so every fetch follows a URL that someone typed, and a URL is a way to aim the console at internal infrastructure.
5. **A module must fit the fleet.** The ABI and the proxy version have to match, and when nothing fits the operator needs the reason in words.
6. **The console should say "a newer release exists"** without ever installing anything, and without a second trust system.

## The shape of a registry

A registry is a static `index.json` plus optional files, served over HTTPS from anywhere. The format is versioned (**schema 2**), carries a **kind** (`sniffer` or `plugin`) and a display name, and lists entries. The two kinds are separate documents, `sniffers` and `plugins`, and each parser refuses the other with a pointer to the right one. It reads the kind and the schema first, so an older index fails with "unsupported schema" and not with "missing field".

An entry holds what a catalogue needs to filter without opening anything: name, description, licence, homepage, **categories** (one to three, from a list the registry owns, 2 to 24 characters of `a-z`, `0-9` and `-`), up to eight lowercase **tags**, and a summary of the newest version: its number, its ABI, whether it is signed and the date. The full version list is either inline or in a **detail file** the entry points to, so the index stays small while a sniffer with thirty versions costs nothing until someone opens it.

A version lists its URL, SHA-256, size, minimum proxy version, ABI, resource limits and, optionally, a signature URL and a documentation string. A plugin version also carries an options schema, capped at 64 KiB, so the console can draw a form for its settings.

### Limits on everything

The reader is written for hostile input and never panics on it.

- The index is capped at **1 MiB**, a detail file at 1 MiB, a signature at 4 KiB, and the release feed at 256 KiB.
- A module may be at most **8 MiB**, the same limit the proxy's loader enforces.
- Declared limits must be sane: memory from 1 byte up to 256 MiB, and a call timeout from 1 ms up to 10 seconds.
- Names are lowercase letters, digits, `-` and `_` only, so a name cannot climb out of its directory. Duplicate sniffers and duplicate versions are refused, and versions must be sorted newest first, which lets the selection logic take the first match.
- Every URL must be `https://` with a host and no user info. The SHA-256 is exactly 64 lowercase hex characters.

The index also has a `stats_url` for download counts, used for a "popular" sort and trusted for nothing else.

## What a signature says

Minisign signatures are optional on a version, and the **official** registry signs everything it builds. The check is deliberately narrow, and the narrowness is the point.

A minisign signature has a **trusted comment**, which is covered by the signature. The release workflow writes a fixed claim into it:

`wayhouse-v1 <kind> <name> <version> <abi> <sha256>`

and verification demands that the comment equal the claim built from the **index entry being installed**. The index itself is unauthenticated, so without this a host could replay an old signed blob under another name or a higher version, and the signature would still be valid. With it, a signature vouches for exactly one artifact. The release feed uses the same idea with the fixed claim `wayhouse-v1 release-feed`, so a signature made for a module cannot be offered as a feed, nor the other way round.

**A signature that is present is always checked when a key is given, and a failure is an error, never a quiet downgrade to unsigned.** With no signature, or no key to check it with, the result is simply "unsigned", and the console makes the operator confirm that. The official key is compiled in, and external registries are installed from at the operator's own risk.

Two keys exist on purpose. The registry key signs code that runs inside the proxy. A **separate release key** signs only a list of version numbers. The private halves live in the release workflow's secrets, and only the public halves are in the source. A leaked feed key could at worst announce a fake release and cannot sign a module.

## Verifying a download

Verification runs in a fixed order, cheapest first, and stops at the first failure:

1. **Size** equals the index entry's size.
2. **SHA-256** equals the entry's hash.
3. **Signature**, as above.
4. **The module itself.** It is parsed and validated, but **no guest code runs**. It must be a core WebAssembly module, a sniffer must have **no imports at all**, and it must carry exactly one four byte ABI section (`wayhouse.abi`; a plugin has its own section name). The declared ABI must equal the one the index claims.

The same checks exist twice on purpose. The console runs them before handing bytes to the fleet upload, and the proxy or controller **validates the module again** when it receives it. So a proxy never needs internet access, and a compromised console cannot smuggle in a module the proxy would reject. Nothing installed from a registry is approved by being there: for a plugin, the operator still approves its capability set (see [plugins in the controller](plugin-runtime-in-controller.md)).

## Does it fit this fleet?

An operator should never see "incompatible" without a reason, so selection returns one. It looks at the **ABI** the fleet speaks and the **version of every proxy** the module would be installed on.

- **ABI matching** follows the loader's rule. While the major version is 0, a minor bump may break the ABI, so the two must match **exactly**. From 1.0 on it is the same major and a module minor no newer than the host's.
- **Minimum proxy:** every known proxy must be at least the version's `min_proxy`, by strict semver (so `0.1.0-rc.1` is older than `0.1.0`). If no proxy version is known the check passes, since unknown is not incompatible.
- **Selection** takes the newest version that satisfies both. When none does, the answer says why in the words the console shows: no versions are listed, the newest is built for another ABI, or it needs a newer proxy than the oldest one running.

## Publishing: a deterministic generator

The registry repositories do not hand edit `index.json`. A generator reads one directory per version (a manifest, the built module and optionally its signature), merges them into the previous index, validates the result with the same parser clients use, and writes it **atomically**, so a failed run leaves the existing file untouched. Output is canonical: pretty JSON, fixed field order, sniffers sorted by name, versions newest first and a trailing newline. The same inputs always give the same bytes, so a diff of the index is a review of what changed. The same tool has modes that only verify a sniffer index, a plugin index or a release feed, so CI can refuse a bad one, and a mode that builds the release feed.

The source manifest an author writes is **strict**: an unknown key is an error, because a misspelt `min_proxy` in a file the author controls should fail loudly, unlike the forward compatible index. Names, ABI strings, categories and tags are checked against the registry's own category list.

Older indexes are carried forward. A schema 1 index is upgraded when the generator merges into it, with catalogue fields taken from the newest version. Signatures in the old format predate the claim and would fail verification, so every carried over version comes back **unsigned** and the next release signs what it builds. The upgrade is honest about it and does not pretend.

## Fetching safely

The registry client lives in the console's backend, not on the proxies (see [the UI backend](ui-backend.md)). It follows operator supplied URLs, so it carries the guards that need:

- **https only,** and the target must be a **public address**. A literal IP is judged up front. A hostname is judged by a custom resolver on every connection, and redirects are re-checked on every hop, up to five. A name that resolves to a private range, or changes its answer between calls, cannot reach it. This is the same allowlist function the plugin host's HTTP capability uses (see [the HTTP toolkit](http-toolkit.md)).
- **A byte cap enforced while streaming,** not after the download, plus short timeouts.
- **A small cache:** indexes live for 5 minutes and up to 64 detail files are kept.

The list of registries is a JSON file. One is the official registry, a built in constant that can be removed. The rest are external. At most 32 are kept, each is identified by the first 12 hex characters of the SHA-256 of its normalised URL, URLs with credentials are refused, and a URL registered as one kind cannot be registered as the other. Reading the catalogue needs the viewer role, installing needs operator, the same level as uploading a module by hand, and **changing the list of registries, which is the trust list, needs admin**.

## The release feed

The release feed is the smallest user of all this. It is a JSON file listing the stable versions that exist, newest first (at most 500), each with a date and a notes link. The release workflow builds it from the GitHub releases list, keeping only releases whose tag is `v<semver>` and skipping drafts and prereleases, validates it with the same parser clients use so that the workflow fails instead of publishing something every client would refuse, signs it with the release key and attaches both files to the release.

The console backend fetches it once a day, with up to an hour of spread so installs do not all call at once, and the first check comes about 30 seconds after start. It goes through the same guarded client, and the feed is trusted only after the signature verifies against the compiled in release key. Anything else is logged and **leaves the last good answer in place**. A release entry's notes link must be a plain `https://` URL and is only ever shown as a link, never fetched. The answer is the newest **stable** release above the running version, or nothing. The feed can be turned off with an empty URL, and the answer then says it is disabled.

It never installs anything. It is a notice, and that is why it can live on a separate, weaker key.

## What was traded away

- **The index is public and unauthenticated by design.** The claim makes that safe for artifacts, but not private: a registry host sees who fetches what.
- **Signatures are optional,** so an external registry can serve unsigned modules. The operator is asked to confirm, and the choice stays theirs.
- **A claim names one exact artifact,** so republishing a fixed build means a new version, never a quiet replacement.
- **Pre 1.0 ABI matching is exact,** which means a minor bump of the host strands existing modules until they are rebuilt. The alternative was a compatibility promise that cannot be kept yet.
- **Two keys to guard.** One more secret in the release workflow, in exchange for a feed key that cannot sign code.
- **Selection is only as good as the proxy versions it knows.** With none reported, it cannot refuse a module for being too new.
- **Download counts are advisory,** fetched from the registry and trusted for ordering only.

## Where to look

- `crates/wayhouse-registry/src/index.rs` and `plugin_index.rs`: the formats and every rule on them.
- `crates/wayhouse-registry/src/catalog.rs`: categories, tags and the shared catalogue fields.
- `crates/wayhouse-registry/src/verify.rs`: the claim, the order of checks and the module inspection.
- `crates/wayhouse-registry/src/compat.rs`: ABI matching and version selection.
- `crates/wayhouse-registry/src/generate.rs`, `manifest.rs`, `legacy.rs` and `bin/`: publishing.
- `crates/wayhouse-registry/src/release_feed.rs` and `release_key.rs`: the feed and its key.
- `crates/wayhouse-ui/src/registry_client.rs` and `registries.rs`: guarded fetches and the list of registries.
- `crates/wayhouse-ui/src/registry_api.rs`, `plugin_registry_api.rs` and `release_watch.rs`: browsing, installing and the daily feed check.
