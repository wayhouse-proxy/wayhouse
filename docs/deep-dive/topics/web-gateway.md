---
title: "The web gateway: Caddy, wayhouse-web-sync and site rendering"
description: How wayhouse publishes ordinary websites through a stock Caddy without writing an HTTP server of its own, how the web section becomes one Caddy JSON document, and why the sync never hands Caddy a document it cannot trust.
---

wayhouse moves bytes, and a website needs more than that: virtual hosts, certificates, redirects, WebSockets, health probes on a path. Writing an HTTP server to do it would turn a small, careful proxy into a large one. The project instead puts a stock **Caddy** behind the proxy and describes the sites in a `web:` section of the same configuration file. A small program, **`wayhouse-web-sync`**, turns that section into Caddy's configuration. This page explains the split and the care taken in the join. The options are in the [Manual](../../manual/index.md) and the generated [Reference](../../reference/index.md).

## The problem

1. **One config, one source of truth.** An operator should not keep a Caddyfile that has to agree with the pools in the proxy config.
2. **The proxy keeps the public port.** Players and web visitors reach the same address, so the proxy has to hand web traffic to Caddy and Caddy has to know who the visitor really was.
3. **Caddy's admin API is a loaded gun.** It can replace the whole configuration, and a bad load can take every site down.
4. **Certificates renew,** and Caddy does not watch files.
5. **Backends change.** A drained backend must stop receiving web requests, and Caddy cannot see wayhouse's operator state.
6. **The proxy must not depend on a web server** that is optional, third party and written by someone else.

## Three parts, loosely joined

- **`wayhouse`** validates the `web:` section and serves the resolved result at `GET /web`. Its data path otherwise ignores the section, apart from the loopback listeners described below.
- **`wayhouse-web-sync`** polls that endpoint, renders one Caddy JSON document and loads it. It is its own binary with its own metrics and flags. It reads a view made for it, plain strings with no behaviour, so it needs the shared types but none of the validation.
- **Caddy** is the stock build. The image list of extra modules is empty today, and the rules say a module may only be added with its licence recorded, with one lacking a licence grant staying out. Its base configuration is only an admin block.

The proxy depends on none of this at runtime. The sync is a separate process, so a missing Caddy is a web problem and never a game problem.

### Traffic flows through the proxy

The `web` pool, named in the section, has Caddy's listeners as its backends. It must set **PROXY protocol v2**, and validation refuses a pool that does not, because Caddy's PROXY listener rejects plain connections. Visitors therefore reach the proxy's own listener first, which can route, filter and rate limit them like any other connection. It forwards to Caddy with a PROXY header carrying the real client address, and Caddy turns that into the client address its logs and `X-Forwarded-For` show (see [the PROXY protocol](proxy-protocol.md)).

## The section and its checks

The `web:` section has a pool, two Caddy listen addresses (`127.0.0.1:8080` and `127.0.0.1:8443` by default), a list of trusted proxies, a loopback address and a port range for generated listeners, and the sites. A site has hosts, upstreams, optional certificate and key files, an optional upstream TLS mode and an optional health path. A site can be parked with `enabled: false`. A parked site is **not checked at all**, so its hosts and pins do not take part in later checks.

Validation is strict because a typo should fail at `--check` and not at render time:

- **Hosts** are plain lowercase hostnames or `*.suffix`, with labels of letters, digits, underscore and hyphen, at most 253 bytes and 63 per label. No ports, schemes, IP literals, all numeric names or trailing dots. A host may be in only one site.
- **Every site needs a host and an upstream,** and each upstream is **exactly one of** an address or a pool.
- **A pool upstream may not be the web pool itself,** since Caddy would forward requests to itself.
- **The health path** must start with `/` and may not contain whitespace, control characters or braces, because Caddy expands placeholders in the path and a stray character would corrupt the probe request line.
- **Upstream TLS** is `verify`, `skip` or `{ ca_file }`. `skip` exists for links that are already authenticated, such as a tunnel.
- **Trusted proxies** are CIDRs, and a bare address means a single host.

### Two ways to reach a pool

A pool upstream is reached in one of two ways, and the choice is about who does the load balancing and what the health view is.

