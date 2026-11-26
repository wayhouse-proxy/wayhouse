---
title: "Pools and health checks: every option"
description: Every setting of a pool, its balancer, health check and backend limits.
sidebar:
    order: 1
---

A pool is a named group of backends plus the rules for picking one and keeping it healthy.
[Your first listener and pool](../getting-started/first-listener-and-pool.md) shows the basics;
this page lists every option.

## Where to set it

| Where           | How                                                                                                                                                                                                                        |
| --------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Config file** | `pools:` in the YAML.                                                                                                                                                                                                      |
| **Admin UI**    | Config, then Pools (`/config/pools`) edits the same keys; the Pools screen (`/pools`) shows live health.                                                                                                                   |
| **API**         | `GET /pools` lists every backend with its health, state and active count. `POST`, `PATCH` and `DELETE` on `/pools/{pool}/backends` change backends at runtime, see [Drain and maintain](../operate/drain-and-maintain.md). |

## The options

| Key                        | Default       | Meaning                                                                                                                                         |
| -------------------------- | ------------- | ----------------------------------------------------------------------------------------------------------------------------------------------- |
| `name`                     |               | Required, unique. Listeners and routes refer to it.                                                                                             |
| `targets`                  | `[]`          | Static backends as `ip:port`. Mutually exclusive with `source`.                                                                                 |
| `source`                   |               | The name of a `backend_sources` entry that supplies the backends, see [Backend sources](backend-sources.md). Mutually exclusive with `targets`. |
| `balancer`                 | `round_robin` | `round_robin`, `least_conn`, `consistent_hash` or `weighted`.                                                                                   |
| `hash_on`                  | `src_ip`      | `consistent_hash` only: hash the client IP (`src_ip`) or IP and port (`src_ip_port`).                                                           |
| `weights`                  | `{}`          | `weighted` only: `"ip:port": weight` (1 or more). A backend not listed weighs 1, also for discovered backends.                                  |
| `connect_timeout_ms`       | `300`         | How long to wait for a backend connection.                                                                                                      |
| `idle_timeout_sec`         | `90`          | Drop a connection or UDP session after this long without traffic. A UDP session reads this once, when it opens.                                 |
| `proxy_protocol`           | `none`        | `none`, `v1`, `v2` or `v2-udp`, see [Keep the client IP](../routing/keep-client-ip.md).                                                         |
| `per_backend.max_sessions` | no cap        | Live sessions per backend, shared by TCP connections and UDP sessions.                                                                          |
| `health_check`             |               | See below.                                                                                                                                      |

### Balancers

- `round_robin`: the next healthy backend for each new connection.
- `least_conn`: the healthy backend with the fewest live connections; good for sessions of
  uneven length.
- `weighted`: weighted round robin; use it for mixed hardware or a canary.
- `consistent_hash`: a rendezvous hash of the client address. It needs no sticky table, holds
  at any worker count and after any idle eviction, skips unhealthy or draining backends (their
  clients move to the next-highest score) and remaps about 1/N of the clients when you add a
  backend. UDP games should use it, because otherwise one player's datagrams spread across
  instances.

### Health check

| Key                              | Default       | Meaning                                                                                                                      |
| -------------------------------- | ------------- | ---------------------------------------------------------------------------------------------------------------------------- |
| `health_check.type`              | `tcp_connect` | `tcp_connect`, `udp_probe` or `none`. A pool used only by UDP listeners must set it: the default can never pass against UDP. |
| `health_check.interval_sec`      | `2`           | Time between checks.                                                                                                         |
| `health_check.timeout_ms`        | `500`         | A check that takes longer fails.                                                                                             |
| `health_check.rise`              | `2`           | Consecutive successes to become healthy.                                                                                     |
| `health_check.fall`              | `3`           | Consecutive failures to become unhealthy.                                                                                    |
| `health_check.send_hex`          |               | `udp_probe`: the bytes to send, as hex.                                                                                      |
| `health_check.expect_hex_prefix` | any reply     | `udp_probe`: the reply must start with these bytes.                                                                          |

Besides these active checks, the data path reports connect failures, so a backend that refuses
connections is skipped before the next check. A backend returns to healthy only through
`rise` successful active checks. Active checks run in the control plane and publish the result
as a new snapshot; they never sit on the data path.

## Backend states

On top of health, an operator can set a backend's state: `enabled`, `draining` (no new
sessions, existing ones continue) or `disabled`. States are set with
`PATCH /pools/{pool}/backends/{addr}` and survive config reloads by address.

## Metrics to watch

`wayhouse_pool_backends{state}`, `wayhouse_backend_active_sessions`,
`wayhouse_healthcheck_total`, `wayhouse_backend_connect_errors_total` and
`wayhouse_lb_selections_total`.
