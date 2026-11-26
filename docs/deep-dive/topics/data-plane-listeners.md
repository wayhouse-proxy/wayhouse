---
title: "Listeners: accept loops, SO_REUSEPORT and port ranges"
description: How wayhouse binds its sockets, spreads accepts over workers with SO_REUSEPORT, survives accept errors, peeks at the first bytes, and swaps listeners on a reload without dropping the port.
---

A listener is where a player first meets the proxy. Everything else, the routing, the pools and the byte pump, only happens if the listener accepts the connection cheaply, refuses the bad ones early, and keeps doing so while its configuration changes under it. This page explains how the listeners are built. The options are in the [Manual](../../manual/index.md) and the generated [Reference](../../reference/index.md). What happens after a connection is routed is in [the TCP byte pump](connection-pump.md) and [UDP sessions](udp-sessions.md).

## The problem

1. **One accept loop is a bottleneck.** A game fleet sees bursts of connections, such as a server restarting and every player reconnecting at once. A single task calling `accept` serialises them.
2. **Running out of descriptors must not become a spin.** When the process has no file descriptors left, `accept` fails and the pending connection stays in the backlog, so a loop that retries at once burns a core and floods the log.
3. **A reload must not drop the port.** Changing a listener's routes or limits should not close the socket players are connecting to.
4. **A second proxy on the same port must be noticed.** `SO_REUSEPORT` lets any same-user socket join, which is exactly what makes a duplicate instance dangerous.
5. **Some games use a whole range of ports,** and some traffic arrives for addresses the machine does not own.
6. **Routing sometimes needs the first bytes,** and a TLS hello does not always arrive in one segment.

## One socket per worker

A listener is not one socket. It is a **group**: one task per worker, each with its **own socket bound to the same address with `SO_REUSEPORT`**. The kernel hashes each new connection onto one of those sockets, so accepts spread across workers with no shared lock and no hand-off between tasks. The number of workers is a process setting, and `0` means one per CPU core. The backlog of every socket is 1,024.

For TCP the kernel balances connections. For UDP it pins each client's four tuple to one worker, which is what keeps the per-worker session table lock free (see [UDP sessions](udp-sessions.md)). `SO_REUSEADDR` is set as well, so a port in `TIME_WAIT` after a restart is not a conflict.

### A second instance must not join silently

The flag that spreads the load has a sharp edge: **any socket of the same user that also sets it can join the group.** A stale or duplicate proxy would start without an error and quietly receive a share of the traffic. So before binding the worker sockets, the manager **probes** each address with a socket that does not set `SO_REUSEPORT`, so it conflicts with every existing binding and fails with `AddrInUse`. The TCP probe keeps `SO_REUSEADDR` so a port in `TIME_WAIT` is not a conflict, and the UDP probe sets neither. The probe is closed before the real bind.

The module is honest about the limit. The probe is not atomic with the bind, so two instances starting at the very same moment can both pass it. It catches the common mistake early and is not a reservation. The real bind still has to handle `AddrInUse`.

### Port ranges and odd addresses

A bind can be a range such as `host:lo-hi`. It resolves to the first port plus a list of extra binds, and **each port gets its own set of worker sockets**. Because that is one socket per port per worker, a range is capped at **1,024 ports**, which also stops a typo like `0-65535` from asking for millions of sockets. Port 0 and reversed ranges are refused. All the ports of one range remain **one logical listener**, sharing a single rate limiter and a single per-source limiter. A prefix mode listener needs exactly one wildcard socket, so it refuses a range.

Two options reach addresses that are not local:

- **Free bind** sets `IP_FREEBIND` (or the IPv6 form) so the socket can bind an address that is not on an interface yet, such as a floating address that arrives later.
- **Transparent** sets `IP_TRANSPARENT` on Linux so the socket accepts connections that a TPROXY rule redirected, and `getsockname` on each accepted stream then returns the address the client really addressed. Off Linux the call fails with an unsupported error. For the reverse direction, `connect_tcp_from` binds the real client address as the source of the backend connection, again with `IP_TRANSPARENT`. If the client and backend are different address families it logs one warning a minute and connects normally.