- **`direct`** (the default): Caddy connects to the pool's backends itself. The sync fetches the pool listing and renders the **enabled** backends as dial targets. A draining or disabled backend is left out, which is how a drain reaches Caddy, since Caddy cannot see operator state.
- **`wayhouse`**: Caddy connects to a **generated loopback listener** that routes to the pool, so the proxy's own balancer, health state and connection tracking apply. The listener is called `web-up-<pool>`, is bound to the loopback address and carries a single always route to the pool. This is the only way the data path touches the section. The pool itself must not use the PROXY protocol, because this hop speaks plain HTTP to the origin.

Ports for generated listeners come from a range, `19000-19999` by default and at most 1,000 wide. A pool may pin its port. Otherwise the port is chosen **deterministically**: the first 8 bytes of the SHA-256 of the pool name choose a starting point, and the allocation walks the range from there to the first port that nothing else holds. A name always lands on the same port unless something collides, so a reload does not move listeners around. The search skips ports used by the operator's own listeners, the admin API and Caddy's own ports, since the range may legitimately span 8080 or 8443. Two pools pinned to one port, one pool pinned to two, a pin outside the range and a pin that something else holds are all refused, and so is an operator listener that already uses a reserved `web-up-` name.

## One cycle of the sync

The sync wakes every 5 seconds by default. A cycle is a straight line with exits, and **every exit that is not a success leaves Caddy on its current configuration.** The rule behind the module is stated in its first lines: never hand Caddy an empty or partial document. A spike found that loading `{}` answers 200 and removes every listener.

1. **Read `GET /web`.** A 404 means there is no section. With no section and nothing ever loaded, the cycle is idle. If a section existed earlier and has been removed, the last known one is used with its sites cleared, so the sites are withdrawn and Caddy keeps its admin block. A read error is logged and the cycle fails at the `read` stage.
2. **An empty site list before anything was loaded is idle,** because sending it would replace Caddy's base configuration with nothing.
3. **Read the pool listing only if some site needs it,** meaning a `direct` upstream exists. A line of the listing the sync cannot parse **fails the cycle**, since dropping a backend it did not understand would silently remove it from Caddy.
4. **Read the certificate files** of every site with a `tls` section. One unreadable file, or one that is not PEM, fails the **whole** cycle, because a document missing a site's certificate would take that site down on load.
5. **Render.** A pure function from the view, the pools and the certificates to one JSON value. A site with a `tls` section and no loaded certificate is an error.
6. **Compare.** An identical document is not sent again, unless a forced resend is due. A document Caddy has already refused is remembered, so a bad configuration is not re-sent every five seconds.
7. **Load.** `POST /load` to Caddy's admin API.

The loader's answer is sorted into three kinds. A 2xx is success. A **4xx is a rejection**: Caddy keeps its previous configuration, the document is remembered, a counter increments and the error carries Caddy's message, which is capped at 500 characters. Anything else, such as a refused connection, a timeout or a 5xx, is a transient failure at the `send` stage and is retried on the next tick.

### Forced resends

Every 60 seconds by default the sync **re-sends even an unchanged document**. An identical load is a no-op for a Caddy that kept its configuration, and it restores one that restarted and lost it. That is cheaper than asking Caddy what it holds and comparing, and it makes the failure mode self healing without a watcher.

### The admin API is guarded

Caddy's admin API is only ever reached over a **loopback TCP address or a unix socket**, and the default is a socket in `/run/caddy/`. A non-loopback address is refused at start, so the sync cannot be pointed at an admin API reachable from the network. The same address is written into every document as `admin.listen`, with `config.persist: false`, so a document can never move or expose the admin API, and Caddy does not write inline private keys to its autosave file. The client speaks hand written HTTP/1.1 over either transport with a timeout, and reads at most 64 KiB of the answer.

The bearer token for the proxy's admin API is resolved from a file, a service credential or an environment variable and never from a command line value, so it stays out of `ps`. The sync refuses to send that token over plain `http://` to a non loopback host, the same rule as the rest of the fleet (see [the HTTP toolkit](http-toolkit.md)). The metrics listener is on loopback by default and needs its own token to be reachable otherwise.

## What a rendered document contains

Two Caddy servers, one for HTTPS and one for HTTP, each with explicit settings that the spike showed to matter.

