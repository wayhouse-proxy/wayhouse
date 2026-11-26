---
title: "Plugins in the controller: timers, events, hooks and routes"
description: How the controller stores plugin installs and modules, runs them on timers and webhooks only on the leader, replicates their state and module bytes, and turns what they declare into routes and ports on the proxies.
---

The [plugin host](plugin-host.md) knows how to run one call safely. This page is about everything around it: where a module and its installation live, what makes a call happen, why only the leader makes it, how a result becomes replicated state, and how a plugin's routes reach the proxies. The operator side is in the [Manual](../../manual/index.md) and the generated [Reference](../../reference/index.md), and the contract in `docs/plugins.md`.

## The problem

1. **Installs are state.** A plugin is installed once, enabled, configured, and has its own persistent state. That must survive restarts and replicate.
2. **Calls have side effects, and a cluster has one world.** If every replica ran every timer, a plugin would register a game server five times.
3. **A module is big, an install is small.** Module bytes are up to 8 MiB, while the Raft log should carry small records.
4. **Triggers come from outside.** A webhook is the one request that is not behind the admin token.
5. **What a plugin declares must reach proxies,** without the controller rewriting the operator's config.

## Installs and blobs

A standalone controller keeps two sibling trees next to the config revisions: `plugin_installs`, from install id to a JSON record, and `plugin_blobs`, from the module's SHA-256 to its bytes. A **blob is content addressed** and shared by every install of the same module, and it is removed with the last install that references it. Unused uploads are limited to 64 MiB in total, and an install's config to 64 KiB.

Installing follows the flow that sniffer installs use, with one more step:

1. `POST /plugins/modules` stores the bytes and returns **what the module declares**, so the operator reads the request before approving it.
2. `POST /plugins` names a stored module and the **capability set the operator approves**. The controller compiles it through the host's bounded pool, and only then records the install.

Every route sits behind the admin bearer layer, and slave controllers do not serve the plugin API.

### An optional plugins file

An operator who prefers files can list the installs they want in a plugins file, reconciled with the database. The reconcile is careful about what it owns. It only manages installs it created itself, and an install made in the UI or the API is never changed, adopted or removed, and a clashing name is reported. An entry is applied only when the capability set its options resolve to hashes to the entry's `approved_sha256`, so a file cannot approve more than the person who wrote it saw. A file that cannot be read changes nothing. The file never carries module bytes, secrets or webhook tokens.

## Timers: the leader ticks

The runner passes over the installs and starts a tick for every enabled install whose approved declaration has `on_timer` and whose interval has elapsed. The first tick comes one **full interval after the runner first sees the install**, a grace period, so enabling a plugin never runs it inside the request that enabled it. A module is compiled on first use through the pool and `init` runs once with its config. Every call runs on the pool, so a slow or busy pool delays or skips ticks and never blocks the controller.

A call returns its effects as a value, as described in the host page. The runner commits the state writes **as one batch that is a compare and set on the install's state revision**, then records the outcome in memory, the last result and the last log lines. The outcome record is not replicated and is lost on restart.

### Under HA, only the leader

Under high availability, **only the Raft leader ticks**. A node that is not the leader drops its schedule and its loaded plugins. When it wins an election, every install starts over with one full interval of grace. By then the entries of earlier terms have been applied, and a plugin never fires the moment a node takes over. A commit is one `PluginState` entry **tagged with the term in which the leader read the state**, so the state machine drops the late result of a deposed leader instead of applying it (see [controller HA](controller-ha.md)). The same rule appears in [certificate issuance](acme.md).

### Module bytes

The Raft log carries install records only. The module bytes are a content addressed blob that each replica keeps in its own store, moved over the peer channel with three pieces:

- The leader **pushes** a module to every member and proposes the install only once **a quorum of voters holds it**, so an install cannot be stranded on a node that dies right after the upload.
- Every replica runs a **catch up loop** that fetches the modules its installs reference and it lacks, from the leader first and then the other members, and **verifies the SHA-256** before keeping them. A replica that was down, joined late or restored a snapshot gets its modules this way.
- A new install is refused while any member runs a build that cannot take part, which the member reports in its `whoami` answer.

A node without `--plugins` answers 404 on these routes, which the quorum push counts as "does not hold the module".

## Webhooks: the one unauthenticated door

A webhook, `POST /plugins/<install id>/hook[/<suffix>]`, is the one endpoint that is not behind the admin token. It is served by a **separate, optional listener** with its own bind address and TLS, off by default, and never on the admin port. Each install has its own bearer token, shown once and stored as a SHA-256 hash in the replicated install record.

A request goes through a fixed order, and the order is the design:

