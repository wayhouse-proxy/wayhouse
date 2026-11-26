---
title: "The tunnel registry: leases, addresses and origins"
description: How the controller hands out tunnel addresses, keeps two registries of origins and proxies as latest-write-wins state with a change log, expires owners that went quiet, and gives each enrolled origin a token and a rotating key of its own.
---

The tunnel needs a source of truth. Every proxy must learn which origins exist, every origin must learn which proxies exist, and nobody should pick an address by hand. That source of truth is a part of the controller: two **registries** and an **address book**. This page covers how they are built. The two ends that use them are [the tunnel client](tunnel-client.md) and [the agent](tunnel-agent.md). The controller flags are in the [Manual](../../manual/index.md) and the generated [Reference](../../reference/index.md).

## The problem

1. **No hand picked addresses.** Two peers with the same tunnel address would silently steal each other's traffic, so the controller must make that impossible.
2. **Many readers, tiny state.** The data is small (a few hundred bytes per registration) but every proxy and every origin wants the whole set and every change to it.
3. **Missing events must not corrupt anything.** A subscriber that reconnects after an outage has to catch up exactly.
4. **Dead owners must not hold addresses forever,** and a controller that was down must not conclude everyone is dead when it comes back.
5. **Keys and tokens are sensitive.** A compromised origin must not be able to impersonate another, nor read its key.
6. **It must work under high availability.** With several controller nodes, an address is allocated once, cluster wide.

## Two registries, one address book

There are two registries with the same shape: `/peers` for origins and `/proxy-peers` for proxies. They share one core and differ only in their registration type, their role and their route prefix. Each is **per name, latest write wins** state. A registration replaces the previous one under that name, which is what makes re registering every 30 seconds harmless.

On disk each registry has two parts in one database: an append only **log** of registrations and removals, which is what a subscriber catches up from, and a small `current` map from name to the latest revision number, so that `GET /peers/{name}` does not scan the log. A log entry and its `current` move are written in **one transaction**, and under high availability the applied Raft index joins it, so replaying an apply step is a no op. The log is compacted every 1,024 writes, so it holds roughly one entry per name plus the slack since the last compaction. A subscriber stream is fed by a broadcast channel with a capacity of 64 updates, and a new subscriber starts by catching up from the log.

An unchanged re registration is detected by normalising the incoming registration the way storing would (the address filled in, `:port` backends expanded, every address in canonical form) and comparing the whole struct. It then writes nothing and emits no event. Refreshing every 30 seconds therefore costs the stream nothing. Under high availability the only periodic write for a live owner is a refresh of `last_seen`, made when it is more than an hour old.

### The names

A registration name is 1 to 64 characters of `A-Za-z0-9._-`, and `subscribe` is reserved so it cannot collide with the stream route. One WireGuard public key belongs to one name within a registry: a second name presenting a key another holds is refused with 409, because peers are keyed by public key and the two registrations would overwrite each other's routes. Re registering under the same name moves to a new key, and deleting a name frees its key. Keys are checked with the same 32 byte base64 rule as the config (see [the config schema](config-schema.md)).

## The address book

One `AddressBook` serves both registries, because origins and proxies share one tunnel network. **Names are unique per role, and addresses are unique globally.** Every claim or release is one short critical section plus one key value transaction across both trees (`by_owner` and `by_address`), so a crash can never leave the two disagreeing.

