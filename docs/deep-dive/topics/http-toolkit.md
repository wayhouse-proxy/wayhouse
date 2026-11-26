---
title: "The shared HTTP toolkit: bearer auth, TLS, SSE and policy"
description: "The small library every fleet binary builds its HTTP on, and the rules it enforces once so each binary cannot get them subtly different: bearer checks, a hardened TLS listener, protocol versions, rotating secrets and a bounded event stream reader."
---

Six binaries speak HTTP to each other: the proxy's admin API, the controller, the aggregator, the UI, the agent and the control process. Each one needs a server with authentication, TLS and limits, and a client with sane timeouts and the same trust roots. Written six times, the copies would drift, and the drift is where security bugs live. `wayhouse-http` is the one crate that holds those rules. This page walks through them. The options are in the [Manual](../../manual/index.md) and the generated [Reference](../../reference/index.md). The crate is also home to two things covered elsewhere: the metrics recorder (see [metrics and logs](metrics-and-logs.md)) and the secret lookup (see [secrets](secrets.md)).

## The problem

1. **One rule, many binaries.** "A token must never go over plain HTTP beyond loopback" is only a rule if every client obeys it.
2. **Servers face hostile peers.** A client that opens a connection and sends nothing, or one byte at a time, must not hold a task forever.
3. **Certificates renew.** A renewed certificate must be served without a restart, and a half written one must not break the server.
4. **Components upgrade at different times.** Two binaries on different versions have to either work together or refuse clearly.
5. **Secrets rotate while processes run,** and the parties that hold a secret reload at different moments.
6. **Streams must be bounded.** Many components follow an event stream, and a hostile or broken peer must not exhaust their memory.

## Outbound clients: one builder

All outbound clients come from one builder. `reqwest` with rustls trusts only the Mozilla roots compiled into the binary, and `--ca-file` adds an operator's own certificate authorities **on top of** those roots, never instead of them. The extra roots are a process wide setting made once from `main`, before any client is built, rather than a parameter threaded through every call site. The deliberate simplification is stated in the module: not setting it means exactly the old behaviour. The builder also sets the protocol header (below) as a default header on every request.

### Tokens never in clear beyond loopback

A client that would send a bearer token over plain `http://` to a non loopback host **refuses to start**, since anyone on the path could read the token and act as this process. `--allow-insecure-controller` turns the refusal into a warning, for a network that is private by construction, such as a container network or a trusted overlay. The flag is recorded once at start for call sites deep in a binary that have no argument list of their own. The agent is a client only, so this rule lives in a part of the crate that does not need the server feature.

### Addresses a client may be sent to

Two outbound paths must not be steered at internal infrastructure: the UI's registry client and the plugin host's `http` capability (see [plugins](plugin-host.md)). They share **one allowlist function**, so the two cannot drift. An address is acceptable only if it is not loopback, private, link local, unspecified, "this network", multicast, broadcast, carrier grade NAT, benchmarking, documentation, reserved, site local or unique local. An IPv4 mapped IPv6 address is judged as its IPv4 form, which closes the usual bypass.

## Servers

### The bearer middleware

Every fleet HTTP server shares one bearer token middleware. It is a **single shared secret**, not per caller identity or roles (those are layered above it, see [the controller core](controller-core.md)). With no token configured, the routes are open, the long standing posture of network boundary only authentication. The rules that refuse or warn about running open on a routable address live in each binary's start up, through shared helpers. The middleware state is independent of the router's own state, which is why one function serves every router. A wrong or missing token is a 401, with a body that a binary can override.

### Limits at the door

Startup checks reject a shared secret shorter than **16 bytes**, whether a bearer token, a peer token or a gossip key, and name the flag it came from.

The connection loop behind the TLS listener is what `axum::serve` does plus the two limits it lacks:

- **A header read timeout** of 60 seconds. HTTP/1 must deliver a full request head in time, and a connection idle between requests is closed by the same timer. Long lived streams such as server sent events or WebSockets are unaffected, because the timer runs only while a request head is awaited. There are also a first byte timeout of 30 seconds and an idle timeout of 60.
- **HTTP/2 keep alive pings** every 30 seconds with a 20 second answer deadline, so a dead HTTP/2 connection does not linger.
- **A connection cap** of 4,096. The loop takes a permit **before it accepts**, so past the cap new peers wait in the kernel's accept queue instead of costing a task.

### The TLS listener

Native TLS is built into every server. A reloading certificate holds the served pair behind an `ArcSwap` and re reads the files every 30 seconds when they change, so a renewed certificate from certbot or cert-manager is picked up without a restart. **A broken or half written replacement keeps the current certificate.** The listener runs each handshake in its own task, so a slow client never blocks accepting, and bounds the handshakes in flight:

