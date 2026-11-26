---
title: "Backend discovery: DNS SRV, Consul, Kubernetes and static"
description: How a pool learns its backends from a service registry, why every source returns a full set instead of changes, and what keeps a flaky registry from emptying a pool.
---

A pool can list its backends by hand. In a cluster, game servers come and go with the scheduler, and nobody edits a file for each one. **Discovery** lets a pool ask a service registry (a DNS SRV record, Consul or Kubernetes) which backends exist right now. The settings are in the [Manual](../../manual/pools/backend-sources.md).

## The problem

1. **Registries speak different dialects.** DNS answers one query at a time, Consul has blocking queries, Kubernetes has watches and resource versions. The proxy wants one model.
2. **A registry hiccup must not cost players their servers.** If the registry times out, or returns nothing during a restart, the pool must not empty.
3. **Changes arrive in bursts.** A rolling update of fifty pods is dozens of events in a few seconds. Rebuilding the whole routing snapshot for each would waste work and flap the pool.
4. **Discovery must stay off the data path.** Connections must never wait for a registry.
5. **The core stays small.** An HTTP client, a DNS stack and the Kubernetes API do not belong in the data path crate.

## One idea: a source returns the whole set

A `BackendSource` has one job: `fetch()` returns **the current full set** of addresses for one pool. It never returns "added" or "removed". The runtime diffs the new set against the stored one, the same model the listener manager uses to reconcile.

This level triggered shape (see also [snapshot and hot reload](snapshot-and-hot-reload.md)) removes a whole class of bugs. A missed event, a duplicated event or a reordered event cannot corrupt the pool, because the next fetch states the truth again. A source that wants to be fast can add a `changed()` method, which resolves when it has reason to believe the set changed, so the loop fetches right away instead of waiting out its interval. Pure polling sources never resolve it, and the interval tick stays as a **resync safety net** for the push capable ones. A missed or spurious signal costs latency or one extra fetch, never correctness.

The trait and the loop live in `wayhouse-core`. The concrete adapters live in the `wayhouse` binary and are injected through a `SourceFactory`, the same seam resolvers and sniffers use, which keeps the core free of HTTP and DNS clients.

## The refresh loop

One control plane task runs **per source**, off the data path. Each turn it waits for the interval tick (15 seconds by default) or a `changed()` signal, fetches, and then does one of four things:

- **A non empty set.** It is stored (sorted and de-duplicated). If it differs from what was stored, the loop notifies the reload task, which rebuilds the snapshot. An identical set costs nothing.
- **An empty set.** Treated as suspicious: the last known good set **stays in place**, a warning is logged and `wayhouse_discovery_refresh_total` counts `result="empty"`.
- **An error.** Same rule: the last known good set stays, and the result is counted.
- **`Withdrawn`.** The one error that is an answer. A source that can tell the difference (a tunnel origin that is no longer registered) says so, and the set is cleared to empty on purpose. That empty set **wins over the file's `targets` seed**, because the pool was told its backend is gone.

So a registry outage looks like silence, not like a mass departure. The cost is that a registry that really has zero instances cannot empty a pool by itself, except through `Withdrawn`.

### The seed and the layering

A pool with a source still has its `targets` in the file as a **seed**. The effective list for a snapshot build is layered, lowest to highest: the discovered set, or the seed until the first successful fetch; then the operator [overlay](pools-and-load-balancing.md) (added minus removed); then health and admin state. A pool starts serving from its seed immediately, and the first fetch replaces it. A restart does one best effort initial fetch, parked sources included, so a restart does not leave a pool with no backends.

### Bursts: coalescing

A push signal does not fetch at once. After the first signal, the loop absorbs the ones that follow until the source has been **quiet for 500 ms**, or **5 seconds** have passed since the first one, whichever comes first. A rolling update of dozens of pods costs one fetch and one snapshot rebuild. The 5 second cap keeps a never ending stream of changes from deferring the fetch forever. Because the fetch is level triggered, nothing the burst carried is lost by waiting for it to settle.

### Lifecycle on reload

