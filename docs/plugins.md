# Plugins

A **plugin** is a WASM module that reacts to a trigger and acts through capabilities
the operator approved (design: plugin system
and automation hooks).
Plugins are not [sniffers](sniffers.md): sniffers run on the proxy data path.

**Status.** The host runtime is built (`crates/wayhouse-plugin-host`) and a standalone
controller can store plugin installs behind an admin API and ticks the enabled ones
(below); the web UI has a Plugins page on top of the install API (see "The Plugins page"). An HA tier replicates installs and state through Raft, serves the same API on every replica
and ticks on the leader only, and the module bytes follow the installs to every replica (see
"High availability"). Secrets, `http`, `routes`, `backends`, webhooks and events are built.

## Module contract (ABI 0.3)

Link `wayhouse-plugin-abi` (it stamps the `wayhouse.plugin-abi` section and exports
`alloc`). The host accepts a module only when the section equals its own version (exact
minor match while the major is 0), and reads the section before compiling anything.

A second custom section, `wayhouse.plugin-caps`, holds the capabilities as JSON. It is read
without running the module and approvals are recorded against the module's sha256:

```json
{
    "triggers": { "on_timer": true },
    "tick_interval_secs": 30,
    "log": true,
    "state": { "max_bytes": 65536 },
    "http": {
        "hosts": [
            { "host": "panel.example", "port": 8443 },
            { "host": "10.0.0.5", "allow_private": true }
        ]
    },
    "secrets": [{ "name": "PANEL_TOKEN", "hosts": ["panel.example"] }]
}
```

Unknown fields are an error. `tick_interval_secs` is required with `on_timer` and at
least 10 and at most 604800 (one week).

Exports: `memory`, `alloc(i32) -> i32`, `init(config_ptr, config_len)`, and `on_timer()` when
declared. The only imports allowed are these, in the `wayhouse` namespace:

| Import                                           | Capability | Behaviour                                                                                                                        |
| ------------------------------------------------ | ---------- | -------------------------------------------------------------------------------------------------------------------------------- |
| `log(level, ptr, len)`                           | `log`      | levels 0 error, 1 warn, 2 info, else debug; lines over 1024 bytes are cut; lines past the per-call limit are dropped and counted |
| `state_get(kptr, klen, out_ptr, out_cap) -> i32` | `state`    | value length, or -1 when absent; nothing is written when the value is longer than `out_cap`                                      |
| `state_put(kptr, klen, vptr, vlen) -> i32`       | `state`    | 0, or -1 when over `state.max_bytes`, the key is over 256 bytes or not UTF-8, or the per-call write limit is hit                 |

| `routes_set(ptr, len) -> i32` | `routes` | declares the plugin's whole route set as a JSON array of `{"host", "backend"}`; 0, or -1 when refused (the reason is a `warn` line in the plugin's log) |
| `port_routes_set(ptr, len) -> i32` | `port_routes` | declares the plugin's whole port route set as a JSON array of `{"key", "backend", "proto", "prefer"?}` (`proto` is `udp`, `tcp` or `both`); 0, or -1 when refused (the reason is a `warn` line in the plugin's log) |
| `port_routes_get(out_ptr, out_cap) -> i32` | `port_routes` | copies out the entry ports the host assigned, as a JSON object `{"<key>": <port>}`; 0 when nothing is assigned yet, -1 when `out_cap` is too small |
| `acme_dns_result(ok, ptr, len) -> i32` | `on_acme_dns` | answers an `on_acme_dns` call: `ok` 1 or 0 and a message of at most 512 bytes (UTF-8); 0, or -1 when `ok` is neither or the message is too long; the last call wins |
| `http_request(ptr, len) -> i32` | `http` | sends the JSON request at `ptr`; returns the length of the response document, or -1 when the request is over the size limit |
| `http_read(out_ptr, out_cap) -> i32` | `http` | copies the pending response document out; -1 (and keeps it) when `out_cap` is too small |

A call that uses a capability it did not declare traps that call (`CallError::CapabilityDenied`)
and leaves the host running.

## Install options and capability bindings

An official, prebuilt plugin cannot know the panel host or the node network of every operator.
ABI 0.2 lets a module declare **options** the operator fills in at install, and bind some
capability values to them (see [the plugin host](deep-dive/topics/plugin-host.md)).

An optional custom section, `wayhouse.plugin-manifest` (JSON, up to 16 KiB), holds a `name`, an
`options` schema and the `bindings`:

```json
{
    "name": "pelican",
    "options": {
        "type": "object",
        "required": ["panel_host"],
        "properties": {
            "panel_host": { "type": "string", "title": "Panel host" },
            "node_cidr": { "type": "string", "default": "10.0.0.0/8" }
        }
    },
    "bindings": [
        { "option": "panel_host", "kind": "hostname" },
        { "option": "node_cidr", "kind": "cidr" }
    ]
}
```

In the capability JSON a value that is exactly `"${opt:panel_host}"` is a placeholder. It is
allowed only where the table says, and only for an option bound with that kind; text around a
placeholder, a placeholder in a key or in any other field is refused when the module is read.

| Kind                 | Where                                                                | The value must be                                            |
| -------------------- | -------------------------------------------------------------------- | ------------------------------------------------------------ |
| `hostname`           | `http.hosts[].host`, `secrets[].hosts[]`                             | a lowercase DNS name or IP literal; never a wildcard         |
| `route_host_pattern` | `routes.hosts[]`                                                     | an exact name or `*.suffix`                                  |
| `cidr`               | `routes.backends[]`, `port_routes.backends[]`                        | `ip` or `ip/prefix`                                          |
| `port_range`         | `port_routes.ports[]`                                                | `port` or `lo-hi`                                            |
| `int`                | `http.hosts[].port`, `routes.max_entries`, `port_routes.max_entries` | a non-negative JSON integer (then the ordinary bounds apply) |

