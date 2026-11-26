---
title: The admin API
description: The small HTTP surface every proxy exposes for operators, load balancers, scrapers and the control plane, and the rules that keep it safe to leave running.
---

Every proxy opens an HTTP port for the humans and machines that look after it. A load balancer asks whether it is ready, Prometheus scrapes it, an operator drains it before a deploy, a launcher pushes a route hint, the controller's tooling reads its pools. This page explains what is on that surface, how it is protected, and why some writes are kept apart from the rest. The endpoints are listed in the [Manual](../../manual/operate/monitor.md) and in the generated [Reference](../../reference/index.md).

## The problem

1. **Many callers, different trust.** A liveness probe has no credentials. An operator's curl has a token. The aggregator fans out with a protocol header.
2. **It must never be the reason a node is down.** The API runs on the same process as the data path. A stuck handler, a huge body or a failed bind must not hurt traffic.
3. **Some writes are dangerous.** Uploading code, adding a backend, writing the config file: each is useful and each is something you want to switch off in an emergency without locking yourself out.
4. **A write that is not applied is worse than one that is refused.** The caller needs to know whether the change took.
5. **Secrets pass through.** The config file contains keys, and a read only viewer must not see them.

## Shape

It is an `axum` router, bound to an internal address only. One route stands outside everything else, `GET /healthz`, which answers `ok` with no credentials, the plain liveness probe convention. Everything else sits behind one layer chain:

1. a **bearer check**, the outermost layer. The token comes from `settings.admin.auth_token`, held as a rotating secret so it can change without a restart. With no token configured, the API is open, and the source says it is bound to an internal address only. The check comes first on purpose: the next layer counts protocol mismatches into the fleet view, and an unauthenticated request must not be able to reach it,
2. a **lenient protocol gate**. The aggregator and controller tooling send a protocol header. An operator with curl does not, so a missing header is accepted, and only another major version is refused.

TLS is optional (`settings.admin.tls`) with a certificate that reloads from disk, and the handshake limits are configurable: how many handshakes may be pending in total and per source, and how fast one source may start new ones. A failed bind ends only the API task and is logged, since a proxy that cannot open its admin port should still carry traffic.

## What is on it

Reads: `/readyz`, `/metrics` (Prometheus text), `/pools`, `/config` (a plain text view of the active snapshot), `/config/schema` (the JSON Schema, with the schema version in a header, so a UI on a proxy with no controller can build forms from the proxy it talks to), `/web`, `/features` and `/sessions`.

`/readyz` has three answers: ready, `draining` and `not ready`, the last two with status 503. **Drain** is two calls, `POST /admin/drain` and `POST /admin/undrain`. Drain makes readiness fail without stopping the data path, in flight sessions keep running, and the operator waits for the active count to fall and then stops the process. Drain and the flag endpoints are never gated by a kill switch, so an operator cannot lock themselves out of the way back in.

`/sessions` lists every live connection and UDP session, with optional filters on listener, pool, protocol and source IP. A source IPv4 address is canonicalised first, so a mapped IPv6 form matches. It is a point in time snapshot: a session that ended before the response is simply absent.

Writes, in rough order of risk:

- `POST /route-hint` records a source IP to pool hint (see [route matching](routing-and-matchers.md)). The pool must exist, the lifetime must be 1 to 3,600 seconds (30 by default).
- `PATCH` `/pools/{pool}/backends/{addr}` sets an operator state, `enabled`, `draining` or `disabled`.
- `POST` and `DELETE` on a pool's backends add and remove one. These edit the runtime [overlay](pools-and-load-balancing.md), so they survive a file reload.
- `/admin/sniffers` lists, uploads, deletes and rolls back sniffer modules ([sniffers](sniffers.md)).
- `PUT /config/file` replaces the config file.
- `POST /config/validate` parses and validates a document and applies nothing, answering `{"ok":true,"schema_version":N}` or a 422 with the error. It is a read in effect, and ungated.

## The kill switches

