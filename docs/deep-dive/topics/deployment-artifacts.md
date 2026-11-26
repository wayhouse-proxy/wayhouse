---
title: "Deployment artifacts: images, compose, Kubernetes and systemd"
description: How the reference container images are built from one Dockerfile, which choices keep them small, pinned and scannable, and how the compose, Kubernetes and systemd material is kept true by lint and smoke tests instead of by promise.
---

Documentation that shows a compose file is only as good as the last time someone ran it. The `deploy/` directory is **reference material**: images, a compose demo and a single box bundle, plain Kubernetes manifests and systemd units. They are not a supported product. They are, however, **built and tested on every relevant change**, so that what the manual shows still starts. This page explains how the artifacts are put together and which failures the checks were written to catch. The options are in the [Manual](../../manual/index.md). The pipeline that tests all this is in [the CI pipeline](ci-pipeline.md) and the one that publishes the images in [the release pipeline](release-pipeline.md).

## The problem

1. **Many programs, one way to ship them.** There are a proxy, a minimal proxy, a controller, an aggregator, a UI, an all in one control program, an agent and a Caddy. Eight images that drift apart would be eight sets of bugs.
2. **Images are an attack surface.** Every package in an image is a place for a vulnerability, and a base image that changes under a tag changes the product.
3. **Container demos rot,** because the failures they hit only show up on a real `up` or in a real cluster.
4. **A builder and a runtime must agree.** A binary built against a newer C library than the runtime has fails at start, long after the build passed.
5. **There is no `.git` in a Docker build,** yet every binary reports its commit.
6. **Secrets must not appear in a manifest,** and the UI must never hold the admin token.

## One Dockerfile, eight targets

A single file defines every image as a target: `wayhouse`, `wayhouse-minimal`, `wayhouse-controller`, `wayhouse-aggregator`, `wayhouse-ui`, `wayhouse-control`, `wayhouse-agent` and `wayhouse-caddy`. The structure is deliberate:

- **One builder compiles every binary,** so dependencies compile once. The minimal proxy is built first, with the optional features off, and set aside, so that the full build that follows leaves the full binary in place.
- **The frontend is built in its own stage,** used only by the UI and the all in one targets.
- **The binaries come from a switch.** `BIN_SOURCE` picks the **builder** (compile inside Docker, self contained, the default) or **prebuilt** (copy binaries compiled outside from a directory). CI uses prebuilt to share one release compile among jobs, and the nightly run and the release use the builder, so the self contained path cannot rot.
- **Runtime stages are distroless,** `cc-debian13` with the `nonroot` user (uid 65532), which has CA certificates and no shell. The builder is `rust:1-trixie` and the glibc versions are chosen to match, which is the reason the CI runner image is pinned (see [the CI pipeline](ci-pipeline.md)).
- **Every base image is pinned by digest,** with the tag kept for readers. A rebuild of a release tag therefore gives the same bases. The header says how to bump one: resolve the tag's index digest, replace it everywhere it appears and let the build, smoke test and scan judge it.
- **`/data` is created and owned by the runtime user in the image,** so a fresh named volume or claim inherits a writable directory. The proxy needs it for the tunnel key file.
- **The binaries embed their dependency list.** The build tool that does this adds a few kilobytes so that an image scanner sees the crates a binary was built from, since a stripped Rust binary otherwise shows nothing.
- **The rust toolchain file is deliberately not copied,** or rustup would fetch formatter and linter components during the build.

The default start commands make each image runnable: the proxy takes a config path, the controller a listen address and data directory, and so on. The UI and the all in one target carry the built frontend at `/web`.

### The commit is passed in

Docker has no `.git` (it is in `.dockerignore`), so the commit reported by every binary comes from a build argument that the build script, the compose file and the release workflow all set. A build that ends up with `unknown` is a bug. The image build script **checks it**: the images are distroless, so the check copies the binary out of a created container and searches it for the commit string. See [the release pipeline](release-pipeline.md) for how the string is compiled in.

### The Caddy image

The Caddy image is stock Caddy with one reason to rebuild it. The official image's Go toolchain lagged the Go security releases, and the scanner flagged several standard library packages and two modules. So a build stage compiles the **pinned Caddy version with a patched Go and two replaced modules**, and the final stage, the official image of the same tag, has that binary swapped in. The same stage:

- removes the **file capability** the base image grants to bind low ports. The proxy runs unprivileged on high ports, and a file capability makes `exec` fail once a deployment drops all capabilities, which is common Kubernetes hardening,
- drops the curl packages that the scanner flagged and nothing needs,
- ships the licence text and the attribution, and the base configuration that the sync expects (see [the web gateway](web-gateway.md)),
- creates the admin socket directory with the same uid as the other images, since the sync shares it and the socket is owner only.

A check script then verifies the result: the image reports the pinned version, starts with every capability dropped, carries the base configuration, **contains the pinned Go toolchain and the replaced modules**, and does not downgrade a replaced module below what Caddy itself asks for. A dropped replace line or a silently older toolchain would otherwise pass the plain version check. A bump of the build stage without the final stage would ship a binary the base does not match, which is why the two versions are checked together. A separate check holds the list of extra Caddy modules, empty today, to the rule that a module needs a recorded licence.

## Static checks that pin real failures

`lint.sh` runs checks that need the Docker CLI and no daemon, mostly by **rendering** the compose files and the Kubernetes overlays and asserting things about the result. Each check is the memory of a failure that only appeared on a real `up` or in a cluster. A sample of what they pin:

