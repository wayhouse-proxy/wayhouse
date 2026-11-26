---
title: "The TCP byte pump: splice, half-close and draining"
description: How wayhouse moves bytes between a player and a backend without copying them in userspace, when it decides a connection is over, and how shutdown waits for the last one.
---

Once a connection is routed, the proxy has one job left: move bytes in both directions until the conversation ends. It sounds trivial, and it is where most proxies spend their CPU. This page explains how wayhouse does it, what "ended" means, and how a shutdown waits for connections that have not ended yet.

## The problem

1. **Copying costs.** The obvious pump reads from one socket into a buffer and writes the buffer to the other. Every byte crosses from the kernel to userspace and back, and every connection needs buffers. With tens of thousands of connections that is a lot of memory and a lot of syscalls whose only effect is to move data the proxy never looks at.
2. **TCP is two one-way streams.** A client may send its last request and close its sending side while still waiting for the reply. A pump that closes both directions at the first EOF cuts off that reply.
3. **A connection can be alive and idle, or stuck.** Players who stop playing leave sockets behind. A peer that stops reading can block a write forever. Something has to decide that a connection is over without punishing a quiet but healthy one.
4. **Shutdown must be graceful.** A restart should stop taking new connections, let the running ones finish and then exit, but not wait forever.

## Copy through the kernel

On Linux the bytes never reach userspace. `copy_spliced` creates one non-blocking pipe per direction per connection and moves data in two steps with `splice(2)`: `socket → pipe`, then `pipe → socket`.

```text
client socket ──splice──► kernel pipe ──splice──► backend socket     (one pair per direction)
```

Each call moves up to 64 KiB, which is the default pipe capacity. The pipe is just a kernel buffer, so the data is handed from one socket's buffers to the other's without being copied into the proxy's memory. The proxy only decides when to call `splice`.

That decision uses Tokio's readiness. The code waits until the source socket is readable, calls `splice` through `try_io`, and treats `EAGAIN` as "not actually ready, wait again". Writing is the same on the other side. Nothing blocks a worker thread.

If creating the pipe fails (for example, the process is out of file descriptors), the connection falls back to `copy_buffered`, a plain loop with a 32 KiB buffer using `try_read` and `try_write`. The same fallback is the only path on platforms without `splice`. The fallback exists so a resource shortage costs speed, not connections.

Both directions share the same two socket references (Tokio's readiness API takes `&self`), so one task runs both directions. There is no `into_split` and no extra task per connection.

## Half-close and the end of a connection

Each direction is copied until its **source** reaches EOF. At that point the pump calls `shutdown(Write)` on the destination, so the peer sees the EOF the proxy just saw. The other direction keeps going.

The rules of the main loop are small:

- A clean EOF finishes **only its own direction**. The reply direction may still be streaming.
- An **error** (a reset, a failed write) or an **idle expiry** in either direction ends the whole connection. The other direction is dropped, and dropping both sockets closes both sides.
- When both directions have finished, the connection is done.

The result is a `ConnOutcome` with the byte count in each direction and the backend. It is filled in as bytes move, so the totals survive an error and the connection log still shows what was transferred.

## One idle clock for both directions

The idle timeout is `idle_timeout` of the pool (the fallback for a resolver target is 90 seconds, with a connect timeout of 300 ms). It is held in a single shared `Progress` object, a last-activity timestamp updated by an atomic maximum. **Either** direction moving bytes resets it. A connection is idle only when **neither** has moved anything for the whole window.

Every wait for readiness is bounded by the time left on that shared clock. When a wait times out, the code looks at the clock again instead of failing, because the other direction may have moved bytes in the meantime. This is why a one-way stream (a download, say) is never cut off for being idle on its quiet side.

The cost of this design is named in the source: a direction stuck on a write because the peer stopped reading is no longer bounded by its **own** timer while the other direction keeps moving bytes. Players who send nothing and read nothing still expire. A stream that never stops in one direction keeps the other alive.

## Connecting, and what the backend learns

Before the pump starts, `handle_tcp` picks a backend from the pool, claims an active-session slot (a guard released on drop, which is also what `max_sessions` counts) and connects with the pool's connect timeout. The result is a passive health observation: a successful connect is a success, a refused or timed out connect is a failure, and a local shortage of descriptors or ports is deliberately not reported (see [Health checks](health-checking.md)). Connect failures are counted in `wayhouse_backend_connect_errors_total` with kind `refused` or `timeout`.

If the pool asks for a PROXY protocol header, it is written to the backend **before any client byte**, so the backend's parser sees it first. A failure to write it counts against the backend. The mechanism has its own Deep dive topic, PROXY protocol and transparent mode.

A resolver can also choose a fixed target instead of a pool. That path (`handle_tcp_target`) has no health check, no per-backend cap and no active-session guard, but it uses the same connect and pump code.

## Tracking and graceful shutdown

Every proxied TCP connection and UDP session holds a `ConnGuard` from the `ConnTracker`. The tracker keeps two things:

- a counter of live connections, kept in a `tokio::sync::watch` channel, and
- a small registry from id to metadata (protocol, listener, peer, local address, pool, backend, start time) that backs `GET /sessions`.

The registry sits behind a mutex that is taken **once per connection** at start, at drop and when routing sets the target. It is never taken per byte. The pool and backend are not known yet when a TCP connection is tracked, because routing has not run, so `set_target` fills them in later.

On shutdown, the accept loops stop first. Then `wait_idle` waits for the counter to reach zero. Because the counter is a `watch` value, `wait_for` has no lost-wakeup race: if the last guard drops just before the wait begins, the wait returns at once. The caller bounds the wait with a grace period, 30 seconds by default, taken from the config when set. After that the process exits and the remaining connections are closed.

## What was traded away

- **No inspection.** With `splice` the proxy cannot see or alter the bytes. That is fine for a transport that is opaque by design. Anything that needs to look at the stream has to do it before the pump starts, as the SNI peek and the sniffers do.
- **Two pipes per connection.** Each direction needs a pipe, which is two file descriptors, so a connection uses four more on top of its two sockets. They are cheap in memory but they count against the descriptor limit, which is why a failed setup falls back to the buffer.
- **Linux specific speed.** Other platforms work, with a userspace copy.

## Alternatives that were not taken

- **`tokio::io::copy_bidirectional`.** It would work everywhere, but copies every byte through userspace, uses its own idle handling and cannot share one idle clock the way the pump does.
- **A task per direction.** It doubles the tasks per connection for no gain, since one task can drive both directions from the same socket references.
- **Closing on first EOF.** It breaks clients that half-close after their request.

## Where to look

- `crates/wayhouse-core/src/proxy.rs`: `handle_tcp`, `pump`, `Progress`, `copy_buffered` and the `splice_impl` module.
- `crates/wayhouse-core/src/drain.rs`: `ConnTracker`, `ConnGuard` and `wait_idle`.
- Manual: [Drain and maintain](../../manual/operate/drain-and-maintain.md) for how an operator uses these properties.
