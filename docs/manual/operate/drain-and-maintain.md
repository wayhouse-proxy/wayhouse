---
title: Drain and maintain
description: Restart or replace backends and proxies without kicking players.
sidebar:
    order: 1
---

Players keep their sessions while you work if you take things out of rotation first.
Everything here uses the admin API on `settings.admin.listen`
(`127.0.0.1:9900` by default). If `settings.admin.auth_token` is set, send it as a bearer
token on every request except `GET /healthz`.

## Replace a backend

1. Add the new backend to the pool. It becomes healthy after `rise` successful checks:

    ```sh
    curl -X POST localhost:9900/pools/lobby/backends -H 'content-type: application/json' -d '{"addr":"10.0.1.13:27015"}'
    ```

2. Set the old backend to `draining`. It gets no new sessions, existing ones continue:

    ```sh
    curl -X PATCH localhost:9900/pools/lobby/backends/10.0.1.11:27015 -H 'content-type: application/json' -d '{"state":"draining"}'
    ```

3. Wait until `wayhouse_backend_active_sessions` for that backend is 0, or until you are
   willing to end the rest.
4. Remove it:

    ```sh
    curl -X DELETE localhost:9900/pools/lobby/backends/10.0.1.11:27015
    ```

Backend states are `enabled`, `draining` and `disabled`. `draining` and `disabled` both stop
new sessions immediately, including UDP affinity; `GET /pools` shows the state of every
backend. States and runtime additions are carried across a config reload by address.

## Restart a proxy

1. Take it out of the load balancer's rotation: `POST /admin/drain`. This flips `/readyz` to
   503 `draining` while the data path keeps serving. `POST /admin/undrain` reverts it.
2. Watch `active_conns` in `GET /config` fall, or `GET /sessions` empty out.
3. Stop the process with `SIGTERM` or `SIGINT`. It stops accepting at once, waits for
   in-flight TCP connections and established UDP sessions, and exits when they finish or
   after `settings.shutdown_grace_sec` (default 30 seconds), whichever comes first.

Reloading the config is different from restarting. A reload that replaces a UDP listener
closes the old sockets immediately and the next datagram of each client opens a fresh session
under the new config; TCP connections are not cut.

## Take the whole fleet's view

With several proxies, the controller and aggregator fan these actions out for you; see the
[fleet documentation](../../10-distributed-control-plane.md) and [Upgrading a fleet](../../upgrading.md).

## Related

- [Monitor a proxy](monitor.md) for the metrics and endpoints used above.
- [Operations design](../../06-operations-observability.md) for the full drain semantics and close
  reasons.
