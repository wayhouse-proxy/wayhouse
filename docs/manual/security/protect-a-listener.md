---
title: Protect a listener
description: Allow and deny lists, GeoIP, rate limits and connection caps.
sidebar:
    order: 1
---

The proxy checks every new connection or UDP session against a filter chain **before**
routing. A blocked client is dropped silently and counted in
`wayhouse_filter_blocked_total{filter=...}`, so you can see what a rule does without
players seeing an error.

```yaml
listeners:
    - name: public-tcp
      bind: "0.0.0.0:7777"
      protocol: tcp
      allow: ["10.0.0.0/8", "192.168.0.0/16"]
      deny: ["10.6.6.6/32"]
      rate_limit:
          per_ip: { rate: 20, burst: 40 }
          per_net: { rate: 200, burst: 400 }
      per_source:
          max_per_ip: 256
          max_per_net: 2048
      geo:
          deny: ["KP"]
      pool: lobby
```

## The filters, in order

1. **`allow` / `deny` (CIDR lists).** `deny` wins. A non-empty `allow` makes the listener
   default-deny: only listed networks get in.
2. **`geo` (country filter).** ISO 3166-1 alpha-2 codes, checked after the CIDR lists; `deny`
   wins and a non-empty `allow` is default-deny. It needs `settings.geo_db`, a MaxMind Country
   `.mmdb` file; startup fails if the file cannot be opened. An address not in the database is
   admitted only when `allow` is empty.
3. **`rate_limit`.** A token bucket on the rate of _new_ connections or sessions, per source IP
   and per /24 (IPv4) or /64 (IPv6). `rate` is permits per second and `burst` the bucket size
   (default: `rate`). Established UDP sessions are not re-checked.
4. **`per_source`.** A cap on _concurrent_ connections or sessions from one IP (`max_per_ip`)
   or one /24 or /64 (`max_per_net`).

`rate_limit` bounds how fast a client can open connections, `per_source` how many it can hold
open. They guard against different abuse, so use both.

## Process-wide caps

Under `settings.limits`, read at startup:

```yaml
settings:
    limits:
        max_connections: 200000
        max_udp_sessions: 200000
        max_new_sessions_per_sec: 20000
```

Omit a key for no cap. Hitting a cap drops the _new_ connection or session; established ones
are untouched.

## Park a rule without deleting it

With `schema_version: 4`, `enabled: false` on a listener, or on one of its `rate_limit`, `geo`
or `per_source` blocks, switches it off while keeping the config in the file. The
[runtime feature flags](../../reference/index.md) can also switch whole protections off, and a
protection flag that is off needs an expiry on its override.

## Check what the filters do

```sh
curl -s localhost:9900/metrics | grep wayhouse_filter_blocked_total
```

The `filter` label tells you which rule dropped the client: `rate_ip`, `rate_net`,
`src_conn_ip`, `src_conn_net`, `geo`, `max_conn`, `max_udp` or `max_new_rate`.

## Related

- [Security and DDoS design](../../07-security-ddos.md) for why the chain is ordered this way and
  what it does and does not stop.
