---
title: Get started
description: Run the proxy with an example config in a few minutes.
sidebar:
    order: 1
---

This page gets one proxy running with the annotated example config, checks that it is
healthy and shows how to change the config without dropping players. Everything else in the
manual builds on it.

> [!WARNING]
> wayhouse is work in progress and the major version is 0, so breaking changes land in minor
> versions. Don't put real players behind it yet.

## What you need

- A Linux or macOS machine with a Rust toolchain (the repository pins `stable` in
  `rust-toolchain.toml`).
- `protoc`, because the gRPC resolver client is generated at build time
  (`apt install protobuf-compiler` or `brew install protobuf`).

Prefer containers? The public images on GHCR need no login; see
[`deploy/README.md`](../../../deploy/README.md) for the image list and the Compose demo.

## Run it

```sh
git clone https://github.com/wayhouse-proxy/wayhouse.git
cd wayhouse

# validate the example config without starting anything
cargo run --release -p wayhouse -- --config deploy/config.example.yaml --check

# run the proxy with it
cargo run --release -p wayhouse -- --config deploy/config.example.yaml
```

`--check` parses and validates the file and exits. It is the first thing to run after every
edit, and it is safe to run next to a live proxy.

`deploy/config.example.yaml` is the annotated reference config. Copy it, point the listeners at your
own backends, and keep the file in version control.

## Check that it is up

The admin API listens on `127.0.0.1:9900` by default:

```sh
curl -s localhost:9900/healthz    # the process is alive
curl -s localhost:9900/readyz     # at least one listener is configured and the instance is not draining
curl -s localhost:9900/pools      # every pool, its backends, their health and state
curl -s localhost:9900/sessions   # every live connection or UDP session
```

[Monitor a proxy](../operate/monitor.md) covers the rest of the endpoints and the metrics.

## Change the config

Edit the file, or send `SIGHUP` (`kill -HUP <pid>`). The proxy builds a new snapshot,
swaps it in atomically and carries backend health over. Listeners that changed are
reconciled by name: stopped and respawned only if their definition differs, added or removed
if they appeared or vanished. A config that fails validation is rejected and the running
config stays in place; the reason is in the log.

Some settings are read once at startup and need a restart: `settings.workers`, `settings.admin.listen`
and `settings.admin.tls`, `settings.limits`, `settings.geo_db` and the `settings.sniffers` block. Pools,
listeners, routes, resolvers and `backend_sources` reload live. The comments at the end of
`deploy/config.example.yaml` list what is live and what is not.

## Logging

Set the level with the `WAYHOUSE_LOG` environment variable, for example
`WAYHOUSE_LOG=debug`.

## Where next

- [Forward a game port to backends](first-listener-and-pool.md): your first listener and pool.
- [Route players by rules](../routing/route-by-rules.md): several pools behind one port.
- [Protect a listener](../security/protect-a-listener.md): allow lists, rate limits, caps.
- [Keep the client IP](../routing/keep-client-ip.md): so backends see real addresses.
- [Drain and maintain](../operate/drain-and-maintain.md): restarts without kicking players.
