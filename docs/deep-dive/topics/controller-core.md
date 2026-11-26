---
title: "The controller: store, roles and the API shell"
description: How the controller keeps an append-only history of config revisions in an embedded store, why every write is validated first, how proxies follow the history as a stream, and how tokens and roles keep one compromised component from running the fleet.
---

A fleet of proxies needs one place that says what the configuration is. The **controller** is that place. It stores every config revision that was ever accepted, checks each new one before accepting it, and lets any number of proxies follow along. This page covers the foundation: the store, the roles, the tokens and the API. Replication between controller nodes is in [controller HA](controller-ha.md), staged rollout in [config rollout](config-rollout.md), and the tunnel registries that live in the same process in [the tunnel registry](tunnel-registry.md). The flags are in the [Manual](../../manual/index.md) and the generated [Reference](../../reference/index.md).

## The problem

1. **One source of truth, many readers.** Hundreds of proxies want the current config and every change, and each must be able to catch up after being offline.
2. **A bad config must never be distributed.** If the controller stores a document no proxy can load, the whole fleet rejects it at once.
3. **History is a feature.** An operator needs to see what changed, compare two revisions and go back.
4. **A small deployment must stay small.** A single node should not need a database server or a consensus cluster.
5. **Components differ in trust.** A proxy needs to read config and fetch secrets. It must not be able to write config, rotate keys or read another component's secrets.

## An append-only log of revisions

The store keeps every accepted config document as a **revision**, numbered upward from one, as the exact bytes that were submitted. The store never parses them. A second small tree holds the pointer to the current revision. There is no editing and no deletion of history. A write writes a new revision, and the pointer moves in the same transaction, so a crash cannot leave a stored revision that is not current, or the reverse.

Rollback follows from this. `POST /config/rollback/{revision}` does not rewind anything. It resubmits that revision's bytes as a **brand new revision** through the normal path. A subscriber therefore sees a rollback as one more ordinary revision, and nothing downstream needs a special case.

When the controller is replicated, the highest Raft log index that the store has absorbed is written in the same transaction as the revision, so a replayed log entry is recognised and skipped. The stores for the tunnel registries use the same trick (see [controller HA](controller-ha.md)).

### The embedded store

The store sits on `redb`, a pure Rust embedded database, behind a small wrapper that keeps the surface of what the code was originally written against: named trees of byte keys and values, ordered range scans, and multi tree transactions. The wrapper's rules are the ones callers rely on:

- **Every write is durable when it returns.** `redb` commits with an fsync, so there is no separate flush step to forget.
- **One writer at a time per database.** A transaction closure runs exactly once and is never retried. Writing to the same database from inside a closure waits forever for the transaction it is inside, and the module says so loudly.
- **A scan holds a read transaction,** so pages written after it started cannot be reclaimed. Scans stay short and are never held across an `.await`.
- **A directory is one `db.redb` file.** A directory left by the old `sled` engine, from builds before 0.3.0, is refused with an error that says there is no migration.

The choice trades the sled model for an engine that is maintained and crash safe, at the price of the single writer rule. The writer count is not a problem for a control plane, where writes are rare and human paced.

### A format marker

The store records its on disk format in a `format` entry. A store with a **higher** number than the build knows is refused on open, with a message telling the operator to upgrade the component. One with no marker is adopted as the current format. The current format is 2, which says that retired config revisions, the Raft log and the persisted snapshot hold no plaintext secret (see redaction in [the config schema](config-schema.md)). Nothing is laid out differently, and the bump exists so that a build from before the scrub refuses a store that has had it. The scrub itself runs on every start, since it is idempotent and cheap once done.

## Validate, then store

`POST /config` runs the same parse and validation a proxy runs when it loads a file, and only a document that passes reaches the store. A rejected submission leaves the previous revision current, which is the proxy's own rule that a bad reload keeps the old snapshot, enforced one hop earlier. `POST /config/validate` does the check and stores nothing.

There is one more gate that no single proxy could provide. The controller knows the **lowest config schema version any live proxy supports**, learned from the proxy registrations, and refuses a document that needs more. A mixed fleet therefore never receives a document that part of it must reject (the version rules are in [the config schema](config-schema.md)). If the registry cannot be read, writes are refused rather than guessed.

A write can also name its base. An edit that started from revision n and finds the current one is n+1 is refused as stale, so two operators do not silently overwrite each other. The actor header is recorded with the revision.

## Following the log

