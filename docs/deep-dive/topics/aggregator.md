---
title: "The aggregator: fleet reads, history and fan-out"
description: How proxies report their own state to a stateless aggregator that answers fleet-wide questions, draws the last hour of charts from counters, relays operator actions, and why it holds no authority and forgets everything on restart.
---

The controller answers "what should the fleet be doing". Nobody answers "what is it doing right now" unless something collects it. That is the **aggregator**: every proxy pushes a summary of itself to it, and it answers fleet wide reads, serves a rolling hour of statistics, and relays operator actions to individual proxies. It is deliberately separate from the controller's write path. The flags are in the [Manual](../../manual/index.md) and the generated [Reference](../../reference/index.md).

## The problem

1. **Reads must not load the data path.** A fleet dashboard cannot make every proxy answer a query for every viewer.
2. **A fleet view has to be bounded.** Hundreds of proxies, and proxies that lie or churn names, must not grow memory without limit.
3. **The write path must not depend on it.** If the aggregator is down, config still flows and traffic still moves.
4. **Telemetry must not become a credential leak.** If a proxy tells the aggregator where its admin API is, and the aggregator later calls that URL with a token, the telemetry channel has become a way to collect the token.
5. **Charts need rates, and proxies report totals.** Counters only ever go up, except when a process restarts.

## No authority, no persistence

The aggregator is **stateless and ephemeral by design**. Its store is a plain in memory map from instance name to the latest summary that instance pushed, and it is never written to disk. That is not a shortcut. The reasoning is that the aggregator carries no authority and forgets nothing important, since every fact it holds is a proxy's own state and will be re pushed on the next tick. Restarting it loses nothing that is not already about to arrive again.

A direct consequence is that **there is no retry buffer** on the proxy's push client. Each push is a full summary of the current state, and the store keeps the latest write per instance. Replaying an old failed push after a fresher one had landed would make the aggregator's view older, not more complete. A failed push is logged and superseded by the next tick. The proxy side is independent of `--controller`: a proxy can push to an aggregator while reading its config from a file, and the reverse. The two are unrelated axes, and the wire shape is duplicated in the proxy rather than pulled in as a dependency, since the proxy never depends on an optional operational tool.

## Ingest: bounded and checked

A proxy builds its summary directly from its live state, with no scraping of its own admin API. It is **a summary and not the session registry**: pool and backend health and operator state, and session counts. The full list of sessions stays on each proxy's own admin API for break glass use.

Every field is bounded, because proxies self report: field text up to 1,024 bytes, 1,024 pools, 4,096 backends per pool, 1,024 listeners and 64 feature flags, with at most 10,000 instances by default. When the store is full, an instance that has been silent for an hour is evicted to make room. Every entry carries `last_seen_ms_ago` computed on the **aggregator's own clock**, never the proxy's, so a proxy with a wrong clock cannot make itself look fresh. An instance is flagged **stale** after 30 seconds without a push.

### Which admin URL to trust

The summary includes the instance's own admin URL, and the fan out later calls it with a fleet wide instance token. Trusting that URL blindly would turn "can push telemetry" into "can collect the admin token". So every URL is checked on ingest against a policy:

- With `--instance-url-allow` entries, the URL's host must match one: an IP in a listed CIDR, or a hostname equal to or under a listed `*.suffix`.
- Without entries, the host must be an **IP literal equal to the address the push came from**, so an instance can only name itself, and never a loopback address, since any local process could push `127.0.0.1:<its own port>` and collect the token. A single box setup lists the loopback range on purpose.
- Either way the scheme must be `http` or `https`, with a host and no user info, query or fragment.

## Fleet reads, straight from the store

`GET /fleet/pools`, `/fleet/sessions` and `/fleet/healthz` are served off the same store, with no call to any proxy, because the data already arrived. There is deliberately no fleet config view: a summary carries state, not config content, and the controller is the one source for config.

The node list is **filtered, sorted, grouped by region and paged on the server**, so hundreds of nodes never travel to the browser whole. A page holds at most 200 rows (25 by default) and a query is at most 128 characters. The same filter backs a resolve call, which a bulk action uses to learn which instances, and how many, a filter selects before the operator confirms. Tags and region are what an instance reports about itself, and tags an operator set live in the controller. The UI backend sends them with each query and they win over a declared tag of the same key, so the aggregator still stores nothing of its own.

