---
title: Keep the client IP
description: Let backends see the real player address.
sidebar:
    order: 5
---

By default the backend sees the proxy's address. If the game needs the real player IP, for
bans, anti-cheat or logs, there are three options. Pick one per pool.

## 1. PROXY protocol (recommended)

The proxy prepends a small header that carries the real source and destination address.

```yaml
pools:
    - name: lobby
      targets: ["10.0.1.11:7777"]
      proxy_protocol: v2
```

| Value    | Use for                                                                     |
| -------- | --------------------------------------------------------------------------- |
| `none`   | The default: no header.                                                     |
| `v1`     | TCP, text header.                                                           |
| `v2`     | TCP, binary header.                                                         |
| `v2-udp` | UDP: the binary header goes on the **first datagram** of each session only. |

The form must match the transport of the listeners that route to the pool. The backend has
to parse the header, so check that your game server or its framework supports PROXY protocol.

> [!WARNING]
> The backend must accept PROXY headers **only from the proxy's address**. Otherwise any
> client could send its own header and spoof its IP.

## 2. Transparent mode (Linux TPROXY)

The proxy opens each backend connection with the player's real `ip:port` as the source, so
the backend needs no protocol support at all.

```yaml
listeners:
    - name: transparent-tcp
      bind: "0.0.0.0:7778"
      protocol: tcp
      transparent: true
      pool: lobby
```

It works for TCP and UDP, IPv4 and IPv6, and is mutually exclusive with `prefix`. It costs more
network work:

- The proxy needs `CAP_NET_ADMIN` (not full root).
- The kernel must redirect inbound game traffic to the proxy with a TPROXY rule, and policy
  routing must deliver marked packets locally.
- The backend's **return traffic must come back through the proxy host**: make the proxy the
  backend's default gateway, or route the client prefixes back through it. Without that the
  client receives replies from the backend's own IP and the connection stalls.

The [transport design chapter](../04-transport-and-client-ip.md) has a complete nftables and
`ip rule` example.

## 3. No preservation

Fine when the game gets the client address elsewhere, for example inside its login token.
Nothing to configure.

## Which one?

Use PROXY protocol when the backend can read it, because it needs no special network layout.
Use transparent mode when you cannot change the backend and you control the routing between
proxy and backend.
