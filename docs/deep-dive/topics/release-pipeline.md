---
title: "The release pipeline: tag, verify, build, publish and sign the feed"
description: How a version number becomes a set of multi-architecture images that cannot be half published, why the maintainer keeps the tag in their own hands, and how the version and commit get compiled into every binary.
---

A release is the moment when a mistake stops being private. The pipeline is built around one rule: **nothing becomes public until everything that can be checked has passed.** This page explains the steps from a merged change to a published image and a signed notice, and the parts that deliberately stay manual. The CI that runs before is in [the CI pipeline](ci-pipeline.md), and the images themselves in [deployment artifacts](deployment-artifacts.md). The signed feed and its format are in [registries and the release feed](registries-and-release-feed.md).

## The problem

1. **Tag, image and binary must agree.** An operator reading `--version`, an image tag and a metric label must see the same number.
2. **A half published release is worse than none.** One architecture published and the other failed leaves a registry that cannot be pulled on half the fleet.
3. **`latest` must never move backwards,** and a pre-release must never become it.
4. **A release is irreversible,** so deciding to cut one is a human act, while the tedious parts should be automatic.
5. **Signing keys are the most valuable secret in the repository,** and the release job builds code.
6. **A source tarball must still build.** The commit hash compiled into a binary cannot require a `.git` directory.

## One number, compiled in

The version in the workspace manifest is the **single source of truth**. Every binary reports it with `--version`, and the build also records a **short commit hash** of 12 characters. The build scripts of the crates take that hash from, in order: the `WAYHOUSE_GIT_SHA` environment variable, `git rev-parse`, and finally `"unknown"`, with a blank value counting as unset. The environment comes first because a Docker build has no `.git` directory, so the CI and the image build script pass the hash in. A source tarball with no git history still builds and reports `unknown`.

The build script asks git where `HEAD` and its reflog live, rather than watching `.git/HEAD` itself, because on a branch that file holds a branch name and does not change between commits. Asking git also makes it work in a worktree, where `.git` is a file. The result is one string, `<version> (<commit>)`, printed by every binary and exposed as a metric label (see [metrics and logs](metrics-and-logs.md)), so a deployment check can look for it.

### Versions stay 0.x

Under 0.x a **minor** bump is anything breaking: the config schema, flags, the HTTP APIs, wire protocols, the sniffer ABI, metric names and on disk formats. A **patch** bump is fixes and compatible additions. Moving to 1.0 is the maintainer's explicit decision and never something tooling does: a policy script fails any manifest that says 1.x or more and any lockfile that disagrees with the manifest, and the only way past it is a deliberate, reviewed edit of the script. The script runs in the CI's checks and again in the release workflow.

## A pull request that proposes the release

A tool called release-please keeps **one release pull request open** on main, titled with the next version. It bumps the manifest and writes the changelog from the **conventional commit titles** of the merged pull requests. Because squash merges make the pull request title the commit, **the title is the changelog entry**: `feat` and `fix` appear, and docs, CI and test changes are hidden. Under 0.x both a feature and a fix bump the patch, and only a breaking change bumps the minor. A minor bump for a compatible addition is requested for one release only, by naming the version in the config and removing it afterwards.

The tool has two limits that the workflow fills in:

- **It cannot update the lockfile reliably,** so a step runs a workspace update on the release branch, and the CI check then proves the manifest and the lockfile agree.
- **A pull request created with the default token does not start workflows,** and the CI gate is required. So the workflow dispatches the CI on the release branch itself after every run. If that ever fails to satisfy the branch rule, a fine grained token is the documented fallback.

### The tag stays manual

The tool is run with its release creation switched off. **It never tags and never publishes.** The maintainer merges the release pull request, waits for CI on the merge commit, and pushes the tag by hand. That keeps the publish workflow's checks in a defined order and keeps the one irreversible step with a person. Because the tool never sees a tag, it would stop opening release pull requests while a merged one is still marked as pending, so a first step in the workflow flips that label to "tagged" once the tag exists.

A pre-release is not made by the tool. Its version is set by hand in a normal pull request, and its tag carries the suffix. A release branch for a batch of interdependent work is documented as an alternative to the default trunk flow, with cherry picks recorded using `-x`, and nothing is backported automatically.

