---
title: "WASM sniffers: the sandbox, the ABI and the loader"
description: How wayhouse runs third party protocol inspectors on every new connection without trusting them, and what the contract between the proxy and a sniffer looks like.
---

Game protocols are many and they change. wayhouse deliberately has **no built in knowledge of any of them**. When a route needs to tell a Minecraft handshake from a Source engine query, it asks a **sniffer**: a small WebAssembly module that looks at the first bytes of a connection and says what it sees. This page explains the contract, the sandbox that makes it safe to run code you did not write on every connection, and how modules are loaded and replaced. The settings are in the [Manual](../../manual/routing/route-by-rules.md).

## The problem

1. **Protocol knowledge does not belong in a proxy.** Compiling parsers for every game into the data plane makes every release a game update, and every parser bug a proxy bug.
2. **The code has to be replaceable at runtime.** A game patches its handshake on a Tuesday. The fix must not need a new proxy build or a restart.
3. **The code is not trusted.** A module may come from a third party. It must not read files, open sockets, look at the clock, eat the process's memory or stall other connections.
4. **It runs on the hot path.** It runs on the first bytes of a new connection, so its cost lands on connection setup.
5. **A broken module must not make things worse.** A bad upload must not silently disable routing, and a crashing reject filter must not let traffic through.

## What a sniffer is

A sniffer is a **core WebAssembly module with no imports**. It cannot call WASI, cannot touch the filesystem, the clock or the network, because the module has nothing to call. It is handed bytes and returns an answer.

The answer is a `RouteHint` with three optional parts:

- a **host**, a name for what the sniffer recognised, which a `sniffer` route can compare against,
- a **key**, an affinity key for the balancer,
- a **reject** flag, which drops the connection.

A sniffer that does not recognise the bytes returns `0`, and that is a clean miss, never an error. The `sniffer` matcher is an ordinary [route matcher](routing-and-matchers.md): it compares the verdict of a named sniffer. A listener lists the sniffers it consults in order, and **the first one that recognises the bytes wins**. Later sniffers are not asked, so a reject from an early recogniser cannot be overridden by a later one.

The official modules live in their own repository, `wayhouse-proxy/sniffers`, and are pinned by hash when fetched. The proxy only ships the contract.

## The ABI

The guest exports three things:

| Export   | Signature                                        | Purpose                                              |
| -------- | ------------------------------------------------ | ---------------------------------------------------- |
| `memory` | linear memory                                    | where the host reads and writes bytes                |
| `alloc`  | `(len: i32) -> i32`                              | returns a pointer to `len` free, host writable bytes |
| `sniff`  | `(in_ptr, in_len, cfg_ptr, cfg_len: i32) -> i64` | the inspection itself                                |

For each call the host allocates through the guest's own `alloc`, copies in the peeked bytes and the module's configured `config` string (a zero length region is passed as `(0, 0)` and never allocated), and calls `sniff`. The return value is `0` for "not recognised" or a packed `(result_ptr << 32) | result_len` pointing at a compact encoding:

```text
byte 0:           flags (bit0 = reject, bit1 = host present, bit2 = key present)
if host present:  u16 LE length, then that many UTF-8 bytes
if key present:   u16 LE length, then that many UTF-8 bytes
```

The host decodes with a bounds checked reader. A truncated result, invalid UTF-8 or a pointer outside the module's memory is `bad_output`, never trusted and never a panic.

Why pass the config in? So one generic module can serve many uses. A byte pattern sniffer needs no recompile to match a new pattern, only a new `config` string in the pin.

### A helper crate, and the version stamp

Writing the guest side by hand is fiddly, so `wayhouse-sniffer-abi` implements it: the `alloc` export, borrowing helpers for the input and the config, and the encoder. It has no host imports and no dependency on any proxy crate, so a sniffer stays a tiny, dependency free module.

Linking it also stamps the module with a **custom section named `wayhouse.abi`**, exactly 4 bytes: major then minor, each a `u16` little endian. The host reads that section with a pure parser **before compiling anything**, so no guest code runs to find out whether the module is acceptable. A module with no section, a duplicate or wrong sized section, or another version is refused. While the major is 0 the minor must match exactly, because a 0.x minor bump is allowed to break the ABI. At 1.0 the rule relaxes to "same major, sniffer minor not above the host's". A test fails if the helper crate and the host disagree about the current version.

## The sandbox

Two bounds keep a module from stalling or ballooning the process.

**Time.** One shared `wasmtime` engine runs with **epoch interruption**. A single ticker thread bumps the engine's epoch every `call_timeout / 2`, and every call sets a deadline of two ticks. The first tick can land anywhere after the call starts, so a call is guaranteed at least half of `call_timeout_ms` and is trapped by `call_timeout_ms` at the latest. It is a ceiling, not an exact limit. The default is 20 ms. The point of this design is that the check costs the guest nothing on the happy path: there is one thread for the whole process, not a timer per call.

**Memory.** Every call gets its own `Store` with limits: at most `max_memory_bytes` of linear memory (16 MiB by default), exactly one memory, one table of at most 10,000 elements and one instance. wasmtime's own defaults allow 10,000 of each and unbounded table growth, which would turn the memory limit into a per memory cap instead of a bound on the module. A failed growth traps instead of returning an error the guest could ignore.

