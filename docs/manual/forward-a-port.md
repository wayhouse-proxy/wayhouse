---
title: Forward a game port to backends
description: Create a listener, a pool of backends and health checks.
sidebar:
    order: 2
---

A **listener** is a port the proxy accepts players on. A **pool** is a group of backend
servers. The smallest useful config is one of each; the listener names the pool it sends
players to.

```yaml
pools:
    - name: lobby
      targets: ["10.0.1.11:27015", "10.0.1.12:27015"]
      balancer: round_robin
      connect_timeout_ms: 300
      idle_timeout_sec: 90
      health_check:
          type: tcp_connect
          interval_sec: 2
          timeout_ms: 500
          rise: 2
          fall: 3

listeners:
    - name: public-tcp
      bind: "0.0.0.0:7777"
      protocol: tcp
      pool: lobby
```

Run `wayhouse --config <file> --check`, then reload (`kill -HUP <pid>`). Players who connect
to port 7777 are forwarded to one of the two backends. The proxy does not look inside the
traffic, so this works for any game that speaks TCP or UDP.

## Where backends come from

A pool takes its backends from one of two places, never both:

- `targets:` is a static list of `ip:port`.
- `source:` names an entry of `backend_sources:`, which hands the pool its current backend
  set: a fixed list, a DNS SRV record, a Consul service, Kubernetes EndpointSlices, or a
  `wayhouse-agent` behind a tunnel. The proxy refreshes a dynamic source in the background; if a
  refresh fails or comes back empty, the last known set is kept so a flaky discovery service
  never empties a pool.

You can add or remove backends of a running proxy without touching the file, see
[Drain and maintain](drain-and-maintain.md).

## Pick a balancer

| `balancer`        | Behaviour                                                                                                                                                                          |
| ----------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `round_robin`     | Each new connection goes to the next healthy backend.                                                                                                                              |
| `least_conn`      | The healthy backend with the fewest live connections.                                                                                                                              |
| `consistent_hash` | A client always lands on the same backend, chosen by a rendezvous hash of its address (`hash_on: src_ip`, the default, or `src_ip_port`). Session affinity without a sticky table. |
| `weighted`        | Like round robin, but `weights:` gives each backend a relative share (1 or more; unlisted backends weigh 1).                                                                       |

For UDP games where a player must keep reaching the same server, use `consistent_hash`.

## Health checks

The proxy checks every backend from the control plane, off the data path:

- `tcp_connect` opens a TCP connection.
- `udp_probe` sends the bytes in `send_hex` and expects a reply that starts with
  `expect_hex_prefix`.
- `none` turns active checks off and relies on passive feedback only.

`rise` consecutive successes mark a backend healthy, `fall` consecutive failures mark it
unhealthy. On top of that, the data path reports connect failures, so a backend that refuses
connections is skipped faster than the next check would notice. An unhealthy backend gets no
new sessions; sessions already open are left alone.

> [!NOTE]
> A pool that only UDP listeners use must set `health_check.type` explicitly: the default
> `tcp_connect` can never succeed against a UDP-only server. Don't point a `udp_probe` at the
> proxy's own listener port, the probe would loop back through the proxy as a fake client.

## Limits per backend

`per_backend.max_sessions` caps the number of live sessions on one backend. TCP connections
and UDP sessions share the cap. Omit it for no cap.

## Related

- Every pool and listener key: [Config keys](../reference/index.md) and
  [Configuration](../05-configuration.md).
- Why the proxy is built this way: [Architecture](../02-architecture.md).
