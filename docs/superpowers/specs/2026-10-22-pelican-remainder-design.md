# Pelican plugin: remainder (live-panel check, webhooks, packaging)

Status: design note, no code. Refs #213. Builds on the [plugin system design](2026-10-07-plugin-system-design.md) and the [automation hooks addendum](2026-10-07-plugin-automation-hooks-design.md); where they say something, this note does not repeat it.

The first `pelican` plugin (wayhouse-proxy/plugins#7) is merged: a timer reconcile that lists servers and publishes `hostname -> node ip:port` routes. Three things remain.

## Requirement: one entry node

Requested by Wueschli: adding, moving or deleting a Pelican server needs no per-host DNS or firewall edit. A wildcard record (`*.mc.example.com`) points at the entry node; the plugin only publishes routes.

What this means in practice, and where it stops:

- The entry node must reach each node's allocation `ip:port` (private network, or a wayhouse tunnel). Pelican already needs those game ports open on the node, so the plugin adds no new rule, but "no firewall edits" holds only if the entry node is allowed to reach the nodes from the start. This is a documentation item, not code.
- Routing is by hostname, so it only works for protocols that carry one: HTTP, TLS (SNI) and games with a sniffer (for example Minecraft Java). UDP games without a hostname (Source, Valheim, ...) cannot be routed this way. Routing those by entry-node port needs a different route key and collides when two nodes both use the same port. **Not designed here**; it needs its own decision if wanted.
- Under HA the plugin runs on the leader only, so the entry node can be a wayhouse cluster.

## 1. Live-panel check (plugins#6, first item)

Nothing has run against a real panel; shapes come from Pelican's source. Plan:

- Verify against a containerised Pelican panel (SQLite, one node, a few allocations, servers created through the Application API), not a production panel. This also gives a repeatable **contract test** instead of a one-off check. Whether Wings is needed for server records to exist is part of the work; if it is, the check falls back to a panel Wueschli provides.
- Questions to settle: the lowest API-key permission level that can read servers and allocations; that `allocation` and `relationships.allocations` appear as read from `ServerData.php`/`AllocationData.php`/`Envelope.php`; what non-null `status` values exist and whether skipping all of them is right (plugins#8); which allocation is the public one when a server has several; whether node `fqdn`/ip semantics allow `node_addresses` to come from `GET /api/application/nodes` instead of config.
- Output: fixtures checked into the plugin, README claims changed from "unverified" to verified or corrected.

## 2. Webhooks (plugins#6, second item)

What is known from Pelican's source (`app/Models/WebhookConfiguration.php`, read 2026-10-22):

- Event names are generated as `eloquent.created|updated|deleted: App\Models\<Model>` for every model, so `eloquent.created: App\Models\Server` and `eloquent.deleted: App\Models\Server` exist. Allocation events exist the same way.
- A webhook configuration has `endpoint`, `headers`, `payload`, `events`, `type` and `scope`. Custom `headers` suggests an `Authorization: Bearer <token>` header can be set, which is what the wayhouse webhook listener expects. Unverified: whether headers are applied as stored.
- Not known: signing, retries, the exact payload for model events (the sample data in the model looks like a server record), and whether a transfer fires an `updated` event.

Design:

- The plugin declares `on_webhook` and treats any call as a **hint to reconcile now**. It does not trust the payload for routing; it re-lists servers. That keeps the plugin correct when events are missed, repeated or reordered, and means payload shape matters little. `on_timer` stays (the addendum requires it for event-driven plugins).
- A burst of events collapses into one reconcile (debounce, using the existing one-queue-slot rule so floods cannot starve the timer).
- Operator setup: enable the install's webhook (`POST /plugins/{id}/webhook` returns the token once), run the webhook listener (off by default, own bind and TLS), and create a Pelican webhook for the server events pointing at it with the bearer header. This must be documented step by step.
- The listener is a second exposed port on the entry node; the README states that it is optional and that without it the poll converges within one interval.

## 3. Packaging (wayhouse#386)

The plugin must be rebuilt per deployment because capability values (panel host, route hosts, backend networks) are fixed in the module. That blocks registry install and is a core ABI question tracked in wayhouse#386; it is not Pelican specific and is not duplicated here. The Pelican work only needs its outcome. Until then the README's build-your-own-caps flow stays.

## Order

1. Live-panel contract test (unblocks everything and settles plugins#8).
2. Webhook hint, after step 1 has confirmed event names and header behaviour.
3. Operator guide (reachability, wildcard DNS, webhook setup), written alongside step 2.
4. wayhouse#386 is independent and decided in core.

## Open for Wueschli

- Is a containerised panel enough as the "live" check, or should it also run once against a real panel?
- Is hostname-only routing acceptable for the first release of the plugin, with UDP-without-hostname games explicitly out of scope?
