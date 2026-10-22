# Web proxy: HTTP(S) sites through Caddy

Wayhouse stays an L4 proxy. To publish web sites (a Pelican panel, Wings, any web UI) through
the same entry node as the game traffic, it forwards ports 80 and 443 to a packaged
[Caddy](https://caddyserver.com) and keeps the real client address with PROXY protocol v2.
`wayhouse-web-sync` turns the `web:` section of the Wayhouse config into Caddy's configuration.
Design: issue #297, the spike findings (#355) and the
[web routing spec](superpowers/specs/2026-10-15-web-routing-design.md).

```
client ──► wayhouse :80/:443 ──PROXY v2──► Caddy (127.0.0.1:8080 / :8443) ──► site upstreams
                │                              ▲
                │ GET /web, GET /pools         │ POST /load (unix socket)
                └────────► wayhouse-web-sync ──┘
```

| Part                | What it is                                                                                       |
| ------------------- | ------------------------------------------------------------------------------------------------ |
| `web:` config       | Sites, upstreams, certificate files; validated by `wayhouse --check` ([05](05-configuration.md)) |
| `wayhouse-web-sync` | Separate process in the `wayhouse` image. Reads the local admin API, renders, loads              |
| `wayhouse-caddy`    | The image with the pinned Caddy, an unprivileged user and the admin socket                       |

Nothing here is on the game data path. If Caddy or the sync is down, only the web sites fail;
Wayhouse's health check takes Caddy out of the `web` pool like any backend.

## Configure

```yaml
schema_version: 3
pools:
    - name: web # Caddy's HTTPS listener
      targets: ["127.0.0.1:8443"]
      proxy_protocol: v2
      health_check: { type: tcp_connect }
    - name: web-http # Caddy's plain HTTP listener (redirects)
      targets: ["127.0.0.1:8080"]
      proxy_protocol: v2
      health_check: { type: tcp_connect }
listeners:
    - { name: https, bind: "0.0.0.0:443", protocol: tcp, pool: web }
    - { name: http, bind: "0.0.0.0:80", protocol: tcp, pool: web-http }
web:
    sites:
        - hosts: [panel.example.com]
          upstreams: [{ addr: "10.0.0.5:8080" }]
          tls: { cert: /certs/panel.pem, key: /certs/panel.key }
        - hosts: [wings1.example.com]
          upstreams: [{ pool: wings1 }] # a Wayhouse pool, also one behind wayhouse-agent
          upstream_tls: skip # Wings serves a private certificate, the tunnel is authenticated
          health_path: /api/system
```

A complete example is [`deploy/compose/wayhouse-web.yaml`](../deploy/compose/wayhouse-web.yaml). The
fields are described in [05 Configuration](05-configuration.md).

## What the sync renders

One Caddy JSON document, loaded with `POST /load` on Caddy's admin API:

- Two servers. **HTTPS** (`web.listen.https`): `proxy_protocol` then `tls`, a route per site. **HTTP**
  (`web.listen.http`): `proxy_protocol` only; a site with a certificate redirects to HTTPS (`308`), a
  site without one is served over HTTP. The redirect target is `https://{host}{uri}` with no port, so it
  is right only when HTTPS is served on 443; behind a non-standard port (for example 8081 to 8444 in a
  demo) browsers follow it to the wrong place.
- `proxy_protocol` accepts headers only from loopback and `web.trusted_proxy`
  (`fallback_policy: REJECT`): a connection from anywhere else that sends a header is dropped, so a
  client cannot claim another address. Loopback is always trusted, because a local `wayhouse` connects
  from it, and each `via: wayhouse` pool gets an unauthenticated loopback listener (`web-up-<pool>`). With
  `network_mode: host` (the compose file) or `hostNetwork` (the Kubernetes manifest), any other process or
  host-network pod on the machine can therefore claim any client address (it ends up in `X-Real-IP` and
  `X-Forwarded-For`) and reach those pools. Run the gateway on a host that runs only trusted workloads.
- Real client address: `X-Forwarded-For` is built from the PROXY header (a client-supplied one is
  replaced) and `X-Real-IP` is overwritten with the same address.
- Every `reverse_proxy` has `stream_close_delay: 1h`, so a reload does not close WebSockets (Pelican
  consoles). Both servers use `protocols: [h1, h2]`; there is no HTTP/3 listener.
- Exact hosts match before `*.suffix` wildcards, across all sites.
- `upstreams`: an `addr` as is; a `pool` with `via: direct` (default) as that pool's **enabled**
  backends, read from `GET /pools` on every cycle, so a drained or disabled backend leaves Caddy
  within one interval (Caddy cannot see Wayhouse's admin state). `via: wayhouse` uses the
  generated loopback listener `web-up-<pool>` (`local` in `GET /web`).
  A site with no usable upstream answers `503`.
- `upstream_tls`: absent is plain HTTP; `verify`, `skip` (no verification) or `{ ca_file: <path> }`
  (a path on the Caddy side). `health_path` adds an active check; passive checks are always on.
- Certificates (`tls: { cert, key }`) are read by the **sync** and rendered inline, not as file
  paths. Caddy does not watch certificate files, and loading an identical document changes
  nothing, so a renewed file only takes effect because it changes the document. The sync re-reads
  the files every cycle.
- The `admin` block is always identical (`listen` equal to `--caddy-admin`, `config.persist: false`).
  Without `persist: false` Caddy writes inline private keys to `autosave.json`.

Caddy's own ACME is **off** (`automatic_https.disable`). Certificates come from files; controller-issued
certificates are a separate work stream (#361 to #366). Running Caddy's built-in ACME next to the sync
is not supported: every load replaces the whole configuration. The recipe items that ACME in Caddy
would need (`http_port: 8080`, `alternate_port`, `disable_redirects`) are deliberately deferred until
that work; the ACME design lives in
`docs/superpowers/specs/2026-10-15-controller-acme-design.md` (with the ACME work, #372).

## Safety rules

The sync never hands Caddy an empty or partial document (`POST /load` with `{}` answers 200 and
removes every listener):

- It waits for the first good `GET /web`. A failed read, a missing `web:` section or an empty site
  list at startup send nothing, so Caddy keeps its base configuration.
- A failed read of `/web` or `/pools`, an unreadable or non-PEM certificate file, or a render error
  sends nothing and leaves Caddy on what it has (`wayhouse_web_sync_errors_total`).
- Removing the last site (or the whole `web:` section) after something was loaded sends a complete
  document with no routes, so the sites really stop.
- If Caddy refuses a document (4xx, for example a bad CIDR) it keeps its previous configuration; the
  sync logs it, counts it in `wayhouse_web_sync_rejects_total` and does not resend the same document.
- An unchanged document is re-sent every `--resend-sec` (60 s): an identical load is a no-op for a
  Caddy that kept its configuration and restores one that restarted.
- The admin API is only reachable on a unix socket or a loopback address; the sync refuses any other
  `--caddy-admin`.

## HTTP-01 challenges

With controller-issued certificates ([`acme.md`](acme.md#http-01)), a site can prove control
with HTTP-01. Give the sync the controller nodes and it adds one route to the **HTTP** server
(port 80), ahead of every site route and redirect:

```text
wayhouse-web-sync --acme-challenge-upstream https://controller-1:7000,https://controller-2:7000
```

Only `/.well-known/acme-challenge/*` is forwarded, for every host, to the first controller
that is up. A site that serves its own files under that path is shadowed by it, so do not set
the flag if no site uses HTTP-01. All controllers must use the same scheme; for https, add
`--acme-challenge-ca-file` if the controllers' certificate is not signed by a CA Caddy
trusts. The route is part of the rendered document, so it exists only while `wayhouse` has a
`web:` section with sites. The sync never sends Caddy a partial document.

## Run

Command line of `wayhouse-web-sync` (`--help` has the rest):

| Flag                                            | Default                 | Meaning                                                               |
| ----------------------------------------------- | ----------------------- | --------------------------------------------------------------------- |
| `--wayhouse-admin`                              | `http://127.0.0.1:9900` | The local `wayhouse` admin API (`settings.admin.listen`, https works) |
| `--wayhouse-token` / env `WAYHOUSE_ADMIN_TOKEN` | none                    | Bearer token if `settings.admin.auth_token` is set                    |
| `--allow-insecure-secrets`                      | off                     | Let the admin token go over plain http to a non-loopback `--wayhouse-admin` (refused by default) |
| `--caddy-admin`                                 | `/run/caddy/admin.sock` | Caddy admin: unix socket or loopback `ip:port`                        |
| `--interval-sec`                                | 5                       | How often it reads Wayhouse (pool changes, renewed certificates)      |
| `--acme-challenge-upstream`                     | none                    | Controller URLs that answer ACME HTTP-01 (see below)                  |
| `--acme-challenge-ca-file`                      | none                    | CA (path on the Caddy side) for an https `--acme-challenge-upstream`  |
| `--metrics-listen`                              | `127.0.0.1:9904`        | `GET /metrics`; a non-loopback address needs `--metrics-token`        |

Caddy has to start with a base configuration whose `admin.listen` is the same address: the image
ships [`deploy/caddy/base.json`](../deploy/caddy/base.json) (`unix//run/caddy/admin.sock`).

**Docker Compose:** `docker compose -f docker-compose.yml -f compose.web.yml up --build` in
`deploy/compose/`. **Kubernetes:** label the gateway nodes and apply
[`deploy/k8s/60-web-gateway.yaml`](../deploy/k8s/60-web-gateway.yaml).

The `wayhouse-caddy` image is stock Caddy, pinned by version and digest in
[`deploy/Dockerfile`](../deploy/Dockerfile) (one place; bumps are manual, the Trivy job scans it like the
other images). It runs as uid 65532 on high ports with a read-only root filesystem and no
capabilities (the base image's `cap_net_bind_service` file capability is removed, which would
otherwise make the binary fail to start once all capabilities are dropped), with data and config
volumes only for Caddy's state. It carries the Apache-2.0 text and a NOTICE under `/licenses`; see
[Third-party notices](third-party-notices.md) for the license and trademark decisions and the rule for
adding modules.
Modules (for example DNS-01) would mean building it with `xcaddy` instead; the Dockerfile says where.

## Topologies

- **Single node.** `wayhouse`, Caddy and the sync on one host, as in the compose file. Caddy listens on
  loopback.
- **Gateway tier.** Run Caddy and the sync on 2 to 3 nodes per region; every other node routes its :80
  and :443 to the `web` pool whose targets are those gateways. Caddy then listens on a routable
  address (`web.listen`) and `web.trusted_proxy` lists the nodes that forward to it. Certificates
  are needed on the gateways only.
- **Origin behind `wayhouse-agent`.** The tunnel interface lives on the node that runs `wayhouse`.
  Caddy has to see it, so it shares the host network namespace (`network_mode: host`,
  `hostNetwork: true`, as both recipes do). With `via: direct` Caddy dials the origin's tunnel
  address itself; the web service's port must be among the backends the agent fronts
  (`--backends`). The real WireGuard path is exercised by the tunnel CI job; it is not part of the
  Caddy end-to-end test.

## Operating notes

- If Caddy starts before Wayhouse marks it healthy, the first connection from outside can be reset;
  once the `web` pool's health check passes (`interval_sec`, 5 s in the examples) traffic flows.
  Lower the interval if that gap matters.
- Metrics (`wayhouse_web_sync_*`) are in [06 Operations](06-operations-observability.md). Alert on
  `time() - wayhouse_web_sync_last_ok`.
- `GET /web` on the Wayhouse admin API (404 without a `web:` section) is what the sync reads; use it to
  see what Wayhouse resolved.
- The Caddy end-to-end test (`crates/wayhouse-fleet-tests/tests/web.rs`, CI job `web-e2e`) runs
  Wayhouse, the sync and the Caddy from the `wayhouse-caddy` image against an origin: real client
  IP at the origin, a spoofed `X-Forwarded-For` replaced, a spoofed PROXY header dropped, a
  WebSocket upgrade, HTTPS with an operator certificate, the HTTP redirect.
  Run it by hand with `CADDY_BIN=<caddy> cargo test -p wayhouse-fleet-tests --test web -- --ignored`.

- **Restart edge.** If the sync restarts while Caddy still serves old sites and `web:` now has zero
  sites, the sync stays idle (it never sends an empty document) and Caddy keeps the stale sites until
  a site exists again or Caddy restarts.
- **Unrouted warning.** The "no listener routes to the web pool" warning counts only direct pool routes;
  a resolver route or routes added later by `plugin_routes` can reach the pool unseen, so the warning
  can be a false positive.
