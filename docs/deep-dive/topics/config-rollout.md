---
title: "Config distribution: intents, canaries and confirm windows"
description: How a change gets from an operator to a fleet of proxies, with a canary stage that only some proxies see, a confirm window that reverts a risky change by itself, an intent log for runtime operations, and tiers that relay each other's history.
---

Writing a good config is half of the job. The other half is getting it to hundreds of proxies without a bad change taking them all down at once, and without locking the operator out of the thing they use to fix it. This page covers how revisions are distributed and protected: **canary stages**, **confirm and revert**, the **intent log** for operations that are not config, and **tiers** that relay. It builds on [the controller core](controller-core.md). The flags and endpoints are in the [Manual](../../manual/index.md) and the generated [Reference](../../reference/index.md).

## The problem

1. **A valid config can still be wrong.** Validation proves a document loads. It does not prove that the admin port is reachable after the change. A change to that port, or to the admin token, can lock the operator out.
2. **Fleets want a staged rollout.** A change should reach a few proxies first and the rest only after it has been seen to work.
3. **Not everything is config.** Adding a backend to a pool, disabling one, or hinting a route is an operation that must reach every proxy running the pool, and that survives restarts, but it is not a change to the structure of the config.
4. **Fleets have layers.** A global controller, regional ones, and proxies behind each. The same history has to flow down without a new mechanism.
5. **The controller must stay out of the data path.** A proxy running on its last good state must keep running while the controller is gone.

## The proxy side: follow, apply, freeze

A proxy started with `--controller` fetches the current config once and then holds a long lived stream of revisions from its last cursor, as described in [the controller core](controller-core.md). Each revision goes through the **same** apply function a file reload uses (see [snapshot and hot reload](snapshot-and-hot-reload.md)), so a controller delivered config gets exactly the checks of a local one, and a revision that fails them leaves the running snapshot in place. A dropped connection reconnects with capped exponential backoff from the last cursor, and **nothing ever clears the running state**. The controller can be down for a day and the proxy serves from its last good snapshot. The proxy reports the revision it last applied in its push to the aggregator, so the fleet view can show how far a rollout got.

The proxy also registers itself with the controller's proxy registry (see [the tunnel registry](tunnel-registry.md)), which is how the controller learns the lowest schema version any live proxy supports.

## Staged rollout: one log, visibility per revision

A staged rollout is built on the same single, never forked revision log. The submission `POST /config?stage=canary&group=<name>` writes an ordinary revision that is **visible only to subscribers that report that group**. Each revision has a small stage record, `{promoted, canary_groups}`, kept in its own tree beside the revisions and never folded into the revision bytes, so the store stays content agnostic.

- A plain submission and a rollback are promoted at once, visible to everyone, which is what existed before staging.
- A subscriber with no group sees only promoted revisions. A subscriber in group G sees the newest revision that is promoted or lists G.
- `POST /config/promote/{revision}` is the one deliberate exception to "every change is a new revision": it flips an existing revision's visibility **in place**. Promotion is not new content, and minting a new revision would make a different document than the one that was observed to work on the canaries.
- A revision with a missing or unreadable stage record is treated as visible to nobody. It fails closed.

The subscribe worker filters each candidate through this visibility. It advances its cursor past a revision it cannot yet see and leaves that revision for the next signal, its own later promote or any later submission, to re evaluate, rather than skipping it forever. A group name is at most 128 characters.

## Confirm and revert

A submission that could lock the operator out is applied **on trial**. The controller records the revision to fall back to and a deadline, and unless someone confirms the revision before then, it **re submits the old bytes as a new revision**. Like any rollback, history is not rewritten.

What counts as risky is a short, explicit list, derived by comparing the old and the new parsed config: the admin listen address, the admin token, the admin TLS settings, the gossip settings, and a listener on the admin port that was added, changed or removed. Each has an id that a response carries as a reason, so a client can show the text without keeping a copy of the list. The first revision cannot lock anyone out and never triggers a window.

The window is chosen by the `confirm_within` parameter: absent means **automatic**, which is 60 seconds for a risky diff and none for a safe one. A number asks for that many seconds, within 10 and 600, and `0` opts out entirely. The revert itself never opens a window, since that would be a loop. A window needs a revision to fall back to.

Where the record lives matters:

- It sits in a tree next to the revisions and is written **in the same transaction as the revision it guards**, so no crash leaves a risky revision applied without its timer.
- It is live only while its revision is still the latest. A newer revision, including a confirm free edit and the revert itself, retires it without any write.
- The deadline is the proposer's wall clock in milliseconds, carried through the Raft log so every replica reads the same value. A skewed proposer shifts the window by the skew, which is why the shortest window is 10 seconds and controller clocks need to stay within a second or two.
- A loop checks once a second. Only the leader acts, and every replica has the record from the log, so a newly elected leader picks up a window the old one opened. A restart resumes from the stored deadline.
- Promoting a risky canary starts a trial that reverts fleet wide, and a canary submitted during a running trial leaves the trial alive.