1. **Count it against a per source rate limit**, before anything is compared, so a token cannot be guessed at line rate. The default bucket is 5 requests a second with a burst of 20.
2. **Authenticate by a constant time comparison** of the token's hash, **before the body is read**. An unknown install, a disabled one and a wrong token all answer 401 alike, so nothing reveals which installs exist.
3. **Count it against a per install rate limit** (10 a second, burst 20) and then read the body up to a cap.
4. **Run it on the leader.** A follower authenticated the caller itself and forwards the request over the authenticated peer channel, only to an `https://` peer, since body and headers would otherwise cross the network in clear text. Otherwise the answer is 503.

The answer is the guest's, but **only after its effects were committed**. A commit that failed is a 503 with a retry hint, and a full queue is a 429. Because effects of a retried request can repeat, every call carries an **idempotency key**, the caller's own header or else a hash of the install and the body, for the guest to forward to whatever it calls.

## Events: hints, not a log

A plugin that declares `on_event` receives three kinds: a new config revision was accepted, a plugin was installed, enabled, disabled or deleted, and a backend's health changed. They are **hints and not a log**. A plugin that must not miss a change reconciles on its timer. Events travel through the same bounded queue as a webhook, so a slow plugin **loses events, counted as dropped, instead of building a backlog**, and only the leader delivers them.

### The backend health board

Proxies report which backends they see up or down. The board keeps the latest word of each proxy and derives **one verdict per backend: down when a majority of the proxies that reported it say down**, the same rule as the gossip domain quorum (see [regional health](gossip-health.md)). A proxy that has not reported for 3 minutes stops counting. The board holds at most 16,384 backends, 256 proxies and 4,096 entries per report, and a report that would exceed a cap is refused. It lives **in memory on the leader only**: a new leader starts empty and refills from the proxies' next full reports. That is acceptable because events are hints.

## The socket side of outbound HTTP

The host's policy engine decides what a plugin may call. The controller supplies the transport that enforces the rules which need a socket. It speaks HTTPS only, uses **no proxy** (a proxy would resolve the name itself and skip the address checks) and follows **no redirects**, since the engine follows them hop by hop and re checks each. It refuses a destination in a private, loopback or link local range unless the host was approved for it. The name is resolved by a resolver that applies the same rule on every connection and returns the address that is then connected to, so a DNS answer that changes between calls, a rebinding attack, cannot reach a private address. An IP literal is checked up front, since a literal skips DNS.

## Routes and ports, materialised

Plugins declare hostnames with backends, or entry ports. The controller does not rewrite the operator's config. It publishes the entries of every enabled install as an **overlay** that each proxy appends to the listeners that opted in, after the operator's own routes, so **operator routes always win** (see [config rollout](config-rollout.md) for the overlay stream). Two installs claiming the same hostname: the earlier install wins, and the later claim is reported and not applied.

Ports take more care, because they are a shared resource and a player may hold an address:

- A plugin declares `{key, backend, proto, prefer?}` and the host owns the mapping from key to **entry port**. One port belongs to one `(install, key)` whatever its protocol, so an entry for both TCP and UDP needs no second port and two installs can never collide.
- Allocation runs in the replicated apply step, so it is **a pure function of the stored ledger and the entry**, and the entry carries the clock. Every replica reaches the same table.
- A released port waits in **quarantine for 24 hours** before another key may take it, so a player with an old address does not land on a different server. The key that released it can take it back at any time.

## What was traded away

- **Outcomes are not replicated.** The last result and log lines of a plugin are in memory and vanish on restart or leadership change. The cost is a thinner view after a failover, and the benefit is a log that carries state and not chatter.
- **A leader only runtime.** One node runs every plugin, so the plugin load sits on one node. For automation that ticks every few seconds at most, this is the safe place to be.
- **Events can be lost,** by design. The timer is the reconciliation path.
- **A grace period** after every election delays a plugin by one interval.
- **A quorum push** makes an install wait for its module to spread, so an upload to a partly down cluster can be refused.
- **A separate webhook listener** means one more port to operate, in exchange for never exposing the admin port.

## Where to look

- `crates/wayhouse-controller/src/plugins.rs`: installs, blobs and the replicated apply.
- `crates/wayhouse-controller/src/plugins/api.rs`: the admin API.
- `crates/wayhouse-controller/src/plugins/runner.rs`: the timer runner and its commits.
- `crates/wayhouse-controller/src/plugins/hooks.rs` and `events.rs`: webhooks and events.
- `crates/wayhouse-controller/src/plugins/backend_health.rs`: the health board.
- `crates/wayhouse-controller/src/plugins/peer.rs`: module bytes between replicas.
- `crates/wayhouse-controller/src/plugins/routes.rs` and `ports.rs`: the overlay and the port ledger.
- `crates/wayhouse-controller/src/plugins/declared.rs` and `net.rs`: the plugins file and the HTTP transport.
