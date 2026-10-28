# Migrating from 0.3.0 to 0.4.0

Breaking changes in 0.4.0 and what to do about each. Nothing here needs a data migration.

## Plugin ABI 0.2: rebuild your plugins

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
   a member reports a plugin support level below 5.

A plugin that wants operator-supplied values (a panel host, a node network) can now declare
options and capability bindings in a `wayhouse.plugin-manifest` section; see
[Install options and capability bindings](plugins.md#install-options-and-capability-bindings). The
official plugins (Pelican, `acme-dns-cloudflare`, `selftest`) are rebuilt in the plugins
repository.
