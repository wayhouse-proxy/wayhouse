# Migrating from 0.2.0 to 0.3.0

This is the operator's checklist for moving a running 0.2.0 deployment to 0.3.0. The general
rules for any upgrade (compatibility window, order of operations, draining a proxy) are in
[Upgrading a fleet](upgrading.md); this page covers what is specific to this release.

**The one step you cannot skip:** the controller's embedded database changed from `sled` to
`redb`, and there is **no migration**. A 0.3.0 controller refuses to start on a data
directory written by 0.2.0. You move that directory aside and start empty, then restore what
you need by hand ([Controller storage](#controller-storage-sled-to-redb)). Everything else in
0.3.0 is additive and opt-in.

## At a glance

| Area                      | 0.2.0         | 0.3.0                                                     | What you do                                                                                                                  |
| ------------------------- | ------------- | --------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------- |
| Controller database       | `sled`        | `redb` (`db.redb` in each database directory)             | **Breaking.** Move the old data directory aside, start empty, restore by hand ([below](#controller-storage-sled-to-redb)).   |
| Protocol                  | 1.1           | 1.2                                                       | Nothing: minors are additive, N and N-1 work together.                                                                       |
| Config schema             | 2             | 3 (adds `web:`)                                           | Upgrade every proxy before submitting a document that uses `web:` ([Config](#config)).                                       |
| Store format marker       | 1             | 1                                                         | Unchanged, but the engine under it changed. The marker does not detect the sled to redb switch; the directory check does.    |
| Plugin ABI                | 0.1           | 0.1                                                       | No bump. New imports are additive; the host still requires an exact minor match ([Plugins](#plugins)).                       |
| Sniffer ABI               | 0.1           | 0.1                                                       | Nothing.                                                                                                                     |
| Plugin support level (HA) | 3             | 4                                                         | Under HA, plugin installs and webhook changes wait until every member runs 0.3.0 ([HA](#ha-controllers)).                    |
| Container images          | six           | seven (adds `wayhouse-caddy`)                             | Pull `wayhouse-caddy` only if you use `web:`. The `wayhouse` image also carries `wayhouse-web-sync` and `wayhouse-certsync`. |
| Web sites through Caddy   | not available | `web:` config, `wayhouse-web-sync`, `wayhouse-caddy`      | Opt-in ([Web proxy](web-proxy.md)).                                                                                          |
| ACME certificates         | not available | controller `--acme`, `wayhouse-certsync`                  | Opt-in ([ACME](acme.md)).                                                                                                    |
| Plugin port routes        | not available | `port_routes` capability, `plugin_routes: { type: port }` | Opt-in ([Port routes](plugins.md#port-routes)).                                                                              |

Nothing in this release changes the data path's wire behaviour, the gossip frame
(`GOSSIP_VERSION` is unchanged, unlike 0.1.0 to 0.2.0) or the sniffer ABI.

## Before you start

Do these while the 0.2.0 controller is still running. All of them are plain reads.

1. **Save the current config.** `GET /config` returns the raw text of the active revision
   (header `X-Config-Revision`). Keep that file; it is what you submit again after the move.
   `GET /config/revisions` lists the history if you want to keep older revisions too; they
   are not restored by any command, you would have to submit them again yourself.
2. **Note the plugin installs, if you use `--plugins`.** `GET /plugins` lists every install
   with its name, module sha256, approved capabilities and `config`. Keep the module files.
   A secret value can never be read back, so have the original values (for example the
   Pelican API key) ready. Keep the **secret key file** (`--plugin-secret-key-file`, now also
   `--secret-key-file`) as well: it is not part of the database and you reuse it.
3. **Note the tunnel registrations, if you use the tunnel.** `GET /peers`, `GET /proxy-peers`
   and `GET /tunnel/addresses` show who is registered with which address.
4. **Stop depending on operator intent.** Drains, runtime backend edits and route hints sent
   through the controller's intent log are lost with the database. Reapply any you still need
   once the new controller is up.
5. **Pick a window.** Proxies and agents keep running on their last known configuration
   while the controller is down, so a short controller gap does not interrupt players. New
   registrations and config changes wait until the controller is back.

## Controller storage: sled to redb

0.3.0 stores everything the controller keeps in `redb` (pure Rust, one `db.redb` file per
database directory). Why: `sled` has had no stable release since 2021 and pulled in
unmaintained crates; the decision is ADR 32 in
[technology choices](09-technology-choices.md). Nothing was in production, so no importer
was written.

A directory counts as a `sled` database when it holds a `conf` or `db` entry and no
`db.redb`. A 0.3.0 controller that finds one stops with:

```text
<path> holds a sled database from a wayhouse build before 0.3.0; this build stores its data in redb and cannot read it (there is no migration): move the directory aside or delete it
```

It never changes the old directory, so you can go back to 0.2.0 with it (see
[Rollback](#rollback)).

### What is lost, and how you get it back

The controller's `--data-dir` holds the root config store plus these subdirectories:
`intent`, `peers`, `proxy-peers`, `tunnel-addresses` and, under HA, `ha`. Plugin installs,
their sealed secrets and state live in the root store (standalone) or in `ha` (HA). Moving
the whole `--data-dir` aside is the simplest correct step, and the table is what that
costs.

| What was in the old database                                               | After starting empty                                                                                                                                                                                                                                                                |
| -------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Config revisions and the `current` pointer                                 | Gone, including history. The controller serves no config until you `POST /config` the file you saved. Revision numbers start again at 1.                                                                                                                                            |
| Operator intent log (drain, backend edits, route hints)                    | Gone. Reapply what you still need.                                                                                                                                                                                                                                                  |
| Backend and proxy registrations (`peers`, `proxy-peers`)                   | Rebuilt by themselves: every agent and tunnel proxy re-registers on its next cycle (default 30 s) and reappears in the registry.                                                                                                                                                    |
| Tunnel address book                                                        | Allocated afresh as owners re-register, so an owner can end up with a different address than before unless it pinned one (a pin is granted only if free). The interface moves to the new address without a restart and the other side follows ([docs/11](11-backend-transport.md)). |
| Plugin installs, approvals, plugin `state`, sealed secrets, webhook tokens | Gone. Upload and install each plugin again, approve its capabilities, set its secrets again and re-enable its webhook (a new token is shown once). The secret key file you kept is reused.                                                                                          |
| Raft log and cluster state (HA)                                            | Gone with the directory. The cluster is initialised again ([HA controllers](#ha-controllers)).                                                                                                                                                                                      |
| ACME sites, account, certificates                                          | Nothing to lose: ACME is new in 0.3.0.                                                                                                                                                                                                                                              |
| Proxy, agent and aggregator state                                          | Untouched. Agents keep their WireGuard key and saved address file; the aggregator holds nothing on disk. Only the controller used `sled`.                                                                                                                                           |

**Restart every proxy that follows the controller.** A proxy remembers in memory how far it
has read the controller's revision stream and asks only for newer revisions. A proxy that
stayed up through the move would ignore a fresh controller's revisions 1, 2, 3 until the
numbers pass the old position. Upgrading a proxy to 0.3.0 restarts it, which resets that
position and fetches the current revision at startup, so do the proxy upgrade after the
controller is restored and you need nothing extra.

### Standalone controller

1. Complete [Before you start](#before-you-start).
2. Stop the 0.2.0 controller.
3. Move the data directory aside (do not delete it yet):

    ```sh
    mv /var/lib/wayhouse/controller /var/lib/wayhouse/controller.sled-0.2.0
    ```

    On Docker or Kubernetes this means pointing the volume at a new empty volume or
    directory, or removing the old one's contents after copying them.

4. Start the 0.3.0 controller with the same flags as before and the same `--data-dir`
   path (now empty). It creates `db.redb` files.
5. `POST /config` with the file you saved.
6. If you use plugins, install them again and set their secrets.
7. Upgrade proxies and agents ([Order of operations](#order-of-operations)).
8. When everything looks right, delete the `.sled-0.2.0` copy. Keep it until then: it is
   your rollback.

### HA controllers

No rolling path has been tested for this change. The simplest route, since the whole tier
starts empty anyway, is to stop and restart the tier together:

1. Complete [Before you start](#before-you-start) on any member.
2. Stop **every** controller of the tier.
3. On **every** node move its `--data-dir` aside as above.
4. Start the nodes on 0.3.0 with the same `--ha-node-id`, `--ha-peers`, `--ha-token` and
   `--tunnel-network` as before (a fresh cluster initialises exactly like a first install,
   see [deployment](12-deployment.md)).
5. `POST /config`, reinstall plugins and secrets, as for a standalone controller.

Two facts that matter here:

- The sled-shaped data cannot be handed from an old node to a new one, so mixing old and new
  members in one Raft group is not a goal. A 0.3.0 member cannot open its old directory.
- The documented node replacement (remove the member, start a node with an empty directory
  and `--ha-join`, add it; it catches up from the leader's snapshot) is the mechanism a
  rolling attempt would use. It is not covered by a test against 0.2.0 leaders, so treat it as
  a rehearsal if you try it. Because the data is not migrated, plan for the tier to be empty
  of everything outside the Raft group.

Under HA the new **plugin support level is 4**. A leader refuses a new plugin install or
webhook change with `503`, and does not commit a port route set, until every member answers
`/raft/whoami` with level 4; a member that does not answer blocks too. ACME writes have the same kind of gate.
Finish upgrading every replica before you touch plugins or turn on ACME
([Plugins](plugins.md#high-availability)).

## Order of operations

The general order in [Upgrading a fleet](upgrading.md#order-of-operations) still applies. For
this release:

1. Aggregator and UI. Nothing stateful; they only need their new image.
2. Controllers: the storage step above. Leaf `slave` tiers first, root last, each tier
   completely before its parent.
3. Proxies and agents, a few at a time (drain, upgrade, rejoin). Proxies must be restarted
   after the controller was reset, see above.
4. New config fields and new features, **only after every proxy runs 0.3.0**: `web:`,
   `plugin_routes: { type: port }`, and anything that depends on them.

## Config

- **`schema_version: 3` and `web:`.** The `web:` section is a schema 3 field. A 0.2.0 proxy
  cannot parse it. The controller refuses (`422`, naming the lowest live proxy maximum) a
  document that needs schema 3 while any live proxy still reports 2, so submit it after the
  last proxy is upgraded.
- **`plugin_routes: { type: port }` is not gated by the schema version.** The field
  `plugin_routes` is schema 2, and a 0.2.0 proxy only knows `type: sni` and `type: sniffer`
  on TCP listeners. A document with `type: port` (or `plugin_routes` on a UDP listener) is
  rejected by a 0.2.0 proxy, which keeps its previous configuration and logs why. Upgrade
  every proxy first; the controller does not check this for you.
- **Nothing existing is removed or renamed** in the config schema between the two tags, so a
  0.2.0 config should load unchanged on 0.3.0. Run `wayhouse --check` on it before the
  upgrade to be sure.
- `config.example.yaml` has a commented `web:` block as a starting point.

## Web sites through Caddy (new, opt-in)

Skip this unless you want to publish HTTP(S) sites through the same entry node.

- Add the `web:` section to the config (schema 3), a `web` pool with `proxy_protocol: v2`
  and a listener (or `sni` route) that sends 80 and 443 to it. The fields are in
  [Configuration](05-configuration.md); the behaviour is in [Web proxy](web-proxy.md).
- Run **`wayhouse-web-sync`** (a binary inside the `wayhouse` image, started with
  `entrypoint: /usr/local/bin/wayhouse-web-sync`) next to `wayhouse` and a Caddy. Without
  a `web:` section it sends nothing and Caddy keeps its base configuration.
- Use the **`wayhouse-caddy`** image (stock Caddy 2.11.7 built with a patched Go and
  `golang.org/x/net`, uid 65532, high ports, no capabilities) rather than the upstream
  `caddy` image. Recipes: [`deploy/compose/compose.web.yml`](../deploy/compose/compose.web.yml),
  [`deploy/k8s/60-web-gateway.yaml`](../deploy/k8s/60-web-gateway.yaml).
- Caddy's own ACME is off; certificates come from files or from controller ACME
  (`wayhouse-certsync`). Running Caddy's built-in ACME next to the sync is not supported.
- First release is hostname-only routing; the real WireGuard path through Caddy is covered by
  the tunnel CI job, not by the Caddy end-to-end test.

## ACME certificates (new, opt-in)

Skip this unless you want the controller to issue certificates.

- Off by default. Start the controller with `--acme --acme-ca <letsencrypt-staging|letsencrypt|URL>
--acme-accept-tos`, plus `--plugins` and a secret key file (`--secret-key-file`, alias of
  `--plugin-secret-key-file`). `--acme-ca` has no default on purpose; try
  `letsencrypt-staging` first. CAs that need external account binding take
  `--acme-eab-kid` and `--acme-eab-hmac-key-file`.
- DNS-01 goes through a DNS-provider plugin using the `on_acme_dns` trigger. The first one is
  `acme-dns-cloudflare` in the plugins repository. HTTP-01 is also supported, with
  `wayhouse-web-sync --acme-challenge-upstream` forwarding the challenge path.
- Gateways fetch certificates with **`wayhouse-certsync`** (binary in the `wayhouse` image,
  started with `entrypoint:`) and a per-gateway token shown once by `POST /acme/gateways`.
- The `acme` cargo feature of `wayhouse-controller` is on by default; build with
  `--no-default-features` to leave issuance out. The ACME routes then answer that ACME is
  not enabled, and `--acme` refuses to start.
- **Key rotation covers the ACME keys.** Rotating the cluster secret key re-encrypts plugin
  secrets with `POST /admin/plugins/secrets/rewrap`; the ACME account and certificate keys are
  re-encrypted by a separate `POST /admin/acme/rewrap`. Do both before you remove the old key
  from the key file. (A bug that left the ACME keys behind was fixed before this release, #413.)
- **Bundle endpoint rate limit.** `GET /acme/bundles` and `GET /acme/bundles/{site}` count
  requests per source address (an IPv6 address by its /64) before the token is checked: a
  source that used its burst of 20 and then asks faster than 5 per second gets `429` with
  `Retry-After: 1`. A gateway polling about once a minute never notices;
  `wayhouse-certsync` tries the next controller on a `429`. Controllers behind a reverse proxy
  or NAT share one budget per address, see [acme.md](acme.md).
- A plain-HTTP, non-loopback controller listener refuses to start with `--acme` unless you
  pass `--allow-insecure-secrets` (the flag now covers `--acme` as well as `--plugins`
  secrets). Serve TLS instead.

Full reference and operations: [Certificates (ACME)](acme.md).

## Plugins

- **The plugin ABI is still 0.1.** The host requires an exact minor match while the major is
  0, so a module built for ABI 0.1 keeps loading on 0.3.0. The new imports
  (`port_routes_set`, `port_routes_get`, `acme_dns_result`) and the new `on_acme_dns`
  trigger are additive: modules that do not use them are unaffected, and a module that
  imports them fails to link on an older host and is refused at install, with no ABI bump.
- **Port routes** (for games without a hostname, UDP included): an operator approves a
  `port_routes` capability (ports, protocols, backend networks, `max_entries`); the **host**
  allocates the entry port per stable key, keeps it stable when the backend moves and
  honours an optional `prefer` hint when it is free and inside the approval. A port a key
  releases, or a deleted install releases, is quarantined for **24 hours**. Entries that find
  no free port are listed as `exhausted`. The Plugins page of the UI lists every entry port,
  entry port first. On the proxy side a listener opts in with
  `plugin_routes: { type: port }` over a port range (one listener per protocol); you open
  that range in the firewall once. Details: [Port routes](plugins.md#port-routes) and the
  [design](superpowers/specs/2026-10-22-port-based-plugin-routing-design.md).
- **Pelican plugin** (plugins repository): changes since 0.2.0 are optional port routes (the
  server's uuid is the key, the allocation port the `prefer` hint), an optional
  `backend_ports` limit in the install's `config` (set it: `routes.backends` limits addresses,
  not ports), a re-read when the panel's server total changes between pages, and an optional
  webhook that only triggers an immediate reconcile. The plugin is built **per deployment**
  from your own `caps.json`; see its README. A Pelican module that declares `port_routes`
  needs a 0.3.0 host.
- **Secret key flag name.** `--secret-key-file` is the new preferred name of
  `--plugin-secret-key-file`; the old name, and `WAYHOUSE_PLUGIN_SECRET_KEY`, keep working.

## Images and binaries

- The `wayhouse` image now also contains `wayhouse-web-sync` and `wayhouse-certsync`. They do
  nothing unless you start them (the image's entrypoint is still `wayhouse`).
- New image `ghcr.io/wayhouse-proxy/wayhouse-caddy` (see above). A release publishes **seven**
  multi-arch images; they are public and need no login.
- `BIN_SOURCE=prebuilt` builds expect `wayhouse-web-sync` and `wayhouse-certsync` in
  `deploy/prebuilt/` as well.

## Smaller changes you may notice

- **Dependencies patched.** The `rustls` and `wasmtime` advisories were fixed and the
  Caddy image is built with a patched Go toolchain. No operator action.
- **UDP send path.** The per-call heap allocations on the UDP send path were removed
  (a performance change, no config or behaviour change).
- **`wayhouse-web-sync` refuses an insecure admin token.** With a token set, a plain-http,
  non-loopback `--wayhouse-admin` is refused unless `--allow-insecure-secrets` is given.
- **Hostnames in `web.sites[].hosts`** get the usual 63 and 253 character and hyphen-edge
  rules, and a site has at most 100 names.
- **ACME CA hosts with a label starting `test`** (such as `test-api.pki.goog`) count as
  untrusted like `staging`.
- **Certsync** no longer downgrades to an older certificate, fetches a deleted or truncated
  PEM again, and runs `--reload-cmd` once at startup.

## Rollback

Going back to 0.2.0 is possible only if you kept the old data directory:

1. Stop the 0.3.0 controller.
2. Move its (redb) data directory aside and put the `.sled-0.2.0` directory back.
3. Start the 0.2.0 controller. Anything submitted to the 0.3.0 controller, such as config
   revisions, registrations, plugin installs or ACME state, is not in that copy and has to be
   redone.
4. Remove `web:`, `plugin_routes: { type: port }` and anything else new from the config
   before a 0.2.0 proxy receives it; older proxies reject what they cannot parse and keep
   their previous configuration.

I could not run a 0.2.0 binary against a directory that 0.3.0 created, so the reverse
direction (an old build pointed at a redb directory) is not described: always go back through
the saved sled directory.

## Troubleshooting

| You see                                                                                        | It means                                                                    | What to do                                                                                                                        |
| ---------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------- |
| `<path> holds a sled database from a wayhouse build before 0.3.0 ... move the directory aside` | The controller found a 0.2.0 data directory (`conf` or `db`, no `db.redb`). | Follow [Controller storage](#controller-storage-sled-to-redb). Never delete it before the new controller is restored and checked. |
| `could not acquire lock: <path> is open elsewhere`                                             | Another controller process holds the redb file open.                        | Stop the other process; there is one writer per database.                                                                         |
| The controller is up but proxies run an old config and ignore new revisions                    | The proxy kept its old position in the revision stream.                     | Restart the proxy ([why](#what-is-lost-and-how-you-get-it-back)).                                                                 |
| `GET /config` answers `404` after the move                                                     | The new controller has no config yet.                                       | `POST /config` with the file you saved.                                                                                           |
| `422` naming a proxy's maximum on `POST /config` with `web:`                                   | A live proxy still reports config schema 2.                                 | Upgrade that proxy first.                                                                                                         |
| A 0.2.0 proxy logs a `plugin_routes` error and keeps its config                                | The config uses `type: port`, which only 0.3.0 parses.                      | Upgrade every proxy before using it.                                                                                              |
| `503` on a plugin install or webhook change under HA                                           | A member does not report plugin support level 4 (older build or no answer). | Finish upgrading the tier, then retry.                                                                                            |
| Plugin installed but doing nothing after the move                                              | Installs and secrets did not survive.                                       | Reinstall, approve, set the secrets again.                                                                                        |
| `429` with `Retry-After: 1` on `/acme/bundles`                                                 | One source address used up its request budget.                              | Poll less often, or expect it when several gateways share one NAT address.                                                        |