- 512 pending in total, 16 per source, where a source is an IPv4 address or an IPv6 `/64`, since anyone with IPv6 has a whole `/64`, and an IPv4 mapped peer counts as its IPv4 address,
- a rate limit per source, 20 new handshakes a second on average with bursts of 64, because without it a source could cycle connects under its pending cap forever,
- 10 seconds for a whole handshake and 3 seconds for the ClientHello inside it.

Refusals are counted by reason (per source cap or rate), and warnings are limited to one per interval. These counters are the exception to the rule that metric names live in the core crate, as the metrics page notes, because this crate has no dependency on the core and must not gain one.

## Protocol versions

Every component to component HTTP request carries a header with the wire protocol, `<major>.<minor>`, currently 1.3. The numbers are independent of the product version. A **major** bump is an incompatible wire change and makes peers refuse each other. A **minor** bump is additive and accepted. Clients send the header through the shared builder, and servers gate their component facing routers with a gate that also **echoes the server's own version on every response**.

A request with no header is refused as well, since a caller without it predates versioning, except on the **lenient** gate used by routes that operators also call by hand with `curl`. An unparsable value or another major is always refused with 426. The refusals are counted by route group, and the aggregator turns a recent increase into the red "outside window" mark on the fleet page (see [the aggregator](aggregator.md)). Admin, UI, `/healthz` and `/metrics` routes are not gated.

For additive changes, a sender can ask whether the caller understands a minor before sending an optional field, through the version recorded on the request. The module notes honestly that nothing uses this yet and that a client side memory of what a peer last said was removed rather than kept unused.

## Rotating secrets

A secret that can change while the process runs keeps the **old value usable for an overlap window**, because parties reload at different moments. Two periods matter, set per call. The old value is **signed with** for one period and **accepted** for another. A secret that peers sign with, such as the gossip key, accepts the old value for **twice** as long as it signs with it, because a replica that reloads late still signs with the old value after an early one has switched. A secret nobody signs with, such as the admin token, accepts the old value just for the overlap. After that only the new value is accepted, and a restart drops the old value at once. Rotating to the value already in use changes nothing, and a second rotation forgets the value before the last. When checking a presented value, **all accepted values are always compared**, so timing does not reveal which one matched.

## Streams that cannot hurt the reader

Every client that follows a subscribe stream reassembles events from a body that arrives in arbitrary chunks. The buffer holds only the **unterminated tail**: complete events are handed out and dropped. A peer that never sends the blank line that ends an event would otherwise grow it without bound, so a tail past **16 MiB** is an error and the caller drops the stream and reconnects with backoff. Bytes are buffered raw and decoded once per complete event, so a multi byte character split across two chunks survives.

A subscriber that waits for bytes drops the stream after **60 seconds** of silence. Every controller stream sends a keep alive comment every 15 seconds, so four missed ones mean the connection died without a FIN or a reset. Without this limit, a half open TCP connection would freeze a subscriber's config for good.

## Small shared pieces

- **One body parser.** A request body becomes a typed value in exactly one function, which is `serde_json::from_slice` with a trailing byte treated as an error. The typed target is the validation, and a body is never parsed by hand elsewhere. Callers keep their own error mapping.
- **One version string.** Each binary prints the same `<version> (<commit>)`, so the commit is compiled into every one of them and a deployment check can look for it.
- **One limit for sniffer modules,** 8 MiB, shared by the proxy route and the routes in front of it so the three limits cannot drift.

## What was traded away

- **A single shared secret.** There are no per caller identities in the middleware. The scoped tokens of the controller sit above it.
- **Process wide trust roots.** A global is simple and means one binary cannot use two different sets.
- **Strict rejects of unversioned callers,** which makes a curl without the header fail on the gated routes, and the lenient gate exists to relax exactly that.
- **A rotation overlap is a window,** and for its length a revoked value still works.
- **Open without a token.** The default posture on loopback is open. The refusal on routable addresses is the binary's job.

## Where to look

- `crates/wayhouse-http/src/lib.rs`: the client builder and the shared constants.
- `crates/wayhouse-http/src/bearer.rs` and `server.rs`: the clear text rule and the bearer middleware.
- `crates/wayhouse-http/src/conn.rs` and `tls.rs`: the connection loop and the TLS listener.
- `crates/wayhouse-http/src/protocol.rs` and `peers.rs`: protocol versions and the gates.
- `crates/wayhouse-http/src/rotating.rs` and `policy.rs`: rotating secrets and secret strength.
- `crates/wayhouse-http/src/sse.rs`: the event buffer.
- `crates/wayhouse-http/src/public_ip.rs` and `json.rs`: the address allowlist and the body parser.
