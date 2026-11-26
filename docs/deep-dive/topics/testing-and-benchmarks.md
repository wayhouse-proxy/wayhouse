---
title: "Testing and benchmarks: real processes, namespaces, fuzzing and load"
description: How wayhouse is tested at four levels from unit tests to a rootless network lab, why the fleet tests start real binaries instead of mocks, what the benchmark harness can and cannot prove, and how the parsers that face untrusted bytes are fuzzed.
---

A proxy that sits in front of a game fleet is judged by the failures nobody wrote a test for. This page explains how the test suite is arranged to find the ones that only appear when real programs talk to each other over real sockets, and where the project is honest about what a test on one machine cannot show. How the suite is run in CI is in [the CI pipeline](ci-pipeline.md).

## The problem

1. **Mocks hide wire bugs.** The bugs that bit hardest were mismatches between what one program sends and another expects. An in process harness shares the author's assumptions on both sides.
2. **Some behaviour needs a network.** A WireGuard tunnel, an underlay with IPv6 and a path between hosts cannot be faked on loopback.
3. **Parsers that see untrusted bytes must never panic,** whatever they are given.
4. **Performance claims need numbers,** and a loopback benchmark can pass for the wrong reasons.
5. **Tests that run side by side collide.** Several test processes share one machine and one pool of ports.
6. **The suite must be fast enough to be run.** A suite that takes an hour is not run before a commit.

## Four levels

- **Unit tests** live next to the code in each crate, run with plain `cargo test` locally and under nextest in CI. They cover the pure logic: matchers, parsers, the balancers, the state machines. Many of the pages in this deep dive point at tests that pin a behaviour, such as the ones that keep the config schema and its generated document in step.
- **Crate level integration tests** start a runtime in process: a proxy with listeners and a backend, in one tokio runtime.
- **Fleet tests** start the **real binaries** as child processes. This is the level where wire shapes are proved.
- **The tunnel lab** starts the real binaries inside nested network namespaces, with real WireGuard between them.

Two kinds of local run exist. `make check` runs formatting, lint and tests, including the minimal build with every optional feature off, and needs no Node and no Docker. The heavier jobs, the browser tests, the images, the tunnel lab and fuzzing, are separate targets, and CI runs them by area (see [the CI pipeline](ci-pipeline.md)).

## Fleet tests: real processes, no shortcuts

The fleet test crate says why it exists in its first lines. Earlier slices of the project were verified by a person standing up a proxy, a controller, an aggregator and a UI and driving them over HTTP. That found whole classes of bug, wire shape mismatches and behaviour that only shows when processes talk over a socket. The fleet tests **automate exactly that shape of test**, instead of inventing a harness that mocks each program's `main`, because an in process shortcut would bring back the blind spot that unit tests already have.

A test spawns the real debug binaries on loopback with operating system assigned ports, talks to them over real HTTP with the **component protocol header** that every client sends, and kills them at the end. The suite covers the areas a fleet cares about: authentication hardening and scoped tokens, native TLS on each server, a pinned TLS enrolment, the all in one process, replicated controllers with joins, snapshots, plugins and port routes, secret purging, the gossip fabric, the tunnel's addresses, the web gateway against a real Caddy, an initial bundle, and **mixed version fleets**.

### Mixed versions

A node one minor version behind must keep working and be sent only baseline fields, and a node on another major must be refused readably. The older side is played in two ways: by a hand built request carrying the older header, or by a **real binary started with a protocol override variable**. The override exists as a cargo feature that every fleet test build turns on. It must never exist in a shipped image, so the deploy lint refuses any image build that enables it (see [deployment artifacts](deployment-artifacts.md)).

### Ports that never collide

Every test needs ports, and several test processes run side by side. A first version asked the operating system for a free port by binding and dropping, and **two callers could receive the same port**, and an outgoing connection could take the port in the window before the child bound it. In a three replica cluster that was not just a failed start. When replica two got replica one's port, `address already in use` killed replica two, and its peer URL then reached replica one, which received its own vote and append request and hit an assertion in the consensus library (#334).

The fix is in two parts. Ports come from **below the kernel's ephemeral range**, where outgoing connections never land. And each one is **claimed with an exclusive file** in a per user temporary directory that every test process of that user shares. A claim is never released and expires after an hour. A dedicated test pins the property that no port is handed out twice.

### Building the binaries once

The tunnel lab builds the binaries before it enters its namespace. A test pins that building again inside is a **no op on a built tree**. A child cargo build that inherits the variables a test run sets would make a dependency's build script rerun, and recompile the crypto library on every call (#67).

## The tunnel lab

WireGuard cannot be tested on loopback, so the lab builds a small internet **without root**. The test process runs inside an outer user, network and mount namespace made by `unshare`, and that is the lab. Each node is a further nested network namespace, kept alive by a sleeping holder process and addressed by that process's id, because the usual namespace tool needs a writable system directory. Each namespace is joined to the lab by a **veth pair** with an IPv4 /30 and an IPv6 /64, and the lab forwards between namespaces, so traffic from one to another really crosses the lab as it would cross the internet. The underlay can be IPv4 or IPv6 independently of the tunnel's own network.

Namespace indices are handed out per process and never reused, because a dead namespace's veth is torn down asynchronously by the kernel, and a later lab reusing the same names would race that cleanup. Since every test runs as its own process, all restarting at one, a new namespace skips indices whose interface already exists.

