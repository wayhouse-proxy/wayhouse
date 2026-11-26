---
title: "Route matching: CIDR sets, SNI peeking and route hints"
description: How wayhouse picks a pool for a connection from nothing but an address and the first few bytes, and why it peeks instead of reading.
---

One listener, one port, many game servers. The proxy has to decide where each new connection goes before it has exchanged a single full message with the client. This page explains how that decision is made, what the matchers can see, how a TLS hostname is found without terminating TLS, and what a route hint adds. The settings are in the [Manual](../../manual/routing/route-by-rules.md).

## The problem

1. **There is very little to go on.** At accept time the proxy knows the client address, the address and port the client connected to, and nothing else. The first bytes may not even have arrived.
2. **Reading is destructive.** If the proxy consumes the bytes to look at them, the backend never sees them. Copying them to the backend means holding and replaying them, which costs a buffer and a code path.
3. **TLS hides the hostname, but only partly.** A TLS server name sits in plaintext in the ClientHello. It is exactly the key virtual hosting needs, but the ClientHello may be split across several TCP segments and several TLS records.
4. **Sometimes something else knows better.** A launcher or control plane may already know which server a player should join, before the player connects.

## A route is a matcher and an action

A listener has an ordered list of routes. Each route has a **matcher** and an **action** (a pool, or an external resolver). The first route whose matcher matches wins. `Always` is the catch all and normally comes last.

The matchers are all in `matcher.rs`, and all of them are pure functions of a small `MatchContext`: the client address, the local address the client connected to, the first bytes, and the result of a sniffer if one ran.

| Matcher       | Looks at                                                 | Needs bytes?   |
| ------------- | -------------------------------------------------------- | -------------- |
| `always`      | nothing                                                  | no             |
| `client_cidr` | the source IP, against a list of prefixes                | no             |
| `dst_cidr`    | the address the client connected to                      | no             |
| `dst_port`    | the destination port, against a list of ranges           | no             |
| `first_bytes` | a prefix and or a length window of the first bytes       | yes            |
| `sni`         | the TLS server name, exact or `*.suffix`                 | yes (TCP only) |
| `sniffer`     | the verdict of a named WASM sniffer, optionally its host | yes            |

Address matchers cost nothing. That is why they are cheap enough to be used on every connection, and why the proxy skips reading entirely when a listener has no byte matcher: `peek_len` is the largest number any route asks for, and zero means "do not peek".

### CIDR matching

A `Cidr` is an address and a prefix length. Matching compares the leading bits of the two addresses, and a prefix of 0 matches every address of its family. IPv4 mapped IPv6 addresses (`::ffff:a.b.c.d`) are treated as the IPv4 address they carry, on both sides, because a dual stack socket reports an IPv4 client that way. Without that, a rule for `10.0.0.0/8` would silently miss every client of a `[::]` listener.

### Hosts

An `sni` pattern is lowercase and is either exact or a suffix. Both `*.example.com` and `.example.com` become the suffix `.example.com`, which matches a proper subdomain and not the bare domain. A trailing dot on the host (`example.com.`, the root label) is stripped first, since a client or a sniffer may send either form.

## Peeking, not reading

The TCP listener uses `MSG_PEEK`: the bytes are copied out of the socket **without being consumed**. The backend later receives them through the byte pump as if nothing had happened. No buffer is held, nothing is replayed, and a listener that routes on bytes costs the same pump as one that does not.

Peeking has a catch. The first `peek` returns whatever has arrived, which may be a fraction of the ClientHello. The listener therefore peeks again, polling every **5 ms**, until it has what it needs or **250 ms** have passed. After the budget, the connection is routed on whatever was seen, and a route that needs more simply does not match.

For TLS, what counts as "enough" is not a byte count. When the first byte is `0x16` (a TLS handshake record), `client_hello_state` reports whether the whole ClientHello is buffered, needs more bytes or can never be one. The state machine follows the specification: a client may split one handshake message over **several consecutive records**, so it joins the payloads of the leading handshake records until the length announced in the message header is reached. The common case, a hello in a single record, borrows the buffer and copies nothing. Only a split hello is copied.

Anything that does not start with `0x16` keeps the plain single peek, because a `first_bytes` prefix fits in the first segment, and a length window is documented as "what one peek returned". There is a hard cap of 4096 peeked bytes.

`extract_sni` is a small bounds checked reader over the ClientHello. Every read can fail, and a failure means "no SNI", never a panic, because the bytes come from an untrusted client.

UDP has no stream, so the matchers look at the **first datagram** of a session. A length window is exact there, and it is only coarse on TCP.

## Route hints: routing decided elsewhere

A launcher can call `POST /route-hint` with a source IP, a pool name and a time to live before a player connects. A listener with `route_hint: true` checks this table before its route list, and a live hint wins, as long as the pool still exists in the snapshot.

The table is an `ArcSwap` over a small map. Reads are lock free, which matters because every connection looks. Writes happen about once per player session, copy the map, drop expired entries as they go and insert the new one. The lifetime is capped at 24 hours, so an absurd time to live cannot overflow the clock arithmetic. IPv4 mapped addresses are normalized so a hint set for one form is found by the other.

The weakness is stated in the source: players behind one NAT share an IP, so several of them wanting different pools at the same time cannot be told apart. For that case use a destination port, a destination address or a token in the first bytes.

## What was traded away

- **A bounded wait on every byte routed connection.** A client that connects and says nothing waits up to 250 ms for the router to give up, and its peek loop wakes every 5 ms. The cost is small, and the benefit is a ClientHello split across segments being routed correctly.
- **No regex.** The matchers are prefixes, ranges and suffixes. They are cheap and they cannot be turned into a denial of service by a crafted input. Anything more complex belongs in a sniffer, which runs in a sandbox with a time budget.
- **TCP length windows are coarse.** They see one peek, not the whole first message.

## Alternatives that were not taken

- **Reading and replaying.** It needs a buffer per connection and a replay path in the pump.
- **Terminating TLS to read the hostname.** It needs certificates for every site and costs CPU, and it would break end to end encryption.
- **A tree or trie of routes.** The route lists are short, and a linear walk of cheap checks is both simpler and fast enough.

## Where to look

- `crates/wayhouse-config/src/matcher.rs`: the matchers, `client_hello_state` and `extract_sni`.
- `crates/wayhouse-config/src/cidr.rs`: prefixes.
- `crates/wayhouse-core/src/route_hint.rs`: the hint table.
- `crates/wayhouse-core/src/listener.rs`: `peek_routing_bytes`.
- Manual: [Route by rules](../../manual/routing/route-by-rules.md).
