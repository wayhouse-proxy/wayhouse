# 08 – Roadmap

What is built, and what is still ahead. The slice-by-slice history of phases 0–14 (status
lists, per-slice notes, milestone cuts) is in `git log` and `CHANGELOG.md`; the design of
each area is in the chapter named below. Open work is tracked in the
[GitHub issues](https://github.com/wayhouse-proxy/wayhouse/issues), not in this file.

## Built

All fifteen phases are built, verified live and covered by `make check`. "Built" means built
and tested, not battle-tested.

| Phase | What it delivered | Where it is described |
|-------|-------------------|-----------------------|
| 0 | Skeleton: workspace, config loading, immutable snapshot, logging, health and metrics endpoints | [02](02-architecture.md), [05](05-configuration.md) |
| 1–2 | L4 TCP and UDP proxy: `SO_REUSEPORT` listeners, pools, `rr` / `least_conn` balancers, health checks, per-backend caps, `splice(2)` / `recvmmsg` / timing-wheel fast paths | [02](02-architecture.md), [04](04-transport-and-client-ip.md) |
| 3 | Routing: route lists and matchers (`port`, `client-cidr`, `dst`, `first-bytes`, `sni`, `sniffer`), `consistent_hash`, UDP prefix listeners, push resolver, port-range listeners | [03](03-routing.md) |
| 4 | External routing logic: HTTP and gRPC resolvers with a TTL-LRU cache | [03](03-routing.md) |
| 5 | Operations: hot reload, graceful drain, admin API, runtime backend add/remove | [06](06-operations-observability.md) |
| 6 | Client-IP preservation: PROXY protocol v1/v2, transparent mode (TPROXY) | [04](04-transport-and-client-ip.md) |
| 7 | Security: ACLs, GeoIP, rate and connection limits, amplification guard, TLS handshake limits | [07](07-security-ddos.md) |
| 8 | Discovery: DNS SRV, Consul, Kubernetes, tunnel sources | [03](03-routing.md), [12](12-deployment.md) |
| 9 | Sniffer loader: WASM sniffers under `wasmtime`, registry, updates and rollback | [sniffers](sniffers.md) |
| 10–11 | Controller, aggregator and web UI: fleet config distribution, an operational UI | [10](10-distributed-control-plane.md) |
| 12 | Fleet hierarchy, HA (Raft), shared intent, canary rollout, RBAC | [10](10-distributed-control-plane.md) |
| 13 | Regional health fabric (gossip) | [10](10-distributed-control-plane.md) |
| 14 | Backend transport: WireGuard tunnel, `wayhouse-agent`, controller-allocated addresses | [11](11-backend-transport.md) |

Added since the phases: plugins ([plugins](plugins.md)), the web front door through Caddy
([web-proxy](web-proxy.md)), ACME ([acme](acme.md)), the sealed secret store and feature
flags ([migrating](migrating.md)), stored UI data ([ui-data](ui-data.md)), enrollment codes,
the solo bundle and `wayhouse-control` ([12](12-deployment.md)).

## Ahead

Deferred on purpose; each has an issue.

- **Vault references** for secrets (`vault://`, #627).
- **SSO** for the web UI (#618).
- **A steering plugin** on top of [global steering](global-steering.md) (#796).
- **Real-world verification:** hardware (#58) and a real Kubernetes cluster (#53).
- **UI leftovers:** a confirmation step on config submit, and browser-level end-to-end tests.
- **v2 design ideas** (CGNAT, gossip load signals, `failure_domain` discovery, #65) and
  open design questions (#69).

## Out of scope

- Cross-instance session handover (shared *session* state). The control plane shares
  config and health, never sessions (ADR 4).
- QUIC-connection-ID-aware sniffing and session keying.
- eBPF/XDP pre-filtering of floods before user space.
- TLS/DTLS termination with a plain backend.