- **The PROXY protocol listener wrapper has a REJECT fallback and an explicit allow list.** Loopback is always allowed, since a local wayhouse forwards from it, plus the configured trusted proxies. Caddy's default policy ignores a header from an unlisted peer and accepts the peer's own address as the client, which would let anyone spoof it. With REJECT such a connection is dropped.
- **Automatic HTTPS is disabled.** Certificates are the operator's, or issued by the controller (see [certificate issuance](acme.md)), never Caddy's own ACME.
- **Protocols are `h1` and `h2` only,** or Caddy opens an HTTP/3 UDP listener nobody asked for.
- **Timeouts:** 10 seconds to read request headers and 2 minutes idle.
- **Certificates are inline PEM** in the document, not file paths. Caddy does not watch files, and an identical load is a no-op, so only a changed document makes it pick up a renewed certificate. Putting the PEM in the document makes a renewal change the document, which is what triggers the reload.
- **Streams outlive a reload.** Every reverse proxy sets a stream close delay of one hour, since otherwise each reload closes every WebSocket.
- **The real client address is forced.** A client supplied `X-Real-IP` is overwritten with the remote host. `X-Forwarded-For` is already rebuilt from the PROXY address, so trusting forwarded headers is not needed.
- **Health checks:** passive, with 3 failures over 30 seconds, and active on the health path every 10 seconds with a 3 second timeout when one is set.
- **Upstream TLS** becomes the transport's TLS block: system roots, skipped verification or a CA file.
- **Upstreams are deduplicated,** keeping the first occurrence, since a repeated address would double its weight in Caddy's balancer.
- **An empty upstream list is a valid document.** The route simply answers 503, which is what a site with no healthy backend should do.

### Order and redirects

Exact hosts are matched **before** wildcards, whatever site they belong to, so a `*.example.com` site cannot shadow `panel.example.com` of another site. A site with a certificate is served on HTTPS and **redirected from HTTP with a 308**. A site without one is served on plain HTTP as well, because without a certificate there is no HTTPS for the host.

### HTTP-01 for every host

When the controller issues certificates with HTTP-01, the CA fetches a token from `/.well-known/acme-challenge/*` on port 80 of the domain. If a challenge upstream is configured, the document puts a **terminal route for that path ahead of every site route** on the HTTP server and forwards it to the controller nodes, for every host. The forward lists the nodes in order with a first-choice policy, a short try window and passive health checks, and all nodes must share one scheme. A CA file is only valid with https nodes. This is what lets a host get a certificate before it has a site of its own (see [certificate issuance](acme.md)).

## Metrics

The sync exports a small set: the time of the last cycle that left Caddy on the intended configuration, a counter of loads, a counter of rejections, a counter of errors by stage (`read`, `render` or `send`), and the duration of the last render. The first one is the one to alert on, as the age of that timestamp, since it covers every failure mode at once. The names are in [metrics and logs](metrics-and-logs.md).

## What was traded away

- **Poll and push, no watch.** A change in the proxy reaches Caddy within one interval, not at once.
- **A forced resend instead of a state check.** One redundant load a minute, in return for not trusting that Caddy still has what it was given.
- **A stock Caddy and a small feature set.** The section covers hosts, upstreams, certificates, health paths and TLS to the upstream. Anything else a web server can do is not offered.
- **Inline certificates in memory.** Private keys sit in the document the sync builds and in Caddy's running configuration, which is why persistence is switched off.
- **One shared view of the pool for `direct`.** The sync sees backends as the listing shows them at that instant, so the view lags a change by up to one interval.
- **The sync is a single process.** If it stops, Caddy keeps its last configuration, which the shutdown message states on purpose.

## Where to look

- `crates/wayhouse-web-sync/src/sync.rs`: one cycle and its exits.
- `crates/wayhouse-web-sync/src/render.rs`: the document and the reasons behind each setting.
- `crates/wayhouse-web-sync/src/caddy.rs` and `admin_addr.rs`: loading and the admin address guard.
- `crates/wayhouse-web-sync/src/source.rs`, `pools.rs` and `certs.rs`: reading the proxy, the pool listing and the certificate files.
- `crates/wayhouse-web-sync/src/acme_forward.rs`: the HTTP-01 forward.
- `crates/wayhouse-config/src/web.rs` and `web/view.rs`: validation, port allocation and the view.
- `deploy/caddy/`: the base configuration and the module list.
