---
title: "External resolvers: HTTP and gRPC with a result cache"
description: How wayhouse lets another service make the routing decision for a new connection, within a time budget, without that service becoming a single point of failure.
---

Static rules cover a lot, and then a matchmaker comes along. A matchmaking service knows which of two hundred servers holds the lobby a player just joined. A rule list cannot know that. An **external resolver** hands the routing decision for one connection to a service you run, while wayhouse keeps its job: it must still answer fast, and it must still behave when the service does not. The settings are in the [Manual](../../manual/extend/resolvers.md).

## The problem

1. **The answer lives somewhere else.** Which server a player belongs on can depend on live state that the proxy neither has nor should have.
2. **The question costs time.** Every call is a network round trip on the connection's critical path, before the first byte is forwarded.
3. **The service will fail.** It will be slow, restarted, deployed or overloaded, and the proxy must have a defined behaviour for each case.
4. **The same question repeats.** A player reconnecting, or many players joining one lobby through one hostname, ask the same thing over and over.
5. **The core must stay small.** An HTTP client and a gRPC stack are heavy, and the data path crate should not carry them.

## The shape: a trait in the core, clients in the binary

`wayhouse-core` owns the contract: a `Resolver` trait with one async method, `resolve(request) -> resolution`, plus the routing integration. The HTTP and gRPC clients live in the `wayhouse` binary and are injected at startup as `Arc<dyn Resolver>`. That keeps the data path crate free of an HTTP client. gRPC is a cargo feature (`grpc-resolver`), because it is the only user of `tonic`, `prost` and `protoc` at build time. Without the feature, a config with a `kind: grpc` resolver fails at startup (and under `--check`) instead of silently running without it.

A route's action is either a pool or a resolver, so a resolver is just another entry in the [route list](routing-and-matchers.md).

### What the resolver is told

A `ResolveRequest` carries the listener name, the client address, the address the client connected to, the TLS server name if the first bytes hold a ClientHello, the **peeked first bytes**, and the routing key a [sniffer](sniffers.md) produced, if one ran. Over HTTP it is a JSON `POST` with the bytes base64 encoded (using a small local encoder, to avoid a dependency for ten lines). Over gRPC it is the same fields in a protobuf message, with bytes as bytes.

### What the resolver may answer

A `Resolution` has three optional fields:

- `pool`: route to this pool and let the balancer choose, as for any rule,
- `target`: connect straight to this address, **skipping pools entirely**: no health check and no per backend cap. It takes precedence over `pool`,
- `ttl_sec`: how long this answer may be cached, overriding the configured default.

A target gets the same per connection settings a pool would: the resolver's own `proxy_protocol`, connect timeout and idle timeout, since a pool-less connect has no pool to read them from. The defaults are 300 ms to connect and 90 s idle.

There is deliberately **no resolver chosen affinity key**. The source records why: a later request could not recover the key without calling the resolver again, and `consistent_hash` pools plus the result cache already cover affinity.

## Failure is a decision, not an exception

Every resolver has an `on_error` policy, and the route walk is written around it. A call can end four ways: a result, an empty answer, a timeout, or a failure (which includes a bad response). The last three are all "no usable answer", and the policy says what happens next:

| `on_error`       | What happens                                                                             |
| ---------------- | ---------------------------------------------------------------------------------------- |
| `reject`         | The connection is dropped.                                                               |
| `fallback_route` | The walk continues to the **next matching route**, usually a static pool.                |
| `stale_ok`       | A cached positive answer is served even if expired, otherwise the connection is dropped. |

An **empty answer** (neither pool nor target) is treated exactly like an error for this purpose, so a resolver that says "I do not know this player" can send them to a fallback pool the same way a crashed one does. Only `fallback_route` keeps walking. Each outcome is counted in `wayhouse_resolver_requests_total` with a `result` label (`ok`, `empty`, `timeout`, `error`).

The time budget is the resolver's `timeout`, 40 ms by default. It is short on purpose. A resolver call sits in front of every uncached connection, and a slow one should fail over to the policy quickly rather than make players wait. The HTTP client enforces it as a request timeout, and the gRPC client as a channel timeout, where a deadline exceeded status maps to `Timeout` and everything else to `Failed`.

A resolver that names an unknown resolver at runtime is logged as an error and the connection is dropped, even though config validation already guarantees the name exists. The code checks anyway, because a routing decision should never panic.

## The result cache

A resolver with a `cache` is wrapped in a `CachedResolver`: a TTL'd **LRU** map behind a mutex, with the lock held only to look up or insert and never across the network call.

**The key is chosen by config.** It is built from a list of parts joined with `|`: the source IP, the source IP and port, the SNI, the routing key, or a slice of the first bytes written as hex. Choosing the key is choosing what "the same question" means. A matchmaker keyed on SNI shares answers between all players of one hostname, and one keyed on source IP is per player. A request that lacks a required part, such as an `sni` key part with no SNI, is **not cacheable** and passes straight through, counted as `uncacheable`.

**Two kinds of entry.** A positive entry holds the answer and lives for the result's own `ttl_sec` or the configured `positive_ttl` (30 s by default). A **negative** entry remembers a failure or an empty answer for `negative_ttl` (2 s by default) and returns an error without calling the service. The negative cache is the protection for the service: a resolver that is down or slow gets one call per key per two seconds instead of one per connection, which is how a struggling service gets room to recover instead of being buried by retries.

**Stale answers.** With `stale_ok`, a failing call falls back to the cached positive entry, expired or not, and counts it as `stale`. This is the setting for a service whose brief outage should not cut off players who were just routed fine.

The cache holds at most `max_entries` (10,000 by default), evicting the least recently used. No answer lives longer than 24 hours whatever the resolver or config says, which also keeps the clock arithmetic far from overflow. Every lookup is counted in `wayhouse_resolver_cache_total` (`hit`, `hit_negative`, `miss`, `uncacheable`, `stale`).

## Reload

The set of resolvers is an `ArcSwap` over the name to resolver map, the same pattern as the [sniffer registry](sniffers.md). A reload swaps the whole set in one step, and every worker sees it at once. A call already in flight holds its own handle and finishes against the old client. The reload task rebuilds the set only when the resolver config actually differs, because a rebuild makes fresh clients and **drops every cache**, which means a short cold window after a change.

## What was traded away

- **A call before the first byte.** An uncached connection waits for the service, up to the timeout. The cache and the negative cache exist to make that the exception.
- **Stale means maybe wrong.** `stale_ok` can send a player to a server that has since been replaced. It is a choice between a possibly outdated route and none.
- **A cache lock.** One mutex per cached resolver, held briefly. It is simple and has not needed sharding.
- **Targets skip health.** A `target` bypasses health checks and caps, because there is no pool to carry them. The service is trusted to name a live address.
- **Reload clears the cache.** A cold window after a change in resolver config is the price of a simple swap.

## Alternatives that were not taken

- **Having the service push routing hints.** The proxy supports that too, through [route hints](routing-and-matchers.md), for the case where a launcher knows the answer before the player connects. A resolver is the pull version, for when the answer is only needed at connect time.

## Where to look

- `crates/wayhouse-core/src/resolver.rs`: the trait, `resolve_route`, `on_error` handling and `CachedResolver`.
- `crates/wayhouse/src/resolver.rs`: the HTTP client and the build step.
- `crates/wayhouse/src/grpc_resolver.rs`: the gRPC client (the `grpc-resolver` feature).
- Manual: [Resolvers](../../manual/extend/resolvers.md).