UDP has three ways to learn where a datagram was addressed. **Plain** uses the socket's own address. **Prefix** asks the kernel for packet info, so one wildcard socket serves a routed prefix. **Transparent** reads the original destination with `IP_RECVORIGDSTADDR`.

## The accept loop

Each worker runs a loop that waits on two things, a stop signal and `accept`. Per connection, the order is deliberate, and cheap checks come first so that a refused connection costs almost nothing:

1. Turn the peer address into its canonical form. A dual stack `[::]` socket reports IPv4 clients as `::ffff:a.b.c.d`, and mapping them back means every filter, matcher and PROXY header sees one spelling per client.
2. The **source ACL**, then the **geo filter**, which **fails closed** if its database did not load.
3. The **rate limit**, then the **per source concurrent cap**, then the **process wide connection cap**.
4. Only then count the connection, register it with the tracker and spawn a task.

Every refusal increments one counter with a `filter` label naming the reason, and logs at debug level. Nothing is allocated for a refused connection beyond the accept itself. The filters are described in [filters and limits](filters-and-limits.md).

The spawned task holds three guards, for the tracker, the global slot and the per source slot, and they release on any exit path, so a panic or an early return cannot leak a slot. The tracker guard is what a graceful shutdown waits for.

### Accept errors

Not every failed `accept` means the same thing, and the loop tells them apart.

- **The peer gave up** during the handshake (aborted, refused or reset). The next `accept` succeeds at once, so pausing only stalls the other connections queued on this worker. These skip the pause, are counted, and the first 64 in a row are free. After that a socket that keeps failing this way backs off like any other error, so it cannot spin the worker.
- **Everything else** is treated as a resource problem: out of descriptors, out of memory, out of buffers or an unknown kind. The worker pauses **50 ms, doubling per consecutive failure up to 1 s**, and stays responsive to a shutdown while it waits. A success resets the streak.

The warning is limited to **one per second** per worker, and the number of failures swallowed in between is reported in the next one. Without that, an out of descriptors event would write a line per spin of the loop. The counter of accept errors is kept per listener.

## Peeking at the first bytes

Many routes need to see the start of the conversation: a TLS server name, a game's handshake bytes, or a sniffer's verdict. A route that needs them sets a peek length, capped at **4,096 bytes**, and a listener whose routes need nothing skips the peek completely.

The bytes are read with `MSG_PEEK`, so they stay in the socket for the backend. The subtlety is that **one peek returns only what has arrived at that instant**, which for a large TLS ClientHello with long extensions can be the first segment of several. The server name sits inside the first record, so a single peek could miss it. The loop therefore re-peeks every 5 ms, within a budget of **250 ms**, when the first byte is a TLS handshake record, until the hello is complete or the wanted length is in hand. A non TLS first byte keeps a single peek, because a byte prefix match fits in the first segment and the documented length window is "what one peek returned".

A client that connects and sends nothing waits out the 250 ms and is then routed as if nothing had been sent, so only address based and `always` routes can match. That bounded wait is the price of routing on content, and it applies only to listeners that need it. A sniffer that rejects drops the connection before the route lookup and before any push hint is considered, so a spoofable hint cannot override a decision made from content (see [sniffers](sniffers.md)). How matchers use the bytes is in [routing and matchers](routing-and-matchers.md).

## Reloading without dropping the port

The listener manager keeps a map from listener name to its running group, and a reload calls `reconcile` with the new snapshot. The rule is a diff by name:

- **Unchanged** config: keep running. Pool and route contents are read live from the snapshot by the running tasks, so only a change to a listener's own identity, such as its bind, protocol or routes, forces a new group (see [snapshot and hot reload](snapshot-and-hot-reload.md)).
- **Changed** config: bind a **new group first**, then stop the old one.
- **New** name: bind and spawn.
- **Removed** name: stop.

