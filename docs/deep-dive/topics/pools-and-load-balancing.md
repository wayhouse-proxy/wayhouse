---
title: Pools, balancers and runtime backend edits
description: How a pool picks a backend, why every strategy is a sort followed by a capacity walk, and how operator edits survive a config reload.
---

A pool is the part of wayhouse that answers the question "which backend gets this player?". It looks small, and it has to be right under concurrency, fair under load and unsurprising when a backend is full, down or being drained. The settings are in the [Manual](../../manual/pools/pools.md). This page explains how it works.

## The problem

1. **Many connections choose at once.** Thousands of accepts per second pick from the same pool on many threads. The choice must not take a lock, or the pool becomes the bottleneck.
2. **"Best" depends on the workload.** For a stateless service the next backend in turn is fine. For a game server that holds player state, the same player should land on the same backend, and a server with half the capacity should get half the players.
3. **Capacity is a hard limit, not a hint.** A backend with `max_sessions` set must never be given more sessions, even when several threads pick it at the same moment.
4. **Operators edit membership at runtime.** The admin API adds and removes backends, and a config reload must not undo that.

## One selection pipeline

Every balancer does the same three steps in `Pool::acquire_for`:

```text
1. keep the backends that take new sessions    (healthy AND operator state = enabled)
2. order them by the balancer's preference
3. walk the list, take the first one with a free slot
```

If step 1 leaves nothing, the pick fails with `no_backend`. If step 3 finds every backend full, it fails with `at_capacity`. Both outcomes are counted per pool and strategy in `wayhouse_lb_selections_total` with a `result` label (`ok`, `no_backend`, `at_capacity`).

Making each balancer **an ordering** instead of a single pick is what keeps the rest simple. Capacity fall-through is one loop for all strategies: if the preferred backend is full, the next in the order gets the session. There is no special case per balancer.

The balancers differ only in step 2:

- **`round_robin`.** One atomic counter is incremented per pick and the list is rotated by it. The only shared write is a relaxed `fetch_add`.
- **`least_conn`.** The list is sorted by each backend's live active-session count. The count is an atomic on the backend, so the sort reads current values without locking. Two threads can read the same counts and choose the same backend, which can overshoot evenness a little but never exceeds `max_sessions`, because the next step is a checked claim.
- **`weighted`.** The same counter indexes into the cumulative weight line of the healthy set, so a backend with weight 3 owns three positions for every one position of a weight 1 backend, and the chosen backend is rotated to the front. The spread is blocky rather than interleaved, which is fine for the handful of backends a pool has. Weights come from the `weights` map and default to 1, so a weighted pool without weights behaves like round robin. The active count is ignored on purpose, so the share is purely the weights.
- **`consistent_hash`.** Every healthy backend gets a **rendezvous** (highest random weight) score for the client key, and the list is sorted by score, highest first. The backend with the highest score owns the client. When a backend joins or leaves, only the clients whose top score was on that backend move. Everyone else keeps their backend, without a ring or a lookup table. The key is the client IP, or IP and port with `hash_on: src_ip_port`. Without a client (the plain `acquire`), it falls back to round robin. The hash is Rust's default hasher, stable for the life of the process. The source only relies on that: a reload rebuilds pools in the same process, and no state is shared across instances, so nothing promises that two proxies map a client the same way. The sort also gives the capacity walk a deterministic order: a full owner hands the client to the second highest score, and always the same one.

### Claiming a slot

`try_acquire` increments the backend's active count first and checks `max_sessions` afterwards. If the new count passes the cap, it decrements again and fails. The check-after-increment shape is what makes the limit exact under concurrency without a lock: a thread that loses the race undoes its own increment. The returned `BackendGuard` holds the slot until it is dropped, even if the connection ends in an error, and it also carries the passive health observations back to the backend.

UDP affinity uses `acquire_addr`: it tries one specific backend (the one a client had before) and returns nothing if that backend is gone, unhealthy, draining or full, so the caller falls back to a normal pick.

## Operator state is separate from health

A backend has two independent flags. **Health** is decided by probes and traffic (see [Health checks](health-checking.md)). **Operator state** is `enabled`, `draining` or `disabled`, set by an admin call. A backend takes new sessions only if it is healthy **and** enabled. `draining` and `disabled` both remove it from selection immediately, including UDP affinity, and existing sessions keep running. They are kept apart on purpose: a health flip must never undo an operator's drain, and an operator's drain must not look like a failure.

## Edits that survive a reload

`POST` and `DELETE` on `/pools/{pool}/backends` change **membership**, not shape. The file stays the source of truth for the balancer, health check and caps. The edits live in a `BackendOverlay`, a map from pool name to the set of added and the set of removed addresses.

Every snapshot rebuild, whether from a reload or an admin edit, runs the pool's base list through `effective_targets`: the file's targets minus removed ones, plus the added ones in sorted order. The file order is preserved. An add cancels an earlier remove of the same address and a remove cancels an earlier add. Both are idempotent.

The overlay takes a short lock only when an admin call writes and once per pool per rebuild. It is never touched on the data path.

There is one race the overlay has to handle. The snapshot is rebuilt **after** an add, so an operator who adds a backend and immediately patches it to `draining` would find nothing to patch. The overlay therefore keeps a **pending admin state** for an added address, and the snapshot build applies it once, when the backend first exists. After a rebuild the backend's state object is carried by address like every other, as described in [Hot reload](snapshot-and-hot-reload.md).

## What was traded away

- **`least_conn` is approximate under contention.** Two simultaneous picks can see the same counts. The cost is a slightly uneven spread, and the benefit is no lock on the hot path.
- **Sorting on every pick.** `least_conn` and `consistent_hash` sort the healthy list per connection. That is fine for the handful of backends a pool holds. A pool with thousands of backends would want a different structure.
- **No cross-proxy guarantee.** Nothing is shared between proxies, and the hash is not specified to be stable across builds, so sticky routing is a property of one proxy process.
- **Blocky weights.** A weight 10 backend gets ten picks in a row before the next one, not an interleaved pattern.

## Alternatives that were not taken

- **A consistent hash ring.** It needs virtual nodes and a lookup structure to rebuild on each change. Rendezvous hashing gives the same minimal movement with a single sort.
- **One function per balancer that returns a backend.** Each would need its own capacity handling. Returning an ordering keeps the capacity walk in one place.
- **Writing runtime edits back to the config file.** It would make an admin call race with an operator's editor. The overlay keeps the file untouched.

## Where to look

- `crates/wayhouse-core/src/pool.rs`: `Pool::acquire_for`, `try_acquire`, `hrw_score` and `AdminState`.
- `crates/wayhouse-core/src/overlay.rs`: `BackendOverlay` and `effective_targets`.
- Manual: [Pools and health checks: every option](../../manual/pools/pools.md) and [Drain and maintain](../../manual/operate/drain-and-maintain.md).
