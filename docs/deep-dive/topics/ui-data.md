---
title: "Stored UI data: dashboards, themes and filters"
description: How the controller keeps the small records the web console shares between operators, why it checks shape and not meaning, how the same replicated area also holds secrets, codes and overrides, and why theme contrast is checked on one node only.
---

The operator console shows dashboards, themes and saved filters, and every operator of a deployment should see the same ones. Keeping them in each browser would give each operator a different console, and a database of their own would be one more thing to run. They live in the controller instead. This page is about that area: what it stores, the rules on writes, and one subtle rule about colour maths and replication. The record formats and limits are in the [Manual](../../manual/index.md) and the generated [Reference](../../reference/index.md). The controller basics are in [the controller core](controller-core.md).

## The problem

1. **Shared state without a new service.** The records are small and rare to change, and the controller already has a replicated store.
2. **The controller must not know the UI's meaning.** A widget id or a design token belongs to the UI, and the UI changes faster than the controller.
3. **Records are untrusted input.** Anyone with an operator role can write one, so each needs a shape check, a size limit and a count limit.
4. **Replicas must agree.** In a cluster, applying the same log entry must produce the same state on every node, whatever CPU it runs on.
5. **Concurrent editing.** Two operators editing one dashboard should not overwrite each other silently.

## One replicated key value area

The records are kept as **named values of a few kinds** in one replicated key value area. The kinds are dashboards, themes, saved filters, the single form layout override, the deployment defaults, operator tags for one node, and also three records that have nothing to do with the UI itself and use the same machinery: feature flag overrides (see [runtime feature flags](runtime-feature-flags.md)), cluster secrets as ciphertext only (see [secrets](secrets.md)) and enrollment codes and origin tokens as hashes only (see [enrollment](enrollment.md)).

A write is an operation: put, delete, rename a dashboard, re encrypt a secret under a newer key, or spend one use of an enrollment code. A standalone controller applies operations directly. A cluster proposes them through Raft, and the state machine applies them **crash idempotently per Raft index**, the same way as the config store and the ACME records (see [controller HA](controller-ha.md)).

Operations that must be atomic are single entries. Renaming a dashboard moves the default that points to it in the same step, so a failed second write cannot leave both names. Re encrypting a secret applies only if the secret still holds the ciphertext it started from, so a `PUT` that landed meanwhile wins. Spending a code's use is applied atomically, so a single use code redeems exactly once however many nodes race.

## Shape, not meaning

The controller stores JSON and checks its **shape, size and names**. What a widget id or a token means is the UI's business. An unknown widget id is therefore kept and the UI shows a placeholder. The effect is that a newer console can write a widget an older controller has never heard of. The same split keeps the roles out of the controller: it checks only its bearer token, and the UI backend, which holds that token, applies viewer, operator and admin rules.

Each record has limits. A dashboard is at most 64 KiB and 100 of them are allowed, a theme 8 KiB and 50, a saved filter 4 KiB and 200, the form layout 64 KiB and one, the defaults 1 KiB and one, and node tags 4 KiB and 1,000. Names are 1 to 64 characters with no `/` or control characters. A saved filter may hold only the fleet filter fields, and any other key is refused. A dashboard holds at most 100 widgets, each with a width from 1 to 12, because the grid has 12 columns. Errors name the path, for example `widgets[0].width must be an integer from 1 to 12`.

Nothing here is a secret and the documentation says so: credentials do not belong in a dashboard, a filter or a layout.

## Optimistic concurrency

A dashboard carries a **revision** that the controller counts up on every save. A `PUT` or rename that sends the revision it was edited from is refused with `409` and the code `stale` when someone saved since, and one without it, an import, overwrites. The answer to a `PUT` that sent a revision carries the next one. A dashboard saved before revisions existed reads as revision 0. This is the same compare and swap idea as the config file write in [the admin API](admin-api.md), at a smaller scale.

## Defaults stay consistent

Deleting the dashboard or the theme that is the deployment default **clears that default on every replica**, so the console never points at something that is gone. A theme may not be named `light`, `dark` or `system`, which are built in, and the default theme may be one of those or a saved one.

## Colour maths on one node only

A theme is held to ten contrast checks, such as text on its background needing at least 4.5 to 1, and a minimum CIEDE2000 colour distance between pairs that must stay apart. A theme that fails any check is refused with `422`, the first failure in words and a list of every failed check with its pair, value and requirement.

The checks run in the controller, as a port of the web validator, and both pass the same shared test cases, so the browser and the controller agree on what fails. The subtle part is **where** they run. They run on the node that takes the request, and the leader checks again before it proposes. They never run when a replica applies the log entry. The reason is in the module's header: the colour maths uses `powf`, `cbrt` and trigonometry, whose last digits may differ between CPU architectures, and a mixed fleet must not disagree on what a log entry did. A check on apply could make an x86 node accept an entry that an ARM node rejects, and the replicas would diverge. Checking before the proposal and trusting the entry afterwards keeps application deterministic.

## What was traded away

- **No schema for widgets.** The controller cannot tell a typo in a widget id from a new widget. The UI shows a placeholder, and that is the cost of letting the console evolve on its own.
- **Last write wins without a revision.** An import overwrites, deliberately, and a client that does not send a revision gets no protection.
- **Counts are capped.** There is room for 100 dashboards and 50 themes, and a deployment that wants more needs a change in the limits.
- **Tags outlive their nodes.** Node tags are not removed when a node disappears, and a prune call exists for the operator to clean them. It never deletes without a list of names, because an aggregator that has just restarted lists only the nodes that have pushed so far.
- **The mixed use of the area** keeps the machinery small but means secrets, codes and dashboards share one replicated structure, so the separation between them is by kind and by the API routes in front of it.

## Where to look

- `crates/wayhouse-controller/src/ui_data.rs`: the kinds, the operations and the store.
- `crates/wayhouse-controller/src/ui_data/api.rs`: the routes, the shape checks and the limits.
- `crates/wayhouse-controller/src/ui_data/contrast.rs`: the theme checks.
- `docs/ui-data.md`: the record formats and the API table.
- `docs/design-system/themes.md`: the ten contrast checks.
