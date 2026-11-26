---
title: "Backend sources: every option"
description: Static lists, DNS SRV, Consul, Kubernetes and tunnel origins.
sidebar:
    order: 2
---

Instead of listing a pool's backends by hand, a pool can name a **backend source** that
supplies them. Define sources under `backend_sources:` and refer to one from a pool with
`source:`.

## Where to set it

Config file: `backend_sources:`. Admin UI: Config, then Sources (`/config/sources`). Backend
sources are read at startup, so changing the list needs a restart; the backends a source
returns refresh by themselves.

## How a refresh behaves

The runtime asks each dynamic source for its **current** address set on a timer and compares
it with what the pool has. A refresh that fails or returns nothing keeps the last known set,
so a broken discovery service never empties a pool. It is counted in
`wayhouse_discovery_refresh_total{result="error"|"empty"}`. Each dynamic source has one
control-plane task; discovery is never on the data path.

## Options

| Key                    | Default | Meaning                                                  |
| ---------------------- | ------- | -------------------------------------------------------- |
| `name`                 |         | Required, unique. A pool's `source:` refers to it.       |
| `type`                 |         | `static`, `dns_srv`, `consul`, `kubernetes` or `tunnel`. |
| `enabled`              | `true`  | `false` parks the source (needs `schema_version: 4`).    |
| `refresh_interval_sec` | `15`    | How often a dynamic source is queried.                   |

### `static`

| Key       | Meaning                                                           |
| --------- | ----------------------------------------------------------------- |
| `targets` | The fixed `ip:port` list. Useful to share one list between pools. |

### `dns_srv`

| Key      | Meaning                                                                                                      |
| -------- | ------------------------------------------------------------------------------------------------------------ |
| `record` | The SRV record to resolve, for example `_game._udp.us.internal.example.com`. The port comes from the record. |

### `consul`

| Key                 | Default                 | Meaning                                                                                                                                                                             |
| ------------------- | ----------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `service`           |                         | The Consul service name.                                                                                                                                                            |
| `consul_addr`       | `http://127.0.0.1:8500` | Base URL of the Consul HTTP API.                                                                                                                                                    |
| `tag`               |                         | Only instances carrying this tag.                                                                                                                                                   |
| `consul_token_file` |                         | A file with an ACL token, sent as `X-Consul-Token`. Re-read while running, so a rotated token is picked up. It is a path, so neither the YAML nor `GET /config` carries the secret. |

Only passing instances count. The adapter uses blocking queries for push-like updates and
resyncs periodically.

### `kubernetes`

| Key         | Default            | Meaning                                       |
| ----------- | ------------------ | --------------------------------------------- |
| `service`   |                    | The Service whose EndpointSlices to read.     |
| `namespace` | `default`          | The Service's namespace.                      |
| `port_name` | first port         | Pick this named port from each EndpointSlice. |
| `api`       | in-cluster address | API server base URL.                          |

Slices are watched and resynced. In a pod the service account token and CA are used. The proxy
needs `list` and `watch` on EndpointSlices; the DaemonSet manifests include the RBAC.

### `tunnel`

| Key      | Meaning                                                                                                                                                                                    |
| -------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `pubkey` | The origin's WireGuard public key (base64, 32 bytes). It selects which `wayhouse-agent` origin this source reads backends from, and pins the key the proxy expects that origin to present. |

Needs the proxy to run with the `--tunnel-*` launch parameters, see
[Backend transport](../../11-backend-transport.md).

## Metrics

`wayhouse_discovery_backends` (how many a source currently has) and
`wayhouse_discovery_refresh_total`.
