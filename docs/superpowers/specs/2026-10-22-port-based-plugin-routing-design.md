# Port-based routing for plugins (UDP games without a hostname)

Status: design note, no code. Refs #403. Builds on the [plugin automation hooks addendum](2026-10-07-plugin-automation-hooks-design.md), [Pelican remainder](2026-10-22-pelican-remainder-design.md) and `docs/03-routing.md` ("Routing without a protocol hint", scheme B). Wueschli decided (2026-10-22) that the first release is hostname-only; this is the design for the rest. Implementation waits for his "implement".

## What exists, what is missing

Core already routes by port. A listener can bind a range (`bind: "host:lo-hi"`, one real socket per port) and a `port` matcher sends each port to a pool. Plugin routes already materialise as an overlay that proxies append to opted-in listeners (`plugin_routes:`, #236). What is missing is only the plugin side:

- The `routes` capability and `routes_set` entries are `{host, backend}`. There is no way to say "entry port".
- `plugin_routes:` accepts `sni` or `sniffer` on TCP listeners only. No port variant, no UDP.
- Nobody owns the mapping "server X is entry port N", so it cannot be kept stable.

## Requirements

1. A Pelican server keeps its entry-node port when it moves between nodes.
2. Two nodes using the same allocation port never collide on the entry node.
3. No per-server firewall or DNS edit: the operator opens one port range once.
4. Additive plugin ABI (no bump, same reasoning as the host:port secret slot).
5. Correct under HA and with tunnel backends.

## Option 1: who allocates the entry port

| | Allocator | Stability | Collisions |
|---|---|---|---|
| **A. Plugin allocates** | Plugin picks ports and keeps the map in its own `state_put` | Good, if the plugin is careful | Plugin cannot see other plugins' ports, so the host must still arbitrate |
| **B. Host allocates (recommended)** | Plugin declares `{key, backend}`; the host assigns a port from the approved range and stores `key -> port` in replicated state | Stable by construction: the key is the server's identity, not its backend | Impossible inside the range; exhaustion is reported |
| C. Derived from the allocation port | `entry = base + node_index * N + port` or identity mapping | Breaks when a node is added, renumbered or has more ports than N | Only avoided by convention |
| D. Operator maps by hand | Per-server entries in the plugin config | Stable | Manual work per server, defeats the point |

**Recommendation: B.** Allocation happens in the replicated apply step, in the same term and revision checked entry as the call's other effects (the existing commit rule), so every replica agrees and a stale leader cannot hand out a port. A move changes `backend` only; the port stays. Plugins stay simple and cannot clash with each other.

Details of B:

- Entry shape: `{key: "<stable id>", backend: "<ip:port>", proto: "udp" | "tcp" | "both", prefer?: <port>}`. For Pelican the key is the server uuid and `prefer` is the allocation port, so the entry port usually equals the number the panel shows. If it is taken, the host picks the lowest free port and reports it. (`prefer` is optional; see decision 3.)
- Keys are namespaced by install (`plugin:<id>`), so one plugin cannot take another's port.
- A released port goes into **quarantine** before reuse, so a player with an old address does not land on a different server. Default 24 h, reuse oldest-first only when the range is otherwise full.
- The table is visible in `GET /plugin-routes` and the UI (key, port, proto, plugin), and the plugin can read its own assignments back through a small host import.

## Option 2: how a plugin declares it

| | Shape | ABI |
|---|---|---|
| **A. New capability `port_routes` + import `port_routes_set` (recommended)** | Approval line shows range and protocols; hostname `routes` untouched | New import, additive: old modules still link, no ABI bump |
| B. Extend `routes` entries with optional port fields | One capability, one import | `RouteEntry` is `deny_unknown_fields`, validation mixes two meanings, approval line becomes ambiguous |

**Recommendation: A.**

```json
"port_routes": { "ports": ["30000-30999"], "proto": ["udp", "tcp"], "backends": ["10.0.0.0/16"], "max_entries": 256 }
```

The host checks every call against the approval: backend inside `backends`, `proto` allowed, `prefer` and assigned ports inside `ports`, duplicate keys refused, at most `max_entries`. Same replace-the-whole-set semantics and commit rule as `routes_set`. A module on a host that predates the import fails to link and is refused at install with a clear message; that is the existing additive-import behaviour, not a version bump.

## Data path

- New opt-in `plugin_routes: { type: port }` on a **port-range listener** (UDP and/or TCP; the operator declares one listener per protocol over the same range). Schema 2, same as the existing opt-in.
- The merge appends, per overlay entry, `{match: {type: port, ports: [N]}, action: {pool: plugin-<hash>}}` after the listener's own routes, so operator routes still win. One-target pools as today.
- An overlay port outside the listener's bind range is skipped and logged (like the existing skipped-pool case). A port in the range with no route is dropped as `no_route`, so opening the range does not expose unclaimed ports to anything.
- Existing UDP behaviour applies unchanged: a session reads its pool once when created, so after a server moves, live sessions finish on the old backend and new sessions use the new one. No new affinity needed because each pool has one backend.

## Firewall and DNS

The operator opens the range once (for example `30000-30999` udp and tcp) and states the same range in the listener `bind:` and in the plugin approval. That is the only firewall edit. One DNS name for the entry node; players connect to `name:port`. Adding, moving or deleting servers touches neither. Range exhaustion is surfaced in the plugin status and UI, not silently dropped.

Known gap: the Pelican panel shows the node's allocation port, not the entry port. For the first version players learn the port from the wayhouse UI or admin API. Whether the plugin can write it back into the panel is unverified and not part of this design.

## HA and tunnel

- **HA.** The plugin runs on the leader only; allocation is part of the replicated commit, so followers and a new leader see the same table. The new entry type is gated for rolling upgrades like any other (the #221 gating rule). Every instance applies the same overlay, so a cluster behind a VIP or several entry nodes agrees on ports.
- **Tunnel.** Backends are checked against the approved networks exactly as for hostname routes, and dialled by the pool as today. Nothing here changes tunnel addressing. UDP through a tunnel peer follows whatever the pool transport supports now; the first slice's test confirms it rather than this note asserting it.
- **Security.** The approved range and backend networks are the hard wall; a plugin cannot claim ports or targets outside them. The approval screen shows range and protocols plainly.

## Slices (after "implement")

1. (#419) Config and core: `plugin_routes: { type: port }` on port-range listeners, merge, validation, docs in `03-routing.md`/`05-configuration.md`.
2. (#420) Plugin host and ABI crate: `port_routes` capability, `port_routes_set` and read-back import, validation, conformance tests.
3. (#421) Controller: replicated `key -> port` table with quarantine, commit in the existing entry, overlay with ports, admin API/UI, HA gating.
4. (#422) HA/fleet test: failover keeps ports, move keeps port, exhaustion reported (tunnel UDP check included).
5. (wayhouse-proxy/plugins#20) Pelican plugin (plugins repo): publish port routes, operator guide update.

Slices 1 and 2 are independent; 3 needs both; 4 and 5 follow 3.

## Decisions for Wueschli

1. Allocator: host-allocated (B, recommended) or plugin-allocated.
2. Declaration: new `port_routes` capability (recommended) or extend `routes`.
3. Keep the optional `prefer` hint, so Pelican servers usually keep their allocation port (recommended), or always use the lowest free port.
4. Quarantine for released ports: 24 h (recommended), shorter, or none.
5. Is "players read the port in the wayhouse UI" acceptable for the first version, or must the port show up in the Pelican panel?
