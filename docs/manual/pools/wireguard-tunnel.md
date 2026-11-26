---
title: Reach origins over WireGuard
description: Connect a proxy to origins that have no public address, with the proxy's --tunnel flags and the origin agent.
sidebar:
    order: 3
---

Some origins sit behind a NAT or a firewall and cannot take inbound connections. The WireGuard tunnel turns that around: each origin runs a small **agent** that dials out to the proxy, and the proxy forwards to the origin through the tunnel. Only the proxy side needs a stable public address.

Three parts work together:

- a **controller** that keeps the registry of proxies and origins (see [Enroll a node](../operate/enroll-a-node.md) to join nodes to it),
- the **proxy** (`wayhouse`) started with the `--tunnel-*` flags,
- one **agent** (`wayhouse-agent`) on every origin.

## Requirements

Both the kernel WireGuard backend and the userspace one need the `CAP_NET_ADMIN` capability. The userspace backend also needs `/dev/net/tun`. In Docker that means running as root with both granted:

```sh
docker run --user 0 --cap-add=NET_ADMIN --device=/dev/net/tun ...
```

A container without them fails at interface start, before any listener binds, so you notice at once rather than running without the tunnel. A build without the `tunnel` feature refuses `--tunnel-iface` at startup and under `--check`.

## On the proxy

Setting `--tunnel-iface` turns the feature on. `--tunnel-controller-url`, `--tunnel-name` and `--tunnel-endpoint` are then required.

| Flag                             | Default               | What it does                                                                                                                                                 |
| -------------------------------- | --------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `--tunnel-iface`                 | off                   | Name of the local WireGuard interface to bring up.                                                                                                           |
| `--tunnel-controller-url`        |                       | Base URL of the controller whose registry to follow. Independent of `--controller`: you can keep the structural config in a file and still use the registry. |
| `--tunnel-controller-token-file` |                       | File with the bearer token for the controller, if it requires one.                                                                                           |
| `--tunnel-name`                  |                       | This proxy's identity in the registry. Any string that is unique across the fleet.                                                                           |
| `--tunnel-endpoint`              |                       | The public `ip:port` the origins dial. Every origin's agent learns it from the registry.                                                                     |
| `--tunnel-listen-port`           | `51820`               | UDP port the interface listens on.                                                                                                                           |
| `--tunnel-address`               | allocated             | The proxy's own tunnel address as `ip/prefix` (for example `10.60.0.1/24`). Omit it and the controller allocates one.                                        |
| `--tunnel-key-file`              | `wayhouse-tunnel.key` | The proxy's persisted WireGuard private key. Created when missing.                                                                                           |
| `--tunnel-register-interval-sec` | `30`                  | How often the proxy registers itself again.                                                                                                                  |
| `--tunnel-userspace`             | off                   | Use the userspace backend and skip the kernel one, for hosts with no `CAP_NET_ADMIN` or no `wireguard` module.                                               |

The controller token can also come from `$CREDENTIALS_DIRECTORY/tunnel-controller-token` or the environment variable `WAYHOUSE_TUNNEL_CONTROLLER_TOKEN`, but never from the command line. The proxy refuses to send a token over plain `http://` to a host that is not loopback unless you pass `--allow-insecure-controller`.

## On each origin

```sh
wayhouse-agent \
  --controller-url https://controller.example:9900 \
  --name origin-1 \
  --backends :25565
```

| Flag                          | Default                 | What it does                                                                                                                                                                  |
| ----------------------------- | ----------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `--controller-url`            |                         | Base URL of the controller's registry.                                                                                                                                        |
| `--controller-token-file`     |                         | Bearer token for the controller, when it has an auth token set.                                                                                                               |
| `--name`                      |                         | This origin's identity. A `tunnel` backend source finds the origin by it.                                                                                                     |
| `--backends`                  |                         | The backends this origin fronts, as `host:port` or `:port` ("my tunnel address plus this port"). Every host must be the origin's own tunnel address.                          |
| `--data-dir`                  | `wayhouse-agent-data`   | Holds the agent's persisted WireGuard key, its identity, which is never regenerated once it exists.                                                                           |
| `--tunnel-psk-file`           | `<data-dir>/tunnel-psk` | The preshared key (base64, 32 bytes). Used when that file exists; enrollment writes it. The controller side holds the same key.                                               |
| `--iface`                     | `wayhouse-agent0`       | Name of the local WireGuard interface.                                                                                                                                        |
| `--listen-port`               | `51820`                 | UDP port the interface listens on.                                                                                                                                            |
| `--address`                   | allocated               | Pin the interface's tunnel address as `ip/prefix`. Without it the controller allocates one from its `--tunnel-network` and the agent saves it to `<data-dir>/tunnel-address`. |
| `--endpoint`                  | none                    | The origin's last known public endpoint, advertised for information only. WireGuard roaming and keepalive take over after the first handshake.                                |
| `--register-interval-sec`     | `30`                    | How often the agent registers again.                                                                                                                                          |
| `--userspace`                 | off                     | Use the userspace backend only.                                                                                                                                               |
| `--peer-pubkey`               |                         | Pin one extra edge proxy by its WireGuard public key. Needs `--peer-endpoint` and `--peer-address`.                                                                           |
| `--peer-endpoint`             |                         | The pinned proxy's public `ip:port`.                                                                                                                                          |
| `--peer-address`              |                         | The pinned proxy's tunnel address (one IPv4 or IPv6 address, routed as a `/32` or `/128`).                                                                                    |
| `--ca-file`                   |                         | PEM file with extra CA certificates to trust for outbound HTTPS, on top of the built-in roots.                                                                                |
| `--allow-insecure-controller` | off                     | Send tokens over plain `http://` to a non-loopback host. Only for a network that is private by construction.                                                                  |

## Use the origins as backends

In the pool, add a `tunnel` backend source that names the origin. See [Backend sources](backend-sources.md) for the exact fields.

## Related

- [Backend transport](../../11-backend-transport.md) explains how the tunnel works.
- [Deployment](../../12-deployment.md) has the container and Kubernetes details.