A `cidr` network must be no wider than `/8` (IPv4) or `/16` (IPv6), so a typo such as `/0` cannot
allow every backend, and an option marked `x-wayhouse-secret` cannot be bound: its value would
land in the granted capability, which an operator reads back.

`options` is a JSON Schema subset: `type`, `properties`, `required`, `enum`, `default`,
`minimum`, `maximum`, `items`, `additionalProperties` (a boolean) and the display hints
`title`, `description`, `group`, `order` and `x-wayhouse-secret`. Anything else (including
`pattern` and `format`) is refused. The install `config` is the option values: option `NAME` is
`config.NAME`, or the schema `default`. A config that does not fit answers `422` with the JSON
path. `config` is still passed to `init` unchanged.

The host substitutes the values and then parses the result with the same code as a literal
declaration, so the **effective capabilities** are always something a literal declaration could
have been. `allow_private`, triggers, `secrets[].name` and the other fields stay literal in the
module: an option changes where a capability points, never what it is.

The operator approves the effective set. For a module with placeholders `POST /plugins` requires
`approved` to equal it exactly (`400` with the set otherwise), and the host compares the effective
set with `approved` again every time the module is loaded, so editing a stored option can never
widen what runs. The upload response carries the `manifest`; its `capabilities` list only what
does not depend on options. `wayhouse-plugin-check <module.wasm> [config.json]` prints the
effective set for a sample config. `POST /plugins` with `"dry_run": true` answers
`{"effective": ...}` for the given `config` without installing (the UI shows that set before it
asks for the approval). `GET /plugins/<id>` returns the install with its module's `manifest`.

## Secrets and the `http` capability

`secrets` declares slots by name (an upper-case letter then `A-Z`, digits, `_`; up to 64 characters), each bound to hosts that the module also
lists under `http.hosts`; the operator approves the set with the module's sha256 like any
other capability. `http.hosts` entries are exact lower-case host names or IP literals, with an
optional `port` (default 443); `allow_private` must be approved per host to reach private,
loopback or link-local addresses.

A slot's `hosts` entries are `host` or `host:port` (`[v6]:port` for an IPv6 literal). A `host:port`
entry binds the slot to that approved port only. A bare `host` binds it to the host's one approved
port, and a module that lists the host on several ports must name one (`panel.example:8443`), so a
secret never reaches a second port of the same host by accident.

**The request.** `http_request` takes UTF-8 JSON, `{"method", "url", "headers", "body"}`
(`body` base64, optional), and `http_read` returns `{"status", "headers", "body"}` (`body`
base64), or `{"error": "..."}` for a refused or failed call. The host enforces: `https` only; a
host and port the module declared; no proxy; no redirect to anywhere but another approved
host (up to 3 hops, each re-checked); at most 8 calls, 256 KiB of request and 1 MiB of response
body per call, 10 s per request and 20 s per guest call; a fixed header denylist (`Host`,
`Content-Length`, ...). Connecting resolves the name once and checks every resolved address, so a
DNS answer that changes cannot reach a private address that was not approved; an IP literal is
checked the same way.

**Secrets never reach the guest.** A header value may contain `${secret:NAME}`. The host expands
it, only in header values (not in the URL or the body) and only when the request goes to a host
the slot is bound to, and re-checks that for each redirect hop. The response (headers, body, error
text) and every log line are scrubbed of the secret's raw, base64 and percent-encoded forms
before the guest or the status sees them.

**Storage.** Operators set a slot with `PUT /plugins/{id}/secrets/{slot}` and `{"value": "..."}`
(16 to 4096 bytes), clear it with `DELETE` and list the slots with `GET /plugins/{id}/secrets`
(set or not, the key id, `ok` or `held: ...`; never a value). Only approved slots are accepted.
A value is encrypted on the controller that received it with XChaCha20-Poly1305 under a cluster
keyring, bound to install, slot and key id, and only the ciphertext goes into the controller database,
the Raft log and snapshots. A follower seals the value itself and sends the ciphertext entry to
the leader (`POST /raft/plugin-secret`); it never forwards the plaintext request. Deleting an
install deletes its secrets. Changes are audit-logged (install, slot, key id, actor; never the
value) at target `wayhouse_controller::plugins::audit`, and a delete or rewrap asks every node to
purge the log, so old ciphertext does not outlive it for long.

**Keys.** `--plugin-secret-key-file` (or `WAYHOUSE_PLUGIN_SECRET_KEY`) names a keyring: one
base64 key of 32 random bytes per line (`openssl rand -base64 32`), the last line active, the
file readable by its owner only. Every controller of a tier needs the same keyring, and the file
is re-read when it changes. Without a key the secrets API answers `409` and a plugin that uses
secret slots is **held**: it is not run, `GET /plugins/{id}/status` shows `held: ...` (no key,
secret unset, key missing, or not authenticating), and other plugins keep running. Plaintext
is kept out of the store and the log, and the controller wipes the buffers it owns, but this is
**best effort**: the HTTP stack copies a request body and an expanded header value into memory
this project does not control (`HeaderValue`, the `PUT` body `Bytes`), and those copies are not
zeroed. Anyone who can read the controller's or the proxy's memory can read live secrets.
Plaintext HTTP on a non-loopback `--listen` refuses to start with `--plugins` and a key unless
`--allow-insecure-secrets` is given.

