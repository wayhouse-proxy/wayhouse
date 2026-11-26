---
title: Install
description: Container images, a single-box bundle, systemd units or a build from source.
sidebar:
    order: 2
---

wayhouse is a set of small programs. For one proxy you only need `wayhouse`; the rest add
central configuration for a fleet.

| Program                                  | What it does                                                                     | Needed for                         |
| ---------------------------------------- | -------------------------------------------------------------------------------- | ---------------------------------- |
| `wayhouse`                               | The proxy: listeners, routing, forwarding.                                       | Always.                            |
| `wayhouse-controller`                    | Stores the config, hands it to proxies, hosts plugins, secrets and certificates. | Central config, plugins, HA.       |
| `wayhouse-aggregator`                    | Reads and acts on a whole fleet.                                                 | The admin UI, fleet-wide actions.  |
| `wayhouse-ui`                            | The admin web UI.                                                                | The admin UI.                      |
| `wayhouse-control`                       | Controller, aggregator and UI in one process.                                    | A single box that wants all three. |
| `wayhouse-agent`                         | Carries traffic from an origin behind a WireGuard tunnel.                        | Origins on another network.        |
| `wayhouse-web-sync`, `wayhouse-certsync` | Feed the Caddy web gateway.                                                      | Publishing websites.               |

> [!WARNING]
> The major version is 0, so breaking changes land in minor versions, and the deployment
> files in `deploy/` are reference material, not a supported product. Each release's upgrade
> steps are in [Upgrading a fleet](../../upgrading.md).

## Containers

The images are public on GHCR and need no login: `ghcr.io/wayhouse-proxy/<image>`, one image per
program, as a multi-architecture manifest for `linux/amd64` and `linux/arm64`. A tag is a
version number without a leading `v`; `latest` follows the newest release that is not a pre-release.

The images are built on a distroless base and run as user 65532 with no shell. The proxy
needs the host network for the ports it listens on; see the Compose files for how the demo does
that.

## One box with Docker Compose

The solo bundle runs the proxy, the controller (with plugins and sealed secrets), the
aggregator and the UI on one machine, all bound to the loopback address. It is the
recommended starting point; [Set up a single box](basic-setup.md) walks through it.

## One box with systemd

The same bundle as systemd units, without containers. Install the binaries into
`/usr/local/bin`, then follow the steps in
[`deploy/systemd/README.md`](../../../deploy/systemd/README.md). The UI also needs the built
frontend in `/usr/share/wayhouse/web`.

## Kubernetes

Plain manifests live in `deploy/k8s/`, with overlays for a single controller, per-node
enrollment, a three-node HA controller and the web gateway. They are only validated by tooling
in CI so far, not applied to a real cluster, so treat them as a starting point. See
[`deploy/README.md`](../../../deploy/README.md).

## From source

You need a Rust toolchain (the repository pins `stable`) and `protoc`:

```sh
git clone https://github.com/wayhouse-proxy/wayhouse.git
cd wayhouse
cargo build --release          # binaries land in target/release/
make ui                        # the UI frontend, only if you run wayhouse-ui
```

Optional Cargo features can be switched off to slim the proxy down (for example
`--no-default-features`); the [feature flags reference](../../reference/index.md) lists them.

## Check the version

Every binary answers `--version` with its version and the commit it was built from:

```sh
wayhouse --version
```
