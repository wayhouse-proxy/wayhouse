---
title: "Config revisions: canary, trial and rollback"
description: Change the fleet's config safely with revisions, a canary stage, a trial window that reverts itself, and the admin API behind the UI's config pages.
sidebar:
    order: 4
---

When a [controller](../../10-distributed-control-plane.md) holds the config, every accepted change is a new numbered **revision** in one history that is never rewritten. The Config pages of the admin UI and the `/api/config` endpoints work on that history. The UI forwards every call to the controller with its own credential, so the browser session never becomes a bearer token.

Reads need the **viewer** role. Writes need the **admin** role.

## Read the history

| Endpoint                                                       | Role   | Returns                                                                                                   |
| -------------------------------------------------------------- | ------ | --------------------------------------------------------------------------------------------------------- |
| `GET /api/config`                                              | viewer | The current config.                                                                                       |
| `GET /api/config/revisions`                                    | viewer | Every revision, with who made it (`actor`) and whether it is promoted and which canary groups it reaches. |
| `GET /api/config/revisions/{revision}`                         | viewer | One revision.                                                                                             |
| `GET /api/config/revisions/{revision}/diff?against=<revision>` | viewer | The difference between two revisions.                                                                     |
| `GET /api/config/schema`                                       | viewer | The schema the config editor validates against.                                                           |
| `GET /api/config/pending`                                      | viewer | The revision on trial, if any, with its countdown. See below.                                             |

## Submit a change

`POST /api/config` takes the config as **raw YAML text** in the body. It accepts these query parameters and drops any others:

| Parameter                  | What it does                                                                                                                                 |
| -------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------- |
| `base=<revision>`          | The revision your edit is based on. If someone else saved in the meantime, the controller answers `409` instead of overwriting their change. |
| `stage=canary`             | Do not roll out to everyone. Use together with `group`.                                                                                      |
| `group=<name>`             | A canary group that gets this revision. Repeat it for more groups.                                                                           |
| `confirm_within=<seconds>` | Apply the revision **on trial** for 10 to 600 seconds. See below.                                                                            |

`POST /api/config/validate` checks a body without saving it. It is the editor's save-time check, and it needs the admin role.

When the config comes from a file instead of a controller, the same call only forwards `base`. A file has no canary stage and no trial window.

## Canary, then promote

A plain submission is promoted at once, so the whole fleet sees it. With `stage=canary&group=eu`, only instances that report the canary group `eu` get the revision. Everyone else keeps the last promoted one, so a canary is invisible to the rest of the fleet.

`POST /api/config/promote/{revision}` makes a canary revision current for everyone. This is the one change that does not create a new revision: it changes who can see an existing one. A canary that is never promoted stays visible only to its groups, so promote it or roll it back.

## The trial window

A trial protects you from locking yourself out. A revision applied with `confirm_within` is on trial: the controller remembers the revision to fall back to and a deadline. Unless you confirm in time, it submits the previous revision again as a **new** revision (the history is kept, and its actor is `auto-revert`).

A 60 second trial also starts by itself when a change touches the admin listener, the admin token, admin TLS, the gossip settings, or a listener on the admin port. Pass `confirm_within=0` to opt out, for automation that moves these settings on purpose. The first revision, a canary revision and a revert never start a trial. Promoting a canary does, by the same rules, because that is when it reaches the whole fleet.

| Endpoint                                      | Role   | What it does                                                                                                                                                                                  |
| --------------------------------------------- | ------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `GET /api/config/pending`                     | viewer | `pending` is `null` or the revision on trial with `revert_to`, `deadline_ms`, `reasons` and `remaining_ms`. The controller's clock drives the countdown, so every operator sees the same one. |
| `POST /api/config/confirm/{revision}`         | admin  | Keeps the revision. `409` when nothing, or another revision, is on trial.                                                                                                                     |
| `POST /api/config/revert?revision=<revision>` | admin  | Ends the trial now and goes back. Only the revision number you name is forwarded.                                                                                                             |

A newer promoted revision ends a trial without reverting. A canary submitted meanwhile does not.

## Roll back

`POST /api/config/rollback/{revision}` makes an older revision the current one again, as a new revision. It answers `409` when that revision's secrets were redacted (the admin token and the gossip key are not kept in plain text in old revisions). Submit the config again with the secret in that case.

## Related

- [Configuration](../../05-configuration.md) for the config file.
- [Operations and observability](../../06-operations-observability.md) for the full contract of each endpoint.