**Rotating a key.** (1) Append the new key to the keyring file on every controller. (2) `POST
/admin/plugins/secrets/rewrap` on any controller: it refuses until every member reports the
active key (`GET /admin/plugins/secrets/keys` lists the key ids each holds), then re-encrypts
every secret under it; a slot an operator rewrote meanwhile is skipped and reported. A member that does not answer is skipped, so a rewrap also works while a replica is down; that replica catches up on its own once it is back, and until it has the new key in its keyring the plugins that use it are held. (3) Remove
the old key from the file. Removing it before step 2 holds the plugins that still use it
(`held: key <id> missing`) until it is put back. With `--acme` the account and certificate keys use the same keyring and are rewrapped by `POST /admin/acme/rewrap` ([acme.md](acme.md)), and the cluster secrets (the admin token and gossip PSK, `PUT /secrets/{name}`) by `POST /secrets/rewrap`; call all that apply before step 3, or proxies that fetch a secret sealed under the removed key fail their next start. The cluster secrets are opened by proxies and agents with their own key files, so add the new key to every proxy and agent before re-encrypting them: a proxy without it cannot read the admin token or gossip PSK, so its next reload is refused and its next start fails (the panel's check covers controllers only). The web UI's Secrets page (admin role; the key-rotation panel is hidden on a controller that runs without plugins, so use the endpoints there) walks the same steps: it lists the key ids each controller holds, refuses to re-encrypt while a node lacks the active key, and has one button each for the plugin secrets (`POST /api/plugins/secrets/rewrap`), the cluster secrets (`POST /api/secrets/rewrap`) and the ACME keys (`POST /api/acme/rewrap`). It keeps each result on screen, says when entries could not be re-encrypted (the old key must stay then), and answers a controller without ACME with "no certificate keys" (`501`). Each rewrap writes an audit line with the actor and the counts.

## Routes

The `routes` capability lets a plugin publish hostname routes:

```json
"routes": { "hosts": ["*.mc.example.com", "play.example.com"], "backends": ["10.0.0.0/16"], "max_entries": 64 }
```

An entry may carry a `kind` (ABI 0.3): `tcp` (the default, a listener route as below) or `web`
(a site of the `web:` front door with the backend as its upstream). The approval lists the kinds
a plugin may publish in `kinds` (absent means `["tcp"]`), so a plugin approved for hostnames
cannot start serving web sites without a new approval: `"routes": {…, "kinds": ["web"]}`. A set
with an entry of a kind that was not approved is refused like any other entry outside the approval.

`hosts` are the names (exact or `*.suffix`) the plugin may claim, `backends` the networks its
backend addresses must be in, `max_entries` (1 to 256) the size of one set. A module cannot ask
for more of any of these than was approved. `routes_set` replaces the plugin's whole set; the host
refuses a set with an entry outside the approval, a duplicate hostname or too many entries, and a
tick that does not call it leaves the set as it was. The set is committed in the same entry as the
tick's state writes (term and revision checked, validated again on every replica), and only when it
changed.

**Materialisation (#236).** The controller does not rewrite the operator's config. It publishes the
routes of every _enabled_ install as an overlay (`GET /plugin-routes`, and a stream at
`GET /plugin-routes/subscribe` that proxies follow), and a proxy started with `--controller`
appends them to each TCP listener that opted in with `plugin_routes:` (config schema 2, see
`docs/05-configuration.md`). The listener says how the hostname is read (`sni`, or a `sniffer`).
Rules:

- Plugin routes go **after** the listener's own routes, so operator routes always win, including an
  `always` fallback.
- Each distinct backend address becomes a one-target pool named `plugin-<hash>`.
- Two installs claiming the same hostname: the earlier install wins; the later claim is not applied
  and is listed under `conflicts` in `GET /plugin-routes`, with `owner` (`plugin:<install id>`) and
  plugin name on every route.
- Exact names match before `*.suffix` patterns, longer suffixes first.
- Disabling or deleting an install withdraws its routes at once. A plugin whose ticks fail keeps
  its last committed set for a grace period, `max(10 x tick interval, 10 minutes)` after its last
  successful tick (on a new leader the clock starts at takeover). After that the status shows
  `routes_stale` (and a WARN is logged once). At twice the grace the leader commits an empty route
  set: the routes leave `GET /plugin-routes` on every replica, while the plugin's state and port
  assignments stay. A plugin that declares its routes on every tick gets them back with its next
  successful tick; one that only calls `routes_set` when something changed should call it every
  tick. A plugin that stops calling `routes_set` while its ticks succeed keeps its set.
- Routes of kind `web` are listed under `web_routes` in the overlay (a separate list, so a proxy
  that predates the kind never sees them). A proxy adds each as a site of its `web:` block
  (`hosts: [host]`, one `addr` upstream) unless an operator site already has that host, which is
  logged and skipped. Without a `web:` block they are ignored.
- A controller started without `--plugins` answers `501` (`404` on an older build) and proxies drop
  the plugin routes at once, keeping their own config; a controller that is only unreachable leaves
  the last overlay in place. A proxy on a config file (no `--controller`) ignores the overlay
  unless it is started with `--plugin-overlay-url <controller URL>` (and `--plugin-overlay-token`
  when the controller needs one): the same stream, merged into the file's `plugin_routes:` listeners
  on every reload. The two flags exclude `--controller`.
- A proxy with no controller at all can take the routes from a file instead:
  `--plugin-overlay-file <path>` holds the document the controller serves on `/plugin-routes`
  (`routes`, `port_routes`, `web_routes`; see the shape above), for example one a script keeps
  current, or to try `plugin_routes:` listeners without plugins. A missing file at start means no
  routes; a file that is not that document, or is over 1 MiB, refuses the start (`--check` too). Once
  running, an edit that is not that document, or a file that goes missing, keeps the last good routes
  and logs one line. Entries the proxy cannot read (a misspelt `host`, a `proto` that is not `tcp`,
  `udp` or `both`) are left out and counted in a warning. The proxy re-reads the file every two
  seconds and rebuilds when its routes changed. It excludes `--controller` and
  `--plugin-overlay-url`.

## Port routes

Games on UDP have no hostname to route by. The `port_routes` capability lets a plugin route by
**entry port** instead (see [plugins in the controller](deep-dive/topics/plugin-runtime-in-controller.md), #403). The operator opens one port range once; the plugin never picks a port.

```json
"port_routes": { "ports": ["30000-30999"], "proto": ["udp", "tcp"], "backends": ["10.0.0.0/16"], "max_entries": 256 }
```

`ports` are the entry ports the host may assign (`"30001"` or `"lo-hi"`), `proto` the transports,
`backends` the networks a backend must be in and `max_entries` (1 to 1024) the size of one set. A
module cannot ask for more of any of these than was approved.

`port_routes_set` replaces the plugin's whole set. An entry is `{key, backend, proto, prefer?}`:

- `key` is the stable identity of the thing being routed (a Pelican server's uuid), 1 to 128
  characters of letters, digits and `-_.:`, unique within the set. The host keeps `key -> entry port`
  for as long as the install exists. Changing `backend` (a server moved to another node) keeps the
  port.
- `prefer` is an optional hint (a Pelican server's allocation port): the host uses it when it is
  inside the approval and free, otherwise it assigns the lowest free port of the approved ranges.
- One entry port belongs to one `(install, key)` whatever the protocol, so `both` needs no second
  port and two installs never collide. Keys are namespaced by install.

The host checks the set against the approval (backend networks, ports of `prefer`, transports,
duplicate keys, the entry cap). The allocation happens in the replicated apply step of the same
Raft entry as the call's state writes (term and revision checked, the entry carries the clock), so
every replica computes the same table and a stale leader cannot hand out a port. The plugin reads
its assignments back with `port_routes_get`.

**Quarantine.** A port released by a key that left the set, or by a deleted install, waits 24 hours
before another key may take it, so a player with an old address does not land on a different server.
The key that released it takes it back at any time. When the approved ports are otherwise full, the
port that has waited longest is reused. Disabling an install keeps its ports reserved; its routes are
withdrawn.

**Exhaustion.** An entry that finds no free port stays unplaced and is listed under `exhausted` in
`GET /plugin-routes` and on the Plugins page; it is retried when ports free up.

**Materialisation.** `GET /plugin-routes` (and its stream) carries a separate `port_routes` list
(`key`, `port`, `proto`, `backend`, `owner`, `plugin`) next to the hostname `routes`, so proxies that
predate port routes keep working. A proxy with `--controller` appends each entry to the listeners that
opted in with `plugin_routes: { type: port }` ([configuration](05-configuration.md)). The operator
opens the range in the firewall once and states the same range in the listener `bind:` and in the
plugin approval; players connect to the node's name at the entry port, which the Plugins page lists
first.

**Disabled installs.** A disabled install keeps its entry ports until it is deleted, so the range does not free up by disabling a plugin.

**HA and upgrades.** Port routes need every member on a build that reports plugin support level 4
(`/raft/whoami`); until then the leader refuses to commit a port route set and the tick reports why.
A module that declares `port_routes` fails to link on a host that predates the imports and is
refused at install; this is the additive-import behaviour, not an ABI bump.

## Webhooks

A plugin that declares `"triggers": {"on_webhook": true}` can be called by an outside system.
Webhooks are off until the operator starts the controller with a **separate listener**, never on
the admin port:

```text
--plugin-webhook-listen 0.0.0.0:8444 --plugin-webhook-tls-cert hooks.pem --plugin-webhook-tls-key hooks.key
```

It needs `--plugins`. Plain HTTP on a non-loopback address is refused at start unless
`--allow-insecure-secrets` is given (the token and body would cross the network in clear text).

**Enabling.** `POST /plugins/{id}/webhook` (admin) creates a token for an install that was approved
for `on_webhook` and answers `{"token": "whk_...", "path": "/plugins/{id}/hook"}`. The token is
shown once; only its SHA-256 is stored and replicated. Calling it again rotates the token (the old
one stops working at once); `DELETE /plugins/{id}/webhook` turns the webhook off. Under HA the
call needs every member to run a build that supports it.

**Calling.** `POST <listener>/plugins/{id}/hook[/suffix][?query]` with `Authorization: Bearer
<token>`. In order:

1. A per-source rate limit (IPv6 by /64) runs first, valid token or not: `429`.
   The source is the TCP peer address. Callers behind one load balancer or NAT therefore
   share a single bucket, and the limit applies to all of them together; there is no
   trusted-proxy header. Put the webhook listener where callers reach it directly. The budget
   is 5 requests per second with a burst of 20; `--plugin-webhook-source-rate` and
   `--plugin-webhook-source-burst` change it (1 to 1,000,000).
2. The token is checked in constant time, against a dummy hash when the install is unknown,
   disabled or has no webhook. Every failure is the same `401`.
3. A per-install rate limit: `429` (10 per second, burst 20 by default; `--plugin-webhook-install-rate`
   and `--plugin-webhook-install-burst`).
4. Only then the body is read, capped at 256 KiB: `413`.
5. The call runs as one of the install's serialized calls (at most 4 hook calls wait per install;
   the timer is never rejected). A full queue is `429` and is counted in the status
   (`hooks_dropped`); a plugin that fails is `502`.

The guest receives the method, suffix, query, headers (never `Authorization` or `Cookie`), the body
(base64) and an idempotency key (the `Idempotency-Key` header, else a hash of the install and
body). It answers with `webhook_respond` (status, headers, base64 body); only plain headers are
passed on, and the response is never cached. The call's state writes are committed **before** the
answer goes out, so a `2xx` means the state is durable; a commit that fails is `503` and the caller
should retry (use the idempotency key to make that safe).

**HA.** Any node may receive the request. A follower authenticates it itself and forwards it to
the Raft leader over the peer channel (`/raft/plugin-hook/{id}`, peer token, protocol gate), but
only over `https` peers: with plain-HTTP peers it answers `503` rather than send the body in clear
text. Run the webhook listener on every node you want to take requests, or point the sender at one.

## Events

A plugin that declares `"triggers": {"on_event": ["config_revision", "plugin_changed"]}` (and
`on_timer`, which stays required) gets `on_event(kind, payload)` calls:

| Kind              | Payload                                                                        | When                                          |
| ----------------- | ------------------------------------------------------------------------------ | --------------------------------------------- |
| `config_revision` | `{"revision": N}`                                                              | the controller accepted a new config revision |
| `plugin_changed`  | `{"id": "...", "change": "installed" \| "enabled" \| "disabled" \| "deleted"}` | an install was added, toggled or removed      |

Events are **hints, not a log.** They use the same bounded hook queue as webhooks (at most 4
waiting per install), so a slow plugin loses events instead of building a backlog; the loss is
counted as `hooks_dropped` in the status. A restart, a failover (only the Raft leader delivers) or
a lagging feed also loses some. The list of kinds only grows. A plugin that must not miss a change
reconciles from the controller's state on its timer and treats an event as a reason to do it
sooner. The `backend_health` kind (ABI 0.3, see "Backend health") goes only to installs whose approved `backends` networks hold the address.

## Backend health (`backends`, ABI 0.3)

The `backends` capability lets a plugin hear about backend health inside networks the operator
approved:

```json
"backends": { "networks": ["10.0.0.0/16"], "max_entries": 256 },
"triggers": { "on_timer": true, "on_event": ["backend_health"] }
```

`backends_get(out_ptr, out_cap) -> len` returns a JSON array of `{backend, up, since}` for the
backends inside `networks` (at most `max_entries`, the most recently changed first); `-1` means
the buffer was too small and `0` that nothing is known. The `backend_health` event carries one
such object per change. Both only ever show backends inside the approved networks, so a plugin
cannot learn the rest of the topology. Declaring the event needs the capability.

**Where it comes from.** Each proxy that talks to the controller (`--controller`, or
`--plugin-overlay-url`) posts its view of its backends to `POST /backend-health` (a full snapshot
when something changed and at least once a minute). The leader keeps `(proxy, backend) -> up` in
memory; a backend is **down when a majority of the proxies that reported it say down**, and a proxy
not heard from for 3 minutes is forgotten. At most 256 proxy names can report at a time. A report names its proxy and the name is not verified: every holder of the proxy token is trusted like any fleet member, so a compromised proxy could skew a verdict, bounded by that cap. A change of verdict becomes one `backend_health` event
through the bounded hook queue. Nothing is stored or replicated: after a failover the new leader
starts empty and refills from the proxies' next reports, and a plugin that needs certainty
reconciles with `backends_get` on its timer. Controllers at plugin support level 7 or higher only;
a mixed cluster refuses to commit a plugin that needs it.

## ACME DNS-01 (`on_acme_dns`)

A DNS-provider plugin declares `"triggers": {"on_timer": true, "on_acme_dns": true}` (the timer
stays required, like for events), exports `on_acme_dns(ptr, len)` and answers with the
`acme_dns_result` import. The controller's ACME runner calls it on the **leader** to set and
remove the TXT record a CA validates ([`acme.md`](acme.md)). The request document is:

```json
{
    "op": "present",
    "fqdn": "_acme-challenge.panel.example.com",
    "value": "<txt value>",
    "zone_hint": "panel.example.com"
}
```

`op` is `present` (add the TXT record) or `cleanup` (remove it); several values may coexist at
one name, so remove only the one in `value`. `zone_hint` is the host being validated; the plugin
finds the provider zone that contains it. The plugin does the provider call with its approved
`http` capability and `${secret:NAME}` slots, and ends with `acme_dns_result(1, ...)` for done or
`acme_dns_result(0, message)` for a refusal. A call that never answers, traps or runs past the
call limit fails that order, and the runner still sends `cleanup`. The message is scrubbed of
every secret the call expanded and shows in the site's `last_error`.

The plugin never sees an ACME account or certificate key. The trigger is additive: modules that
do not declare it are unaffected, so the ABI minor does not change. Calls go through the same
bounded hook queue as webhooks, so a slow provider fails one issuance and never builds a backlog.

## Approval

`PluginHost::load(bytes, approved)` (`load_with_config(bytes, approved, config)` for an install
with options) takes the capabilities the operator approved for the module's sha256. A module whose own declaration asks for more (a new trigger, `log`,
`state`, or a bigger state cap) fails with `CapsNotApproved`, so a module cannot grant
itself anything. It then runs with its own declaration, which is never wider than the
approved set.

## Limits

Every call runs in a fresh `Store` with a memory cap, an epoch deadline
(`Limits::call_timeout`; it restarts after each `http_request` that went to the network, since that wait is bounded by the 10 s / 20 s HTTP limits instead, so a call's total time can be about the 20 s HTTP budget plus up to `call_timeout` of guest execution per segment between requests, at most 8 requests), a table-element cap (`Limits::max_table_elements`, checked at instantiation and on growth) and the log and state-write limits above. Modules are capped at
8 MiB. A call returns its `Effects` (state writes, log lines) and applies nothing itself:
the caller commits them as one entry, which is how the controller will tag them with the
leader term (automation hooks spec, "Commit rule").

## Compile bounds and the compile pool

Before a module is compiled, `bounds::check` walks it with `wasmparser` and rejects
structure out of proportion to a real plugin (`ModuleError::TooComplex`, naming the
bound): functions, types, locals per function (1000) and in total (50 000), block nesting depth, declared table (at most `Limits::max_table_elements`, 10 000) and
memory sizes, globals, imports, exports and total code bytes (`Bounds`, set through
`Limits::bounds`). `CompilePool` runs `PluginHost::load` on a fixed set of worker threads
behind a bounded queue: a full queue answers `Busy` at once and a slow compile answers
`TimedOut` (the compile itself cannot be interrupted, so it keeps its worker until done).
The controller must load modules through the pool, never on its main loop. The workspace sets `panic = "abort"` for the release profile, so a panic in a compile worker (for example a wasmtime bug) ends the process rather than silently shrinking the pool; the bounds exist to keep hostile input away from that path.

A plugin's state holds at most 1024 keys (a `state_put` of a new key past that returns -1). A declared `state.max_bytes` above 1 MiB is rejected, and `PluginHost::new` refuses a zero
`call_timeout`.

## Checking a built plugin

`wayhouse-plugin-check <module.wasm> [config.json]` (crate `wayhouse-plugin-host`) loads the module with
its own declaration as the approved set, calls `init` and, if declared, `on_timer` once,
and prints what the host saw, including which capabilities were used. It exits 1 when the
host would reject the module, a call fails, or the plugin used a capability it did not
declare. Plugin repo CI is meant to run it on every build.

**Official plugins in this repository's CI.** `.github/pins/plugins.lock` pins a commit of
`wayhouse-proxy/plugins`. The `sniffers e2e` job builds the plugins at that commit and runs
`wayhouse-plugin-check` (built from the checkout under test) on each module, so a host change
that breaks an official plugin fails here and not at the next plugin release. A plugin built
for another ABI minor than the checkout's cannot load while the major is 0: that case is a CI
warning, which is what an ABI bump looks like until `.github/pins/plugins.lock` moves to the plugins
commit that re-pinned the ABI crate. Move the lock in a follow-up after the plugins PR merges.

## Installing a plugin (controller API)

Start a controller with `--plugins` (off by default; standalone or with `--ha-peers`). With
`--role slave`, or without the flag, `/plugins` answers `501` with the reason. The routes sit
behind the same admin bearer token as `/config`; `X-Actor` is recorded as `created_by`.

| Request                                                       | What it does                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| ------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `POST /plugins/modules` (raw `.wasm` body, up to 8 MiB)       | Validates ABI and capability sections, keeps the bytes by sha256, returns `{sha256, size, abi, capabilities, manifest}` for the operator to read.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| `POST /plugins` `{name, sha256, approved, config?, enabled?}` | Compiles the stored module through the bounded pool with `approved` as the operator-approved set; `422` when the module declares more than `approved` or fails a bound, `503` when the compiler is busy. Records the install and returns it (`201`).                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                          |
| `GET /plugins`, `GET /plugins/{id}`                           | The install records.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                          |
| `POST /plugins/{id}/enable`, `POST /plugins/{id}/disable`     | Flip `enabled`; a disabled install is not ticked, and enabling it restarts its first-tick wait.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               |
| `PUT /plugins/{id}/suspended` `{"suspended":["log",...]}`     | Switch capabilities off without touching the approval; the plugin's next call gets "not approved" for them, and `{"suspended":[]}` resumes it with no new approval. Names: `log`, `state`, `http`, `routes`, `port_routes` (triggers are switched off with `disable`). Suspending `routes` or `port_routes` withdraws the routes and ports the plugin already committed (the stored table is kept, so resuming brings them back); suspending `log` or `state` makes each tick fail, and those failures count like any other. Replicated under HA (plugin support level 6).                                                                                                                                                                                                                    |
| `PUT /plugins/{id}/config`                                    | Replace `config`: `{"config": {...}, "approved": {...}}`. The options are checked and the module is loaded with them first (`422` names the path). For a module with bound options an edit that moves the resolved capability set answers `409` with `"reason": "needs_approval"`, `approved` (current) and `effective` (new); a `409` with `"reason": "stale"` means the approved set moved under the edit; repeat the call with `approved` set to exactly the `effective` set to confirm it. `approved` otherwise absent or unchanged (`400`). Under HA a follower forwards it, every replica applies it as one entry, and the runner reloads the plugin on its next tick. Only the approved set is checked for staleness, so two concurrent edits that leave it alone are last-write-wins. |
| `GET /plugins/{id}/status`                                    | What the ticks did: `ticks`, `last_tick_unix`, `last_ok`, `last_error`, `consecutive_failures` and the last 100 log lines. In memory only; empty before the first tick.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       |
| `DELETE /plugins/{id}`                                        | Removes the install; the module blob goes with the last install that uses it.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                 |

Two more controller routes: `DELETE /plugins/modules/{sha256}` drops an uploaded module that no install uses, which is the way out of a full orphan cap (`507` on upload), and `GET /plugins/file/plan` is the dry run of the plugins file (see below). A webhook call may carry a path after the hook, `POST /plugins/{id}/hook/{*suffix}`, which the plugin sees as the request path.

An install has a random id (route ownership will be `plugin:<id>`), a name (`a-z`, `0-9`, `-`, up
to 64 characters), the approved capability set bound to the module sha256, and a non-secret
`config` (up to 64 KiB) that will be handed to `init`. Installs and blobs live in the
controller's `redb` database; a standalone controller keeps them there, an HA tier keeps them in
the Raft state machine's database, replicated (the blobs travel out of band, see "High availability").

## Ticks (standalone controller)

A scan runs once a second. An enabled install whose approved declaration has `on_timer`
is called every `tick_interval_secs`; the first tick comes one full interval after the
controller (or the enable) first sees the install, so enabling never runs a plugin inside
the request. The module is compiled on first use through the bounded pool, `init` runs
with the install's `config` each time the module is loaded (first tick after a controller
restart, after disable then enable, and after a failed load) against the state already stored,
so it must tolerate existing state, and every call runs on the pool: a busy or slow pool
skips the tick (shown in the status) and never blocks the controller; a skipped tick is retried at the next scan, not a whole interval later. One call per
install is in flight at a time.

A call returns its state writes; the controller applies them as one batch only if the
install's state revision is still the one the call read (compare-and-set), then bumps the
revision. A call that failed (trap, timeout, over a limit) commits nothing, is recorded in
the status, and the plugin is called again at its next interval. Deleting an install
removes its state; disabling keeps it, and enabling again resumes against it. `created_by` is a label taken from
the `X-Actor` header the caller sent, not a verified identity (the admin token is what authorises the call). This is the single-node form of the design's term-checked commit; the HA form is below.

## The plugins file (`--plugins-file`)

Instead of clicking, a controller can reconcile its installs with a YAML file (needs `--plugins`):

```yaml
plugins:
    - name: pelican-main
      sha256: 9f2c0000000000000000000000000000000000000000000000000000000041ab # the stored module
      enabled: true
      config:
          panel_host: panel.example.com
      approved_sha256: 41ab0000000000000000000000000000000000000000000000000000000041ab
```

`approved_sha256` is the SHA-256 of the capability set the options resolve to, as JSON. Do not
compute it by hand: `GET /plugins/file/plan` (the dry run, admin token) lists what a reconcile
would do and, for an entry whose hash is wrong, prints the real one in `effective_sha256`. The
file never carries module bytes, secrets or webhook tokens: upload the module (or fetch it from a
registry) first, and set secrets through the API.

- **Runs** at start, on `SIGHUP`, and every 30 seconds, so a module uploaded after the file was
  written is picked up. Under `--ha-peers` only the leader reconciles, after it has applied every committed entry; installing the `SIGHUP` handler means a hangup no longer ends the process; the writes go through Raft
  like any other install write.
- **Ownership.** The file only changes installs it created (their `created_by` is
  `plugins-file`). An install made in the UI or API is never changed, adopted or removed; an
  entry whose name clashes with one is reported as `refused`.
- **Safety.** A file that cannot be read or parsed changes nothing, and that includes an empty or comment-only file: the `plugins:` list is required, and `plugins: []` is how to remove every managed install on purpose. An entry is applied only if
  the module is stored and its approval hash matches; otherwise it is `refused` with the reason.
  Removing an entry removes its install. To change an entry's module, give the entry a new name
  (the old install is removed; the new one starts without the old one's secrets).
- **Read-only in the API.** Config edits, enable, disable and delete of a managed install answer
  `409` with `reason: managed_by_file`. Secrets, the webhook and suspending capabilities stay
  available.
  The web UI shows such installs with a "managed by file" badge, keeps their options read-only, and an admin can open "Show file plan" on the Plugins page for the dry run (`GET /plugins/file/plan`).

## The Plugins page (web UI)

`wayhouse-ui` proxies the controller's `/plugins` API (`/api/plugins*`, needs `--controller-url`). Listing is viewer-level; installing, editing and removing need the admin role. The Plugins page lists installs with their approved capabilities in plain words, and a "Port routes" table lists every entry port the plugins hold (entry port first, then key, protocol, backend and plugin); a notice names the keys still waiting for a free port. A controller that does not serve plugins (off, slave) shows its reason instead of a list.

The routes behind the page are the controller's, relayed by the UI under `/api/`. Reads need the viewer role, everything else the admin role.

| UI route                                                             | Controller route                  | What it does                                                                    |
| -------------------------------------------------------------------- | --------------------------------- | ------------------------------------------------------------------------------- |
| `GET /api/plugins/{id}`                                              | `GET /plugins/{id}`               | One install record.                                                             |
| `GET /api/plugins/{id}/status`                                       | `GET /plugins/{id}/status`        | Ticks, last error and the log.                                                  |
| `POST /api/plugins/modules`                                          | `POST /plugins/modules`           | Upload a module (raw body, up to 8 MiB).                                        |
| `PUT /api/plugins/{id}/config`                                       | `PUT /plugins/{id}/config`        | Replace the options, with the approval rules above.                             |
| `POST /api/plugins/{id}/enable`, `POST /api/plugins/{id}/disable`    | the same without `/api`           | Start or stop the ticks.                                                        |
| `GET /api/plugins/{id}/secrets`                                      | `GET /plugins/{id}/secrets`       | The secret slots and whether each is set. Never a value.                        |
| `PUT /api/plugins/{id}/secrets/{slot}`, `DELETE ...`                 | the same without `/api`           | Set or clear one write only secret.                                             |
| `POST /api/plugins/{id}/webhook`, `DELETE /api/plugins/{id}/webhook` | the same without `/api`           | Enable the webhook (the token is shown once) or revoke it.                      |
| `GET /api/plugins/file/plan`                                         | `GET /plugins/file/plan`          | The plugins file dry run.                                                       |
| `GET /api/plugins/secrets/keys`                                      | `GET /admin/plugins/secrets/keys` | The ids of the keys that seal plugin secrets, on every member. No key material. |
| `GET /api/plugin-routes`                                             | `GET /plugin-routes`              | The routes and ports the plugins publish.                                       |

"Install plugin" opens a wizard (`/plugins/new`): upload the module (up to 8 MiB), fill in its options (a form generated from the manifest's option schema; options marked `x-wayhouse-secret` are set under Secrets after the install), then see what the options resolve to (a `dry_run` install) and approve it in a prompt. Nothing is installed before the tick, and the UI approves exactly the set the controller resolved, never an edited one. An install's page (`/plugins/<id>`) has tabs for its options (an edit that moves the approved capabilities shows the old and the new set and needs a new tick), its secrets (write-only values, held reasons shown), its webhook (the token is shown once; enabling again rotates it) and its status and log. Registry fetch (P5): the UI can list a plugin registry (`GET /api/plugin-registries`, `GET /api/plugin-registries/{id}/plugins`, `POST /api/plugin-registries`) and fetch a module with `POST /api/plugin-registries/{id}/fetch {name, version?, allow_unsigned?}`. A fetch checks size, SHA-256, the ABI (0.2 exact) and, when the registry is official, the minisign signature, then only uploads the module to the controller; it never approves or installs. An unsigned module needs `allow_unsigned: true` (otherwise 409 `needs_confirm`). It is refused when the `registry_install` feature is off. Fetching and adding a plugin registry need the admin role, like the module upload itself.

In the wizard, "From a registry" replaces the file upload: it lists the plugins of a registry with the options each asks for (from the index's display hint) before anything is downloaded, marks a version "signature checked on fetch" (official registry, key configured) or "signature not verified", and greys out one built for another plugin ABI. "Fetch" asks you to accept the risk when the signature cannot be verified (a module from an unsigned or external registry; an external registry also gets its own warning). A fetched module continues at the options or the review step like an uploaded one, and the page says where it came from; the capability prompt is unchanged.

## High availability

With `--ha-peers` every replica holds the same installs and the same per-install state, in the
log and in snapshots (module blobs are in neither; see "Module bytes"). The entries are an install, enable or
disable, delete (state goes with it), and one state commit per plugin call. Applying is
deterministic and a refusal is a normal answer, never a storage error: an install whose id
exists, a malformed install (id, name, sha256, size, `created_by`, config size), a state commit
over the host's bounds, a stale revision, or a commit tagged with another term than the one the
entry was appended in changes nothing, on every replica alike. The plugin trees record the
highest Raft index applied, in the same transaction as the entry and restored from a snapshot, so
a replay after a crash or a snapshot install changes nothing.

**The API.** `GET /plugins` and `GET /plugins/{id}` answer from the replica that got the request.
Enable, disable and delete are proposed through Raft, and a follower forwards them to the leader
like the other write routes. An upload, an install and a status read run on the leader (a
follower forwards the whole request): the leader needs the module bytes to compile and check the
approval before it proposes the install, and only the leader has tick results. The leader checks
the record before proposing and every replica checks it again when applying.

**The tick.** Only the Raft leader ticks. A node that is not the leader drops its schedule,
its loaded plugins and its statuses; when it wins an election every install starts over with a
full interval of grace, so a new leader has applied the earlier terms' entries before it runs a
plugin. A tick's state writes are one `PluginState` entry tagged with the term the leader read the
state in and the state revision the call read: if leadership moved or another commit got in first,
the state machine drops it and the status says so. A commit is refused before it reaches the log
when over the host's state limits (1 MiB, 1024 keys, 256 writes per call). Status is in memory on the leader and starts
empty after a failover.

**Module bytes (slice 7).** The Raft log carries install records only; the module (up to 8 MiB)
is a content-addressed blob each replica keeps in its own store and moves over the peer channel
(`--ha-token`, the protocol gate):

- The leader pushes a module (`PUT /raft/plugin-blob/{sha256}`, verified against its hash on
  arrival) to every member and records the install only once a quorum of voters holds it, so an
  install cannot be stranded on a node that dies right after the upload. A quorum that cannot be
  reached answers `503`.
- Every replica fetches the modules its installs reference and it lacks (`GET
/raft/plugin-blob/{sha256}`, leader first, then the others, verified before it is kept) every
  few seconds: a replica that was down, joined late or restored a snapshot catches up this way.
  Until it has the module a node runs nothing for that install; a leader that cannot find one
  logs a repeated warning, since it would otherwise silently run nothing.
- Rolling upgrades: a new install is refused with `503` unless every member answers
  `/raft/whoami` with the current `plugin_support` level (4; a build before module replication
  leaves it out). The level is cluster-wide, not per feature: since port routes raised it from 3
  to 4, every new install and every webhook change waits for all members to be on a build at
  level 4, not only installs that declare `port_routes`. A member started without `--plugins`, or as a slave, reports `0`, so every
  replica needs `--plugins` before plugins are installed or their webhooks, secrets or key
  rotation change. A leader without `--plugins` would otherwise run no ticks and fail webhook
  forwards silently.
- The same gate covers secret changes, so setting, deleting or rewrapping a secret answers `409`
  while any cluster member (voter or learner) is unreachable. Revoking one credential therefore
  needs every member up; disabling or deleting the install still works as emergency containment. Existing installs keep working through an upgrade. Upgrade every replica before the first
  install; the gate does not cover a replica added later with an old build.
- An install that lands on a leader that has meanwhile lost leadership answers `503 retry` and is
  never forwarded for a second hop.
- A controller removes the module blobs no install references (uploads that were never installed,
  and cleanups a crash interrupted) once at startup, so upload and install belong in one sitting.
  Under HA the sweep waits until the node knows a leader and has applied its whole log, because a
  blob pushed for a committed install that is not replayed yet looks unused. Uploads no install
  uses are capped at 64 MiB in total (`507`), so an open controller cannot be filled with them.
- A delete that lands between an install's blob check and its commit removes the module on every
  replica; the leader puts it back and pushes it again once the install is recorded.

**Turning HA on.** Installs, state and modules of a controller that ran standalone are not carried into an HA tier (the tier keeps its own in the Raft state machine's database, and there is no pre-HA import for plugins): install them again.

**Snapshot restore.** A plugin snapshot is replaced in one transaction (installs, state and the
cursor together). The other stores' `replace_stores` write one database at a time and are left as
they are: a crash in between leaves the node's last applied index unchanged, so openraft
installs the snapshot again, and installing is a full replace.