## What the tag triggers

Pushing a tag of the form `v1.2.3` (with an optional suffix) runs one workflow, and **nothing else triggers it**. It has four stages.

### 1. Verify

- **The tag is `v` plus the workspace version.** A small script compares them and prints the version. The tag, the image tags and what `--version` and the build metric report are therefore one number.
- **Only the highest stable tag moves `latest`.** A lower stable tag pushed later, such as a backport, must not move it backwards, so the job lists the tags, sorts them as versions and compares.
- **The tagged commit is on main,** proved with an ancestry check against main.
- **CI passed on that exact commit.** The job asks for a successful CI run for it. The error message says what to do if there is none: wait, or run the CI by hand on main when the commit changed only docs.

### 2. Images, one native build per architecture

The eight reference images are built on **native runners** for amd64 and arm64, with no emulation and no cross compiling, using the self contained builder path inside Docker. Each image must report the right version and carry the commit, and the **compose smoke test** must pass on the freshly built images. The Caddy image is checked against the Caddy version pinned in the Dockerfile instead, since its version is not wayhouse's. A failure on either architecture ends the run with nothing public.

The tested images then get **one extra metadata layer** carrying the standard labels, source, version, revision and licence, with the same filesystem layers beneath. The Caddy image is Apache-2.0, and the others are dual licensed. They are saved as a tarball per architecture and handed on.

### 3. Publish, in an order that cannot mislead

Only this job has permission to write packages. It loads both tarballs and pushes in two phases:

1. the **per architecture tags**, `<version>-amd64` and `<version>-arm64`, for every image,
2. then the **multi-architecture tags**, built as manifests that point at both, first every version tag and only then the moving `latest`.

The order means **a registry error part way through never moves `latest` onto a mixed version set.** A pre-release gets its own tag and never moves `latest`. A build or smoke failure on either architecture leaves nothing public, and only a registry error mid push can leave a partial set. The pushes are idempotent, so the answer is to re-run the workflow.

### 4. The release feed

After publish, a job creates the GitHub release if there is none, builds the feed from the releases list, signs it and attaches it with its signature (the format and the reasons for a separate key are in [registries and the release feed](registries-and-release-feed.md)). It runs **after** publish so that a problem here, such as a missing signing secret, turns the job red but never withholds the images.

The secrets are **split by step**, so code that runs in one step never sees the secret of another:

- **building the tool** runs dependency build scripts and procedural macros, which execute arbitrary code, so that step has neither the signing key nor the token,
- **signing** has only the two signing secrets and runs nothing but the signing program, with the key written to a temporary file that is removed on exit and a trusted comment that names the feed,
- **the token** goes only to the release client and to the project's own checking tool.

The signing secrets live in a protected **environment** limited to version tags, with the maintainer as a required reviewer, so the job waits for approval when a tag is pushed. Before attaching, the job **verifies the signature the way a client does**, using the public key compiled into the build it just made, so a feed that would not verify in the field is never uploaded. A pre-release never becomes the feed's latest.

## What was traded away

- **A person pushes the tag.** One more manual step per release, in exchange for the irreversible act being a decision. The workflow's checks stand between a wrong tag and the registry.
- **Everything is rebuilt at release time,** inside Docker, instead of reusing the CI artifacts. It costs the longest job in the pipeline and makes the published bytes the ones that were tested, with the self contained build path exercised nightly so it does not rot.
- **A partial publish is possible.** The two phase push narrows it to registry errors, and idempotent pushes make the repair a re-run, but it is not atomic.
- **No automatic patching of old releases.** Every fix ships as a new tag from main, which is fine before 1.0.
- **The release feed can fail after the release.** That is the intended order. A signing problem should not block users from the software.
- **Two kinds of keys to protect** (the feed's and the registry's), as described on the registry page.

## Where to look

- `.github/workflows/release.yml`: verify, images, publish and the feed.
- `.github/workflows/release-please.yml` and `.github/release-please/`: the release pull request.
- `.github/RELEASING.md`: the procedure, versioning rules and branching models.
- `.github/scripts/check_release.py` and `check_version_policy.py`: the tag and version checks.
- `crates/wayhouse-http/build.rs` and `build_sha.rs`: the commit and version string.
