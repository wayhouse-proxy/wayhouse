---
title: "Controller HA: Raft, snapshots and membership"
description: How several controller nodes act as one, with an embedded Raft group replicating the config log, the intent log and the tunnel registries together, and what it takes to bootstrap, join, import old data and change membership safely.
---

One controller is a single point of failure for everything that changes: new config, new origins, rotated keys. Proxies keep running on their last good state while it is down, so an outage does not stop traffic, but nobody can change anything. **High availability** lets several controller nodes serve as one. This page explains how, and especially the parts that are easy to get wrong. The base layer is [the controller core](controller-core.md). The flags are in the [Manual](../../manual/index.md) and the generated [Reference](../../reference/index.md).

## The problem

1. **No external dependency.** A small fleet should not need an etcd or a database cluster to get a second controller.
2. **One history, not several.** Config revisions, operator intents, registrations and addresses must agree on every node. An address allocated twice, on two nodes, would be a real outage.
3. **Clients must not care.** A proxy, the UI and `curl` talk to whichever node they reach, and a write on a follower must still work.
4. **Cluster changes are dangerous.** Adding a node with the wrong id or address can produce two leaders. A node that restarts must not re initialise the group.
5. **A controller that grew up single** has data that must not be lost when it becomes the first node of a cluster.

## One Raft group per tier

The controller embeds `openraft`. Rather than a separate group for each kind of data, **one Raft group per controller tier replicates everything together**: the config log, the intent log and the registry entries are interleaved in the same Raft log. Two independent groups would make "this revision and that address change" impossible to order against each other.

The embedded store (see [the controller core](controller-core.md)) stays the state machine. Raft adds a replicated log and a leader election in front of it, and applying a committed entry is **not a new code path**. It calls the same index aware apply function that a direct write uses when HA is off, which is why the store writes the applied index in the same transaction as the revision.

A write is a `WriteRequest`, a tagged value for each kind of change: a config submission (carrying its rollout stage and actor, so every replica attributes the revision identically), an intent, a promotion, the start and end of a confirm trial, a relayed revision from a parent tier, and the registry operations (register an origin or a proxy, release, a touch that refreshes `last_seen`, and an expiry). New variants are added as separate entries, so a log written by an older build still decodes.

### Determinism

Replicas must compute identical state from identical entries. Anything that depends on the clock is fixed by the proposer: a registration carries the **leader's clock** at proposal time, so `first_seen` and `last_seen` are the same on every node. An expiry entry carries the cutoff and re checks `last_seen` when it applies, so a re registration that committed earlier in log order wins. Registry application is crash idempotent for the same reason the store is: the applied index joins the transaction.

## Clients do not see it

Everything that writes goes through one function, `propose_write`. It proposes to Raft, and if this node is not the leader, it **forwards the request to the leader over HTTP**, with the caller's authorization and actor headers. A proxy, the UI and `curl` never need to know HA exists. Reads are served from the local replica, so a follower can briefly lag, which matters for token revocation, as the deployment notes say.

With the `slave` role, only the leader runs the upward relay from the parent tier and proposes each parent revision as an entry. The relay cursor, the highest parent revision absorbed, sits beside each log and replicates with it, so a newly elected leader resumes where the group left off.

## Timing and snapshots

The Raft timing is set for a LAN: heartbeats every 250 ms, and an election timeout drawn between 800 and 1,500 ms. A snapshot is built every `SNAPSHOT_AFTER` log entries, **5,000** by default and adjustable with `--ha-snapshot-after`. After a snapshot `openraft` purges the log and keeps the most recent 1,000 entries, and a lagging follower or a new learner that needs entries older than that catches up by **installing a snapshot**, which replaces both stores.

Snapshots travel as JSON, where each byte costs several characters. They are sent in **256 KiB chunks** so that a chunk stays well under the body limit of the `/raft/*` routes, and each chunk is given **30 seconds** instead of the library's default of 200 ms, which a catch up of a real registry never meets. Snapshots and the persisted Raft log hold no plaintext secrets, which is what store format 2 guarantees.

The Raft log is itself kept in two trees of the embedded store, so it survives a restart as well as the state machine does.

## The peer surface

Replicas talk over `/raft/append`, `/raft/vote` and `/raft/snapshot`, posted with `reqwest`, plus `/raft/whoami` and, for upgrades, `/raft/pre-ha`. These routes can rewrite replicated state, so they are gated by a **separate peer token**, `--ha-token`. HA does not start without one, a token that is given must be at least 16 bytes, and, unlike the client facing admin token, **there is no opt out**.