**No state between calls.** Every call builds a fresh `Store` and `Instance`. Nothing a module remembers survives into the next connection, so one player's bytes can never influence another's verdict. The cost is an instantiation per call. The [latency ledger](../../latency-ledger.md) records the measurement: **p50 8 to 10 µs and p99 12 to 26 µs** for the first party sniffers, well inside the 500 µs budget the project sets for added latency. The faster designs held in reserve (a pre-instantiated module, pooled instances, warm instances) turned out not to be needed.

Each call is counted by result in `wayhouse_sniffer_calls_total` (`ok`, `unrecognised`, `timeout`, `trap`, `bad_output`) and timed in a histogram per sniffer name.

### Keeping the runtime responsive

A sniffer is arbitrary code that may run for its whole budget, so it must not run on a thread that serves other connections. The accept path runs it on the **blocking pool**, and **at most 32 calls run at once**. The pool is shared with DNS lookups, so a flood of connections hitting a slow module must not take all of it. The permit moves into the blocking closure, so it is released when the call really ends, even if the task awaiting it is dropped, because a blocking call cannot be cancelled. The limit counts running calls, not waiting callers.

With no sniffer configured on the listener, or with the sniffer feature flag off, nothing is spawned at all.

### Failing closed

If a call **panics**, the listener drops the connection (or the datagram) and counts it as `sniffer_error`. This is deliberate. A `reject` sniffer that crashes on a hostile packet must not let that packet through. Panic log lines are limited to one per second, because a module that panics on every datagram would otherwise write a line per packet.

A positive `reject` is applied before anything else. On TCP the connection is dropped and counted as `sniffer_reject`. On UDP there is no session and no reply, and a content based reject outranks a source address route hint because the address of a datagram can be spoofed and its content cannot.

## The loader

At start the proxy builds one **loader**: the engine, the epoch ticker and a scan function. `call_timeout_ms` belongs to the engine, so it is fixed for the life of the process, like the worker count. The module directory and the pins can change with a config reload.

A scan reads every `*.wasm` file in the directory. The module is named after its file stem, so `a2s.wasm` is the sniffer `a2s`. Each file goes through the same checks:

1. size within 8 MiB, not empty,
2. the ABI version section, read without compiling,
3. compilation,
4. **no imports at all**,
5. the three exports with exactly the right types,
6. a test instantiation under the memory limit, so the module is vetted under the same limits it will run under. Validation and calls share one function that builds the store, so the two cannot drift apart.

Only regular files count. A directory named `x.wasm` is skipped, and if it is a pinned name the scan fails instead of ignoring it silently.

### Pins

`settings.sniffers.modules` can **pin** modules: a name, the SHA-256 of the file, and an optional config string. Once any pin exists, every file in the directory must have a matching pin, and a mismatch or an unpinned file fails the **whole scan**. So does a pinned module that does not validate, because skipping it would quietly downgrade a pinned deployment to "no sniffer". A pin with no file yet is fine, which allows pin first and upload second.

With no pins, one bad `.wasm` file is logged and skipped so it cannot block rescans of the rest.

### Reload and rollback

A failed scan leaves the running registry and the running pins untouched. The registry is an `ArcSwap` over the name to sniffer map, so a replacement is visible to every worker at once with no replumbing, and a lookup on the hot path takes no lock.

The admin API can upload a module. It validates the bytes, checks them against the pins and swaps the file, **keeping the previous version** as a hidden `.<name>.wasm.prev` file. If the new file is later rejected by a scan, the loader falls back to the previous version when that one passes the same checks, with pins meaning its hash equals the pin. The sniffer then runs flagged as a **fallback**, which the admin listing shows, and an error is logged. So a bad upload degrades to the last good module instead of removing the sniffer. Swaps, rollbacks and deletes are serialised by one lock.

A route that names a sniffer which is not loaded can never match. The listener logs a warning for it at start, instead of letting the route sit there looking alive.

## What was traded away

- **An instantiation on every call.** It costs microseconds and buys isolation between connections and a simple model. Instance reuse would be faster and would let state leak.
- **The timeout is a ceiling.** A call started just before a tick gets less than the full budget, so a production `call_timeout_ms` is not a guarantee that every call under it completes.
- **A strict ABI version.** While the major is 0, sniffers must be rebuilt for each minor ABI bump.
- **Only the first bytes.** A sniffer never sees later data and never writes. A protocol that cannot be told apart in its first packet cannot be sniffed.
- **One wasm runtime in the proxy binary.** `wasmtime` is a large dependency, which is why it lives in the binary and not in `wayhouse-core`. The core only knows the `Sniffer` trait.

## Alternatives that were not taken

- **Built in parsers.** Fast and simple, and the proxy would be a game by game maintenance burden.
- **A regular expression matcher in the core.** The core matchers stay prefixes, ranges and suffixes. A pattern that needs more belongs in a module, which runs inside the sandbox with a time budget.

## Where to look

- `crates/wayhouse-core/src/sniff.rs`: the `Sniffer` trait, the registry, `sniff_first_blocking` and the 32 call limit.
- `crates/wayhouse/src/sniffer_loader.rs`: the ABI documentation, the engine, `new_store`, validation, `scan` and the fallback logic.
- `crates/wayhouse-sniffer-abi/`: the guest helper crate.
- `crates/wayhouse/src/admin.rs`: upload, rollback and delete.
- [Latency ledger](../../latency-ledger.md): the measured cost.
- Manual: [Route by rules](../../manual/routing/route-by-rules.md).
