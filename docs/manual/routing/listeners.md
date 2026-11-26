---
title: "Listeners: every option"
description: Every setting of a listener, with where to set it in the file, the UI and the API.
sidebar:
    order: 0
---

A listener is a port the proxy accepts players on. This page lists every setting of a
listener, grouped by what it does. [Your first listener and
pool](../getting-started/first-listener-and-pool.md) shows the simplest case.

## Where to set it

| Where              | How                                                                                                                                                                                                                                                           |
| ------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Config file**    | `listeners:` in the YAML. Every key below lives there.                                                                                                                                                                                                        |
| **Admin UI**       | Config, then Listeners and routes (`/config/listeners`). The form is drawn from the config schema, so it offers exactly the keys on this page, with the schema's description as help text.                                                                    |
| **Proxy API**      | `GET /config` shows the active listeners. For a proxy that runs from a file, `PUT /config/file` replaces the file (only when started with `--allow-config-write`, which needs an admin token). `POST /config/validate` checks a document without applying it. |
| **Controller API** | `POST /config` submits the whole YAML. The controller validates it before storing it; `POST /config/rollback/{revision}` re-submits an older one.                                                                                                             |

A reload reconciles listeners **by name**: a listener whose definition changed is stopped and
started again, a new one is added, one that disappeared is removed and drains. Changing a
listener's name therefore counts as removing one and adding another.

## Identity and transport

| Key        | Meaning                                                                                                                                               |
| ---------- | ----------------------------------------------------------------------------------------------------------------------------------------------------- |
| `name`     | Required, unique. It is how a reload tells listeners apart and the `listener` label on metrics and logs.                                              |
| `bind`     | Required. `host:port`, or `host:lo-hi` for a port range. A range opens one real socket per port (at most 1024), all with the same routes and filters. |
| `protocol` | `tcp` or `udp`.                                                                                                                                       |
| `enabled`  | `false` parks the listener: it is still validated but not started. Needs `schema_version: 4`.                                                         |

## Where traffic goes

| Key             | Meaning                                                                                                                                                                                          |
| --------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `pool`          | Shorthand: send everything to this pool. Mutually exclusive with `routes`.                                                                                                                       |
| `routes`        | A priority-ordered list of rules; the first match wins. Each has a `match` and an `action`. See [Route players by rules](route-by-rules.md).                                                     |
| `route_hint`    | `true` lets a `POST /route-hint {src_ip, pool, ttl_sec}` hint override the routes for that source, as long as the hinted pool still exists.                                                      |
| `plugin_routes` | TCP only. Lets enabled controller plugins append hostname routes after this listener's own routes. `type` is `sni` or `sniffer` (with `sniffer:` naming the sniffer). Needs `schema_version: 2`. |

A route's `match` takes a `type` and the keys that type uses:

| `match.type`  | Keys                                                                  |
| ------------- | --------------------------------------------------------------------- |
| `always`      | none                                                                  |
| `client_cidr` | `cidrs`: source prefixes                                              |
| `dst`         | `cidrs`: destination prefixes                                         |
| `port`        | `ports`: numbers or `"lo-hi"` ranges                                  |
| `first_bytes` | `prefix` (`"hex:..."` or `"ascii:..."`) and/or `length: { min, max }` |
| `sni`         | `host`: exact, `*.suffix` or `.suffix`                                |
| `sniffer`     | `sniffer`: the sniffer's name, and `host`: host patterns              |

A route's `action` is exactly one of `pool` (a fixed pool) or `resolver` (ask an external
service, see [Resolvers](../extend/resolvers.md)).

## Protection

All of these run before routing, in this order: CIDR lists, GeoIP, rate limit, per-source
cap. See [Protect a listener](../security/protect-a-listener.md) for what each does.

| Key                                               | Meaning                                                                                                                           |
| ------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------- |
| `allow`, `deny`                                   | CIDR lists on the client address. `deny` wins; a non-empty `allow` is default-deny.                                               |
| `geo.allow`, `geo.deny`                           | Country codes. Needs `settings.geo_db`.                                                                                           |
| `rate_limit.per_ip`, `rate_limit.per_net`         | Each `{ rate, burst }`: new connections or sessions per second. `burst` defaults to `rate`.                                       |
| `per_source.max_per_ip`, `per_source.max_per_net` | Concurrent connections or sessions from one IP or one /24 (v4) or /64 (v6).                                                       |
| `first_packet_gate`                               | UDP only. Open a session only if the first datagram matches a `first_bytes` route or a sniffer hint. Needs at least one of those. |

`geo`, `rate_limit` and `per_source` each also take `enabled: false` to park them.

## Addressing

| Key           | Meaning                                                                                                                                                                                                                               |
| ------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `prefix`      | UDP only. One wildcard socket serves a whole routed prefix; the real destination of each datagram picks the `dst` route and replies leave from the same address. `bind` must be a wildcard. Datagrams outside the prefix are dropped. |
| `freebind`    | TCP only. Lets the listener bind an address that is not (yet) on an interface (`IP_FREEBIND`).                                                                                                                                        |
| `transparent` | Linux only. Bind with `IP_TRANSPARENT` and use the real client address as the source towards the backend. Mutually exclusive with `prefix`. See [Keep the client IP](keep-client-ip.md).                                              |

> [!NOTE]
> The schema's own help text for `transparent` says "TCP only", but UDP listeners support it
> too and the validator accepts it; the transport chapter describes both.

## Process-wide settings that affect listeners

These are under `settings:` and are read once at startup (a restart applies a change):

| Key                                                                                    | Meaning                                                         |
| -------------------------------------------------------------------------------------- | --------------------------------------------------------------- |
| `workers`                                                                              | Accept loops per listener. `0` means one per CPU core.          |
| `shutdown_grace_sec`                                                                   | How long to wait for in-flight connections on shutdown.         |
| `limits.max_connections`, `limits.max_udp_sessions`, `limits.max_new_sessions_per_sec` | Process-wide caps.                                              |
| `geo_db`                                                                               | Path to a MaxMind Country database, needed by any `geo` filter. |

## Metrics to watch

`wayhouse_listener_connections_total`, `wayhouse_active_connections`,
`wayhouse_active_udp_sessions`, `wayhouse_listener_bind_failures_total` and
`wayhouse_accept_errors_total`, all labelled by `listener`. See [Monitor a
proxy](../operate/monitor.md).
