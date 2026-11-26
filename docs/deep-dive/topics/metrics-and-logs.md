---
title: Metrics, connection logs and error types
description: How wayhouse counts what it does without slowing it down, why metric names live in one file, what the labels leave out on purpose, and how failures are typed so each one has a name.
---

A proxy that cannot be observed cannot be run. This page is about the three ways wayhouse tells an operator what is happening: **Prometheus metrics**, **log lines** and **typed errors**. It explains the rules behind them, in particular what is deliberately not measured. The scrape endpoint is in the [Manual](../../manual/operate/monitor.md), and the generated [Reference](../../reference/index.md) lists every metric with its labels.

## The problem

1. **Names are an API.** Dashboards and alerts break when a metric is renamed, and a metric named differently in two places is two metrics.
2. **Observability must not cost the data path.** A counter increment sits next to a `splice` call. Anything that takes a lock or allocates per packet is a cost to every player.
3. **Labels are the danger.** Every distinct label value is a new time series. A label on client address, or on a backend that is created at runtime without bound, turns a metric into a memory leak in the scraper.
4. **Failures need names.** "Connection failed" is not actionable. Whether it was a refused connect, a timeout or a PROXY header write decides what to do.
5. **Every binary should report in the same shape.** The proxy, the controller, the aggregator and the UI are scraped by one Prometheus.

## Names live in one file

Every metric name the proxy emits is a constant in `metrics_defs.rs`, with a doc comment giving its type, its labels and the meaning of each label value. Call sites use the constant, never a string literal, so a name cannot drift between the place that counts and the place that documents. The generated reference and the coverage check read from here too.

The one exception is written into the file's header: the `wayhouse_tls_handshakes_*` names live in `wayhouse-http`, because that crate has no dependency on the core and must not gain one.

The metrics themselves come from the `metrics` crate facade, which turns `counter!(...).increment(1)` into a call on whichever recorder is installed. The proxy installs a Prometheus recorder and serves it from the [admin API](admin-api.md). The fleet binaries (controller, aggregator, UI) share a small helper in `wayhouse-http` that installs the recorder and mounts `GET /metrics` behind the same bearer check as the rest of their API. When the all in one control process runs three components in one process, the second `install` call shares the first recorder and adds its own build info to it.

### One series that is always there

