---
title: Tokens and secrets
description: Where each binary reads its tokens from, the --*-token-file flags, and how to generate a first set with the controller's init command.
sidebar:
    order: 2
---

wayhouse binaries never take a token or password as a command-line value, because that value would show up in `ps`, `/proc/<pid>/cmdline` and `docker inspect`. Every secret has the same three sources.

## The lookup order

For a secret called `<name>` (for example `auth-token`), the first source that exists wins:

1. the file named by `--<name>-file`,
2. `$CREDENTIALS_DIRECTORY/<name>`, which is what systemd's `LoadCredential=` provides,
3. the environment variable `WAYHOUSE_<NAME>` with dashes turned into underscores (for example `WAYHOUSE_AUTH_TOKEN`).

A trailing newline in a file is dropped. An empty secret is an **error**, never "no secret", so a mis-mounted file cannot switch authentication off by accident.

## Token files by binary

| Flag                             | Used by                   | The token is for                                                                                                                                        |
| -------------------------------- | ------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `--aggregator-token-file`        | `wayhouse`, `wayhouse-ui` | What the proxy presents on every push to `--aggregator`, or what the UI presents to `--aggregator-url`, when the aggregator has its own `--auth-token`. |
| `--instance-token-file`          | `wayhouse-aggregator`     | What the aggregator presents when it fans a command out to each proxy's admin API (`settings.admin.auth_token`). A separate secret from `--auth-token`. |
| `--plugin-overlay-token-file`    | `wayhouse`                | What the proxy presents to `--plugin-overlay-url`, if the controller requires one.                                                                      |
| `--tunnel-controller-token-file` | `wayhouse`                | What the proxy presents to `--tunnel-controller-url`. See [Reach origins over WireGuard](../pools/wireguard-tunnel.md).                                 |
| `--ha-join-proof-file`           | `wayhouse-controller`     | The join proof that came with a controller replica's enrollment code. See [Enroll a node](../operate/enroll-a-node.md).                                 |

The environment variables follow the rule above: `WAYHOUSE_AGGREGATOR_TOKEN`, `WAYHOUSE_CONTROLLER_TOKEN` (the token a UI or node uses to reach the controller) and `WAYHOUSE_INGEST_TOKEN` (the telemetry token the controller hands to an enrolled aggregator or proxy, the same secret as the aggregator's `--ingest-token`) all name a secret the same way.

Bearer tokens are only sent over `https://`. Plain `http://` to a host that is not loopback is refused unless you pass `--allow-insecure-controller`, which is meant for a network that is private by construction.

## Generate a first set of secrets

`wayhouse-controller init` creates the secrets and certificates for a first controller. The `--target` decides where they go:

| Target       | Writes                                                                                         |
| ------------ | ---------------------------------------------------------------------------------------------- |
| `systemd`    | Credential files for `LoadCredential=` plus the certificates. Needs no Docker.                 |
| `compose`    | An env file for `docker compose` plus the certificates.                                        |
| `k8s-secret` | Secret manifests on stdout, or in the file named by `--output` (mode 0600, never overwritten). |

| Flag                | What it does                                                                                                                                                   |
| ------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `--dir`             | Base directory. Default `/etc/wayhouse` for `systemd` and the current directory for `compose`.                                                                 |
| `--credentials-dir` | `systemd` only: the credentials directory, instead of `<dir>/credentials`.                                                                                     |
| `--env-file`        | `compose` only: the env file to write, instead of `<dir>/.env`.                                                                                                |
| `--tls-dir`         | The certificate directory, instead of `<dir>/tls`.                                                                                                             |
| `--san`             | An extra DNS name or IP the certificates must be valid for. Repeat it. `localhost`, `127.0.0.1`, `::1` and the target's own service names are always included. |
| `--namespace`       | `k8s-secret` only: the namespace of the Secrets and of the service names in the certificates. Default `wayhouse`.                                              |
| `--control`         | Lay out for `wayhouse-control` (controller, aggregator and UI in one process): one `control.pem` valid for all three.                                          |

## Cluster secrets in the console and API

Secrets that a config refers to as `{ secret: name }` are sealed by the controller. The console page `/settings/secrets` and the API show **names, key ids and times, and never values**.

| Request                      | Role  | What it does                                                                                 |
| ---------------------------- | ----- | -------------------------------------------------------------------------------------------- |
| `GET /api/secrets`           | admin | The secret names with their key id and when each was set.                                    |
| `PUT /api/secrets/{name}`    | admin | Seals a new value. The body carries the value, and the UI backend neither logs nor keeps it. |
| `DELETE /api/secrets/{name}` | admin | Removes the secret.                                                                          |
| `POST /api/secrets/rewrap`   | admin | Re-seals the cluster secrets under the current key, as a step of a key rotation.             |

The sealed form that proxies fetch is not reachable through the UI.

## Related

- [Protect a listener](protect-a-listener.md) for limits and filters on the data path.
- [Enroll a node](../operate/enroll-a-node.md) to hand a node its secrets with a one-time code instead.
