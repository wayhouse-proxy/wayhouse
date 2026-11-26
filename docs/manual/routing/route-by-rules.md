---
title: Route players by rules
description: Send different players to different pools behind one port.
sidebar:
    order: 1
---

With `pool:` a listener sends everyone to one pool. With `routes:` it picks a pool per
connection. Rules are checked top to bottom and **the first match wins**. `pool` and `routes`
are mutually exclusive on one listener.

```yaml
listeners:
    - name: public-tcp
      bind: "0.0.0.0:7777"
      protocol: tcp
      routes:
          - match: { type: client_cidr, cidrs: ["10.0.0.0/8"] }
            action: { pool: internal }
          - match: { type: port, ports: [7777, "8000-8100"] }
            action: { pool: lobby }
          - match: { type: always }
            action: { pool: lobby }
```

A connection that matches no rule is dropped silently, so end the list with an `always` rule
if every player should land somewhere.

## Matchers

| `type`        | Matches on                                                                                                                                                                       |
| ------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `always`      | Everything. Use it as the last rule.                                                                                                                                             |
| `client_cidr` | The client's source IP, as a list of prefixes.                                                                                                                                   |
| `dst`         | The destination IP the client connected to, as `cidrs:`. Only distinguishes addresses the host serves separately.                                                                |
| `port`        | The destination port: a number or a `"lo-hi"` range.                                                                                                                             |
| `first_bytes` | The first bytes of the connection: a `prefix:` (`hex:...` or `ascii:...`, up to 512 bytes) and/or a `length: { min, max }` window. On TCP, the length is what one peek returned. |
| `sni`         | The hostname in a TLS ClientHello, with `host:` patterns that are exact, `*.suffix` or `.suffix`. TCP only. The proxy only peeks at the hello; TLS ends at the backend.          |
| `sniffer`     | A verdict from a WASM sniffer, see [Sniffers](../../sniffers.md).                                                                                                                |

## Route by hostname (TLS)

```yaml
routes:
    - match: { type: sni, host: ["*.eu.example.com", "lobby.example.com"] }
      action: { pool: eu }
    - match: { type: always }
      action: { pool: lobby }
```

## Route by protocol, on UDP

Valve's A2S queries start with `0xFFFFFFFF`. A `first_bytes` rule recognises them without any
game knowledge in the proxy:

```yaml
- name: query-and-game
  bind: "0.0.0.0:27016"
  protocol: udp
  first_packet_gate: true
  routes:
      - match: { type: first_bytes, prefix: "hex:ffffffff" }
        action: { pool: realtime }
      - match: { type: always }
        action: { pool: realtime }
```

`first_packet_gate: true` opens a UDP session only when the first datagram matches a
`first_bytes` rule or a sniffer verdict. Spoofed floods then never reach the session table;
what it drops is counted in `wayhouse_datagrams_dropped_total{reason="first_packet_gate"}`.
It needs at least one `first_bytes` rule or sniffer, otherwise it would drop everything.

## Let something else decide

- **Sniffers** read the first bytes with sandboxed WASM modules and report a protocol or
  hostname. Official ones cover Minecraft, A2S, QUIC, WireGuard, OpenVPN, RakNet and
  TeamSpeak 3. See [Sniffers](../../sniffers.md).
- **Resolvers** ask your own service which pool to use: `action: { resolver: matchmaker }`,
  over HTTP or gRPC, with a cache and an `on_error` policy (`reject`, `fallback_route` or
  `stale_ok`). Define them under `resolvers:`.
- **Route hints** let a launcher `POST /route-hint {src_ip, pool, ttl_sec}` before the player
  connects, on listeners with `route_hint: true`. The hint wins over the rules for that
  source until it expires.

## Many ports at once

- `bind: "0.0.0.0:28000-28001"` opens one socket per port in the range (at most 1024) with the
  same routes and filters. Add a `port` rule to route per port.
- `prefix:` on a UDP listener lets one wildcard socket serve a whole routed prefix; `dst`
  rules then pick the pool per destination address. Point each sub-domain's DNS at its own
  address in the prefix.

## Related

- [Routing design](../../03-routing.md) for the full semantics.
- [Plugins](../../plugins.md) can add hostname routes to a listener with `plugin_routes`.
