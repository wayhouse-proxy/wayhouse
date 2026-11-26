---
title: Runtime feature flags and kill switches
description: How an operator turns one capability off on a running fleet in seconds, without a deploy, and why a kill switch for security needs rules of its own.
---

Sooner or later a part of the proxy misbehaves in production. A country filter blocks half a region, a sniffer module chews CPU, the upload endpoint is being abused. The fix is a new config revision, a review and a rollout, and during a bad hour that is too slow. **Feature flags** are the faster tool: a named on/off switch per capability that an operator can flip on a live fleet. This page explains how the switches are defined, where their value comes from, why some are one way only and how the data path reads them for almost nothing. The proxy's `GET /features` endpoint is in the [Manual](../../manual/operate/monitor.md), and the fleet view in the [Fleet API](../../manual/operate/fleet-api.md).

## The problem

1. **A bad hour needs a fast switch.** Turning one thing off should not need a build, a restart or a full config rollout.
2. **A kill switch is itself dangerous.** Some flags turn off protection, such as rate limits and ACLs. A stolen token that can switch those off, or a switch left off by accident for a month, is an attack.
3. **A flag that does nothing is worse than none.** An operator who flips a switch that nothing reads believes they have acted.
4. **Old nodes must not silently ignore a switch.** In a mixed fleet, a node that does not know a flag must not pretend it applied it.
5. **The check is on the hot path.** Rate limits and ACLs are consulted for every connection. A flag read cannot take a lock.

## One registry

Every flag lives in a single static table, `REGISTRY`, in `wayhouse-config`. An entry says:

- the **name** and the **default** (every flag is on by default),
- the **scope**: which process reads it, the proxy or the controller,
- whether a change applies **live** or at startup,
- the **override mode**: both ways, or off only,
- whether it is a **protection**,
- whether it is **wired**: whether the runtime consults it yet,
- the config schema version that introduced it,
- one sentence on what turning it off **risks**, and one on what happens to connections that are **already open**.

The risk and impact sentences are not comments. They are data, shown by the UI next to the switch, so an operator sees "Client address allow and deny lists are bypassed. New connections only." before flipping it.

At the time of writing there are 14 flags. On the proxy: `gossip_verdicts`, `sniffers`, `port_routes`, `rate_limit`, `per_source`, `acl`, `geo`, and three admin write switches (`admin_write.backends`, `admin_write.sniffer_upload`, `admin_write.route_hint`). On the controller: `plugin_webhooks`, `registry_install`, `acme_issuance` and `tunnel_registration`. The generated [reference](../../reference/index.md) lists the current set.

A test enforces that **no registry flag stays unwired**: a flag that nothing reads is a kill switch that does nothing, so wiring one means flipping `wired` in the same change as the code that reads it.

## Where a value comes from

The effective value of a flag is decided by precedence, from strongest to weakest:

1. **Build availability.** A flag whose code the binary was compiled without (the `gossip` or `wasm-sniffers` cargo feature on the proxy, the `acme` feature on the controller) reports `available: false`, value off, source `build`. It is not an error. It is the honest answer.
2. **A controller override.** Set by an operator through the controller, fleet wide.
3. **The `features:` section of the config.** Part of a config revision, so it is per node.
4. **The registry default.**

Every flag reports its **source** next to its value (`default`, `config`, `override`, `build`), exported as two metric families: `wayhouse_feature_enabled` per flag, and `wayhouse_feature_source` per flag and source. The question "why is this off here?" has an answer on the dashboard.

## Rules for dangerous switches

Two properties of a flag change what an override is allowed to do.

**Off only.** Flags that open new network exposure, such as `port_routes` (plugin entry listeners), `plugin_webhooks` and `registry_install`, are `off_only`: an override can switch them off but never on. A stolen token can then close a door and cannot open one. An override that tries to turn one on is rejected, and an override with an "on" value for an off only flag is ignored by the effective value logic too.

**Protection flags expire.** Turning off `rate_limit`, `per_source`, `acl`, `geo` or `sniffers` removes protection. An override that does that **must carry an expiry**, and the expiry may be at most **seven days** after it was set. A longer one is refused with a message that says to renew instead. The protection always comes back on its own, even if everyone forgets.

Controller and proxy **both** run these rules. The controller checks an override when it accepts it. The proxy checks it again when it reads the document, so a document that skipped the check (a bug, a hand edited file) cannot hold a protection off forever. An item that fails the check is logged and skipped, not the whole document.