Feature flags roll up the same way: per flag, how many nodes have it on, off, or cannot run it, with up to 50 instance names listed and at most 256 distinct flag names, so strange self reported names cannot make the answer unbounded (see [runtime feature flags](runtime-feature-flags.md)).

### Version skew

Each instance reports its product version and wire protocol, and the aggregator compares each with the newest version in the fleet. **None** is the same version, or an older build that reports none. **Within window** is the same protocol major and a minor at most one behind, the supported N and N minus one window, shown yellow. **Outside window** is another major, more than one minor behind, or an instance that refused a request for an incompatible protocol recently, shown red. The mismatch rule needs a recent increase in a refusal counter and not just a non zero counter, so an old refusal does not paint a node red forever.

## History: a ring of minute buckets

Proxies push cumulative counters, and a chart wants rates. The history module is a **ring buffer of one minute buckets, 61 deep** (about an hour), fed by every accepted push. For each instance it keeps the previous sample and adds the **difference** to the minute buckets that it covers.

- A counter that went **down** means the process restarted, and its new value is the whole increase.
- A **gap** between pushes spreads the increase over the minutes it spans, so a silent instance never shows as a spike when it comes back.
- Series are **aggregates only**: listener, pool, region and fleet. There is no per node series, so the buckets do not grow with the fleet. Only the one baseline per instance does, and baselines of silent instances are pruned after the longest possible spread.
- A bucket holds at most 1,024 series of each kind. Past that, new names are dropped and the fleet total undercounts until the names age out.
- Gauges, such as pool backends and sessions, are sampled once per instance per minute, from its first push in that minute.

Like the rest of the aggregator, history lives in memory, and a restart starts the charts empty.

## Fan-out: relaying actions, deciding nothing

The aggregator proxies the admin verbs to a single instance or to all of them. Draining one instance is **targeted**, since there is no sensible reading of "drain the whole fleet". Adding or removing a backend and route hints are **broadcast**, and the answer reports a result per instance and never fails outright because one instance did. The aggregator decides nothing and stores nothing of its own. It relays to whatever URL an instance reported, after the policy above.

The forwarded path is where the care goes. Path parameters reach the handlers decoded, so a caller can put `/`, `..`, `?` or `#` into one segment. Pasting them into a string would let one verb's route address a different admin path on every instance, carrying the instance token. A target therefore holds the path as **a list of segments**, refuses the dangerous ones up front, and pushes them onto the base URL through a method that percent encodes whatever it is given. Calls use a 5 second connect timeout and a 60 second timeout, and cap an instance's answer at 8 MiB.

## Tiers

A tier's aggregator is itself a valid leaf to its parent. With `--parent-url` it pushes **every instance it knows**, each renamed `"{tier name}/{instance}"`, to the parent's ingest. Namespacing keeps the parent's schema, storage and fleet endpoints unchanged, since it just sees more, longer named instances, and it keeps this tier's local names intact for its own view and fan out, because only the copy sent upward is renamed. As with the proxies, there is no retry buffer.

## What was traded away

- **No history beyond an hour,** and none across a restart. A real time series database is the answer for longer, which Prometheus already is (see [metrics and logs](metrics-and-logs.md)).
- **Aggregates only.** There is no per node chart, which keeps memory flat and costs a drill down.
- **Possible undercount** past 1,024 series in a bucket.
- **A trusted network.** The URL policy limits what a pusher can name, not who may push. Pushing needs the ingest token, which must differ from the admin token.
- **Eventually consistent.** A view is as old as the last push, which the stale flag shows.

## Where to look

- `crates/wayhouse-aggregator/src/ingest.rs`: the payload and the store with its bounds.
- `crates/wayhouse-aggregator/src/trust.rs`: the admin URL policy.
- `crates/wayhouse-aggregator/src/api.rs`, `nodes.rs`, `features.rs` and `skew.rs`: the reads.
- `crates/wayhouse-aggregator/src/history.rs`: the minute buckets.
- `crates/wayhouse-aggregator/src/fanout.rs` and `target.rs`: relaying and safe URLs.
- `crates/wayhouse-aggregator/src/parent_push.rs`: tiers.
- `crates/wayhouse/src/aggregator_client.rs`: the proxy's push client.