Ending a trial (confirm or revert) is gated differently from starting one. Under HA, **starting** a window needs every member to report a build that decodes the new entries, because an old replica could not apply them, and a member that does not answer blocks it. **Ending** one is blocked only by a member that answered with a build that cannot decode, since an unreachable member must never stop a revert, which is what gets the operator back in.

## The intent log

Some changes mean the same thing on every proxy that runs a named pool: add a backend, remove one, set a backend's operator state, hint a route. They are **intents**, kept in a second log beside the config log, in its own database with the same append only store. Their effect on a proxy is the same as calling the admin API's equivalent endpoint, and in fact an intent applied from the controller is not a new code path but a new _source_ for the same mutation, through `backend_overlay().add/remove`, the backend's admin state and the route hint table.

Two decisions shape it.

- **Pool scoped verbs only.** Draining one instance targets a single process, and the controller has no instance identity to address it with, so whole instance drain stays on the aggregator's fan out and the direct admin API.
- **Minimal validation.** There is no config validator to lean on for a bare operation, so submission checks the operation itself (addresses and IPs parse, a state is a known value), the same checks the proxy's admin API runs, moved one hop earlier so a malformed operation is never broadcast.

On the proxy, the intent client starts from the beginning of the log on every process start, and does not persist a cursor. This is safe because every operation is idempotent to replay (add or remove a specific backend, set a state, set a hint with a fresh lifetime), and replaying the whole log is exactly how a restarted instance catches up with what operators have done.

## Plugin routes

Routes that controller plugins declare reach proxies through an **overlay**, a separate stream, `GET /plugin-routes/subscribe`, that sends a full document on connect and on every change. The proxy keeps the last overlay it received and merges it into the config text before parsing, for listeners that opted in (see [the config schema](config-schema.md)). A change asks for a re fetch of the config. A controller without plugins answers 501 (or 404 from an older build), the overlay is cleared and the proxy retries slowly and quietly. An unreachable controller keeps the last overlay, again freezing on last known good. The re fetch after a change is retried with a growing delay, so a withdrawn route does not linger until the next unrelated change.

## Tiers: relay and adoption

A controller in the **slave** role subscribes to a parent controller exactly the way a proxy subscribes to a standalone one. Principle five, pull by subscription, is applied recursively and is not a new mechanism. The relay lands every parent revision in its own store through the one function that bypasses the write gate, and a dropped parent connection reconnects from the last applied cursor while the tier keeps serving what it held. The intent log has its own relay.

Under high availability only the **leader** runs the relay. If every follower subscribed on its own, each would apply the same parent revision itself and number it differently, corrupting the replicated log. The leader proposes each parent revision as a Raft entry carrying the parent's revision number, which becomes the relay cursor in the same transaction, so a new leader resumes from the replicated cursor. An entry not above the cursor, such as the late proposal of a deposed leader, is skipped on every replica alike.

**Adoption** turns a running standalone controller into a slave of a newly introduced parent without a restart, with `POST /admin/adopt`. The two correctness problems, whose history outranks whose, and writes being reordered across the flip, are solved by the same simple rule: **adoption is refused unless both the config and intent stores are still empty.** A controller that has accepted real writes is replaced by a fresh one to join a hierarchy, and is not reconciled in place. On success the controller seeds from the parent's current config, flips the shared role cell, and starts the same relay tasks a slave started at boot would run. Afterwards it is indistinguishable from one that was installed as a slave.

## What was traded away

- **A parent that went back is a problem.** A relay cursor above what the parent now sends means the parent's counter went backwards, because it was wiped or restored, and the tier skips the entries. Tiers do not paper over a parent whose history was rewritten.
- **One group per canary submission.** The type allows several, but a submission takes one `group`.
- **Clock dependence for windows.** The confirm deadline is a wall clock value, and the 10 second floor is the margin for skew.
- **A short list of risks.** Only changes known to lock out an operator open an automatic window. Other mistakes are caught by validation or by an operator asking for a window explicitly.
- **No whole instance intents,** and no way to target one proxy from the log.
- **Adoption is only for empty tiers.** The cost is a rebuild instead of a merge.

## Where to look

- `crates/wayhouse-controller/src/api.rs`: stages, promote, the subscribe filter and the confirm loop.
- `crates/wayhouse-controller/src/confirm.rs`: the risk list, the window rules and the pending record.
- `crates/wayhouse-controller/src/intent.rs` and `intent/`: the intent log, its API and its relay.
- `crates/wayhouse-controller/src/relay.rs`, `parent_client.rs` and `adopt.rs`: tiers.
- `crates/wayhouse/src/controller_client.rs`, `intent_client.rs`, `proxy_register.rs` and `plugin_overlay.rs`: the proxy side.
- `docs/10-distributed-control-plane.md`: the design.
