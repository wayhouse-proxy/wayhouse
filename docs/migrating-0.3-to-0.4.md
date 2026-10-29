# Migrating from 0.3.0 to 0.4.0

This is the operator's checklist for moving a running 0.3.0 deployment to 0.4.0. The general
rules for any upgrade (compatibility window, order of operations, draining a proxy) are in
[Upgrading a fleet](upgrading.md); this page covers what is specific to this release.

> **Skeleton.** The 0.4.0 slices fill in their own sections as they land. Nothing is
> final until the release notes say so.

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

Not yet filled in.

## Feature flags

A registry of named flags (`gossip_verdicts`, `sniffers`, `port_routes`, and others, see
[configuration](05-configuration.md)) with a `features:` config section and `GET /features` on the
proxy and the controller. Nothing to migrate: every flag defaults to on. Flags are reported before
the runtime reads them (`wired: false`); the emergency override layer (`PUT /features/overrides/{name}` on the controller) is documented in [configuration](05-configuration.md). Under HA, upgrade every controller before the first override (the UI data level is now 3, which also carries operator node tags).

## Deploy layout

Not yet filled in.
