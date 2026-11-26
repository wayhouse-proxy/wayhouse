---
title: "Regional health: a SWIM gossip fabric"
description: How proxies in one failure domain share what they see of each backend, so one proxy's bad network path cannot take a server out of rotation alone, and a dead server is noticed by everyone at once.
---

Each proxy checks its backends on its own. That is simple and it is not always right. A proxy with a flaky network path to one game server sees it as down while a hundred other proxies in the same rack row see it up. In the opposite case, a server dies and every proxy has to find out separately, each after its own failure streak. The **regional health fabric** lets the proxies of one failure domain compare notes. The `settings.gossip` options are listed in the generated [Reference](../../reference/index.md).

## The problem

1. **One viewpoint is a weak witness.** A single proxy's probe failure can be a local problem: its NIC, its route, its own overload.
2. **The fleet is slow to learn together.** Each proxy needs its own `fall` failures before it stops sending players to a dead server.
3. **A shared verdict must not become a single point of failure.** A control plane that decides health for everyone would take the whole region with it when it breaks.
4. **Gossip is an attack surface.** The mesh speaks UDP to the network. Forged members and forged votes must not be able to take backends out of rotation.
5. **It must be optional and rebuildable.** A proxy without a mesh, or with an empty one, must behave exactly like a proxy that never had one.

## The shape: votes, not verdicts

The fabric does not decide anything. Every proxy publishes **its own opinion** of each backend, and every proxy reads the combined votes. The combination rule is deliberately small: a backend is **down for the domain** when at least `quorum_fraction` of the instances that have voted on it say it is down. With no votes at all the answer is "not down". An empty view means "fall back to local only behaviour", never "assume down".

The authority model is one sided on purpose. The domain verdict can **only push a backend down**. It sets a `domain_down` flag next to the local `healthy` flag, and a backend counts as healthy only when its own checks say so **and** the domain does not override it. It never revives a backend. Only this proxy's own `rise` streak can clear the local flag. So the fabric adds a way to learn about failures sooner and from more angles, and cannot talk a proxy into sending players to a server its own probes see as dead.

The health sweep does both jobs. After each active check it publishes the result into the mesh, and every sweep it refreshes each backend's `domain_down` from the current quorum verdict. The `gossip_verdicts` [runtime flag](runtime-feature-flags.md) switches the reading side off: the verdict is ignored and a standing override is cleared, while the mesh itself keeps running and established connections are untouched.

## Membership: SWIM, from a library

Membership uses **SWIM** (scalable, weakly consistent, infection style process group membership) through the `foca` crate: members probe each other, ask peers to probe indirectly, spread news of joins and failures by piggybacking it on the ordinary messages, and mark silent members as suspect before declaring them down. The proxy uses `foca`'s LAN preset, which assumes sub-millisecond round trips. That is correct for a single failure domain such as one availability zone, a rack row, and not meant to span a wide area link. The mesh is control plane only: it runs as one task, never on the hot path, and a proxy's identity in the mesh is its configured `bind` address. It announces itself to the configured seeds at start.

Using a library means a protocol that has been thought about by people who do nothing else. The project owns the framing around it and the health data on top.

## Health as a last writer wins register

The votes ride on `foca`'s own broadcast channel. Each vote is a small **register**: which backend, up or down, which instance said so (the `origin`) and a version, `changed_at`. A newer version from the same origin about the same backend **replaces** the older one, so a register never has a history, only a latest value per origin and backend.

The version is wall clock milliseconds, made strictly increasing per process, so it keeps growing across a restart of the origin. An earlier design used an uptime counter that restarted at zero, and a restarted proxy's votes were ignored until it passed its old value. Versions are only ever compared for one origin, never across origins, so clocks do not have to agree between proxies for this to work.

The rules for accepting a register are where most of the engineering is:

- **Bounded memory.** At most 256 instances may hold a register about one backend, and at most 8,192 backends are tracked. A flood of forged registers cannot grow the table without bound.
- **No registers from the future.** A version more than 60 seconds ahead of our clock is dropped. Otherwise a forged value near the largest integer would pin its origin's register forever, because only a newer version replaces one.
- **Only members vote.** A register whose origin is not in the current member set is dropped. `foca` keeps a broadcast for a while after its origin left and may deliver it again, and a departed instance's old verdict would vote once it rejoined. When a member leaves, its registers go with it.
- **Registers expire.** Each live origin refreshes its votes on every health probe, so a register not heard for 10 minutes is ignored. An expired register can be replaced by a **lower** version when the origin itself sent it, so a clock that stepped forward and was corrected, or a restart under a lower clock, cannot pin a stale vote above the origin's new ones. A relayed copy still has to be newer, otherwise an old verdict a peer kept would come back to life after expiry and be rebroadcast.
- **Rate limited complaints.** Drops are counted by reason and warned about at most once a minute per origin, with the table of warned origins itself capped.

## Authentication and replay

Every datagram is framed and signed. The frame is an 8 byte sender timestamp, a magic byte, a version byte, the payload and a 32 byte **HMAC-SHA256** tag computed with the domain's pre-shared key. The receiver checks in this order, cheapest and most decisive first: length, the tag (a bad or missing tag is dropped **silently** and counted, never answered), the timestamp, then the magic byte and version. A timestamp more than 30 seconds from the receiver's clock is dropped as stale. This is also the clock skew the mesh tolerates between instances. A replay inside that window is still possible, and the source says so.

The magic byte and version byte exist so the wire format can change. A frame from before versioning also passes the HMAC, and the magic byte stops it from being mistaken for a current one. A datagram with another version is dropped **before its payload is decoded**. There is no overlap window for gossip: during a rolling upgrade, nodes on different versions simply do not exchange health, and each keeps relying on its own checks. The first mismatch logs a warning, later ones only count.

### Rotating the key

The key can be changed without a restart. During the overlap after a rotation, datagrams under the old key are still sent, and they are accepted for twice that long, because a replica that reloads late still signs with the old key after an early one switched. Replicas that reload at different times keep talking. A datagram is tried against every key the mesh currently accepts, and counted as an authentication failure only if none matches.

## What was traded away

- **A shared secret, not identities.** One key per domain. Anyone with it can vote, which is why the registers are bounded and only members count.
- **Replay inside 30 seconds.** Documented, not solved. A replayed vote is a vote the origin already cast.
- **Wall clocks.** Versions and the freshness window use wall time. The skew limits are what keep that safe.
- **No cross domain gossip.** The preset assumes one low latency domain. Fleet wide health is the aggregator's job, not the mesh's.
- **Rejoining is not fast.** The identity renewal that would let a declared down instance rejoin quickly is left at its default, a simplification the source flags for revisiting.
- **Pushing down only.** A domain that is wrong about a backend being down is corrected only by the local `rise` streak on each proxy, which is the cost of never letting a vote revive anything.

## Alternatives that were not taken

- **Local checks only.** This is still exactly what a proxy does without `settings.gossip`: the fabric is additive to the local `rise` and `fall` logic and never replaces it.

## Where to look

- `crates/wayhouse-core/src/gossip.rs`: the mesh task, framing, the register merge and `quorum_down`.
- `crates/wayhouse-core/src/health.rs`: where each sweep publishes and reads the verdict.
- `crates/wayhouse-core/src/pool.rs`: `domain_down` and `observe_domain`.
- [Health checking](health-checking.md): the local checks the votes sit on top of.
