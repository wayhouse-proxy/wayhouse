---
title: Registries and update checks
description: Browse and install sniffers and plugins from a registry, check which installed sniffers are out of date, and see the signed release feed that tells the console about new versions.
sidebar:
    order: 5
---

A **registry** is a static, HTTPS served index of sniffer modules or plugin modules. The UI fetches it, checks every file against its recorded hash, and only then hands the bytes to the controller or the proxies. The proxies never talk to a registry themselves. The index format is on the [sniffers page](../../sniffers.md), and how fetches are verified is in [registries and the release feed](../../deep-dive/topics/registries-and-release-feed.md).

An **external** registry is anything you add yourself. The **official** one is built in and cannot be removed. The console marks each registry with its risk, `official` or `external`, because a module from an external registry runs with whatever trust you give it.

## In the console

- `/sniffers` has a **Browse registries** view. Pick a registry, search, open a sniffer to see every version with its ABI and whether the version fits the proxies you run, then install. An install goes to every proxy.
- **Check for updates** compares what each proxy runs with what every registry offers and lists the sniffers that have a newer version.
- A plugin is fetched from a plugin registry on the plugin installs page, and the module then appears under `/plugins/:id` like an uploaded one.

Adding and removing a registry, and fetching a plugin, need the admin role. Installing a sniffer and checking for updates need the operator role. Listing and reading need the viewer role.

## Over the API

The UI exposes these routes, and the console uses nothing else.

| Request                                          | Role     | What it does                                                                                                                                                                     |
| ------------------------------------------------ | -------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `GET /api/registries`                            | viewer   | The sniffer registries as `{id, name, url, official, risk}`, and whether the list survives a restart (`persistent`).                                                             |
| `POST /api/registries`                           | admin    | `{"url": "https://...", "name": "optional"}` adds one. `400` for a bad URL or one that carries credentials, `409` for too many registries or a URL of the other kind.            |
| `DELETE /api/registries/{id}`                    | admin    | Removes an external registry. `404` when there is none by that id.                                                                                                               |
| `GET /api/registries/{id}/sniffers`              | viewer   | The catalogue, with each sniffer's newest version and whether it fits this release's sniffer ABI.                                                                                |
| `GET /api/registries/{id}/sniffers/{name}`       | viewer   | One sniffer with every version, the host ABI, and whether the proxies you run are new enough (`min_proxy_checked`, `min_proxy_note`).                                            |
| `POST /api/registries/{id}/install`              | operator | `{"name": "...", "version": "1.2.3"}` installs it on every proxy. Without `version` the newest release for this ABI is used.                                                     |
| `POST /api/registries/updates/check`             | operator | Re-reads every registry, asks every proxy what it runs, and answers with the sniffers that have a newer version, grouped by what is installed.                                   |
| `GET /api/plugin-registries`                     | viewer   | The plugin registries, shaped like the sniffer ones.                                                                                                                             |
| `POST /api/plugin-registries`                    | admin    | Adds a plugin registry, same body as for sniffers.                                                                                                                               |
| `GET /api/plugin-registries/{id}/plugins`        | viewer   | The plugin catalogue.                                                                                                                                                            |
| `GET /api/plugin-registries/{id}/plugins/{name}` | viewer   | One plugin with every version, whether each fits the controller's plugin ABI, and whether its signature can be checked here (`verifiable`).                                      |
| `POST /api/plugin-registries/{id}/fetch`         | admin    | `{"name": "...", "version": "...", "allow_unsigned": false}` downloads a module and uploads it to the controller. `allow_unsigned` confirms a module with no verified signature. |

Things that surprise people:

- **Signatures are checked for the official registry only,** and only when the build carries its key. An external registry is verified by hash, which proves the file is the one the index names and not that the index is honest. If the official registry lists no signature for a version, the install is refused, so an index host cannot drop the signature and have the module accepted.
- **An install can be switched off** with the `registry_install` feature (see [feature flags](../operate/feature-flags.md)). If the controller cannot be asked, the install is refused too.
- **A proxy that pins its sniffers** refuses a module that is not on its list and answers `409` with a reply starting `pinned:`. The install result lists each proxy and what it answered.
- A fetch that picks a version without being told one takes the newest release, never a pre-release, built for the ABI this controller speaks. A named version built for another ABI is refused with `422`.

## The release feed

The console tells you when a newer stable release exists. Once a day, with up to an hour of spread so installs do not all call together, and shortly after start, the UI backend downloads a **signed release feed** and verifies its signature against a release key that is compiled in. A feed that fails the check is ignored.

`GET /api/release` answers any signed in role:

```json
{
    "current": "0.4.0",
    "enabled": true,
    "latest": { "version": "0.4.1", "date": "2026-12-01", "notes_url": "https://..." },
    "checked_at": 1790000000
}
```

`latest` is present only when the feed names a stable release above `current`. `checked_at` is empty before the first good check. By default the feed is read from the latest GitHub release of the project. Point `--release-feed-url` at a mirror, or give it an empty value to turn the check off, in which case `enabled` is `false` and nothing is fetched.

## Publishing your own registry

`wayhouse-registry-gen` builds and checks the files. The two commands that deal with the release feed are:

```sh
# Build release-feed.json from the JSON that `gh api repos/<owner>/<repo>/releases --paginate` prints.
wayhouse-registry-gen release-feed --releases releases.json --out release-feed.json

# Check a feed against its signature. Without --key the compiled-in release key is used.
wayhouse-registry-gen verify-release-feed release-feed.json release-feed.json.minisig --key "<base64 public key>"
```

`--key` is meant for tests and for mirrors you sign yourself. The index format, `generate`, `verify` and `verify-plugin` are described on the [sniffers page](../../sniffers.md).
