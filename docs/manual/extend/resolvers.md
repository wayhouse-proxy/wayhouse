---
title: "Resolvers: every option"
description: Ask your own service which pool or backend a player should get.
sidebar:
    order: 0
---

A **resolver** is an external service the proxy asks, per connection or UDP session, where to
send a player. Use one when the decision depends on something the proxy cannot know, such as
a matchmaker that assigned a player to an instance. A route uses it with
`action: { resolver: <name> }`.

## The request and the answer

The proxy sends the resolver this information (over HTTP as JSON, or over gRPC):

```json
{
    "listener": "public-udp",
    "src": "203.0.113.7:51343",
    "dst": "198.51.100.10:7777",
    "sni": "eu.example.com",
    "first_bytes_b64": "AAECaGVsbG8=",
    "routing_key": "eu.example.com"
}
```

and expects either a pool, which then load-balances as usual, or a fixed target:

```json
{ "pool": "match-eu-1", "target": null, "ttl_sec": 30 }
```

`ttl_sec` in the answer overrides the configured positive TTL for that result.

## Where to set it

Config file: `resolvers:`. Admin UI: Config, then Resolvers (`/config/resolvers`). A route
refers to a resolver by name.

## Options

| Key                         | Default | Meaning                                                                                                                                                             |
| --------------------------- | ------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `name`                      |         | Required, unique.                                                                                                                                                   |
| `type`                      | `http`  | `http`, or `grpc` (with an endpoint like `http://host:port`).                                                                                                       |
| `endpoint`                  |         | The URL of the service.                                                                                                                                             |
| `timeout_ms`                | `40`    | Give up on a call after this long.                                                                                                                                  |
| `on_error`                  |         | What to do on failure or timeout: `reject` (drop), `fallback_route` (continue with the next rule) or `stale_ok` (serve a cached answer if there is one, else drop). |
| `enabled`                   | `true`  | `false` parks it (needs `schema_version: 4`).                                                                                                                       |
| `proxy_protocol`            | `none`  | Header for a `target` answer, which has no pool to take the setting from. A resolver-chosen pool uses its own `proxy_protocol`.                                     |
| `target_connect_timeout_ms` | `300`   | Connect timeout for a `target` answer.                                                                                                                              |
| `target_idle_timeout_sec`   | `90`    | Idle timeout for a `target` answer.                                                                                                                                 |

### Cache

An optional `cache:` block remembers answers so the resolver is not asked for every
connection.

| Key                      | Default | Meaning                                                                                                                                               |
| ------------------------ | ------- | ----------------------------------------------------------------------------------------------------------------------------------------------------- |
| `cache.key`              |         | A list of parts joined into the cache key: `src_ip`, `src_ip_port`, `sni`, `routing_key` or `first_bytes:<a>:<b>` (bytes a to b of the first packet). |
| `cache.positive_ttl_sec` | `30`    | How long an answer is kept.                                                                                                                           |
| `cache.negative_ttl_sec` | `2`     | How long a failed lookup is kept.                                                                                                                     |
| `cache.max_entries`      | `10000` | Cache size.                                                                                                                                           |

Choose the key so that it identifies what the resolver decides on. A key that includes a
session token from the first bytes, such as `first_bytes:0:16`, gives each token its own entry.

## Related: route hints

Where a resolver pulls, a launcher can push: `POST /route-hint {src_ip, pool, ttl_sec}` stores a
short-lived `src_ip → pool` hint that listeners with `route_hint: true` consult first. Players
behind one NAT address cannot be told apart by it.

## Metrics

`wayhouse_resolver_requests_total`, `wayhouse_resolver_cache_total` and
`wayhouse_route_hints_applied_total`.