The route, the sniffer upload and the backend edit groups each sit behind a [runtime feature flag](runtime-feature-flags.md) (`admin_write.route_hint`, `admin_write.sniffer_upload`, `admin_write.backends`). While a flag is off, the gate answers **403 before the handler runs and before the body is read**, with a message pointing at `GET /features`. Only the routes that change a pool, a sniffer module or a route hint carry a gate. The gate is a small middleware that reads one bit, so it costs nothing when open. The failure mode it covers: a leaked token. An operator switches the three write groups off fleet wide, and the reads, the drain and the flag endpoints stay open for the cleanup.

## Writing the config file

`PUT /config/file` is the one write with a protocol of its own, because it changes what the proxy is.

- **It is off by default.** The proxy must be started with `--allow-config-write`, otherwise the answer is 403 with that instruction.
- **It is compare and swap.** The caller states which version it edited, either by `If-Match` with the ETag of its read, or by `?base=` with the revision header. With neither, the answer is `428 Precondition Required`. A file that changed in the meantime answers `409` with the code `stale`. Two editors cannot silently overwrite each other.
- **It validates before it writes.** An invalid document is a 422 with the parser's message, and a file or directory that is not writable (a read only mount) is a 503 that says so.
- **It reports whether it applied.** After writing, the handler asks for a reload and waits up to **5 seconds** for the live snapshot to be swapped, comparing pointers. The answer says `applied: true` or, if the swap did not happen in time, saved but not confirmed, with a note that the proxy may still be reloading **or may have refused the file and kept the running config**. A save that is a lie is the thing to avoid, and the reload keeps the old snapshot when it rejects a new one (a bad TLS path, a port in use).
- **It hides secrets on read.** `GET /config/file` returns the file with inline secrets masked, and a save of the shown text keeps them. If the file does not parse, the secrets cannot be located, so the answer is a 422 telling the operator to fix it on the host rather than risk leaking. `?previous=1` returns the copy the last write kept. Bodies are limited to 1 MiB, and the file work runs on the blocking pool, not on a reactor thread.

## Uploading code

A sniffer upload is vetted before it touches the disk: the body limit is the module limit, the bytes must be a loadable module, and on an instance with pinned modules the hash must match a pin, otherwise the answer is 409, because the next rescan would reject the file and a stray unpinned file makes the next startup fatal. The file is written to a temp file of its own and renamed, the previous version is kept as a hidden dotfile, and one lock serialises upload, rollback and delete. Nothing is written when any check fails. [The sniffers page](sniffers.md) covers the rest.

## Two small neighbours

`procinfo` samples the process's own open file descriptor count from `/proc/self/fd` (Linux only) into a gauge on a timer, and reports the `RLIMIT_NOFILE` soft limit, the cap that count is actually bounded by. Every proxied connection holds descriptors, so the limit is a real capacity limit and worth graphing. It lives in the binary because it is host observability, not data path state.

`live_interface` is the WireGuard side of the [backend tunnel](../../11-backend-transport.md): it holds the running interface and the tunnel address it carries, and moves the address while the process runs, keeping the device, its key and its peers so established sessions survive. The library can add an address but not remove one, so the old address is deleted with a netlink call of its own and the new one assigned after it. Deleting first matters for IPv4: deleting a primary address also deletes the secondaries on its subnet, so an address added first would be swept away. If assigning the new address fails, the old one is put back if possible, and a `dirty` flag makes the next call repair the interface even when asked for the address it believes it has.

## What was traded away

- **An open API with no token.** Possible: a token is optional, not mandatory, and the API is meant for an internal address.
- **A coarse kill switch.** Three groups, not one flag per endpoint.
- **Plain text for some views.** `/config`, `/sessions` and the drain answers are plain text, made for a human with curl and `grep`. Machine readers use the JSON endpoints.
- **A wait is not a guarantee.** The apply confirmation is a five second best effort, and it says so when it cannot confirm.
- **Point in time lists.** `/sessions` is a snapshot of one moment, as the source itself notes.

## Where to look

- `crates/wayhouse/src/admin.rs`: the router, the gate, and every handler.
- `crates/wayhouse/src/config_file.rs`: the version tags, the compare and swap write and the redaction.
- `crates/wayhouse/src/procinfo.rs`: the descriptor gauge.
- `crates/wayhouse/src/live_interface.rs`: the live address change.
- Manual: [Monitor](../../manual/operate/monitor.md) and [Drain and maintain](../../manual/operate/drain-and-maintain.md).
