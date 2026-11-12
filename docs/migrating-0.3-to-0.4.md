# Migrating from 0.3.0 to 0.4.0

This is the operator's checklist for moving a running 0.3.0 deployment to 0.4.0. The general
rules for any upgrade (compatibility window, order of operations, draining a proxy) are in
[Upgrading a fleet](upgrading.md); this page covers what is specific to this release.

Every change is additive or opt-in except the plugin ABI (rebuild your plugins), the quoting rule
for tokens (see Secrets), the controller tokens (see below) and the aggregator's loopback rule (see Aggregator). On a single box, the [solo bundle](12-deployment.md#one-box-the-solo-bundle)
runs all of it with one command.

Opt-in: `--settings-dir` on each program lets the UI's Process flags page change their command-line
flags with a trial that goes back by itself ([how](12-deployment.md#process-flags-from-a-settings-file---settings-dir)).
Without the flag nothing changes.

## Upgrade order, in one place

1. Back up the controller's `--data-dir` (and, if you use sealed secrets, the key file: a lost
   key makes every stored secret unreadable).
2. Upgrade **every controller replica** (HA: one at a time, followers first). Until the last one
   runs 0.4.0 the leader refuses plugin installs, confirm-and-revert windows, stored UI data
   writes, feature-flag overrides, operator node tags and cluster secret writes (`409`).
3. Upgrade the aggregator, then the UI backend, then the proxies.
4. Only now write `schema_version: 4` into a config that uses `features:`, `enabled:` or
   `{ secret: name }`: an older proxy refuses it.
5. Rebuild and re-approve plugins (below).

## Enrollment codes and tunnel keys (new, opt-in)

`wayhouse-agent enroll` with a one-time code gives an origin a controller-made tunnel preshared
key ([how](12-deployment.md#enrolling-an-origin-agent-with-a-one-time-code)). Nothing changes
for an origin that does not enroll. Upgrade every controller replica first (the stored-data level
is now 7 and a mixed HA cluster refuses to mint or redeem), then every proxy, then enroll origins:
a 0.3.0 proxy ignores the key and cannot complete the handshake with an enrolled origin.
With `--auth-token`, an enrolled origin gets a token of its own (not the shared agent token), and
its key can be rotated without re-enrolling (`POST /peers/{name}/psk/rotate`, [how](11-backend-transport.md#rotating-the-key)).
An origin enrolled earlier keeps working on the shared token and cannot rotate until it
re-enrolls. Proxies, aggregators and UI processes enroll the same way (`wayhouse enroll`, `install --systemd`;
[roles and the step-up secret](12-deployment.md#enrolling-a-proxy-an-aggregator-or-a-ui)). Breaking
for the mint call: `POST /enrollments` takes `name` (the old `origin` still works) and a `role`.

## Aggregator: loopback `admin_url`s need `--instance-url-allow`

Breaking, security. Without `--instance-url-allow`, the aggregator used to accept an `admin_url`
whose host was the pushing connection's own IP. That included `127.0.0.1`: any local process could
push `http://127.0.0.1:<its own port>` and have the fan-out send the fleet-wide `--instance-token`
to it. The default rule now refuses a loopback host (`400` and a warning in the aggregator's log).

If your instances really are on the aggregator's own box (the solo bundle, a single-host compose),
list the range on purpose: `--instance-url-allow=127.0.0.0/8`. The solo systemd unit and both compose
files already do. Instances on other addresses are unaffected.

## Config schema

The config schema is now 4. Nothing changes for a document that does not use the new `features:`
section. To use it, set `schema_version: 4`, but only after every proxy that will receive the
document runs 0.4.0 (an older proxy refuses it).

## Plugin ABI 0.2 rebuilds

The plugin ABI is now 0.2. The host requires an exact `major.minor` match while the major is 0, so
a plugin built against ABI 0.1 is refused at upload:

```text
the plugin declares ABI 0.1, this host speaks 0.2
```

1. In the plugin's `Cargo.toml`, point `wayhouse-plugin-abi` at a 0.4.0 revision and rebuild.
   A plugin with literal capabilities needs no other change.
2. Upload the rebuilt module and approve it again: approvals are recorded against the module's
   sha256, so a rebuilt module is a new module.
3. Under HA, upgrade every controller before installing: the leader refuses plugin installs while
   a member reports a plugin support level below 5. The same holds for editing an install's
   config (`PUT /plugins/{id}/config`).

Plugins that are already installed keep their stored 0.1 module, which no longer loads: the
install stays listed, its status shows the ABI error as `last_error`, and it does not run. That
includes the ACME DNS plugin, so certificate issuance through it pauses until the rebuilt module is
installed. Install the rebuilt module, then delete the old install.

A plugin that wants operator-supplied values (a panel host, a node network) can now declare
options and capability bindings in a `wayhouse.plugin-manifest` section; see
[Install options and capability bindings](plugins.md#install-options-and-capability-bindings). The
official plugins (Pelican, `acme-dns-cloudflare`, `selftest`) are rebuilt in the plugins
repository.

## Config changes that can lock you out are applied on trial

A `POST /config` that changes `settings.admin.listen`, the admin token or TLS, `settings.gossip`
or a listener on the admin port now starts a 60 second window: unless you confirm the revision
(`POST /config/confirm/{revision}`), the controller re-submits the previous revision. A script
that makes such a change on purpose passes `confirm_within=0` (no window) or confirms. See
[Operations: confirm and revert](06-operations-observability.md). Under HA, upgrade every
controller first: a request that would start a window answers `409` while a member runs an older
build.

## Stored UI data

The controller now stores dashboards, custom themes, saved filters, the form layout override and the
deployment defaults (see [Stored UI data](ui-data.md)). Nothing to migrate. Under HA, upgrade every
controller before the first save: a write is refused (`409`) while a member runs an older build.

The web UI now opens on a **Dashboard** (a 12-column grid of widgets) instead of the Fleet page. A default dashboard shows until an operator customizes and saves one; nothing to migrate.

## Secrets

`settings.admin.auth_token` and `settings.gossip.psk` can be `{ secret: <name> }` (config schema 4).
The value lives sealed in the controller's cluster secret store (the Secrets page, or
`PUT /secrets/{name}`; names are `a-z`, `0-9` and `-`, values 8 to 4096 bytes) and is never
shown again. To use it:

1. Give the controller a key: `--secret-key-file <file>` (the same file as `--plugin-secret-key-file`,
   one base64 32-byte key per line, last one active, mode 0600). Without a key the controller
   refuses secret writes, and refuses plain HTTP on a non-loopback address once it holds a key
   (`--allow-insecure-secrets` overrides, for a private network).
2. Give every proxy that fetches its config from a controller the **same key file**
   (`--secret-key-file` or `WAYHOUSE_PLUGIN_SECRET_KEY`): the controller sends the value sealed and
   the proxy opens it. A proxy on a file config keeps its own store instead:
   `--secrets-file <path>` with `--secret-set NAME` (value on stdin), `--secret-delete NAME` and
   `--secret-list`.
3. Reference it: `auth_token: { secret: admin-token }`. The value is read once at start, so a
   rotated secret applies at the next restart. A reference that cannot be resolved stops the
   start with a message naming the secret; a reload does not check that it exists, so `PUT` the
   secret first and run `--check`.

Plaintext values still work and are warned about. An unquoted all-digit or `true`/`false` token
(`auth_token: 1234567890123456`) is now refused with a "must be quoted" error: add quotes.

## Plugin registry and the plugins file

The Plugins page can install from a plugin registry ("From a registry" in the install wizard). A
fetch checks size, SHA-256 and the ABI, and checks the minisign signature when the registry is the
official one (the official key is built in); an unsigned module needs an explicit confirm. A fetch
only uploads the module: capabilities are still approved by you. `registry_install` is a feature
flag, so it can be switched off.

Instead of clicking, a controller started with `--plugins --plugins-file plugins.yaml` can
reconcile its installs from a file (at start, on `SIGHUP`, every 30 s). The installs the file
creates are read-only in the UI and API (`409 managed_by_file`); the UI badges them and "Show file
plan" prints the dry run. Details: [plugins](plugins.md#the-plugins-file---plugins-file).

## Feature flags

A registry of named flags (`gossip_verdicts`, `sniffers`, `port_routes`, and others, see
[configuration](05-configuration.md)) with a `features:` config section and `GET /features` on the
proxy and the controller. Nothing to migrate: every flag defaults to on. Every flag is read by the
runtime (`wired` in `GET /features`); the emergency override layer (`PUT /features/overrides/{name}` on the controller) is documented in [configuration](05-configuration.md). Under HA, upgrade every controller before the first override (the UI data level is now 3, which also carries operator node tags).

## Parking items with `enabled: false`

Listeners, backend sources, resolvers, web sites and a listener's `rate_limit`, `geo` and
`per_source` blocks take `enabled: false` (config schema 4). The default is on, so nothing
changes for an existing file. Details in [configuration](05-configuration.md) ("Parking one
item"). Declare `schema_version: 4` only once every proxy that receives the document runs 0.4.0.

## Controller tokens: proxies and agents get their own

A controller started with `--auth-token` now refuses to start without a proxy token and an agent
token (`--proxy-token-file`, `--agent-token-file`; 16 bytes each, all three different). Proxies and agents used to present the admin
token, so one compromised edge box could submit config, install plugins and overwrite cluster
secrets. Now:

1. Generate two tokens (`openssl rand -hex 32`) and give every controller replica `--proxy-token-file`
   and `--agent-token-file` (see the next section: tokens are files now).
2. Point every `wayhouse` proxy's `--controller-token` and `--tunnel-controller-token` at the proxy
   token, and every `wayhouse-agent`'s `--controller-token` at the agent token.
3. Keep the admin token for the UI, operators, slave controllers (`--parent-token`) and
   `wayhouse-certsync`.

The compose files, systemd units, k8s manifests and `deploy/solo/init.sh` already do this; an
existing `.env` needs `WAYHOUSE_PROXY_TOKEN` and `WAYHOUSE_AGENT_TOKEN` added. The exact route lists
are in [deployment](12-deployment.md#controller-tokens-admin-proxy-agent). Upgrade and reconfigure the
controllers first, then move proxies and agents to their tokens.

## Deploy layout

`deploy/compose` gains overlays: `compose.solo.yml` (the controller with `--plugins` and a sealed
secret key created once by a `keys` step, the proxy with the same key, nothing published beyond
the host loopback), `compose.solo-acme.yml` (adds `--acme`, staging CA by default; including it is
your acceptance of the CA's terms) and the existing `compose.web.yml` and `compose.tunnel.yml`.
`wayhouse-controller init --target compose` (what `sh deploy/solo/init.sh` now calls) writes random
tokens and a UI password into `deploy/compose/.env` (mode 0600, never overwrites) and a private CA
with the controller's and aggregator's certificates into `deploy/compose/tls/`; the solo bundle
serves https from them. An existing solo `.env` has no `WAYHOUSE_TLS_DIR`: run `init` in a new
directory and start the bundle from it, or add the certificates by hand. Without Docker, `deploy/systemd/` has the same units and a
`wayhouse-solo.target`. Compose hands the tokens to each container as Docker secrets and the units
use `LoadCredential=`, so no token is on a command line. See [deployment](12-deployment.md).

## Tokens and passwords are no longer command-line flags (breaking)

`--auth-token`, `--proxy-token`, `--agent-token`, `--ingest-token`, `--metrics-token`, `--instance-token`, `--parent-token`,
`--ha-token`, `--controller-token`, `--aggregator-token`, `--tunnel-controller-token`,
`--plugin-overlay-token` and `--ui-password` are gone: a value on the command line is visible in
`ps`, `/proc/<pid>/cmdline` and `docker inspect`. Each has a `--<name>-file PATH` flag instead (for
example `--auth-token-file`). A program finds each secret `<name>` in this order:

1. the file given with `--<name>-file`,
2. `$CREDENTIALS_DIRECTORY/<name>` (what systemd's `LoadCredential=<name>:<path>` provides),
3. the environment variable `WAYHOUSE_<NAME>` (`WAYHOUSE_AUTH_TOKEN`, `WAYHOUSE_UI_PASSWORD`, ...).

A file's trailing newline is dropped, and an empty value is an error. The `-file` flags are not
read from a settings file, like the other credentials. To migrate, write each value to a file (mode
0400, readable by the service user) and replace `--auth-token=$X` with `--auth-token-file=/path`.
Docker secrets (`/run/secrets/<name>`), Kubernetes secret volumes and `LoadCredential=` all produce
such files. `wayhouse-web-sync` already read its tokens from the environment and is unchanged.

## A token over plain http to a remote host is refused (breaking)

`wayhouse`, `wayhouse-agent`, `wayhouse-aggregator` and `wayhouse-ui` used to warn and carry on
when a bearer token was configured for a plain `http://` URL on a non-loopback host
(`--controller`, `--aggregator`, `--plugin-overlay-url`, `--tunnel-controller-url`,
`--controller-url`, `--aggregator-url`, `--parent-url`). They now refuse to start: anyone on the
path could read the token and act as that process. Use an `https://` URL, or, on a network that is
private by construction, pass `--allow-insecure-controller` (a warning is logged at start). It can
also be set in a settings file, like the other non-secret flags. A URL to loopback, and a URL with
no token, are unaffected. The solo bundle (compose and systemd) needs no flag: `wayhouse-controller
init` made a CA and certificates, the controller and aggregator serve them and the clients trust the
CA (`--ca-file`). The Kubernetes manifests are https too: they mount the `wayhouse-tls` Secret from
`wayhouse-controller init --target k8s-secret` (**create it before applying; an existing 0.3 install must add it**),
and only the aggregator keeps the flag. The demo Compose file (`docker-compose.yml` alone, with its `change-me`
values) is plain http on a private network and keeps the flag; `compose.solo.yml` is its https form.

The same rule now covers the controller's own calls: `--parent-url` with a `--parent-token`, every
`--ha-peers` entry and every address given to `POST /admin/ha/members` (the Raft token goes there;
a bare `host:port` is plain http), and the aggregator's `admin_url`s: with an `--instance-token`
the aggregator rejects at ingest a plain-http `admin_url` that is not loopback, so such an instance
disappears from the fleet view until it reports an `https://` URL or the aggregator gets
`--allow-insecure-controller`. The controller has the same `--allow-insecure-controller` flag now.
The Kubernetes aggregator sets it (node admin URLs are http); the HA overlay uses `https://` peers and
needs no flag. An HA cluster across hosts should use `https://` peer addresses.

**An existing Kubernetes HA cluster does not carry over.** Raft stores the member addresses at bootstrap and `--ha-peers` is read only then, so a cluster bootstrapped with `http://` peers keeps them while the replicas now serve only https, and it will not re-form (a rolling update also loses quorum while pods mix both). Recreate the cluster (delete the StatefulSet and its PVCs) or re-add each member with `--ha-join` and its `https://` address.

## What works on one instance

Every 0.4.0 feature works on a single controller and a single proxy: feature flags and node tags,
`enabled: false`, sealed secrets (key file on both), the plugin registry and the plugins file,
the UI data store, confirm-and-revert. HA only changes the upgrade order above.
