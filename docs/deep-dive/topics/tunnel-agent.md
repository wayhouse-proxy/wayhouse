---
title: "wayhouse-agent: WireGuard on the origin side"
description: How a small separate binary next to a game server joins the tunnel from behind NAT, registers itself, follows the fleet of proxies and survives address changes and key rotations.
---

The proxy half of the backend tunnel is described in [the tunnel client](tunnel-client.md). This page is about the other end: **`wayhouse-agent`**, the process that runs on the host of the game server, the _origin_. It needs no public address and no open inbound port. The flags are in the [Manual](../../manual/index.md) and the generated [Reference](../../reference/index.md).

## The problem

1. **The origin cannot be reached.** It may sit behind a home router or in a network the proxies cannot route to.
2. **The game server must not change.** It binds normally on a local address and should never know a proxy is involved.
3. **The fleet of proxies changes.** A proxy added after the origin was deployed must be picked up without restarting the agent.
4. **Identity must be stable.** A pool pins an origin by public key, so the key cannot be regenerated on every boot.
5. **It must work where the kernel cannot help.** Containers and small hosts often have no `wireguard` module or no `CAP_NET_ADMIN`.

## A separate binary

The agent is its own crate and its own process, like the controller, the aggregator and the UI. The reason is the dependency: WireGuard and netlink libraries stay out of the proxy binary that carries the traffic. (The proxy does contain a client for its own half, behind the `tunnel` cargo feature.) The agent does not proxy or inspect anything. The WireGuard interface moves the packets, and the game server listens on its own loopback or interface address as usual.

It does five things:

1. keeps a **private key** that is the origin's identity,
2. brings up **one local WireGuard interface**,
3. **registers** the origin's public key and backend addresses with the controller,
4. **subscribes** to the registry of proxies and keeps them as peers of the interface,
5. follows **address changes and key rotations** from the controller.

## Identity and the interface

The key is kept in the data directory (`--data-dir`) and is never regenerated once it exists, because `backend_sources[].pubkey` pins against it.

The interface is one shared device named `--iface` (default `wayhouse-agent0`) listening on `--listen-port` (default 51820), with every proxy as a peer scoped by its own allowed address. The kernel backend is tried first and boringtun userspace is the fallback, with `--userspace` to skip the guaranteed failing first attempt on hosts that have no kernel module or no `CAP_NET_ADMIN`. The MTU is set to 1420 for both backends for the reason described in the client page: boringtun's device defaults to 1500, which leaves no room for WireGuard's overhead over an IPv6 underlay.

### The kick

A freshly added peer on the origin has no reason to talk until its keepalive fires. With the kernel backend that is about 2 seconds, but boringtun arms a peer's persistent keepalive only **25 seconds** after the peer is created, and the proxy has no endpoint for this origin, so nothing else would start the handshake. The agent therefore sends one empty UDP datagram to the proxy's tunnel address, on the discard port, right after adding a peer. It is best effort, and the keepalive still covers a failure. Every peer also carries a persistent keepalive of 25 seconds, which keeps the NAT mapping open indefinitely.

## Registering, before the interface exists

The controller allocates the tunnel address, so the order at startup is the same as on the proxy: **register first**, because the answer is the interface's address. The registration holds the name, the public key, the backend addresses the origin fronts, an optional public endpoint (informational only, since WireGuard's roaming makes it unnecessary after the first handshake), an optional pinned address and the generation of the preshared key it holds. The backends are written as `host:port`, or `:port` for "my tunnel address plus this port"; every host must be the origin's own tunnel address.

- The HTTP client has a connect timeout of 5 seconds and a request timeout of 10, which is what makes the retry budget real. Without them, a controller that accepts the TCP connection and never answers would block startup and the refresh loop forever.
- An answer is classified: a 4xx other than 408 and 429 is a **refusal** that no retry can fix, and a transport error or 5xx is worth retrying.
- The answer is saved next to the key (`<data-dir>/tunnel-address`). A restart can come up from the saved address while the controller is down, and the controller keeps an owner's address sticky, so the saved value stays valid unless an operator released it.
- Registration repeats every `--register-interval-sec` (30 by default). It doubles as the lease refresh and as the way to learn a changed address.

