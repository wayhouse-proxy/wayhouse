---
title: "The CI pipeline: path-scoped jobs, one gate and warm caches"
description: How the continuous integration is organised so that a docs fix costs seconds and a core change is tested everywhere it matters, what the single required check means, and how the cargo caches stay warm without bloating.
---

wayhouse is a Rust workspace with a React frontend, container images, a tunnel that needs kernel features, fuzz targets and a documentation site. A pipeline that ran everything on every change would take most of an hour and burn the runners. A pipeline that ran too little would let a break through. This page explains how the pipeline tells the two apart. It is a developer page: nothing here changes how wayhouse behaves, only how confidently it ships. The release side is in [the release pipeline](release-pipeline.md), the images in [deployment artifacts](deployment-artifacts.md) and the tests themselves in [testing and benchmarks](testing-and-benchmarks.md).

## The problem

1. **Most changes touch little.** A docs edit or a UI tweak does not need the WireGuard end to end test.
2. **A required check must always report.** If a branch rule needs a job and a path filter skips that job, the pull request waits forever.
3. **Main must stay green without testing it twice.** Everything reaches main through a merge queue, which already tested the exact tree.
4. **Compiles dominate the time.** A cold cargo build of this workspace is much longer than its tests.
5. **A CI system is also an attack surface.** Third party actions and scanners run with the repository's token, and tags of popular actions have been replaced with malware before.
6. **Surprises should be deliberate.** A runner image upgrade that changes the C library can break a release binary.

## The shape of a run

The workflow runs on pull requests, on the **merge queue**, on a manual dispatch and **nightly**. There is deliberately **no push trigger**: a commit reaches main only after the queue tested it, so a second full run on the push would test the same tree again. The push to main instead runs a much smaller workflow whose only job is to warm the caches (below). A newer push to the same ref cancels an older run of it.

The token is read only for every job, since nothing in CI pushes, comments or releases, and a compromised step or action cannot write to the repository. The few jobs that need more, such as uploading scan results, ask for exactly that permission themselves.

Every job is **pinned to Ubuntu 24.04**, not to the "latest" label. The comment in the file gives the reasons: release binaries built on a newer image could need a newer C library than the runtime image has, the tunnel test depends on a kernel module and an AppArmor setting, and caches are only reused between jobs on the same image. Moving the label is therefore a deliberate edit, done for all jobs together. Every job also has a timeout, so a stuck step cannot hold the gate for the six hour default.

## Deciding what to run

The first job, **changes**, maps the list of changed files to the areas that need testing, using a script with its own self tests.

- **Rust areas come from `cargo metadata`,** not from a hand kept list of crates. A changed file belongs to the package whose directory holds it, and a job runs when that package is one of the packages the job builds, **or anything those depend on** through path dependencies, transitively. A crate that gains a dependency is picked up with no edit to the script.
- **Non Cargo paths** are listed per area on top of that: the config schema for the UI forms, the pins for the sniffer end to end job, the deploy directory and the Makefile for the images and the tunnel.
- **A lockfile only affects its own workspace.** The root lockfile cannot change what the fuzz crate, which has its own workspace, builds, while the root manifest can.
- **When in doubt, run everything.** A workflow edit, a scheduled or manual run, a failed diff, a failed `cargo metadata`, a rename that leaves a job's root matching no package, and a path under `crates/` that no package owns all select every area. The root manifest, the toolchain file and the cargo config select every Rust area.
- **Docs only changes** (markdown files, `docs/`, licences) select no code job at all.

The script states what it cannot see: a crate that reads another crate's files at compile or test time. Today that is the fleet tests reading the HTTP crate's fixtures, and the HTTP crate is already in every job the fleet tests are in.

## The jobs

The ones that always or usually run:

- **lint:** formatting, clippy over all targets and features, and a separate check of the agent crate, because alone it pulls the HTTP crate **without its server feature**, and every other build unifies that feature on. It is the only one that proves the client only build still compiles.
- **test:** the whole workspace under nextest with a profile that reports every failure and writes a JUnit file, which is rendered as a table on the run's summary page and kept for a week.
- **test minimal:** the minimal edge build with every optional feature off, for the proxy, the core and the controller.
- **musl:** the five main binaries for a static musl target, plus a test that the transparent socket options are known to a musl libc.
- **checks:** the small no build checks in one runner, described below.

Gated on the area they concern:

- **build release,** on amd64 and arm64, compiles the release binaries once with the dependency list embedded, so a scanner can read the crates from the finished binary. The arm64 build runs on a native arm runner, because x86 binaries cannot run on arm64, so it compiles its own. The minimal proxy is built first and set aside under another name.
- **ui:** the frontend's unit tests, type check, build, the demo build and the Playwright browser tests, with the browser cached by version.
- **tunnel:** the WireGuard end to end tests in rootless network namespaces, for **both** the kernel and the userspace backend, as a matrix (see [the backend tunnel](tunnel-client.md)).
- **sniffers e2e:** the real loader against the **official sniffers**, fetched at the tag and checksum pinned in a lock file, on both architectures, because wasmtime generates native code per architecture. On amd64 the same job builds the pinned official plugins and runs the plugin host's conformance check on them, so a host change cannot silently break them. A hash mismatch or a missing asset fails loudly.
- **web e2e:** wayhouse, the sync program and the real Caddy taken out of the image that ships, so the test runs the shipped bytes (see [the web gateway](web-gateway.md)).
- **deploy:** builds all eight images, checks each, runs the compose, solo and all in one smoke tests and validates the Kubernetes manifests. An arm64 twin builds and smoke tests the arm images, since an arm only break found at release time would be found too late.
- **trivy:** scans those images and the lockfiles. It is informational by the owner's choice: it never fails the run, and findings appear as a table, as annotations and in the Security tab.
- **fuzz:** a short smoke run of each fuzz target on nightly Rust, 45 seconds each, uploading any crash input.