A `SourceManager` keeps one refresh task per pool. On every reload it diffs the snapshot's source configuration against the running tasks by pool name: unchanged keeps running, a changed config stops the old task and starts a new one (whose first fetch overwrites the now stale set), a new pool spawns one and a removed pool stops its task and **forgets its stored set**, so a later re-add starts clean. A source that fails to build is logged and skipped, and the pool keeps its last known good set.

## The adapters

### DNS SRV

An SRV lookup returns targets and ports. Port 0 is not dialable and a target of `.` means "this service is not available here" (RFC 2782), so both are skipped. A target that is an IP is used directly, otherwise it is resolved to A and AAAA records. It is behind the `dns-srv` cargo feature, because it is the only user of `hickory-resolver`. The system resolver config is preferred, with a default fallback if it is unavailable, so a missing `resolv.conf` does not abort startup.

### Consul

The adapter lists the **passing** instances of a service, so Consul's own health checks decide who is in. An instance's address is the service address, or the node address when that is empty. Optional tag filter and an ACL token read from a file.

It is push capable through **blocking queries**. Every answer carries an `X-Consul-Index`. The adapter remembers it and asks Consul to hold the next query open for up to 300 seconds until the index moves. A larger index means "something changed, fetch". The same index means the wait ran out, ask again. A missing, zero or **backwards** index means the old one is meaningless, as Consul's contract says, so the adapter drops it and starts over from a fresh fetch. The HTTP timeout for the watch client covers the 300 seconds plus Consul's own jitter of up to a sixteenth of the wait.

### Kubernetes

The adapter reads the **EndpointSlices** of a service, using the pod's own service account credentials (token and CA from the standard paths). It skips FQDN slices, since the proxy dials IPs, picks the port by name (or the first one), keeps only endpoints that are not explicitly not ready, and de-duplicates an address that appears in several slices.

It watches with the slice list's `resourceVersion`, asks for bookmarks and a 300 second server timeout. A `410 Gone` means the version expired and the next step is a fresh list. The service account token is **read from its file again** once a minute and kept if a read fails, because projected tokens are rotated under a running process and the file is briefly absent mid rotation.

### Failing watches, quietly

Watch requests are paced, not looped. A failing watch (a missing `watch` permission, say) doubles its retry delay from 5 seconds up to 60 seconds. A watch that "times out" in under a second was not really waiting, such as a server that ignores the wait parameter, and is paced like a failure so it cannot spin. The warning is logged once per outage, not on every retry, and the poll tick keeps the set fresh meanwhile.

### One bad instance does not freeze the pool

For adapters that resolve several instances (DNS SRV targets, Consul entries) one bad instance, say a name that does not resolve, must neither freeze the whole pool at its last set nor drop its backend on a transient error. A small `ResolveCarry` keeps each instance's **previous addresses for one cycle**. A second failure in a row drops it. If **nothing** resolved at all, that is an error, and the pool keeps its last known good set as usual. Every host name lookup is limited to 5 seconds, because the system resolver has no deadline of its own and the lookups run one after another.

## What was traded away

- **An empty set is never believed.** A genuine zero instances answer cannot clear a pool, which is the safe default and the cost of it.
- **Up to the interval of delay for polling sources.** DNS SRV is polled. Consul and Kubernetes push, with the interval as a net.
- **A rebuild per change.** A changed set rebuilds the whole snapshot, which is what keeps every other part consistent. Coalescing keeps that rare.
- **IPs only from Kubernetes.** FQDN endpoint slices are skipped.
- **Discovered backends have no weights or per backend options.** Those come from the pool.

## Alternatives that were not taken

- **Applying deltas.** The source states that the runtime never applies add or remove deltas. A missed event would otherwise stay wrong until a resync.

## Where to look

- `crates/wayhouse-core/src/discovery.rs`: the trait, `refresh_loop`, `coalesce` and the stored sets.
- `crates/wayhouse-core/src/sources.rs`: the `SourceManager` and its reconcile.
- `crates/wayhouse/src/discovery.rs`: the Consul and Kubernetes adapters, the watch pacing and `ResolveCarry`.
- `crates/wayhouse/src/dns_srv.rs`: the SRV adapter.
- Manual: [Backend sources](../../manual/pools/backend-sources.md).