The controller URL may be plain `http://` only to a loopback host. Sending a bearer token in the clear to another host is refused unless `--allow-insecure-controller` says the network is private by construction. The token comes from a file, the credentials directory or the environment, never from the command line (see [secrets](secrets.md)).

## Following the proxies

An origin has to learn every proxy, including ones that appear later. The agent subscribes to the controller's **proxy peers** registry and reconciles every registered proxy onto its interface, the mirror image of what a proxy does with origins. The same rules apply: the same registration as already applied is skipped, a peer is replaced by removing and re adding it (the userspace backend cannot modify one), and a subscribe connection that drops reconnects with capped backoff from 500 ms to 30 seconds while the interface stays as it was.

One detail is specific to this side. The controller emits no event for an unchanged re registration, so a peer that failed to configure would never be retried on an open stream. The agent therefore closes the stream after a configure failure, 30 seconds later, and the reconnect replays the whole registry.

`--peer-pubkey`, `--peer-endpoint` and `--peer-address` still pin one proxy by hand. This is the bootstrap path, and it suits a deployment too small for the registry. It converges to the same interface state a registered proxy would produce, so the two do not conflict. The pinned proxy is routed as a single host route (`/32` or `/128`), never `0.0.0.0/0`, which would collide with every other proxy's route.

## Changes while it runs

**A new address.** If a registration answer names a different address from the one the interface carries, the agent moves the interface without a restart, keeping the device, key, port and peers, using the same delete first netlink step as the proxy (the old address goes first, because deleting an IPv4 primary address would take the new one with it). The other side drops and re adds its peer for the new address, which discards the session the agent still believes in, and its packets would be ignored until a keepalive timed out, longer than a proxy waits for a backend. So after the change the agent re creates every peer as the device reports it, which resets the session, and kicks the handshake. A peer that cannot be added back is retried once and remembered for repair.

**A rotated tunnel key.** Every interval the agent asks the controller for its preshared key (`GET /peers/{name}/psk`, with its own token). When the answer names a newer generation than the one it holds, it writes the key to its file, switches every peer on the live interface to it, records the generation, and registers again naming the new generation's secret. **That registration is the acknowledgement**: only then do the proxies fetch the new sealed secret and re peer the origin. The two ends differ for the few seconds between the agent's switch and that registration reaching the proxies, and a handshake in the window simply retries. The order is deliberate: the key file, then the device, then the generation file. A crash in between leaves a generation older than the key, and the next poll offers the same one again, which is safe to repeat.

An agent without a key of its own (one set by hand, or one that never enrolled) gets 401 or 404 and stops asking. An agent whose key file is read only, such as a mounted Secret, does not rotate and logs why.

## What was traded away

- **Duplicated code.** The key store, the address store, the netlink helper and the live interface are mirrored from the proxy rather than shared, since the two binaries have no common library and the precedent in the project is to copy small pieces.
- **Short stalls on change.** An address change or a key rotation re peers the origin, so traffic can pause for a few seconds.
- **Two paths to a proxy.** The registry and the manual pin both exist. That is a little redundancy that keeps small and bootstrap setups simple.
- **An origin is only an interface.** The agent never sees the game traffic, so it cannot report on it. Health of the backend is the proxy's job, as for any other backend.

## Where to look

- `crates/wayhouse-agent/src/main.rs`: the flags and the start order.
- `crates/wayhouse-agent/src/keypair.rs` and `address_store.rs`: the persisted identity and address.
- `crates/wayhouse-agent/src/interface.rs`: bring up, MTU and the handshake kick.
- `crates/wayhouse-agent/src/register.rs`: the registration and its retry rules.
- `crates/wayhouse-agent/src/proxy_subscribe.rs`: following the proxies.
- `crates/wayhouse-agent/src/live_interface.rs` and `netlink_addr.rs`: moving the address.
- `crates/wayhouse-agent/src/psk_rotate.rs`: the key rotation.
