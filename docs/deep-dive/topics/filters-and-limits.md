---
title: "The filter chain: ACLs, GeoIP, rate limits and caps"
description: How wayhouse decides whether to accept a new connection or session at all, in what order, and how each check stays cheap and bounded when it is being attacked.
---

Before wayhouse looks at what a connection wants, it decides whether to accept it. This is the filter chain: a short series of checks on the source address that run **before** any buffer or task exists. This page explains the order, why each check is built the way it is, and how they hold up when the checks themselves are the target. The settings are in the [Manual](../../manual/security/protect-a-listener.md).

## The problem

1. **Refusing has to be cheaper than accepting.** An attacker who opens thousands of connections wins if the proxy spends a task, buffers and a backend slot on each before deciding to say no.
2. **The defence must not become the weakness.** A rate limiter keeps state per source. If a flood of distinct, possibly spoofed sources can grow that state without bound, the defence eats the memory it was protecting.
3. **A proxy must never be a reflector.** An error reply to a spoofed source aims traffic at a victim.
4. **A policy must not silently disappear.** If a country filter is configured and its database fails to load, letting everyone in is the worst possible failure.
5. **Different kinds of limit answer different questions.** How fast new connections arrive, how many are alive at once and how many the whole process can hold are three separate things.

## The order

Every accepted socket goes through the same chain in the listener's accept loop (`listener.rs`), and the UDP listener applies the same checks to a client's first datagram:

```text
accept
  1. ACL            source IP allowed by the CIDR allow and deny lists?
  2. GeoIP          source country allowed?
  3. rate limit     token bucket per source IP and per source network
  4. per-source cap how many are alive from this IP and this network
  5. global caps    live TCP connections, live UDP sessions, new sessions per second
  ─────────────── only now: count the connection, spawn the task, peek, route
```

The order is from the **cheapest and most decisive** check to the one that holds the most state. The ACL is a lookup in a list the config already holds. The country lookup reads an in-memory database. Only then do the checks that touch shared counters run, so traffic that a list would have refused never touches a lock.

Each refusal increments `wayhouse_filter_blocked_total` with a `filter` label (`acl`, `geo`, `rate_ip`, `rate_net`, `src_conn_ip`, `src_conn_net`, `max_conn`, `max_udp`, `max_new_rate`) and then the socket is dropped. There is **no error reply**, on purpose: the proxy never sends anything to an address that has not been admitted, so a spoofed source gets nothing back.

The ACL and the GeoIP filter are also behind runtime feature flags, so an operator can turn a misbehaving filter off without a restart. The mechanism is described in the Deep dive topic on runtime feature flags.

### ACLs and GeoIP

`deny` wins over `allow`. A non empty `allow` list turns the default to deny. The country filter uses the same rule over ISO country codes, using a MaxMind database loaded once at start. It is **fail closed**: a listener that declares a `geo` filter but whose database did not load drops every connection. A policy is never skipped because something it depends on is missing.

### Rate limits: token buckets with two keys

New connections (and new UDP sessions) are limited by a **token bucket** per key. A bucket holds up to `burst` tokens, refills at `rate` per second, and a connection takes one. There are two keys:

- the source **IP**, and
- the source **network**, which is the /24 for IPv4 and the /64 for IPv6. An IPv4 client on a dual stack socket (`::ffff:a.b.c.d`) is keyed by its IPv4 /24, not by the all zero IPv6 prefix.

The network bucket exists for floods spread thinly over many addresses of one prefix, each of which stays under its own per IP limit. A connection needs a token from **every configured bucket**. If either refuses, **nothing is consumed from either**, so a refused source does not use up its own allowance.

The network bucket is checked first, for a reason that is easy to miss. A source refused there must leave nothing in the per IP table, or an attacker spoofing many addresses inside one prefix could fill the per IP table with entries for sources that were never admitted.

### Keeping the limiter bounded

The state is one `Mutex<HashMap>` per kind. The lock is held briefly and never across an `.await`. To keep memory bounded:

- once a map holds **100,000** entries, buckets that have refilled to full (idle sources) are pruned. The scan runs at most once per second, so a table full of live buckets does not cost a full scan on every connection,
- at a hard cap of **200,000** entries, a source with no bucket yet is **refused** after one prune attempt. It fails closed. Memory stays bounded however many distinct sources arrive,
- the refusal is logged at most once a minute per bucket kind, because the counter looks the same as an ordinary over rate drop.

Whether a bucket is idle is computed without changing it, because its stored token count lags behind until the next refill.

### Concurrent caps

A rate limit says how fast. A **per source cap** says how many are alive at once from one IP and one network (`max_per_ip`, `max_per_net`). It keeps two counters per source and a guard that releases both when the connection or session ends, removing an entry that reaches zero. A reload swaps the caps inside the limiter instead of replacing it, because a new limiter would forget the connections already admitted, and the cap would grow by one full cap on every reload. A listener with no cap set never takes the lock.

The **global limits** are process wide: the number of live TCP connections, the number of live UDP sessions, and a new sessions per second bucket. They are built once at start and cannot change by reload. A refusal happens before any buffer or task is allocated, and established connections are untouched.

## What was traded away

- **A single lock per limiter.** It is a brief lock on a map, not a sharded structure. It is simple and has not been the bottleneck; the source notes a sharded map or a timing wheel as a later performance step.
- **Fail closed means collateral damage.** At the hard cap, legitimate new sources are refused along with the flood. Refusing a flood is the goal, and refusing a few good players during one is the price.
- **Silent drops are hard to debug.** The player sees a timeout, not a reason. The counters and debug logs carry the reason.
- **Source address based.** All of this keys on the source IP, so players behind one carrier grade NAT share a bucket and a cap.

## Alternatives that were not taken

- **Reply with a refusal.** It helps honest clients and helps attackers more, through spoofed sources.
- **Filter after routing.** It would let the filters see more, at the cost of doing the expensive work first.
- **An unbounded table with periodic cleanup.** It is simpler and loses exactly when it matters, during a flood of distinct sources.

## Where to look

- `crates/wayhouse-core/src/listener.rs`: the order of the checks in the TCP accept loop.
- `crates/wayhouse-core/src/ratelimit.rs`: the token buckets, keys and bounds.
- `crates/wayhouse-core/src/src_conns.rs`: the per source concurrent cap.
- `crates/wayhouse-core/src/limits.rs`: the process wide caps.
- `crates/wayhouse-core/src/geo.rs`: the country database.
- Manual: [Protect a listener](../../manual/security/protect-a-listener.md).