A peer address is either `id=host:port` (plain HTTP, the original form) or `id=https://host[:port]`, a base URL that can point at a TLS terminator in front of the replica. The address is stored as given in the Raft membership, so it takes effect when the cluster is bootstrapped, and a changed address is a membership change (below). HA peers are expected to use `https://`, since the token travels in the clear on plain HTTP.

## Starting a cluster

There are two ways to start, and they are mutually exclusive on one node.

- **`--ha-peers`** bootstraps a cluster from a static member list. A node with it calls `initialize`, so it is the flag for the first start of the first nodes, never for a node added later.
- **`--ha-join`** starts empty and waits to be added. A new or replacement node always starts this way. With the enrollment flags, a node asks a running replica to add it (`POST /enroll/ha-join`), using a proof that came with its join code, and keeps asking until it is a voter. Without them it waits for an admin to add it.

Both need `--ha-node-id`, or a file holding one whole number, which is what an enrolled replica was given. After a join, the node restarts with `--ha-join` again.

### Initializing the registries

A cluster has to agree on the tunnel network once. Until it is recorded, every registry write is refused. A leader initialization task runs on every node. While this node is the leader and nothing is recorded, it asks the voters what pre HA data they hold and proposes either `SetTunnelNetwork`, when nobody has data, or `Import` of **exactly one** source's data. With several sources and no `--ha-import-source`, it **initializes nothing and logs an error rather than guess**. Once a value is recorded, each node compares its own `--tunnel-network` with it, logs one error naming both on a mismatch (writes then answer 503), and the task ends.

### Upgrading a single controller

A controller that already holds registrations can become the first node of a cluster. At start, a node with `--ha-peers` that finds registry directories with data but no applied index marker knows they predate HA. It **renames them aside** as `<dir>.pre-ha` and never deletes them, so the state machine starts on fresh databases. The leader picks one node's set aside data, serves it over `/raft/pre-ha`, and proposes it as one `Import` entry that is applied once. The leader waits for every voter to answer before it initializes, so writes answer 503 until then, and a voter that is down holds up initialization.

## Changing members

Membership changes use admin routes, answered by any node and forwarded to the leader: list voters, learners and the leader, add, remove and re address. They are logged with their actor.

The dangerous case is pointing a node id at another node's address, which `openraft` warns can produce two leaders. So before an add or an address change, **the leader asks the node at the given address who it is** (`/raft/whoami`) and refuses unless the answer carries the expected id. An add also needs an empty log, or an existing learner of this cluster. An add waits for the new node to catch up, by log or by snapshot, which can take minutes, and the forwarding request is allowed a little longer than the leader's wait. Other answers are explicit: a node already a voter, an unknown id, removing the last voter, and no leader. Removing the current leader is allowed.

## What was traded away

- **A leader for every write.** Throughput is that of one node, plus a forwarding hop from followers. For a control plane that writes a few times an hour, this is the right place to be simple.
- **Followers can lag on reads,** so something just written on the leader may not be visible on a follower for a moment.
- **A LAN assumption.** The timeouts are for nodes in one site. A group spread over a wide area link would see elections.
- **One tunnel network per cluster,** recorded at initialization, with a mismatch answered by refusing writes rather than diverging.
- **An operator in the loop for ambiguity.** Several pre HA sources need a choice, and a failed add needs a human.
- **The peer token is a shared secret** for all replicas, not per node.

## Where to look

- `crates/wayhouse-controller/src/ha/mod.rs`: the Raft types, `WriteRequest`, the timing and the token check.
- `crates/wayhouse-controller/src/ha/log_store.rs` and `state_machine.rs`: the persisted log and the apply path with snapshots.
- `crates/wayhouse-controller/src/ha/apply_registry.rs` and `cluster_state.rs`: registry application and the recorded network.
- `crates/wayhouse-controller/src/ha/client.rs`: `propose_write` and leader forwarding.
- `crates/wayhouse-controller/src/ha/routes.rs` and `network.rs`: the `/raft/*` routes and their client.
- `crates/wayhouse-controller/src/ha/init.rs`, `import.rs` and `join.rs`: initialization, the pre HA import and joining.
- `crates/wayhouse-controller/src/ha/members.rs` and `peers.rs`: membership changes and peer addressing.
- Deployment notes: `docs/12-deployment.md`, "HA membership".