Bring up order matters and is part of the lab. The **origin must register before the proxy starts**, because the proxy's tunnel backend source pins the origin's key, which the test reads back from the controller's registry (see [the backend tunnel](tunnel-client.md) and [the origin agent](tunnel-agent.md)). The scenarios run on both tunnel backends, the kernel module and the userspace implementation, as a CI matrix, with serial execution, and a tmpfs mounted over the runtime directory.

## Fuzzing the parsers that see untrusted bytes

Three libFuzzer targets cover the code that reads bytes from strangers or untrusted files:

- **The TLS ClientHello reader** that extracts the server name from the first bytes of a connection. It is hand written and runs on the peek buffer of every connection (see [routing and matchers](routing-and-matchers.md) and [listeners](data-plane-listeners.md)). It must never panic.
- **Route matching** with a fuzzed first bytes buffer against a fixed set of matchers, which exercises the byte matchers, the server name matcher and the host patterns.
- **The config parser** on arbitrary UTF-8, which is the whole validation path. It must return an answer, never panic (see [the config schema](config-schema.md)).

The fuzz crate is its **own workspace** with its own lockfile, so nightly Rust and the sanitizer flags stay out of the main build. Committed **seed** inputs give each run a start, and the corpus the run grows is git ignored. CI runs a 45 second smoke of each target on every change to that area and uploads any crash input. A longer pass is a make target with a configurable time. A crash is reproduced by running the target on the saved file.

## Benchmarks that say what they cannot say

The benchmark crate measures against the project's non functional targets, and its header states which targets it **cannot** judge. It has three modes.

- **Latency.** The client, the proxy runtime and the backend share one process, which makes it fast and **structurally blind** to anything that appears only with a separate proxy under load. It reports the **added** latency, the proxy's round trip minus a direct round trip over many sequential requests, against a target of **0.5 ms at the median and 2 ms at the 99th percentile**. It also reports single stream throughput for context and the idle memory growth per held connection. Held background connections add scheduler contention, but with few workers they saturate the box and degrade the direct baseline too, so the verdict is only meaningful with little background load, which is also the intent of the target. A strict flag turns a miss into a failure.
- **Concurrency.** It builds the real release proxy and runs it as a **separate operating system process**, then ramps the number of held connections through steps. At each step it reads the proxy's own memory and open descriptor count from the system's per process files, which is meaningful now that the proxy is its own process and not polluted by the harness's client sockets, and measures the added latency of a probe connection. A debug build is never used, since its overhead represents nothing. The ramp is capped by the client's ephemeral port range for connections to one destination, typically around 28,000, far below the targets for hundreds of thousands of connections. It reveals most classes of concurrency bug at a scale beyond the in process harness, without reaching the full numbers.
- **UDP affinity.** The load test behind the decision to retire the UDP sticky table. A wave of distinct clients, each with its own loopback source address, opens sessions at a set rate against a real proxy, and the harness reports achieved opens per second, open latency percentiles, memory and descriptors, and the backend spread. It then waits for every session to idle out and samples memory again, since what remains is state that outlives sessions. A **warm wave** brings the same clients back in shuffled order and reports **retention**, the share that lands on the same backend as before. A round robin pool is the no affinity baseline, and a consistent hash pool is the real thing. The old per worker sticky table retained only about a third of clients with four workers, which is why it was replaced (see [UDP sessions](udp-sessions.md) and [pools and load balancing](pools-and-load-balancing.md)).

A separate micro benchmark times a 16 datagram UDP send and receive round and counts heap allocations.

**What is deliberately not claimed:** the target of 20 Gbit/s aggregate throughput needs real network cards, since loopback bandwidth exceeds it and a local pass would mean nothing, and the targets for replicated availability need real hosts and a real network. The harness says so in its header and its readme. The load test workflow is **manual only**, on a small shared runner, so its numbers are read as relative and never absolute.

## What was traded away

- **Real processes are slower and need ports.** A fleet test costs seconds, and the port claim machinery exists only because of it. In exchange the tests catch what a mock cannot.
- **The lab needs a permissive kernel.** Unprivileged user namespaces and, for the kernel backend, the WireGuard module. The CI job sets the restricting AppArmor option and loads the module, and the tests are marked ignored so a plain run does not fail on a machine without them.
- **Loopback benchmarks are relative.** They compare strategies and builds on the same box. They do not prove an absolute service level.
- **Fuzzing in CI is a smoke test.** Forty five seconds per target finds shallow crashes. Depth needs the longer manual pass.
- **Test and shipped code share a feature switch** for the protocol override. It is checked by lint, but it is still a way for test only behaviour to exist in a binary, and the check is the guard.

## Where to look

- `crates/wayhouse-fleet-tests/src/lib.rs`: the harness, the port claims and process helpers.
- `crates/wayhouse-fleet-tests/src/netns.rs` and `tunnel.rs`: the namespace lab and the tunnel bring up.
- `crates/wayhouse-fleet-tests/tests/`: the scenarios.
- `crates/wayhouse-bench/`: the three benchmark modes and their readme.
- `crates/wayhouse-config/fuzz/`: the fuzz targets and seeds.
- `.github/workflows/load-test.yml`: the manual load test.
- `Makefile`: the local entry points for each level.
