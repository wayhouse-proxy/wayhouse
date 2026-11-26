---
title: "UDP: session tables, batching and the amplification guard"
description: How wayhouse turns a connectionless protocol into sessions, spreads them over cores without locks, expires them cheaply and avoids becoming a reflector.
---

Most game traffic is UDP, and UDP has no connections. There is no handshake to hang a backend choice on and no close to tell the proxy a player left. This page explains how wayhouse builds sessions out of datagrams, and why the listener looks the way it does. The settings are in the [Manual](../../manual/routing/listeners.md).

## The problem

1. **A backend choice has to stick.** A game server keeps per player state, so every datagram of one player must reach the same backend. Something has to remember the choice.
2. **Nothing says when it is over.** The proxy has to guess from silence, for tens of thousands of sessions, without scanning them all every second.
3. **The packet rate is the cost.** Small datagrams at high rates make the syscall the bottleneck, not the bytes. A pump that does one `recv` and one `send` per datagram spends its time in the kernel boundary.
4. **UDP can be spoofed.** The source address of a datagram is whatever the sender wrote. A proxy that answers whoever writes to it can be turned into a reflector that aims traffic at a victim.
5. **A route is not always known at once.** A resolver may need a network call to say where a new client goes, while more datagrams of that client keep arriving.

## One socket per core, one table per socket

The listener runs one task per listener **and** worker. Each task owns its own datagram socket bound with `SO_REUSEPORT`. The kernel hashes a client's 4-tuple to one of these sockets, so a given client always lands on the same worker.

That single property removes the locks. The session table is a plain `HashMap` that only its own task touches, so there is nothing to lock, nothing to share between cores and nothing to contend on. The price is that the kernel's hash decides the balance between workers. If one client sends far more than the rest, one core carries it.

A session is keyed by the client address, and in prefix or transparent mode by `(client, destination)`, because the same client can talk to different addresses of a routed prefix. In prefix mode the socket is bound to the wildcard address and asks the kernel for `IP_PKTINFO`, which reports the **destination** address of every datagram. That address feeds the `dst` matcher in routing, and replies leave with the same address as their source (`sendmsg` plus a pktinfo control message), so a client sees answers from the address it wrote to. A datagram to an address outside the prefix is dropped.

## What a session holds

A new client's first datagram goes through routing (the same [snapshot](snapshot-and-hot-reload.md) and rules as TCP). If routing picks a pool, the session acquires a backend slot for its whole life. That is what makes `least_conn` count sessions and `max_sessions` apply. The session then holds:

- an **upstream socket**, bound to a local port and `connect(2)`-ed to the chosen backend. Because the socket is connected, the kernel delivers only that backend's replies to it. Reply routing needs no table lookup,
- a **reply task** that moves datagrams from the upstream socket back to the client,
- the guards that give back the global `max_udp_sessions` slot, the per source concurrent-session slot and the graceful shutdown count when the session goes away.

In transparent mode the upstream socket is bound to the client's own address, so the backend sees the real client IP, and replies are sent from an `IP_TRANSPARENT` socket bound to the address the client first wrote to.

### The reply task and batches

Receiving and sending both batch. On Linux the ingress path pulls up to **16 datagrams per wakeup** with `recvmmsg(2)`. Consecutive datagrams of one session go upstream with one `sendmmsg(2)` call. The reply task moves up to 16 datagrams per wakeup as well, using a batch buffer that is **per thread**, shared by every session on it. An earlier version kept a 64 KiB buffer per session, which stayed resident after a burst. A per thread batch is only ever used inside one synchronous section and never across an `.await`, which is what makes sharing it safe. Other platforms do one datagram per syscall.

The reply task waits for readability **and** for socket errors. A connected UDP socket that gets an ICMP port unreachable reports `ConnectionRefused` instead of data, and that error has to wake the task. It is used as passive health evidence, as is the first reply of a session (see [Health checks](health-checking.md)).

If the reply task ends for any reason, it flags the session as dead. The next datagram from the client replaces the session instead of being forwarded into a table entry that can no longer answer.

## Expiring sessions with a timing wheel

Idle expiry uses a single level **timing wheel** with 512 slots of one second each. A session is filed in the slot of the second its idle deadline falls in. Every second the hand moves one slot and only that slot is looked at.

The hot path never touches the wheel. Each datagram only stores the current time in the session's atomic `last_ms`. When the hand reaches a slot, each entry there is checked against its live `last_ms`. A session that is really idle is evicted. One that was refreshed is filed again at its new deadline. A session whose timeout is longer than the wheel (more than 511 seconds) is re-filed until it expires. Each session has exactly one entry, and entries carry the session id, so an old entry cannot evict a newer session that took over the same key.

The work per second is proportional to the sessions due in that second, not to all sessions, and eviction is accurate to one second.

## Not becoming a reflector

The rule, stated in the source, is that the proxy only ever sends toward a client that has an **established session**, meaning one that sent a datagram first. A datagram that matches no session and fails routing or admission produces no reply at all, so the proxy never answers an unsolicited packet.

This does not make a forged source harmless. A datagram with a spoofed source address can still open a session for that address if it passes admission, and the backend's replies then go to the spoofed address. What limits the damage is that every new session has to be admitted first, and the guards below bound how many sessions one source and the whole listener can hold.

A new session also has to pass the other guards first: the access list and country filter (each behind a runtime feature flag), the per source rate limiter and the per source and global session caps.

## Routes that need a network call

When an external resolver has to choose the route, the first datagram cannot be forwarded yet. The worker holds a **pending** entry for that client and buffers a few datagrams that arrive meanwhile: at most 4 datagrams per pending session, at most 1024 pending sessions per worker and at most 1 MiB of buffered bytes per worker. Anything past those limits is dropped and counted as `pending_full`. The bound exists because the buffered state would otherwise grow with the attacker's rate if a resolver stalled or the sources were spoofed.

## What was traded away

- **One reply task per session.** It is simple, and the cost per task is small, but a million sessions would mean a million tasks. The latency ledger lists this as an open simplification.
- **Kernel balance.** Workers are balanced by the kernel's hash of the client 4-tuple, not by load.
- **Stickiness with a hard cap.** The affinity table is bounded and is cleared **as a whole** when the cap is exceeded, with no LRU. That is cheap and safe, and the cost is that some players get a new backend choice after an overflow.
- **Silence is the only close.** A client that leaves keeps its session until the idle timeout runs out, holding a backend slot.

## Alternatives that were not taken

- **A shared session table behind a lock.** It would let any worker serve any client but puts a lock on every datagram.
- **A per second scan of all sessions.** It is simple, and costs more the more sessions you have, at exactly the moment you have the most.
- **A connectionless forward with no sessions.** It would be stateless, but could not give a player a stable backend or count sessions.

## Where to look

- `crates/wayhouse-core/src/listener_udp.rs`: the listener, `Session`, `open_session`, `IdleWheel`, the receive and reply batches.
- `crates/wayhouse-core/src/net.rs`: the sockets, `SO_REUSEPORT` and the transparent binds.
- `docs/latency-ledger.md` ([Latency ledger](../../latency-ledger.md)): the measured costs and the open simplifications.
- Manual: [Listeners: every option](../../manual/routing/listeners.md).
