---
title: "Plugins: the WASM host, capabilities and bounds"
description: How wayhouse runs untrusted automation as WebAssembly modules, why a plugin can only do what its declaration says and an operator approved, and which limits stop a hostile module before, during and after a call.
---

A fleet operator wants automation: register game servers with a panel when they start, publish routes, answer a webhook, set a DNS record for a certificate. Writing that into the controller would put every panel's quirks in the core. Letting anyone run arbitrary code in the controller is worse. **Plugins** are the answer: small WebAssembly modules that react to a trigger and act only through **capabilities** that an operator approved. This page covers the host that runs them. How the controller schedules and wires them is in [plugins in the controller](plugin-runtime-in-controller.md). They are not [sniffers](sniffers.md), which run on the proxy's data path. The operator side is in the [Manual](../../manual/index.md) and the generated [Reference](../../reference/index.md), and the module contract is in `docs/plugins.md`.

## The problem

1. **Modules come from outside.** A plugin may come from a registry the controller does not vet. It must be assumed hostile or buggy.
2. **The controller is a high value process.** It holds the keyring and the config. A plugin that can read arbitrary memory, call arbitrary hosts or hang forever is an incident.
3. **Approval must be reviewable.** An operator needs to see what a module asks for before it runs, without running it.
4. **Credentials must be usable but not readable.** A plugin that calls a panel needs a token, and must never be able to print it.
5. **Calls must be deterministic to replicate.** In a cluster, the effect of a call must be a value that can be committed once, by the leader, and not a side effect applied wherever the call happened.

## One module, a fixed contract

A plugin is a core WebAssembly module, built against the `wayhouse-plugin-abi` crate, which stamps an ABI version into a custom section and exports `alloc`. The host reads the section **before compiling anything** and accepts a module only when it equals its own version, with an exact minor match while the major is 0. The module exports `memory`, `alloc`, `init`, and an entry point for each trigger it declares, such as `on_timer`. Its only imports are the host functions in the `wayhouse` namespace, one family for each capability.

A module is at most 8 MiB, the same limit as sniffers.

## Declare, approve, enforce

The central idea is that **what a plugin may do is data, read without running it**.

A second custom section, `wayhouse.plugin-caps`, holds the declaration as JSON: triggers, timer interval, log, state size, outbound HTTP hosts, secrets and their hosts, routes, port routes, backends. Unknown fields are an error, because a declaration that names a capability this host does not know must be an install time error and not a silently ignored, and therefore unreviewed, request. The timer interval is at least 10 seconds and at most a week. The state ceiling is 1 MiB.

The operator approves against the module's **SHA-256**. A new build of the module is a new hash, so a plugin cannot swap in different code under an old approval. At runtime the host enforces the intersection: a call that uses a capability it did not declare traps that call with `CapabilityDenied` and leaves the host running.

An optional third section carries a manifest of install options and bindings, so an official prebuilt plugin can ask the operator for the panel's hostname at install time and bind a capability value to it.

## Limits before, during and after

### Before compiling: structure

Compiling costs time and memory in proportion to a module's structure, and compilation cannot be interrupted once started. So a walk with `wasmparser`, which generates no code, rejects a module whose declared structure is out of proportion to any real plugin, naming the bound it hit. The defaults allow 5,000 functions, 1,000 types, 1,000 locals per function and 50,000 in total, nesting depth 200, 10,000 table elements, 1,024 memory pages, 1,000 globals and exports, 16 imports and 4 MiB of code.

Compilation itself runs on a **bounded pool off the caller's thread**: a fixed number of workers, a bounded queue where a full queue answers `Busy` at once, and a caller side timeout. A timed out job that has started still holds its worker until it ends, and one still queued is dropped. The bounds are the honest answer to "compilation cannot be cancelled".

### During a call: a fresh sandbox

**Every call gets a fresh store and instance.** Nothing survives in guest memory between calls, so a module cannot carry a corrupted state or a leaked secret from one call to the next. The defaults limit a call to **5 seconds** by epoch interruption (so between half and the whole of it, as for sniffers), **64 MiB** of memory and 10,000 table elements. A call may keep 64 log lines, each cut at 1,024 bytes, with the rest counted as dropped, and make 256 state writes. State keys are at most 256 bytes and a plugin holds at most 1,024 of them, because the byte cap alone lets tiny keys pile up and inflates the per call clone of the state and every snapshot.

