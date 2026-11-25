# HANDOVER

Current state and the traps. This file is the *current-state + gotchas* layer only; CI
caps it at 16 KB. Design is the source of truth in [`docs/`](docs/); locked decisions are
the ADR table in [`docs/09-technology-choices.md`](docs/09-technology-choices.md). History
lives in `git log` and [`CHANGELOG.md`](CHANGELOG.md), open work in the
[GitHub issues](https://github.com/wayhouse-proxy/wayhouse/issues). Write what is true
*now*; delete what stopped being true instead of appending a dated note.

Last updated: 2026-11-26 (0.4.0 released).

## Current state

**Released: 0.4.0.** All roadmap phases (0–14) are built, individually verified live, and
covered by `make check`. "Built" means built and tested, not battle-tested: there are no
production users yet, so breaking changes are fine until 1.0. Releases are manual tags
(`vX.Y.Z`, steps in [`RELEASING.md`](RELEASING.md)); release-please only opens the release
PR and never tags. `release.yml` publishes eight images (amd64 + arm64) to GHCR, but only
if the tag equals the workspace version, CI is green on main, and the whole set built and
passed the compose smoke test.

Every CI job blocks except `trivy` and `audit` (informational). The repository is public,
so hosted Actions minutes are free; the cost is wall time and queue waits.

### Not done / not verified (all tracked in issues)

- Deferred features: vault references (#627), SSO (#618), a steering plugin (#796). Real
  hardware checks (#58), a real-cluster check (#53), v2 design ideas (#65), open design
  questions (#69; cross-instance session failover is assumed **no**, ADR 4).
- Never verified outside CI: `make deploy-images` / `make deploy-smoke` and the compose
  tunnel override (CI `deploy` job only); the k8s manifests are only schema-validated
  (kubeconform), never applied to a cluster; the systemd units (Caddy, web-sync, certsync,
  `wayhouse-control`) were never started on a real host; `wayhouse-ui --proxy-url` and the
  proxy overlay file were not run against a real file-config proxy (#476).
- Nightly late-joiner stall (#800) never reproduced; watch the nightly runs.
- Left for after 0.4.0: one tracker issue lists the review nits (reload does not re-check
  secret references, a few UI nits, the step-up revoke limiter refund).
- The Debian base image carries Zstd alerts (#145–151); a base-image re-pin is tracked.

### Traps that bit before

- **Upgrade every controller replica before the first write of any new kind.** Levels: UI
  data 9, plugin support 6. An older leader answers `409` until the last replica upgrades.
  The operator checklist is [`docs/migrating.md`](docs/migrating.md).
- **CodeQL flags names that look like secrets** in log lines, test fixtures and field
  names: keep such names out of messages, avoid hard-coded passwords and `http://` literals
  in tests, keep file paths out of axum handler `State`, and do not parse request bodies
  just for logging. Alerts get fixed, not dismissed.
- **Plugin ABI is an exact minor match while the major is 0.** An ABI bump means every
  plugin is rebuilt and re-approved.
- **New HTTP client code must not hard-code `http://`**; build clients through
  `wayhouse_http::{client, builder}` so `--ca-file` and TLS work everywhere.
- **Tokens and secrets:** the admin token and gossip PSK take `{ secret: name }`; tokens
  also come from `--<name>-file`; the sealed store needs `--secret-key-file` and a lost
  key makes every stored secret unreadable.

### Owner decisions that still hold

| Question | Decision |
|----------|----------|
| Self-hosted CI runner | No: unsafe on a public repo (fork PRs would run on it). |
| Reference images | Docker only, multi-arch, GHCR, on `vX.Y.Z` tags. Kubernetes packaging is #88. |
| Release feed | Signed with a separate release minisign key; the secrets live in the `release` environment. |
| Plugin and sniffer repos | `wayhouse-proxy/plugins` and `wayhouse-proxy/sniffers`; this repo keeps only the ABI crates and pins sniffer releases in `sniffers.lock`. |
| Process | Nothing ships without a bug + security audit of every change since the last audit. Flaky tests count as bugs. |

## Workflow gotchas

- **Run `cargo fmt --all` as its own step before `make check`.** `make check` only reports
  formatting diffs, and piping it through `tail` hides an early `fmt-check` failure: read
  from the top or check the exit code ([`CONTRIBUTING.md`](CONTRIBUTING.md#build-and-test)).
- **Disk fills up in long sessions.** `target/debug` can reach tens of GB and the linker
  then fails with a bare exit 1. `rm -rf target/debug/incremental` is the cheap fix.
- **`protoc` is a build requirement** (gRPC resolver codegen); CI installs `protobuf-compiler`.
- **`make deploy-lint` needs a UTF-8 locale** (`LANG=C.UTF-8 LC_ALL=C.UTF-8`); its ruby
  checks fail on the Dockerfile under `C`/POSIX.
- **A config file directly under `/tmp`** causes ~200 ms reload log spam (`notify` on
  tmpfs). Harmless; run live tests from a real directory.
- **`shutdown_grace_sec: 30`** in `config.example.yaml` means `timeout N wayhouse` does not
  kill at `N`. Use `timeout -s KILL`. Never `pkill -f <pattern>` from an agent shell (it
  matches and kills the shell itself); use `pkill -KILL -x <exact-process-name>`.
- **Fixed-sleep + connect tests flake under load.** Use `connect_when_listening`
  (`crates/wayhouse-core/tests/tcp_forward.rs`) instead of lengthening a sleep. A UDP test
  against `echo_backend` that runs past ~3 s needs a `udp_probe` health check, because the
  default `tcp_connect` check marks the backend unhealthy.
- **Rust handler ↔ TS frontend wire-shape mismatches** are a recurring bug class (a proxy
  silently dropping upstream headers; `JSON.parse` on a text body). A thin proxy needs an
  explicit test that a header survives the hop.
- **`--tunnel-*` and `wayhouse-agent` need `CAP_NET_ADMIN` + `/dev/net/tun`**, even with the
  userspace `boringtun` backend. The rootless tunnel lab (`make tunnel-e2e`, `unshare -Urnm`)
  needs neither root nor Docker, but needs `unshare`, `ip`, `nsenter`. `/pools` health is
  optimistic (a new backend is `healthy` before the tunnel is up): wait for a real round
  trip. `backend_sources[].name` must equal the origin's `wayhouse-agent --name`.
- **Nextest runs every test in its own process**; anything that assumed one shared process
  breaks (the tunnel lab's namespace counter was one such case).
- **Locally installed tools** (none are in the repo): `cargo-nextest`, `actionlint`
  (v1.7.7), `hadolint` (v2.12.0; its `DL3008`/`DL3006` warnings are expected),
  `kubeconform` (v0.6.7), `cargo-audit`.

## Invariants that bite if you forget them

(Full list in [`AGENTS.md`](AGENTS.md) "Architecture invariants". These have a subtlety.)

- **Exactly one place builds and stores a `Snapshot`: `reload::apply`.** Runtime edits
  (backend overlay, discovery refresh, admin) are extra *rebuild triggers*: they mutate a
  persistence layer (`BackendOverlay`, `Discovery`) and then call `request_reload()`.
  Backend *state* (`PATCH`) mutates atomics on the live `Backend` and needs no rebuild.
- **`Sniffers` and `Resolvers` registries are `ArcSwap`-backed** (unlike the snapshot, which
  every worker re-`load`s). Every worker clone points at the same instance, so `replace()`
  from the reload task is visible instantly.
- **Listeners are reconciled by name** (`ListenerManager::reconcile`), not rebuilt with the
  snapshot. An unchanged listener keeps its route rules captured at spawn; only the pool
  contents they resolve to are read live. Any `ListenerConfig` change (bind, protocol,
  routes, `prefix`, `freebind`, `route_hint`, ACL, `rate_limit`, `geo`, `transparent`) stops
  and re-spawns it; a same-bind rebind is gapless via `SO_REUSEPORT`.
- **Startup-only config** (reload does not re-read): `workers`, `settings.limits`,
  `settings.geo_db`, and the `settings.sniffers` engine params (`call_timeout_ms`,
  `max_memory_bytes`; only the `dir` contents are live).
- **No `unsafe` in the host crates.** `socket2`, `nix` (`IP_PKTINFO`, `recvmmsg`, `splice`)
  are safe wrappers. The `unsafe` that exists is the guest-side glue in
  `wayhouse-sniffer-abi` and `wayhouse-plugin-abi`, and a counting allocator in a
  `wayhouse-core` test.
- **UDP amplification guard:** the proxy never sends to a client without an established
  session; a dropped datagram (routing, ACL, rate, gate) gets no reply. Covered by
  `crates/wayhouse-core/tests/amplification.rs`.
- **Per-connection cost is documented:** a new per-connection or per-datagram task, hop or
  allocation goes into [`docs/latency-ledger.md`](docs/latency-ledger.md).

## Testing

`make check` runs fmt, clippy `-D warnings` and `cargo test --all`, including
`crates/wayhouse-fleet-tests` (spawns the real binaries and drives them over HTTP; adds
wall-clock time). Not part of `make check`:

- `make tunnel-e2e` / `make tunnel-e2e-ci` (the namespace lab; `TUNNEL_BACKEND=kernel|userspace`);
- `make ui-test` (vitest); `make deploy-lint` (needs the docker CLI + ruby);
  `make deploy-images` / `deploy-smoke` / `deploy-scan` (need a Docker daemon; the scan
  also needs `trivy`);
- the CI helper scripts in `.github/scripts/` (`changes_test.py` needs `cargo`; CI runs
  them all in the `changes` job);
- TPROXY / `IP_TRANSPARENT` e2e (needs `CAP_NET_ADMIN`; covered by parse/reject and
  `connect_tcp_from` fallback tests; recipe in `docs/04`) and the UDP `prefix:` e2e (needs
  Linux with `IP_PKTINFO` and loopback `127.0.0.2/3`);
- the `#[ignore]`d sniffer tests, run by the `sniffers-e2e` CI job after
  `make sniffers-fetch` against the sniffers pinned in `sniffers.lock`.

Loader tests assemble WASM from inline WAT via the `wat` crate; `wasmtime` is a normal
dependency and needs no `wasm32` target (only the sniffers repo does).

## CI and infra

- Workflow `.github/workflows/ci.yml`. Docs-only pushes (`**.md`, `docs/**`, `LICENSE-*`)
  skip the heavy jobs (`ci-ok` still reports on a PR); a newer push cancels an older run.
- A `changes` job (`.github/scripts/changes.py`, tested by `changes_test.py`) decides which
  path-scoped jobs run from `cargo metadata`: `ui`, `sniffers`, `tunnel`, `deploy`, `fuzz`.
  `test` and `audit` always run for non-docs pushes; `trivy` follows `deploy`. A workflow
  edit, a failed diff, the **nightly schedule (03:17 UTC)** and `workflow_dispatch` run
  everything, so `deploy` and `fuzz` double as nightly canaries. Adding a job or changing
  which packages it builds means updating `ROOTS` in `changes.py` *and* its test.
- Cargo caching is `.github/actions/cargo-cache` (rolling snapshot per `main` push, restored
  by prefix; PRs read but do not write). A lockfile or toolchain bump starts cold by design;
  if a code push without a lockfile change is back near cold timings, the cache key is the
  first suspect. A warm full run is roughly 20 min wall clock.
- Rust tests run under `cargo nextest --profile ci` (`.config/nextest.toml`); JUnit XML is
  rendered to the run summary by `test_summary.py`. A `build-release` job compiles the
  release binaries once and `deploy` builds the images from them (`BIN_SOURCE=prebuilt`);
  the nightly `deploy` uses the in-Docker build so that path cannot rot.
- Every job is pinned to `ubuntu-24.04`; moving to a newer image is a deliberate edit of all
  jobs together (re-check `tunnel`: AppArmor userns sysctl and the `wireguard` module, and
  release glibc vs the distroless runtime). Every third-party action is pinned to a commit
  SHA with the tag in a trailing comment; bump by hand.
- `trivy` runs the official `aquasec/trivy` image pinned by digest (not `trivy-action`,
  whose tags were hijacked in 2026-03) and `cosign verify`s it each run. Release binaries are
  built with `cargo auditable build` (0.7.7 in CI and the Dockerfile; bump together).
- `make audit` runs `cargo audit` over both lockfiles. New advisories arrive on a schedule
  you do not control and are fixed by lockfile bumps.
- Workspace lints: curated `[workspace.lints.clippy]` in the root `Cargo.toml`; every crate
  opts in.
- `Cargo.toml` declares `MIT OR Apache-2.0` with both license files. No explicit owner
  decision on record; confirm if it matters.
- Remote `github.com/wayhouse-proxy/wayhouse`, branch `main`. The HTTPS credential helper
  may log a harmless "nonexistent Windows path" warning before falling back.
