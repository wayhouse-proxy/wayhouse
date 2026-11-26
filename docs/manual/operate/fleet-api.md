---
title: Operate the whole fleet
description: The /api/fleet endpoints that read every proxy at once and apply a drain, a backend change or a route hint to all of them.
sidebar:
    order: 5
---

With many proxies you do not want to call each admin API in turn. A `wayhouse-aggregator` collects what every proxy pushes to it and fans commands back out. The admin UI's Fleet pages sit on top of it through the `/api/fleet` endpoints. The UI forwards every call to the aggregator with its own token, so the browser session never becomes a bearer token.

Reads need the **viewer** role. The verbs that change something need the **operator** role. The UI records the session's user name with each verb as `X-Actor`.

If the UI has no `--aggregator-url`, every `/api/fleet` call answers `503`. If the aggregator is configured but cannot be reached, it answers `502`.

## Read the fleet

| Endpoint                                       | Returns                                                                                 |
| ---------------------------------------------- | --------------------------------------------------------------------------------------- |
| `GET /api/fleet/pools`                         | Every pool across the fleet with its backends.                                          |
| `GET /api/fleet/sessions`                      | The live sessions across the fleet.                                                     |
| `GET /api/fleet/healthz`                       | The health summary of the fleet.                                                        |
| `GET /api/fleet/stats?range=<span>`            | Charts data. `range` is a span of time and the aggregator validates it.                 |
| `GET /api/fleet/features`                      | For every runtime feature flag, how many nodes have it on, off or do not have the code. |
| `GET /api/fleet/instances/{instance}/sniffers` | The sniffer modules one instance runs.                                                  |

Statistics are kept in memory only. A restart of the aggregator starts the charts empty.

## Act on the fleet

The verbs on a pool, a backend, a route hint or a sniffer module are sent to every known instance and answer `{"results": [...]}` with one entry per instance. Drain and undrain name one instance and pass that instance's own answer back unchanged. An instance that cannot be reached has `status: null`. One that refuses carries `detail`, the first 200 bytes of its answer. A broadcast never fails or stalls because of one bad instance. Every route, with its parameters and answers, is in the [Aggregator API reference](../../reference/index.md).

| Endpoint                                         | Body                                       | What it does                                                                                |
| ------------------------------------------------ | ------------------------------------------ | ------------------------------------------------------------------------------------------- |
| `POST /api/fleet/instances/{instance}/drain`     |                                            | Takes one instance out of rotation (its `/readyz` flips). `404` if the instance is unknown. |
| `POST /api/fleet/instances/{instance}/undrain`   |                                            | Puts it back.                                                                               |
| `POST /api/fleet/pools/{pool}/backends`          | `{"addr": "10.0.1.13:27015"}`              | Adds a backend to a pool on every instance.                                                 |
| `PATCH /api/fleet/pools/{pool}/backends/{addr}`  | `{"state": "enabled\|draining\|disabled"}` | Sets the operator state of a backend everywhere.                                            |
| `DELETE /api/fleet/pools/{pool}/backends/{addr}` |                                            | Removes the backend from the pool on every instance.                                        |
| `POST /api/fleet/route-hint`                     | `{"src_ip", "pool", "ttl_sec"}`            | Pushes a route hint to every instance. Only listeners with `route_hint: true` apply it.     |

Against the aggregator directly the same calls are `POST /fleet/instances/{instance}/drain` and `POST /fleet/instances/{instance}/undrain`.

The per-instance versions of these calls are the admin API verbs described in [Drain and maintain](drain-and-maintain.md).

## Sniffer modules

| Endpoint                                   | What it does                                                                           |
| ------------------------------------------ | -------------------------------------------------------------------------------------- |
| `POST /api/fleet/sniffers?name=<name>`     | Uploads a `.wasm` sniffer module as the raw request body (`application/octet-stream`). |
| `DELETE /api/fleet/sniffers/{name}`        | Removes the module.                                                                    |
| `POST /api/fleet/sniffers/{name}/rollback` | Goes back to the previous version of the module.                                       |

See [Sniffers](../../sniffers.md) for how a module is written and loaded.

## Related

- [Monitor a proxy](monitor.md) for what one proxy reports.
- [Operations and observability](../../06-operations-observability.md) for the aggregator's full contract.
