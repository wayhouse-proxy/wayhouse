# Web routing: the `web:` section and upstreams through Wayhouse (2026-10-15)

Refs #297. Builds on the Caddy front-door model (rough design: "Caddy as Wayhouse's packaged web
proxy", with addendum 3 on origins behind `wayhouse-agent`). Scope here is the **Wayhouse side only**:
configuration, validation, and the routing that gets a web site's traffic to Caddy and from Caddy to its
origin. The Caddy renderer (`wayhouse-web-sync`), deployment recipes and certificates are separate slices.

## Goal

An operator publishes HTTP(S) sites through the same entry node as game traffic. Wayhouse stays on the
public ports and does not parse HTTP. Two paths need routing support:

1. **Entry to Caddy.** Wayhouse's `port` / `sni` matchers send :80 and :443 traffic to a pool (default
   `web`) whose backends are Caddy listeners, with `proxy_protocol: v2` so Caddy sees the real client
   address. All of this exists in the data path today. This slice makes it a validated, documented
   contract and proves it end to end.
2. **Caddy to origin.** A site's upstream is either a plain `ip:port`, or a **pool name**. A pool upstream
   is reached in one of two ways:
   - `via: direct` (default): the sync resolves the pool's backends and gives them to Caddy as upstreams.
     No Wayhouse code involved beyond exposing the pool.
   - `via: wayhouse`: config expansion adds a **loopback-only listener** for the pool, and Caddy's
     upstream is that loopback port. The connection then takes the normal Wayhouse path: pool balancing,
     health checks, drains, tunnel backends, and forwarding to another node that holds the origin peer.

Non-goals: Wayhouse parsing HTTP or inbound PROXY protocol; Caddy process management; certificates and ACME
(owned by the controller ACME work, which adds `web.acme` and `web.sites[].tls` to this same section; see
"Fields reserved for other slices").

## Schema (config schema 3)

```yaml
schema_version: 3
web:
  pool: web                          # pool in front of Caddy (default "web")
  listen: { http: 127.0.0.1:8080, https: 127.0.0.1:8443 }   # where Caddy listens (default shown)
  trusted_proxy: []                  # addresses/CIDRs Caddy trusts PROXY headers from (e.g. peer gateway /32s)
  internal_bind: 127.0.0.1           # loopback address of generated listeners (default shown)
  internal_ports: "19000-19999"      # port range for generated listeners (default shown)
  sites:
    - hosts: [panel.example.com, "*.apps.example.com"]
      upstreams:
        - addr: 10.0.0.5:8080        # plain address
        - pool: panel                # a pool; via defaults to direct
        - pool: wings1
          via: wayhouse              # generated listener web-up-wings1 on the loopback range
          port: 19010                # optional pin, inside internal_ports
```

`web:` is optional. Without it nothing changes. The data path ignores everything except the generated
listeners.

### Validation

- `web.pool` must name a pool; that pool must set `proxy_protocol: v2` (Caddy's PROXY listener wrapper
  rejects a plain connection). It may be remote (a gateway node's Caddy); nothing requires it to be local.
- `hosts`: non-empty, each an exact hostname or `*.suffix`; no host appears in two sites (Caddy would
  match ambiguously).
- `upstreams`: non-empty; each is exactly one of `addr` (a socket address) or `pool` (a defined pool);
  `via` and `port` apply only to `pool`; `port` only with `via: wayhouse`.
- A pool used by a `via: wayhouse` upstream must not set `proxy_protocol` (the next hop is HTTP).
- `listen` may be non-loopback (a gateway node's Caddy cannot bind loopback only). `trusted_proxy` is a list
  of IPs/CIDRs (spike #355: a remote `web` pool needs the other node's /32 in Caddy's `proxy_protocol`
  `allow`). A non-loopback `listen` with an empty `trusted_proxy` warns.
- `internal_bind` must be a loopback address. `internal_ports` is `lo-hi`, at most 1000 ports.
- A pinned `port` lies inside the range and is unique among pins.

### Expansion

For every distinct pool used with `via: wayhouse` (sorted by name) the validator appends a TCP listener
`web-up-<pool>` bound to `internal_bind:<port>` with one `always` route to that pool. It appears in
`Config::listeners` like an operator listener, so `Snapshot`, `ListenerManager` reload-by-name, metrics and
`GET /config` need no change. A name clash with an operator listener is an error.

Port choice is **stable under adding or removing other pools**: a pinned port wins; otherwise the port is
the pool name's SHA-256 mapped into the range, advancing to the next free port on collision (with pins,
other generated ports, and any operator listener bind on the same address). Pools are processed in sorted
order so the result is deterministic.

The resolved `Config.web: Option<WebConfig>` carries, per site, each upstream in final form
(`Addr(SocketAddr)`, `Pool{name}`, `Local{pool, addr}`) so the sync can render without redoing the
allocation.

### Warnings (`--check`, startup, reload)

- No TCP listener routes to `web.pool`.
- `web.pool` has no backend that matches `web.listen` (informational; a pure gateway client can ignore it).

## Client IP

Caddy sets `X-Forwarded-For` from the PROXY header, so origins behind either `via` mode see the client
address in HTTP. On the `via: wayhouse` hop the TCP peer is Caddy's loopback; that is expected and
documented, since Wayhouse has no inbound PROXY parsing and none is needed for HTTP.

## Tests

- Config: parse and validate every rule above (golden error text), expansion shape, port stability when a
  pool is added or removed, collision handling, schema gate (`web` needs `schema_version: 3`).
- Data path (`wayhouse-core/tests`): client to a `port`/`sni` route to a `web` pool backend that reads and
  checks a PROXY v2 header carrying the client address; and a `via: wayhouse` upstream: a stand-in for
  Caddy connects to the generated loopback listener and reaches an origin behind a pool, then the backend
  is drained / marked down and the next connection is refused or fails over, exercising the existing pool
  semantics.
- Fuzz harness `parse_config` already covers the new section through `parse_str`.

## Fields reserved for other slices

Agreed with the controller ACME work (shape only; each side owns its own fields):

| Field | Owner |
|---|---|
| `web.acme`, `web.sites[].tls`, `web.sites[].gateways` | controller ACME / certificates |
| `web.caddy_admin` and anything the renderer needs | `wayhouse-web-sync` |
| `web.pool`, `web.listen`, `web.internal_*`, `web.sites[].hosts`, `web.sites[].upstreams` | this slice |

Both slices add fields to the same raw struct; `deny_unknown_fields` stays on. A field lands in
`version::FIELD_SINCE` at schema 3 together with its owner's change.

## Slices

Status: slice 1 is issue #360 (schema, validation, schema 3, docs); the `via: wayhouse` generated listeners are
#368, the end-to-end data-path proof and docs chapter #369, the machine-readable view #370. The expansion and
data-path tests above ship with those, not with #360. #360 resolves `via: wayhouse` upstreams as
`Pool { name, via, port }`.

1. `web:` schema, validation, schema 3, `config.example.yaml`, `docs/05-configuration.md` (#360).
2. `via: wayhouse` expansion: the generated loopback listeners (#368).
3. Data-path end-to-end tests (entry matcher to `web` pool with PROXY v2; `via: wayhouse` path) and the
   docs chapter on the web front door in `docs/03-routing.md` (#369); the deployment recipe is the
   deployment slice's.
4. Machine-readable web view for the renderer (#370).

Each slice ships as its own PR.
