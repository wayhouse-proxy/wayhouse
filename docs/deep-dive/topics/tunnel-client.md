---
title: The backend tunnel on the proxy side
description: How an edge proxy reaches game servers that have no public address, by running one shared WireGuard interface and keeping its peers in step with a controller registry, without ever touching the data path.
---

The first assumption in the original requirements was that backends sit on a trusted internal network. That stops being true once proxies are spread over several regions and the game servers are somewhere else: another cloud, a rented box, sometimes a machine behind a home router. This page is about the proxy half of the answer, the WireGuard **backend tunnel**. The origin half is [the agent](tunnel-agent.md), and the registry in the middle is [the tunnel registry](tunnel-registry.md). The flags are in the [Manual](../../manual/index.md) and the generated [Reference](../../reference/index.md).

## The problem

1. **Backends that cannot be dialled.** A game server behind NAT has no inbound port. Something has to connect from the inside.
2. **The link must be encrypted and authenticated.** The traffic crosses the public internet.
3. **Nobody should exchange keys by hand.** With N proxies and M origins, hand edited peer lists are N times M mistakes.
4. **The data path must not learn about tunnels.** The pump, the UDP sessions and the health checks all dial a plain `SocketAddr`. A second way to connect would reach into every one of them.
5. **A control plane outage must not cut live traffic.** The tunnel is on the path of real players.

## Nothing new on the wire

The design starts from prior art: relay networks that hide the origin address, reverse tunnel tools such as rathole and frp, and Headscale's idea of a control plane that hands out WireGuard peer configuration. None of them is embedded. Real WireGuard is the entire data plane, since it already works when only one side has a public endpoint. The origin needs only to send outbound UDP once, WireGuard's roaming updates the peer's address on every authenticated packet, and a keepalive keeps the NAT mapping open. The proxy side always has a public address, so the case that WireGuard cannot solve, both sides behind restrictive NAT, does not arise.

The consequence is the key design decision: **the data path does not change.** Once an interface exists on the proxy host, an origin's backends are addresses in the tunnel's subnet, and `connect_backend`, the UDP upstream dial and the health checks all work unmodified. A tunneled backend looks like any other routable address.

## One shared interface, many peers

The proxy brings up **one** WireGuard interface (`--tunnel-iface`) and puts every origin on it as a peer, each scoped by its own allowed address. The alternative, one interface per origin, would multiply routing table entries without any isolation that per peer allowed addresses do not already give. Each origin is routed as a host route, `/32` for IPv4 or `/128` for IPv6, to the address the controller assigned it.

Interface management goes through the `defguard_wireguard_rs` library, which hides the kernel netlink backend and the `boringtun` userspace backend behind one API. The proxy tries the **kernel** backend first and falls back to userspace with a warning if it is not available, unless `--tunnel-userspace` asks for userspace outright. The MTU is set to **1420** explicitly for both. The kernel module defaults to that value, but boringtun's TUN device comes up at 1500, which leaves no room for WireGuard's 80 bytes of overhead over an IPv6 underlay; this was found in an IPv6 end to end test. 1420 also stays above IPv6's minimum of 1280.

The WireGuard private key lives in a file (`--tunnel-key-file`). It is generated on first start, **created with mode 0600 from the first byte** (opened with `create_new`, so a chmod after the write never leaves a window in which another local user can open it), and tightened if a later start finds it loose.

All of this is behind the `tunnel` cargo feature. A build without it replaces the module with a stand in that refuses `--tunnel-iface` at startup, and under `--check`, with a message naming the feature. A proxy that silently never forwarded tunneled traffic would be much worse than one that does not start.

## Starting up: the controller is the address authority

The controller allocates tunnel addresses, so no operator picks one. That fixes the order at startup:

1. Load or generate the key.
2. **Register with the controller before the interface exists.** The answer carries the address. The registration holds the name, public key, public endpoint, an optional pinned address, a `boot_id` and the refresh interval. Registration retries for about 30 seconds.
3. Decide the address. The controller's answer wins. A **transient** failure (the controller is unreachable, or answers 408 or 429) falls back to the address saved next to the key, in `<key file>.address`, and logs why. A **rejection** (409 or 422) never falls back, because a stale saved value must not hide an address conflict. With no saved address and no controller, startup fails with the reason.
4. Bring the interface up with that address and the network's prefix.
5. Start two tasks: the reconcile task and a periodic re registration.

The re registration is also how an address change arrives. Every answer carries the address the controller currently holds, and if it differs from the interface's, the proxy moves the interface **without a restart**, keeping its key, port and peers, so established sessions survive. The library can add an address but not remove one, so a small netlink module deletes the old address itself. The old address goes first, since deleting an IPv4 primary address would otherwise take its secondaries with it. If assigning the new address fails, the old one is put back and the next registration tries again. (The same mechanism is described in the [admin API page](admin-api.md) under its small neighbours.)

