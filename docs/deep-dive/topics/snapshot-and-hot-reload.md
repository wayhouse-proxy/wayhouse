---
title: Immutable snapshots and hot reload
description: How wayhouse changes its whole configuration while connections are running, without a lock on the data path and without losing what it has learned about its backends.
---

A game server proxy has one hard rule about change: **nobody should notice**. A player in a match must not be dropped because an operator added a backend, and a new player must not wait because a reload is in progress. This page explains how wayhouse changes its configuration under load, and what it gives up to do it.

## The problem

Three requirements pull against each other.

1. **The data path cannot take a lock for configuration.** Every accepted connection and every UDP datagram has to look up its listener, route and pool. A mutex around that state would serialize all of them behind the slowest reader, and a reload holding it would stall traffic.
2. **A change has to be all or nothing.** A config that names a pool that does not exist, or a secret that cannot be read, must not leave the proxy half updated. The old configuration has to keep serving.
3. **Learned state must survive.** A backend that failed three health checks is `unhealthy`. If a reload built every pool from scratch, the first connections after each reload would be sent to a backend the proxy already knew was down. The same goes for an operator's `draining` state and for the count of live sessions per backend, which `max_sessions` enforces.

The triggers add a fourth problem. A reload starts on `SIGHUP`, on a change to the config file, on a request through the admin API and when the controller pushes a new revision. Editors write files in bursts, and the admin API writes temporary files next to the config. Reacting to every event would rebuild the proxy several times for one edit.

## The design

The whole configuration is one immutable value, the `Snapshot` (`crates/wayhouse-core/src/snapshot.rs`). It holds the listeners, the pools, the global limits and echoes of the settings that other tasks diff against (resolvers, backend sources, the web section, the feature flags). The runtime keeps the current snapshot behind an `ArcSwap` from the `arc_swap` crate.

```text
             readers (every connection, every datagram)
                        │  load: no lock, no wait
                        ▼
   ┌──────────────── ArcSwap<Snapshot> ────────────────┐
   │ current: Snapshot #41                              │
   └───────────────────────▲────────────────────────────┘
                           │ store: one atomic pointer swap
   reload task:  parse → validate → secrets → sniffers → Snapshot::build(cfg, prev)
```

A reader loads a pointer and keeps its own reference to that snapshot for as long as it needs it. A swap never changes a snapshot a reader already holds. It only changes which one the **next** reader gets. The old snapshot is freed when its last reader lets go. This is the whole concurrency story for configuration: readers never wait for the writer, and the writer never waits for readers.

### Building the next snapshot

`Snapshot::build` takes the new config and, as `prev`, the current snapshot. For every pool it works out the backend list in three steps:

1. The base list is the pool's `targets`, or, for a pool with a backend source, what discovery last found (falling back to `targets` until the first successful refresh).
2. The runtime **overlay** is applied. This is what the admin API added or removed at runtime, so a backend added with `POST /pools/{pool}/backends` is still there after a reload.
3. `Pool::new` builds the pool and carries state across **by address**.

The carried state is a separate shared object per backend (`BackendState`), made of atomics: the healthy flag, the operator state (`enabled`, `draining`, `disabled`), the number of active sessions and the health-check streaks. A backend that exists in both pools shares that object, so the new snapshot sees the old one's knowledge, and the sessions that were counted against the old snapshot are still counted. A new backend starts healthy, and its first active check is placed at a jittered offset inside one check interval, so a pool that appears all at once does not probe every backend in the same instant.

### The order of a reload

`apply_config` in `crates/wayhouse/src/reload.rs` is shared by every trigger. A config from a file and a revision pushed by the controller take exactly the same path. Only how the config was obtained differs. The order matters because every step before the swap can still refuse the reload:

1. **Secrets.** References to secrets are read again on every reload. A secret that cannot be read, or a value that is too weak, rejects the reload and the old secrets keep working. A transient failure is reported as `SecretsUnavailable` so that a caller that can ask again will.
2. **Sniffers.** The sniffer directory is scanned **before** the swap. If a pinned module no longer validates, the whole reload is rejected, so new routes never run against the old sniffer set.
3. **Build and swap.** The snapshot is built and stored. Straight after, and with no `await` in between, the new sniffer set is swapped in.
4. **Reconcile.** Everything that lives outside the snapshot catches up: secrets rotate (the old value keeps working for an overlap period), runtime feature flags are published, listeners are reconciled by name (added, stopped or rebound), discovery refresh tasks are reconciled when a `backend_sources` entry changed, and the resolver clients are rebuilt only when `resolvers` changed, so an ordinary reload never throws away a resolver's cache.

The listener reconcile runs on **every** reload, not only when listeners changed. The snapshot is already stored by then, so after a failed bind the running group no longer matches it. Reconciling again on the next reload, even of the same file, is what retries the bind. When there is nothing to do it is a cheap comparison.

### Triggers and the debounce

The reload task waits for any of `SIGHUP`, a file-watch event (the `notify` crate) or a request from the admin API. After a trigger it waits until the triggers have been quiet for **200 ms**, but never longer than **2 seconds** in total. The cap exists so a steady stream of events cannot starve the reload forever. Temporary and backup files the admin API writes next to the config are ignored by the watcher. If the watcher cannot start, the proxy logs a warning and falls back to `SIGHUP` only.

A failed reload is visible in two places: the counter `wayhouse_config_reload_total` with `result="failed"` goes up and the log says why. The gauge `wayhouse_config_version` holds the time of the last **successful** reload.

## What was traded away

- **Some settings cannot change live.** `workers`, `limits`, `geo_db`, `admin.listen`, `admin.tls` and the sniffer engine limits are read once at start. They belong to things that exist once per process (the thread pool, the admin socket, the WASM engine). Changing them needs a restart.
- **Memory.** During a swap two snapshots exist, and an old one stays alive while any connection that loaded it is still running. That is small next to the connections themselves, because a snapshot holds configuration, not traffic.
- **A running session is not moved.** An established connection or UDP session stays on the backend it was given. A reload changes where **new** sessions go, which is why draining a backend means waiting for its sessions to end.

## Alternatives that were not taken

- **A `RwLock` around the config.** Simple, but every reader pays for the lock and a slow reload blocks the data path.
- **Mutating pools in place.** It avoids building a new value but makes all-or-nothing impossible, and a half applied change is the worst failure for a proxy that is supposed to be boring.
- **Restarting on every change.** It would drop every session, which is exactly what a game server proxy exists to avoid.

## Where to look

- `crates/wayhouse-core/src/snapshot.rs`: the snapshot and `build_with_sources`.
- `crates/wayhouse-core/src/pool.rs`: `Pool::new` and the shared `BackendState`.
- `crates/wayhouse/src/reload.rs`: the triggers, the debounce and `apply_config`.
- `crates/wayhouse/src/config_file.rs`: the admin API's compare-and-swap write of the config file (temp file, rename, `.prev` backup); applying it is the ordinary reload.
- Manual: [Drain and maintain](../../manual/operate/drain-and-maintain.md) for what an operator does with runtime backend changes.
