# Plugins

A **plugin** is a WASM module that reacts to a trigger and acts through capabilities
the operator approved (design: [plugin system](superpowers/specs/2026-10-07-plugin-system-design.md)
and [automation hooks](superpowers/specs/2026-10-07-plugin-automation-hooks-design.md)).
Plugins are not [sniffers](sniffers.md): sniffers run on the proxy data path.

**Status.** The host runtime is built (`crates/wayhouse-plugin-host`) and a standalone
controller can store plugin installs behind an admin API and ticks the enabled ones
(below); the web UI has a Plugins page on top of the install API (see "The Plugins page"). An HA tier replicates installs and state through Raft, serves the same API on every replica
and ticks on the leader only, and the module bytes follow the installs to every replica (see
"High availability"). Secrets, `http`, `routes`, webhooks and events are built; `backends` is not.

## Module contract (ABI 0.1)

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
(`held: key <id> missing`) until it is put back. With `--acme` the account and certificate keys use the same keyring and are rewrapped by `POST /admin/acme/rewrap` ([acme.md](acme.md)).

## Routes

The `routes` capability lets a plugin publish hostname routes:

```json
"routes": { "hosts": ["*.mc.example.com", "play.example.com"], "backends": ["10.0.0.0/16"], "max_entries": 64 }
```

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
- Disabling or deleting an install withdraws its routes at once. A plugin whose ticks fail keeps its
  last committed set, and so does one that stops calling `routes_set`: the set stays until the next successful tick replaces it or the install is deleted or disabled. (The spec's grace period with an alert is not built; see the follow-up issue.)
- A controller started without `--plugins` answers `501` (`404` on an older build) and proxies drop
  the plugin routes at once, keeping their own config; a controller that is only unreachable leaves
  the last overlay in place. A proxy without `--controller` ignores the overlay.

## Port routes

Games on UDP have no hostname to route by. The `port_routes` capability lets a plugin route by
**entry port** instead (design: `docs/superpowers/specs/2026-10-22-port-based-plugin-routing-design.md`,
#403). The operator opens one port range once; the plugin never picks a port.

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
   trusted-proxy header. Put the webhook listener where callers reach it directly; there is
   no setting to relax the source limit.
2. The token is checked in constant time, against a dummy hash when the install is unknown,
   disabled or has no webhook. Every failure is the same `401`.
3. A per-install rate limit: `429`.
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
sooner. A backend-health event is not built yet.

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

`PluginHost::load(bytes, approved)` takes the capabilities the operator approved for the
module's sha256. A module whose own declaration asks for more (a new trigger, `log`,
`state`, or a bigger state cap) fails with `CapsNotApproved`, so a module cannot grant
itself anything. It then runs with its own declaration, which is never wider than the
approved set.

## Limits

Every call runs in a fresh `Store` with a memory cap, an epoch deadline
(`Limits::call_timeout`), a table-element cap (`Limits::max_table_elements`, checked at instantiation and on growth) and the log and state-write limits above. Modules are capped at
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

`wayhouse-plugin-check <module.wasm>` (crate `wayhouse-plugin-host`) loads the module with
its own declaration as the approved set, calls `init` and, if declared, `on_timer` once,
and prints what the host saw, including which capabilities were used. It exits 1 when the
host would reject the module, a call fails, or the plugin used a capability it did not
declare. Plugin repo CI is meant to run it on every build.

## Installing a plugin (controller API)

Start a controller with `--plugins` (off by default; standalone or with `--ha-peers`). With
`--role slave`, or without the flag, `/plugins` answers `501` with the reason. The routes sit
behind the same admin bearer token as `/config`; `X-Actor` is recorded as `created_by`.

| Request                                                       | What it does                                                                                                                                                                                                                                         |
| ------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `POST /plugins/modules` (raw `.wasm` body, up to 8 MiB)       | Validates ABI and capability sections, keeps the bytes by sha256, returns `{sha256, size, abi, capabilities}` for the operator to read.                                                                                                              |
| `POST /plugins` `{name, sha256, approved, config?, enabled?}` | Compiles the stored module through the bounded pool with `approved` as the operator-approved set; `422` when the module declares more than `approved` or fails a bound, `503` when the compiler is busy. Records the install and returns it (`201`). |
| `GET /plugins`, `GET /plugins/{id}`                           | The install records.                                                                                                                                                                                                                                 |
| `POST /plugins/{id}/enable`, `/disable`                       | Flip `enabled`; a disabled install is not ticked, and enabling it restarts its first-tick wait.                                                                                                                                                      |
| `GET /plugins/{id}/status`                                    | What the ticks did: `ticks`, `last_tick_unix`, `last_ok`, `last_error`, `consecutive_failures` and the last 100 log lines. In memory only; empty before the first tick.                                                                              |
| `DELETE /plugins/{id}`                                        | Removes the install; the module blob goes with the last install that uses it.                                                                                                                                                                        |

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

## The Plugins page (web UI)

`wayhouse-ui` proxies the controller's `/plugins` API (`/api/plugins*`, needs `--controller-url`). Listing is viewer-level; upload, install, enable, disable and delete need the admin role. The page lists installs with their approved capabilities in plain words. "Upload plugin" sends the module (up to 8 MiB) to the controller, shows what it declares and installs it only after the operator ticks the approval box; the UI approves exactly the declared set, never an edited one. Below the installs, a "Port routes" table lists every entry port the plugins hold (entry port first, then key, protocol, backend and plugin), and a notice names the keys still waiting for a free port. A controller that does not serve plugins (off, slave) shows its reason instead of a list. Not in the page yet: registry install, secrets, config editing.

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
  `/raft/whoami` with the current `plugin_support` level (3; a build before module replication
  leaves it out). A member started without `--plugins`, or as a slave, reports `0`, so every
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