### The `boot_id`

A restarted proxy has a fresh interface with no endpoint for any origin, and an origin behind NAT can only be reached by its own handshake. With kernel WireGuard the origin's session to the old process still looks valid, so without help the origin would wait for the 120 second rekey, an outage of about two and a half minutes. Every proxy registration therefore carries a random 128 bit `boot_id`, new for each start. The agent compares whole registrations, so a changed `boot_id` makes it remove and re add the proxy's peer, which drops the dead session, and the persistent keepalive of the re added peer starts a new handshake at once.

## Following the registry

The reconcile task holds a long lived `GET /peers/subscribe` connection to the controller's registry, an event stream of registrations and removals, and applies each event to the interface. It always subscribes from the start, since it keeps no revision cursor of its own, but it does keep what it last applied for each name. That matters more than it looks.

**A peer is reconfigured by removing and re adding it**, because the userspace backend answers "not supported" when asked to configure a pubkey it already has, unlike the kernel's upsert. Removing a peer that does not exist is a harmless no op on both backends, so the proxy does it every time. The agent re registers on a fixed interval whether or not anything changed. Without a skip for an identical registration, every refresh would tear down the handshake the previous one had just completed, and a stable tunnel would never stay up. This was found in live end to end testing.

The decision is a small pure function, `plan`, so it can be tested without a device:

- the same registration as already applied is skipped,
- a registration whose public key is held by **another name** is skipped, and the first time it happens a warning names the holder, since otherwise an operator sees a registered peer that never connects,
- a new or changed registration reconfigures its peer, and if the same name now carries a different key the old key's peer is removed as well, so it does not stay live with its routes,
- a removal event removes the matching peer.

A registration is recorded as applied only if the device accepted it. A failed configure must not be remembered, because the next identical event would be skipped and the peer would stay missing until its content changed. A malformed registration is logged and skipped.

**Removal needs an event.** An origin that goes silent keeps its peer. Only a tombstone from the controller, for a deleted origin or an expired lease, removes one. That is the "last known good" rule applied to WireGuard peers instead of routing config.

When the connection drops, the task retries with capped exponential backoff from 500 ms to 30 seconds, and leaves the interface exactly as it is. A controller outage costs the ability to learn about changes and nothing else.

### The preshared key

An origin enrolled with a one time code holds a controller made preshared key, mixed into the Noise handshake as a symmetric key, so a recorded handshake stays safe even against a later quantum computer. The registration names a secret and never contains the key. The proxy fetches the **sealed** value, opens it with its own key file (`--secret-key-file`) through the rollback floor described in [secrets](secrets.md), and refuses to configure the peer without it instead of falling back to an unkeyed peer. It accepts only the name the controller derives from that origin's own name, never another origin's secret and never a cluster secret. One origin's bad secret does not hold back the others. Only an outage of the fetch stops the remaining fetches on that stream.

## The `tunnel` backend source

A pool uses a tunnel through the existing discovery seam, not a new concept: a `backend_sources` entry of type `tunnel` and a pool that names it. Its `fetch()` asks the registry for the origin's currently registered, tunnel internal backend addresses, and the [discovery loop](backend-discovery.md) diffs and reconciles them like any other source. An origin can change its backend addresses by re registering, without any config change.

Two details come from the trust model. The source's name is the origin's registered name, and the source pins the origin's **public key**: every fetch verifies that the registered key still matches before trusting the backend list, so a name later registered under another key is refused. And when the registry answers 404 for an origin this source has already seen, the source returns `Withdrawn`, so the pool is cleared on purpose rather than frozen on stale addresses. An origin that has never registered is just "no addresses known yet", and the usual rule keeps the last known good set.

## What was traded away

- **Only the proxy side is dependable.** An origin must be able to reach some proxy's public endpoint. Two sides that are both behind carrier grade NAT would not work.
- **A key change drops sessions.** Removing and re adding a peer, and rotating a preshared key, interrupts traffic for a few seconds until the handshake completes again.
- **A second protocol copy.** The wire shape of a registration is duplicated here and in the controller and the agent, as other cross process shapes are, because the binaries share no library.
- **No hot path tunnel awareness.** The cost is that the data path cannot say a connection went through a tunnel. That is the point of the design.

## Where to look

- `crates/wayhouse/src/tunnel_boot.rs`: the flags, the start order and the two tasks.
- `crates/wayhouse/src/tunnel_client.rs`: the key file, `bring_up`, the subscribe loop, `plan` and `reconcile_peer`.
- `crates/wayhouse/src/tunnel_address.rs`: address resolution and the saved address.
- `crates/wayhouse/src/netlink_addr.rs`: removing an address and a link.
- `crates/wayhouse/src/tunnel_source.rs`: the `tunnel` backend source.
- `docs/11-backend-transport.md`: the original design and the locked decisions.
