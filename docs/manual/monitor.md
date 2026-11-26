---
title: Monitor a proxy
description: Health endpoints, Prometheus metrics, live sessions and connection logs.
sidebar:
    order: 7
---

## Health endpoints

| Endpoint        | Tells you                                                                                                                                                                               |
| --------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `GET /healthz`  | The process is alive. Needs no token.                                                                                                                                                   |
| `GET /readyz`   | Config loaded and valid, at least one listener bound, not draining. Returns 503 `draining` after `POST /admin/drain`.                                                                   |
| `GET /pools`    | Every pool with its backends, their health, `state=` and active count.                                                                                                                  |
| `GET /config`   | A plain-text view of the active snapshot: listeners, pools, `draining` and `active_conns`.                                                                                              |
| `GET /sessions` | Every live connection or UDP session: id, transport, listener, `peer=`, `local=`, `pool=`, `backend=` and `age=`. Filter with `?listener=`, `?pool=`, `?proto=tcp\|udp` or `?src=<ip>`. |
| `GET /features` | Every runtime feature flag with its value and where the value comes from.                                                                                                               |
| `GET /metrics`  | Prometheus metrics.                                                                                                                                                                     |

Use `/readyz` for load balancer checks and `/healthz` for process supervisors.

## Metrics

Scrape `GET /metrics`. Start with these:

| Question                                | Metric                                                                         |
| --------------------------------------- | ------------------------------------------------------------------------------ |
| How many players are connected?         | `wayhouse_active_connections`, `wayhouse_active_udp_sessions` (per `listener`) |
| Is traffic flowing?                     | `wayhouse_bytes_total`, `wayhouse_packets_total`                               |
| Are connections getting through?        | `wayhouse_listener_connections_total{result=...}`                              |
| Are backends healthy?                   | `wayhouse_pool_backends{state=...}`, `wayhouse_healthcheck_total`              |
| Are backends refusing us?               | `wayhouse_backend_connect_errors_total`                                        |
| Is a filter blocking players?           | `wayhouse_filter_blocked_total{filter=...}`                                    |
| Are packets being dropped?              | `wayhouse_datagrams_dropped_total{reason=...}`                                 |
| Did the last reload work?               | `wayhouse_config_reload_total`, `wayhouse_config_version`                      |
| Are we running out of file descriptors? | `wayhouse_fd_open` against `wayhouse_fd_limit`, `wayhouse_accept_errors_total` |

The [metrics reference](../reference/index.md) lists every metric with its labels, generated
from the source.

## Connection logs

Each session writes one structured JSON event when it closes, for example:

```json
{
    "event": "session_close",
    "listener": "public-udp",
    "pool": "match-eu",
    "backend": "10.1.0.12:7777",
    "bytes_c2s": 184320,
    "bytes_s2c": 942080,
    "duration_ms": 812345,
    "close_reason": "idle_timeout"
}
```

`close_reason` is one of `client_close`, `backend_close`, `idle_timeout`, `drain`, `limit`,
`backend_down` or `error`. Control the log level with `WAYHOUSE_LOG`.

## Dashboards and alerts

A good first alert set: no healthy backend in a pool, `readyz` failing, `wayhouse_fd_open`
near `wayhouse_fd_limit`, and a sudden rise in `wayhouse_filter_blocked_total`. The
[operations chapter](../06-operations-observability.md) lists dashboards and alert ideas in
detail.
