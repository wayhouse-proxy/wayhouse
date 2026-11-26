---
title: PROXY protocol and transparent mode
description: The two ways wayhouse tells a backend who the real player is, why there are two, and what each costs.
---

A proxy changes the source address. The backend sees a connection from the proxy, not from the player, and a game server that bans, rate limits or logs by IP is suddenly blind. wayhouse can hand the real client address to the backend in two different ways. This page explains both, how they work and how to choose. The settings are in the [Manual](../../manual/routing/keep-client-ip.md).

## The problem

1. **The backend wants the client IP.** For bans, logs, anti cheat and region logic.
2. **The backend cannot always be changed.** One game server can parse a header. Another is a closed binary that only looks at the packet's source address.
3. **A fix must not cost the data path.** Whatever carries the address has to work for TCP and UDP and must not add a copy of every byte.

The two answers match the two kinds of backend:

- **Tell it in the stream.** The proxy writes a small header in front of the data. The backend has to understand it. This is the PROXY protocol, and it works through any network.
- **Make the packets look right.** The proxy opens its connection to the backend **from the client's own address**, so the backend sees the real source without understanding anything. This is transparent mode, and it needs network setup.

## PROXY protocol: a header in front of the stream

`proxy_protocol.rs` only ever **writes** headers. Parsing them is the backend's job, which keeps the proxy's side tiny: 194 lines including tests.

The version is set per pool (or per resolver, for a resolver chosen target):

| Mode     | What goes on the wire                                                                                                                           |
| -------- | ----------------------------------------------------------------------------------------------------------------------------------------------- |
| `v1`     | A text line, `PROXY TCP4 src dst sport dport` and CRLF (`TCP6` for IPv6).                                                                       |
| `v2`     | A binary header: a 12 byte signature, a version and command byte, an address family and transport byte, a length, then the addresses and ports. |
| `v2-udp` | The same binary form with the datagram transport byte, for UDP.                                                                                 |
| `none`   | Nothing.                                                                                                                                        |

`src` is the connecting client and `dst` is the address the client connected to **on this proxy**. Both describe one hop. A v2 header with IPv4 addresses is 28 bytes: 16 for the fixed part and 12 for the addresses and ports.

Three details come from real deployments:

- **Mapped addresses are unwrapped.** A dual stack listener reports an IPv4 client as `::ffff:a.b.c.d`. Both ends are converted to plain IPv4 first. Without that, a mixed pair would fall through to the "unknown" form and the backend would receive no address at all.
- **There is a safe fallback.** If the two addresses somehow have different families, v1 sends `PROXY UNKNOWN` and v2 sends the `LOCAL` command with no addresses. The backend then uses the real socket addresses, as the specification asks.
- **Order matters.** On TCP the header is written to the backend **before any client byte**, so the backend's parser sees it first. A failure to write it counts against the backend as passive health evidence. On UDP, only `v2-udp` applies, and the header is prepended to the **first datagram of the session only**. Every later datagram goes out untouched, so the backend learns the client once per session and the data path pays nothing per packet. Configuration validation rejects `v1` and `v2` on a UDP listener.

## Transparent mode: spoofing the source on purpose

With `transparent: true` on a listener, the proxy connects to the backend from the client's address. Linux supports this with the `IP_TRANSPARENT` socket option, which lets a socket bind an address that does not belong to the machine.

For **TCP**, `connect_tcp_from` creates a socket, sets `IP_TRANSPARENT` (or `IPV6_TRANSPARENT`), binds the client's address as the local end and connects. The kernel then keeps that foreign address as the packet source. If the client and backend have different address families, binding is impossible, so it falls back to an ordinary connect and logs a warning at most once a minute, because the mismatch would otherwise repeat on every connection.

For **UDP** the listener has to receive datagrams that were redirected to addresses that are not local (Linux TPROXY). It reads the original destination of each datagram from a control message. The upstream socket is bound to the client's address, and replies are sent from a second `IP_TRANSPARENT` socket bound to the address the client first wrote to, so the client sees answers coming from where it sent its packets. See [UDP sessions](udp-sessions.md).

The price is the network around it. The backend's replies must travel **back through the proxy** rather than straight to the client, so routing, firewall marks and `iptables` or `nftables` rules have to steer them. The process needs `CAP_NET_ADMIN` or `CAP_NET_RAW`, but not full root. Transparent mode is Linux only, and on other platforms `set_ip_transparent` returns an unsupported error.

## Choosing

| Question                               | PROXY protocol                | Transparent mode                     |
| -------------------------------------- | ----------------------------- | ------------------------------------ |
| Must the backend understand something? | Yes, it must parse the header | No                                   |
| Network setup                          | None                          | Routing and firewall rules           |
| Works across routers and clouds        | Yes                           | Only if replies return via the proxy |
| Privileges                             | None                          | `CAP_NET_ADMIN` or `CAP_NET_RAW`     |
| Platforms                              | All                           | Linux                                |

Where the backend can be changed, the PROXY protocol is the easier default. Transparent mode is for backends that cannot be.

## What was traded away

- **Only writing.** The code only encodes headers. Nothing in it parses one, so a load balancer in front of wayhouse cannot pass the true client address through a PROXY header.
- **No header for UDP in the text form.** Only the binary `v2-udp` form is available on a UDP listener, and it goes on the first datagram only.
- **Transparent mode moves the problem into the network.** The proxy code is small. The setup is not.

## Alternatives that were not taken

- **A custom header.** Backends would need new code for wayhouse alone. PROXY protocol is the same convention HAProxy and others use, so many servers already parse it.
- **Rewriting the payload.** It would need to understand every game protocol.

## Where to look

- `crates/wayhouse-core/src/proxy_protocol.rs`: the encoders.
- `crates/wayhouse-core/src/net.rs`: `connect_tcp_from`, `set_ip_transparent` and the transparent UDP binds.
- `crates/wayhouse-core/src/proxy.rs` and `listener_udp.rs`: where the header is written.
- [Transport and client IP](../../04-transport-and-client-ip.md): the design chapter.
- Manual: [Keep the client IP](../../manual/routing/keep-client-ip.md).