Unknown names are an error everywhere. A kill switch that an older node silently ignores is worse than a rejected config, so a `features:` section with an unknown flag fails validation and names the known ones. Controller flags cannot appear in a proxy config at all, because the proxy does not read them: they are set by an override on the controller.

## How the fleet learns about an override

An override is a stored record on the controller. Each proxy **polls** `GET /features/overrides` every **15 seconds** and republishes when the document changed. A few details come from running this for real:

- **An unreachable controller keeps the last document.** A control plane outage must not flip switches back. The failure is logged once when it changes, so a rotated token shows up once and not every 15 seconds.
- **An old controller clears the live set.** A 404 or 501 means a controller without the feature, so the overrides are dropped. The saved copy on disk is **not** wiped, because a later restart relies on it.
- **Expiry is judged on the proxy.** A controller that goes away cannot hold a protection flag off past its time, because the proxy compares each override against its own clock on every poll and when it reads them.
- **A restart keeps the kill switch.** The overrides are written to a file in the shape the controller serves them, through a temp file of its own and an fsync before the rename, so a crash leaves the old file or the whole new one. At start, before the first listener binds, the saved overrides are loaded and applied. The first connections already see them, even with the controller unreachable.

When `port_routes` changes, the plugin entry listeners are part of the built configuration, so the proxy asks for a **rebuild** of the snapshot. Every other proxy flag takes effect without one.

## Reading a flag costs one load

The data path reads flags through `wayhouse_core::flags::on(flag)`. The effective values are published into a single `AtomicU64` **mask**, one bit per registry position. A read is one relaxed atomic load and a shift. No lock, no map lookup, no allocation, so the ACL check can ask on every connection.

The position constants (`ACL`, `GEO`, `RATE_LIMIT` and the rest) are computed at compile time by `index_of(name)`, and a name that is not in the registry is a **compile error**, so a typo cannot become a flag that is silently always on. A second compile time assertion keeps the table within 64 entries, because the mask is one 64 bit word. Every bit starts on, which is also what a process that never publishes gets, so a test binary or a build without the poller runs with every capability enabled.

Publishing replaces the whole mask in one store. A reader sees the old set or the new one, never half of each.

For tests, a per thread override (`off_here`) switches flags off for one thread until its guard drops, so tests that flip a flag cannot race the others in the same process.

## Seeing the fleet

Each proxy reports its effective flags in its push to the aggregator. The aggregator rolls them up (the fleet features endpoint): per flag, how many nodes have it on, how many off, how many cannot run it because their build lacks it, and up to 50 instance names that run it off. A roll-up lists at most 256 distinct flag names, because proxies report their own names and the answer must stay bounded however many strange nodes report. So the UI can say "gossip_verdicts is off on 2 of 14 nodes" and show a node whose build lacks the flag.

## What was traded away

- **Up to 15 seconds of delay.** Polling is simple and survives an outage of the controller. A push channel would be faster and more fragile.
- **Booleans only.** A flag is on or off. There is no percentage rollout, no per listener scope. A capability that needs finer control gets a config option instead.
- **A fixed registry.** A flag is code. Adding one is a change to the table and to the code that reads it. That is the point: nothing can be switched that a developer did not decide to make switchable.
- **A limit of 64 flags.** The mask is one word. The assertion makes the limit loud instead of silent.
- **Dotted names and the schema table.** A flag whose name has a dot (`admin_write.backends`) cannot get its own entry in the table that records which config schema version introduced a field, so all such flags count as part of schema 4 as a whole. The source notes that a dotted flag added later needs that matcher taught first.

## Alternatives that were not taken

- **Flags only in the config file.** They would work, but flipping one means a revision and a rollout, which is the slow path this exists to avoid.
- **A general flag service.** Percentage rollouts and targeting rules are a product of their own. The registry is deliberately a small, auditable list of safety switches.
- **Letting a stored override live forever.** For protection flags, the expiry and the seven day cap make "I forgot to turn it back on" impossible.

## Where to look

- `crates/wayhouse-config/src/features.rs`: the registry, `check_override`, the effective value logic and `Mask`.
- `crates/wayhouse-core/src/flags.rs`: the data path's one bit read.
- `crates/wayhouse/src/features.rs`: the poll loop, the saved copy and the metrics.
- `crates/wayhouse-controller/src/features.rs`: the controller's own flags; `ui_data/api.rs` checks an override when it is accepted.
- `crates/wayhouse-aggregator/src/features.rs`: the fleet roll-up.
- Manual: [Monitor](../../manual/operate/monitor.md) and [Fleet API](../../manual/operate/fleet-api.md).