- **Allocation.** With `--tunnel-network` the controller allocates the lowest free host address. The network can be IPv6 (`/64` to `/120`, the default in the examples, a ULA that cannot clash with an origin's own LAN) or IPv4 (`/16` to `/30`). One family per controller, one address per peer. The network and all ones addresses are never handed out, and IPv4 written as IPv6 (`::ffff:a.b.c.d`) is refused, since it would give one host two addresses.
- **A cap of 65,534 entries.** Allocation scans the pool, so either family holds at most 65,534 entries, pins included. The capacity the API reports is the smaller of that and the host count.
- **Sticky.** An allocation sticks to the `(role, name)` pair, so a restart gets its address back.
- **Pinning.** A registration may ask for a specific address, which is granted only if it is free, otherwise 409. Without `--tunnel-network` the controller is pin only: it enforces uniqueness and allocates nothing.
- **Backends must be on the registrant's own address.** `:25565` means "my address, port 25565" and is stored expanded.
- **A claim outlives a failed registration.** If the claim succeeds and the rest of the registration is refused (backends not on the claimed address, say), the claim is kept, so the corrected retry gets the same address. The price is documented: a stream of distinct names with bad backends can use up the pool. The endpoints are bearer gated, and `DELETE` and the stale flag are the remedy.

### Changing the network

At startup the controller refuses to run when stored addresses fall outside `--tunnel-network`, naming up to ten of them. The choices are to restore the previous network, or to pass `--tunnel-readdress`, which starts the controller, warns once and gives each such peer a new address at its next registration. A running peer keeps its old address until it restarts, so its traffic is interrupted until then. The flag is refused without a network and together with the high availability flags, and is harmless when nothing is outside the network.

## Releasing and expiring

`DELETE /peers/{name}` and `DELETE /proxy-peers/{name}` free the address and emit a **tombstone**. Subscribers turn it into a WireGuard peer removal, and a pool's `tunnel` source turns it into `Withdrawn`, which clears the pool (see [backend discovery](backend-discovery.md)). Deleting an origin also revokes its token and every generation of its tunnel key.

`GET /tunnel/addresses` lists the table with a `stale` flag (older than `--tunnel-stale-after`, 14 days by default), and a daily log warning names stale entries. The UI shows the same table with a **Release** button for admins.

### Leases

With `--tunnel-lease-ttl` set (off by default), an owner not re registered for that long is released exactly as if an operator had deleted it. The rules that make this safe come from two failure modes.

- **A controller that was down.** After it starts, or after a node becomes the high availability leader, the sweeper waits **an hour** before its first sweep (`STARTUP_GRACE`), so owners get the chance to re register before anyone is judged. Without that, a controller down for longer than the TTL would expire everyone the moment it returned.
- **A `last_seen` that lags.** Under high availability a live owner's `last_seen` is only refreshed once it is older than an hour, so it can look up to an hour old. The TTL must be at least **two hours**, which is twice that lag, and the flag refuses anything shorter than that but non zero.

The sweep runs every tenth of the TTL, clamped between one minute and one hour. For each owner that is too old, it asks the registry to expire it, and **the check runs again where the write lands**, under the registry lock or when the Raft entry applies, so a re registration that commits first wins. Under high availability only the leader sweeps. An owner that comes back after expiry is allocated afresh and may get a different address. Hours to days is a typical TTL, far above the 30 second register interval.

## What an enrolled origin holds

An origin that enrolled with a one time code gets two things of its own.

**A token.** Redeeming the code issues the origin a random bearer token, in place of the shared `--agent-token`. Only its SHA-256 is stored, in the replicated store, so a lookup is one read and no copy of the token exists on the controller. The guard on `POST /peers` holds an origin to its own name, and the shared agent token no longer works for an origin that has a token of its own. So a compromised agent can neither re register another enrolled origin nor read its key. A re enroll replaces the token and the old one stops working at once. Proxies still share one token.

**A key, in generations.** The origin's preshared key is a sealed cluster secret, one per generation (see [secrets](secrets.md)). The registration names the generation the proxies use, so flipping to a new key is a single registration write. `POST /peers/{name}/psk/rotate` seals generation n+1 beside the active n and answers `202`. Nothing changes for the proxies until the agent fetches it, applies it and re registers naming n+1. **That registration is the acknowledgement and the flip.** While the agent has not acknowledged, n stays active, and a second rotate answers `409`. The next rotation removes generations older than the active one. A re enroll starts again at generation 0 and removes the others. Registration bodies are capped at 64 KiB, and a registration holds at most 64 backends.

## High availability

With `--ha-peers`, both registries and the address book are replicated through Raft. A registration or `DELETE` on any node is forwarded by a follower to the leader and allocated once, cluster wide. The leader records its `--tunnel-network` when the cluster is first initialised, and that recorded network applies from then on. A node whose flag differs logs an error naming both and answers registry writes with 503, though an unchanged re registration is still answered 200. Until the network is recorded, writes answer 503 as well ("cluster is initializing its registries"). Reads are served from the local replica either way.

## What was traded away

- **Latest write wins, no history.** The log exists for catch up, not for audit. A superseded registration is compacted away.
- **A scan to allocate.** It is simple and the cap keeps it cheap, at the price of a hard limit of 65,534 peers.
- **A leaked claim.** The kept claim after a failed registration helps honest retries and can be abused by someone who holds the bearer token.
- **Origins share one token unless enrolled.** The shared agent and proxy tokens are coarse. Enrollment narrows the origin side only.
- **Expiry is optional.** Without a TTL a dead origin keeps its address until an operator releases it. The stale flag and the daily warning are the only pressure.
- **One family per network.** Mixed families are an operator's responsibility in pin only mode, since peers of different families cannot reach each other.

## Where to look

- `crates/wayhouse-controller/src/registry.rs` (and `registry/`): the shared registry core, the log, `current` and the stream.
- `crates/wayhouse-controller/src/peers.rs` and `proxy_peers.rs`: the two registries' types and routes.
- `crates/wayhouse-controller/src/addresses.rs` (and `addresses/api.rs`): the address book and `GET /tunnel/addresses`.
- `crates/wayhouse-controller/src/lease.rs`: the sweeper and its limits.
- `crates/wayhouse-controller/src/origin.rs`: origin tokens and key generations.
- `docs/11-backend-transport.md`: the design and the locked decisions.