- **Example tokens meet the 16 byte minimum** that the services enforce, or the demo controller exits at start.
- **A rendered command must not interpolate a variable** that was meant to be a value. Distinct sentinel values make this visible, since a rendered file holds values and not variable names.
- **Every Kubernetes overlay must build,** and the rendered DaemonSets are checked, not the source files.
- **The proxy reports a unique instance name,** because the default would collide between nodes.
- **The seed image's curl rejects `--opt=value`,** so options must be separate arguments, or the seed step exits and the proxy never starts.
- **The replicated controller overlay** swaps the single writer deployment and its shared volume for a three node stateful set, takes node ids from the pod ordinal and passes the peer token as a file.
- **TLS between services** uses a secret that `wayhouse-controller init` makes. Controller and aggregator serve it, every client trusts the authority, and the plain HTTP opt out survives only where the aggregator must reach node admin URLs that are plain.
- **No key sits in a manifest,** since init creates the secret and the lint never renders it.
- **Per node enrolment** puts the code in a secret that is never in a manifest, names the pod after its node and keeps the shared tokens off the proxy.
- **The UI never holds the admin token.** It presents the scoped UI token, and the controller is started with it.
- **Network capability** is in the bounding set only for non root, since tunnel services run as root, and `net.*` sysctls are not set in the host network namespace, where the runtime refuses them.
- **Release images must not honour the test protocol overrides.** The cargo feature that lets the fleet tests fake a protocol version must not be enabled by anything that builds or ships an image.
- **Prebuilt mode must agree end to end:** the stage selector, the prebuilt stage, the runtime stages and the build plumbing.
- **The observability references are checked:** every metric name used by the shipped dashboard and alert rules must exist (see [metrics and logs](metrics-and-logs.md)).
- **A failing `up` must still dump logs and tear down,** in a project that cannot clobber the demo.

## Smoke tests

The smoke script runs against a started compose demo and **polls**, up to a minute for each check, since services start asynchronously. It proves the real wiring end to end:

- every service answers its health route,
- the controller serves the seeded config, which proves the proxy pulled revision one,
- the aggregator reports the proxy instance, which proves the controller and aggregator wiring and the token between them,
- the UI login works with the configured password,
- **every service's build metric carries a real commit,** not `unknown`,
- **a wrong token is refused,** proving the gate is actually on.

A second script adds what the single box bundle needs, and runs for the all in one overlay as well, over TLS from the authority that init made. The matching images are also built and smoke tested on arm64 (see [the CI pipeline](ci-pipeline.md)).

## The shapes

- **Compose.** A runnable control plane demo, with overrides for the WireGuard tunnel and the Caddy web front door, and the **single box bundle**: the demo plus plugins, a sealed secret store and HTTPS from the first start. Its overlays add automatic certificates and a variant that runs the controller, aggregator and UI as one process (see [wayhouse-control](all-in-one-control.md)). Ports are published only on loopback. A private key is mounted only by the service that serves with it, and everything else gets the authority volume.
- **Kubernetes.** Plain manifests: a namespace, an example secret, the controller, aggregator and UI, the proxy's permissions and its DaemonSet. Kustomize overlays add a single controller, a three node replicated one, enrolled variants of each, and the web gateway as sidecars. Components add the agent and per node enrolment. No manifest carries a key.
- **systemd.** The same single box bundle as units under one target, with a sysusers file, a key generation unit and a drop in for the all in one variant. The units use credentials for secrets, and the web gateway units are separate and optional.
- **Observability.** A reference dashboard and alert rules, checked as above.
- **A solo initialiser.** A thin wrapper over `wayhouse-controller init`, which writes tokens, a certificate authority and certificates for compose, systemd or a Kubernetes secret and prints no secret.

## Scanning

The scan script runs Trivy over each image and the lockfiles they were built from, **informational**: it exits non zero locally if anything was found, and CI reports without blocking. It looks at the operating system packages of the distroless base, the crates that the embedded dependency list names, and secrets in the image. Lockfile scans cross check the images and catch a binary built without the embedded list. For arm64 the report names carry a suffix and the lockfile scans are skipped, since the files and findings are the same. Accepted findings go in an ignore file, each with a reason (see [the CI pipeline](ci-pipeline.md) for how the scanner itself is verified).

## What was traded away

- **Reference only.** The demo and manifests are examples, not a supported deployment, so a site still has to adapt them.
- **Distroless means no shell.** Debugging an image needs a sidecar or a copy out of a container. That is also why the commit check copies the binary out.
- **Pinned digests need maintenance.** A base image does not update itself, and the Caddy build arguments are not tracked by the dependency bot, so bumps are manual and checked by CI.
- **Rebuilding Caddy is a burden** taken on only because of the lag in the official image, and the Dockerfile says to drop the stage once the lag ends.
- **Lint by rendering is not a cluster.** It catches the failures that were seen before, and the smoke tests catch more, but a manifest that is valid and wrong in a new way is still possible.
- **One image per program.** Eight images are more to publish than one multi tool image, in exchange for each carrying only what it runs.

## Where to look

- `deploy/Dockerfile`: the targets and the Caddy build stage.
- `deploy/build-images.sh`, `check-image-commit.sh` and `check-caddy-image.sh`: building and checking images.
- `deploy/lint.sh`: the static checks.
- `deploy/smoke.sh` and `solo-smoke.sh`: the end to end checks.
- `deploy/scan-images.sh`: the scanner.
- `deploy/compose/`, `deploy/k8s/`, `deploy/systemd/` and `deploy/solo/`: the shapes.
- `deploy/observability/`: the dashboard and alert rules.