`GET /config/subscribe?since=<revision>` is how a proxy follows. It gets every revision after `since`, read from the store in pages of 256, and then a live tail of whatever is accepted next, as server sent events. A proxy reports the newest schema version it understands, and revisions that need more are not sent. It may also name a **group**, which is the canary mechanism covered in [config rollout](config-rollout.md).

The design principle is that the subscriber's cursor is the only delivery state, and it lives in the subscriber. The tail is fed by a broadcast channel with a capacity of 64. A subscriber that falls behind a burst simply re runs the catch up query from where it got to. Because the store never forgets a revision, nothing is lost, only replayed. This is the same level triggered idea as [backend discovery](backend-discovery.md), applied to config: a missed or duplicated signal costs a little work and cannot corrupt anything.

A subscription is a worker per stream, so the number is capped. At most **1,024 streams** may be open at once. One past the cap gets a 503 with a retry hint, and a stream that ends, even by a panic, returns its slot, because the slot is an RAII guard around a semaphore permit. A fleet has one stream per proxy and tier, far below the cap.

Other reads: the history list, one revision's raw text, a line diff against the current revision or another one, the schema, and the pending and confirm endpoints of a staged rollout.

## Two roles

A controller is either **standalone** or a **slave**. Standalone accepts writes directly and is the top of its own tree. A slave never accepts a write of its own: a direct submit is refused with 403, and its store is filled only by relaying a parent controller's revision stream. This is how a regional tier follows a central one.

The role is an install time fact and is **never inferred from connectivity**. A slave that loses its parent freezes on its last known good revision. It does not promote itself, because two tiers that both believe they are the authority would fork the history. The one way a role changes while running is an explicit, operator triggered adoption.

The role lives in a shared cell that both the config API and the intent API hold a clone of, so a flip is seen by both. Adoption has to be atomic with respect to writes. A direct write holds a shared gate from its role check through its store commit, and adoption holds the exclusive side from its quiescence check through the role claim. No write can read "standalone" before the claim and still commit history after the second quiescence check. Of several concurrent callers that try to flip the role, exactly one wins, through a compare and set.

## Tokens by what they unlock

With `--auth-token` set, the whole `/config*` surface is behind a bearer check, and the controller takes more tokens that each unlock less:

- **admin** (`--auth-token`): everything.
- **proxy** (`--proxy-token`): what a proxy calls, which is config fetch and subscribe, sealed secrets, plugin routes, intent and the tunnel peers. A compromised proxy cannot administer the fleet.
- **agent** (`--agent-token`): peer registration and the proxy peers stream only.
- **ui** (`--ui-token`): the routes the operator console proxies, and not sealed secrets, tunnel keys or the machine streams. Without it, enrolling a `ui` hands out the admin token.
- **ingest** (`--ingest-token`): handed to a redeemed aggregator or proxy enrollment so it can push telemetry.
- **metrics** (`--metrics-token`): only `GET /metrics`, so a scraper need not hold the admin token.

The proxy, agent and ui tokens each require the admin token to be set, and every token is at least 16 bytes. Every token is read from a file, the credentials directory or the environment, never from the command line (see [secrets](secrets.md)). Without any token, the API is open, and that is accepted only on a loopback listen address or with an explicit `--insecure-no-auth`. Origins that enrolled hold a token of their own, covered in [the tunnel registry](tunnel-registry.md).

## What was traded away

- **History never shrinks.** Every revision stays, since the store is the audit trail. The cost is disk, which at a few kilobytes per revision is small.
- **A single writer per node.** Fine for a control plane, and it keeps the model simple.
- **No automatic promotion.** A lost parent needs a person. The alternative risks two authorities.
- **Coarse roles.** Six tokens are a short list, not an identity system. Finer permissions are the job of the operator console in front of the controller.
- **A hard stream cap.** A fleet larger than 1,024 streams on one controller needs the regional tier, which is what slaves are for.

## Where to look

- `crates/wayhouse-controller/src/store.rs`: the revision log, the format marker and the applied index.
- `crates/wayhouse-controller/src/kv.rs`: the embedded store wrapper and its rules.
- `crates/wayhouse-controller/src/api.rs`: submit, validate, history, rollback, subscribe and the stream cap.
- `crates/wayhouse-controller/src/slot.rs`: the stream slot guard.
- `crates/wayhouse-controller/src/role.rs`: the role cell and its write gate.
- `crates/wayhouse-controller/src/serve.rs` and `main.rs`: the flags and the router.
- `docs/10-distributed-control-plane.md`: the design.