`wayhouse_build_info` is a gauge fixed at `1` with the labels `component`, `version`, `commit` and `protocol` (the wire protocol's `major.minor`). It is set when the recorder is installed, so a scrape of a freshly started binary is never empty. Because every binary reports the same label set, one query answers "which versions and which protocol are running across the deployment", and the protocol label is what you look at during a rolling upgrade.

## What gets counted

The catalogue is large. Its structure follows the connection's path:

- **Listener and connection.** `wayhouse_listener_connections_total` by `result` (accepted, no route, sniffer reject, sniffer error), the active connection and UDP session gauges, bytes and packets by direction, a connection duration histogram and the UDP dropped datagram counter with a `reason`.
- **The filter chain.** `wayhouse_filter_blocked_total` with one `filter` label value for each check in [the chain](filters-and-limits.md). An operator can see which defence is doing the work.
- **Pools.** Backends per state, active sessions per backend, health check results, load balancer selections by `strategy` and `result` (so `at_capacity` shows pools that need more room), backend connect errors by `kind`, and PROXY protocol headers written.
- **The control plane.** Discovery refreshes and the size of the discovered set, config reloads by result, listener bind failures, feature flag value and source, resolver requests and cache outcomes, sniffer calls and call time.
- **The mesh.** Gossip members, messages, and one counter for each reason a datagram or register is dropped (bad authentication, stale, wrong version, future, non member), plus the per backend `domain_down` gauge.
- **The process.** Open file descriptors and the descriptor limit, and `accept` errors.

Two conventions are worth knowing. A counter that records an outcome carries the outcome as a `result` label, so a ratio of good to bad is one query. And error counters use a fixed, short list of reason values, never a free text error string, because a label built from an error message is a different series for every message.

## What the labels leave out

Reading the labels shows what was chosen, and the observability design notes record the choices explicitly:

- **No `pool` label on traffic metrics.** A connection's pool can change across a reload, while the listener is stable. Bytes, packets and durations are therefore per listener. Pool level numbers (selections, sessions, health) have their own metrics.
- **No `protocol` label.** A listener has one fixed protocol, so splitting by `listener` already splits by protocol.
- **Filter and cap refusals are not listener results.** They have their own counter, so `listener_connections_total` stays about routing and `filter_blocked_total` about admission.
- **A planned metric is a note, not a promise.** The design notes list the finer grained latency and saturation metrics that were deferred, under a heading that says none of them exist in the code today. The reason given is that they cost something on the hot path, and are not worth building before a real debugging need shows that the aggregate metrics are not enough. A metric that was a duplicate by typo, and another describing a buffer pool that was never built, were struck from the plan instead of being left to mislead.

The `backend` label is the exception to "bounded labels". It is an address, so it grows with the number of backends, which is bounded by the pools an operator configures or discovers. That is why `backend` appears only on the handful of metrics where per backend numbers are the point.

## Logs

Log lines use `tracing`, with a formatter that writes to standard error and a filter read from the `WAYHOUSE_LOG` environment variable (default `info`). Standard `RUST_LOG` style directives apply, so one module can be turned up without the rest.

A closed TCP connection is one `info` event, `connection closed`, with the listener, the peer, the backend, bytes in each direction and the duration in milliseconds. A connection that fails before the byte pump starts is a `warn` event, `connection failed`, with the typed error. There is no per packet logging.

The design notes describe a richer **structured JSON connection log**, one event per session close with route, resolver, pool, packet counts, setup time and a `close_reason` from a fixed set. That is the intended shape, but it is a design target, and the closing event that exists today is the tracing line above. Where this page and the design notes differ, the code is what runs.

Repetitive failures are rate limited at the source instead of in the log pipeline. A failing `accept` (typically out of descriptors) pauses the worker, backing off from 50 ms up to a second, and logs at most one warning per second with a count of what was suppressed, while the counter carries the true rate. Gossip drop warnings are limited to one per minute per origin. A module that panics on every datagram logs once per second. The rule is the same each time: the counter keeps the number, the log keeps one example.

## Typed errors

The core's public API returns typed errors, defined with `thiserror`, so a caller can match on the kind of failure instead of searching a message string:

- **`ProxyError`**, why one TCP connection failed before the pump started: no backend to hand out (itself a `PickError`, either no healthy backend or all at capacity), the connect failed, the connect timed out, or writing the PROXY header failed (with a `role` of backend or target).
- **`ListenerError`**, why a listener could not start: the bind failed.
- **`SourceError`**, why a discovery source could not be built or produce a set: unreachable, bad response, `Withdrawn` (the one error that is an answer and clears the pool) and a public key mismatch for a pinned peer. See [backend discovery](backend-discovery.md).
- **`ResolveError`**, timeout or failure, which the [resolver](external-resolvers.md) route logic turns into its `on_error` policy.

A type has two consumers. The log gets a message that names the address and the cause. The metrics get the matching label: a refused connect and a connect timeout each increment `wayhouse_backend_connect_errors_total` with their own `kind`, and the error returned carries the same distinction. The same event is therefore visible as a typed value in code, a counter in a dashboard and a line in a log, and the three agree because they come from one place.

## What was traded away

- **Cardinality is the operator's responsibility for `backend`.** There is no cap. The label is bounded only by configuration.
- **Aggregate numbers over detail.** Per listener bytes, not per pool. A question that needs more means a log, a [session listing](admin-api.md) or a new metric.
- **A log line, not a trace.** Setup tracing is described in the design notes as optional and not part of the metric set.
- **Documentation can lag the code.** The design notes still describe the structured log that is not built, which is why the coverage check and the generated reference read from the source and not from prose.

## Where to look

- `crates/wayhouse-core/src/metrics_defs.rs`: every metric name, its type and labels.
- `crates/wayhouse-core/src/error.rs`: the typed errors.
- `crates/wayhouse-http/src/metrics.rs`: the recorder install, `wayhouse_build_info` and the shared `/metrics` route.
- `crates/wayhouse-http/src/labels.rs`: the validated instance labels (`--region`, `--tag`), at most 16 tags with bounded key and value characters, so a pushed label can never become unbounded input.
- `crates/wayhouse-core/src/listener.rs`: where the connection counters and the closing log line are written.
- `docs/06-operations-observability.md`: the design notes and the catalogue.