**All the sockets are bound before anything is stopped.** A listener that cannot bind is reported to the caller and, if it had a running group, the old group keeps running and the change is retried on the next reload, since the running config still differs. A reload therefore never leaves a dead listener behind. At startup the rule is stricter: it is all or nothing, and the first bind failure aborts the start.

Probing needs one exception. When a change keeps the same address, the old group of this very manager still holds it, and the probe would refuse the port. Addresses that overlap one the manager's own groups hold are **exempt**, where overlap means the same port with the same IP, or either side a wildcard of the same family. That also covers two listeners swapping ports in one reload.

The per source limiter is **carried over** to the new group and only re-capped. Connections the old group admitted still hold their slots, so a fresh limiter would let `max_per_ip` more connections in on every reload.

### TCP is simple, UDP is not

The old TCP group is signalled, and its accept tasks return at once, so the reload waits for them. Established connections are not touched, since they belong to tasks of their own and the tracker.

UDP has a harder problem. A worker with live sessions only returns once every session has idled out, and a chatty client can delay that forever. At the same time, if an old worker's socket stays in the `SO_REUSEPORT` hash, the kernel keeps sending new flows to it. So the new group is handed the list of addresses it binds, and **an old worker whose address overlaps one closes its socket at once**, ending its sessions, and every flow reaches the new group, which opens fresh sessions. The reload waits for those workers, up to about a second (200 polls of 5 ms), and moves on. A UDP group of a removed listener, or on a port nobody takes over, keeps draining in the background in a list of **retired** groups, and finished ones are reaped. The reload never waits for it.

A stopped group is parked in `retired` **before** the first await. That way a shutdown racing a reload, or a cancelled reload, still finds and joins every group instead of losing one with a local variable.

The groups map is a plain mutex that is never held across an await. Stopped groups are moved out first and awaited afterwards.

## Shutdown

Shutdown sets the draining flag, signals every group, and waits for the accept tasks, the health checker and then the **connection tracker to go idle**, all bounded by a grace period of **30 seconds** by default. When it expires, the number of connections still in flight is logged and they are dropped as the runtime goes away. A listener stopping stops accepting at once, and the connections it already holds are the ones the grace period is for (see [the byte pump](connection-pump.md)).

## Small shared helpers

- **Canonical addresses.** One function unmaps IPv4 mapped IPv6 addresses. It is for policy decisions, and reply traffic keeps the address the socket reported.
- **A monotonic clock.** Milliseconds since process start, cheap enough to store in an atomic for "is this due yet" checks. The once-a-minute family mismatch warning uses it with a compare and set, so the workers log it once between them without a lock.
- **Local resource errors.** Descriptor, memory, buffer and ephemeral port exhaustion are recognised as the proxy's own shortage, which health logic reads as "unknown" so that a shortage here never ejects a backend (see [health checking](health-checking.md)).

## What was traded away

- **No balancing between workers.** The kernel hashes connections, so a few heavy flows can land on one worker. The cost of fixing that would be a shared queue and its lock.
- **The probe is a check and not a lock,** as above.
- **Range binds multiply sockets.** A 1,024 port range on many workers is a lot of descriptors, which is why the cap exists.
- **The peek adds latency** of up to 250 ms for a silent client on a listener that routes on content. Listeners that do not route on content pay nothing.
- **A UDP reload briefly resets sessions** on a handed over address, in exchange for never splitting a flow between the old and the new group.

## Where to look

- `crates/wayhouse-core/src/net.rs`: binding, probing, transparent and free bind.
- `crates/wayhouse-core/src/listener.rs`: the TCP accept loop, accept error backoff and the peek.
- `crates/wayhouse-core/src/listeners.rs`: groups, reconcile and the hand-off.
- `crates/wayhouse-core/src/runtime.rs`: worker count, start and shutdown.
- `crates/wayhouse-core/src/util.rs`: canonical addresses and the clock.
- `crates/wayhouse-config/src/parse.rs`: the bind range parser.