### After a call: effects as a value

A call does not change anything. Its effects, the state writes and the log lines, **are returned as a value** and never applied by the host. The caller commits them as one entry. This is what lets the controller tag the entry with the leader's term and reject it when stale, which is the same pattern as ACME issuance (see [certificates](acme.md)) and what makes plugins safe to run under [controller HA](controller-ha.md).

## Outbound HTTP, as a policy engine

The `http` capability is a policy engine first and a socket second. The engine decides everything that does not need a socket, so it can be tested without one:

- Only approved `host:port` pairs, and only over HTTPS.
- **Redirects are re checked** on every hop against the same rules, and at most 3 are followed.
- **No private address** unless the host was approved for it, and the resolved address is **pinned for the connection**, so a name cannot resolve to a public address at check time and a private one at connect time.
- A call may make at most 8 requests, with a 10 second timeout each and a 20 second budget for the whole call. A request is at most 256 KiB, a response 1 MiB and at most 64 headers.
- Everything the guest gets back is scrubbed.

### Secrets

A plugin refers to a secret as `${secret:NAME}`. The host expands it **in header values only, and only for hosts the secret slot is bound to**. The value lives only for the host call that expands it and is zeroed when dropped. It is never passed to the guest and never printed. A scrubber then removes every form a destination could echo back, raw, base64 and percent encoded, from what the guest sees (see [secrets](secrets.md)).

## What a plugin may affect

Beyond log, state and HTTP, the other capabilities each change something in the fleet, and each is bounded by what was approved:

- **routes**: the hostnames a plugin may claim, and the networks they may point to. A plugin hands over its **whole** route set in one call, and the host checks every entry. Operator routes always win, since plugin routes are appended after them.
- **port routes**: entry ports a plugin may ask for. The controller assigns the ports and keeps each key's port stable.
- **backends**: the controller hands each plugin only the backends inside its approved networks, so a plugin cannot learn the rest of the topology.
- **webhooks and events**: the controller authenticates the caller and applies size and rate limits before the guest sees anything. The host bounds what the guest returns: a status, a few allowlisted headers and a capped body. The authorization header never appears.
- **`on_acme_dns`**: a DNS provider hook for certificate challenges, answered through a result call, with the plugin never seeing an ACME key.

## Conformance

A conformance check loads a built module with its own declaration as the approved set, calls `init` with an empty config and `on_timer` once under the default limits, and reports anything the host would refuse and any capability the plugin used without declaring. The same code runs in the plugin repository's CI and in a command line check, so an author finds a refusal before an operator does.

## What was traded away

- **A fresh instance per call** costs start up time on each call, in exchange for no state to leak.
- **A closed set of capabilities.** A plugin cannot ask for something the host does not know. That is a feature, and it means new needs mean new host code.
- **Exact minor match** between module and host while the ABI is 0.x, so plugins are rebuilt with each host that changes the minor.
- **The approval is per hash,** so every rebuild needs a new approval.
- **Structure limits are coarse.** They stop a pathological module, not a slow but well formed one, which the call timeout handles.

## Where to look

- `crates/wayhouse-plugin-host/src/module.rs` and `caps.rs`: the ABI and declaration sections.
- `crates/wayhouse-plugin-host/src/bounds.rs` and `pool.rs`: structure checks and bounded compilation.
- `crates/wayhouse-plugin-host/src/runtime.rs`: calls, limits and effects.
- `crates/wayhouse-plugin-host/src/http.rs` and `secret.rs`: outbound HTTP policy and secret handling.
- `crates/wayhouse-plugin-host/src/routes.rs`, `port_routes.rs`, `backends.rs`, `webhook.rs` and `acme_dns.rs`: the capabilities.
- `crates/wayhouse-plugin-host/src/conformance.rs`: the conformance check.
- `crates/wayhouse-plugin-abi`: the crate plugins build against.
- `docs/plugins.md`: the module contract.