### The scanner is verified too

Two choices in the Trivy job come from supply chain incidents. It does not use the vendor's action, because in March 2026 tags of that action and of its setup action were force pushed to a credential stealer. It runs the official **image, pinned by digest**, as a container step. A digest pin stops a later swap but cannot show that the image was genuine when pinned, so the job also **verifies the keyless signature** of that exact digest on every run. That check is blocking on purpose, since it guards the scanner and not a finding. The vulnerability database is cached by day, and a cache hit holding no database file does not skip the download.

### What `checks` holds

One runner does the work that needs no build:

- the version policy (0.x only, and the lockfile agrees with the manifest) and the upgrading document's version table against the code, **always**, because they also guard release pull requests,
- when docs changed: the formatting check, **relative links and anchors**, and the **coverage check** that every config key, flag, metric and feature is in the hand written docs or in a short allowlist. It is here and not in the site deploy so that an undocumented item blocks a merge, while a gap can never block the site from deploying (see [the documentation site](docs-site.md)),
- when code changed: the RustSec advisory audit, which is informational, and **cargo deny** for licences, sources and bans, which **does fail** the run.

## One gate

The branch ruleset requires exactly one check, **ci-ok**. It always runs, including on docs only changes where every other job is skipped. It looks at the results of all the others: a skipped job counts as passed, and a **failed or cancelled** one fails the gate. That solves the path filter problem and keeps the ruleset short, and a new job only has to be added to one list. The cost is that someone must remember to add it, which is why the gate's needs list is the thing to check when a job is added.

## Caches that stay small

Compiling is the bulk of every run, so the cargo caches matter, and the pipeline has its own cache action for them. The stock rust cache action saves only when its exact key misses, and its key was the lockfile hash, so a snapshot froze at the last lockfile change and every later run recompiled everything newer. The replacement works differently:

- **Only runs on main save a snapshot,** with a key that includes the commit, so every push saves a new one. Pull requests and the merge queue **only restore**, because a save there is never read by another ref and its post step cost minutes on the critical path.
- **Restore is by prefix:** toolchain, lockfile and ISO week. There is deliberately **no broader fallback**, so a lockfile or toolchain change and the start of every ISO week begin cold. cargo never prunes its target directory, and a rolling snapshot would otherwise keep every stale artifact it ever saw. Without this a test cache reached 4.7 GB after three pushes, and the caches of ten namespaces 13 GB against the repository's 10 GB limit.
- **Debug info is off** in dev and test builds, which makes the target directories about 45 percent smaller. That setting must match between the main workflow and the warm up one, or the warmed artifacts do not fingerprint match.
- Each kind of job has its **own cache namespace**.

The **cache warm** workflow runs on every push to main that is not docs only, and on Monday 00:00 UTC, right after the week rolls over, so the namespaces are warm before the first working hours pull request. Each entry compiles what the same named job compiles, with no test run, no images and no scans. Runs queue and one waiting run is kept, and it never cancels a running one, because **a cancelled run saves no cache at all**. A drifted command only makes the cache less useful and never makes a result wrong, so it is a best effort mirror and not a second source of truth. The nightly full run saves the same caches again as a backstop.

## Smaller pieces

- **Actions are pinned by commit.** Every third party action in the workflow is referenced by a full commit hash with the version in a comment, never by a movable tag.
- **A tiny install action** wraps the system packages the builds need, such as the protocol buffer compiler.
- **The pull request title** must be a conventional commit, because the squash merge title becomes the commit on main and the changelog entry. That check is advisory and not part of the gate.
- **Dependabot** watches the documentation site's npm dependencies, grouped weekly. The title of its pull requests follows the same convention.
- **Scripts have tests.** Every script under `.github/scripts` has a test next to it, and the change detection job runs them first, so a broken script is caught before it makes a wrong decision.

## What was traded away

- **No second run on main.** A bad merge result that the queue did not catch would show up at the nightly run, not at once. The queue tests the exact tree, so this is rare.
- **A skipped job counts as passed.** The path scoping is only as good as its rules, which is why they err towards running more and why the nightly runs every job.
- **The sniffer job depends on another repository.** An outage of the sniffers repository or a missing release asset blocks the pull requests that run it. The remedy is to re-run once, like any flaky dependency.
- **Informational scans.** Trivy and the advisory audit report and never block, because new advisories land on their own schedule and would otherwise break unrelated pull requests.
- **Cold Mondays.** Weekly cache resets cost one slow day of builds in exchange for bounded cache size.
- **Merge queue runs are slow.** The queue builds on a temporary branch and a queued change waits for those ahead of it.

## Where to look

- `.github/workflows/ci.yml`: every job and the gate.
- `.github/scripts/changes.py`: the area logic and its test.
- `.github/workflows/cache-warm.yml` and `.github/actions/cargo-cache/`: the cache.
- `.github/workflows/pr-title.yml` and `.github/dependabot.yml`: the small checks.
- `.github/scripts/`: the helpers for reports, pins, links and policy checks.
- `.config/nextest.toml`: the profile used by CI.
