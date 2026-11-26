---
title: Enroll a node
description: Join a proxy, agent, aggregator, UI or controller replica to a controller with a one-time code, and run it as a systemd unit.
sidebar:
    order: 3
---

Every node binary (proxy, agent, aggregator, UI and controller; `<node-binary>` below) has two shared subcommands, `enroll` and `install`. They replace copying tokens around by hand: you mint a one-time **enrollment code** on the controller, redeem it on the node, and the node saves the secrets it needs.

## Redeem a code

```sh
<node-binary> enroll \
  --controller-url https://controller.example:9900 \
  --pin sha256:<hex> \
  --code-file /root/enroll-code
```

| Flag                 | What it does                                                                                                                                                      |
| -------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `--controller-url`   | Base URL of the controller. Must be `https://`; plain `http://` is accepted only for loopback.                                                                    |
| `--pin`              | The controller's TLS leaf certificate as `sha256:<hex>`. The UI shows it when you mint the code. Cannot be combined with `--controller-ca`.                       |
| `--controller-ca`    | A PEM file with the CA that signed the controller's certificate, trusted instead of the built-in roots.                                                           |
| `--data-dir`         | Where the node's files are written (mode 0700). Default `wayhouse-<role>-data`.                                                                                   |
| `--code`             | The code on the command line. It is visible in `ps`, so prefer one of the options below.                                                                          |
| `--code-file`        | A file holding the code.                                                                                                                                          |
| `--name`             | This node's name, for a code minted with a name prefix (one code for many pods, such as a DaemonSet's init container). The controller refuses a name outside it.  |
| `--advertise`        | Controller replicas only: the address the other replicas reach this one on, as `--ha-peers` takes it (`https://host:port`). The cluster dials it to add the node. |
| `--skip-if-enrolled` | `enroll` only: exit 0 without redeeming when `--data-dir` is already enrolled, so an init container that runs again does not spend another use of the code.       |

The code is looked up in this order, and the first one found wins:

1. `--code`
2. `--code-file`
3. the file named by `WAYHOUSE_ENROLL_CODE_FILE`
4. the value of `WAYHOUSE_ENROLL_CODE`
5. a prompt, when you run it on a terminal

If none of these gives a code, the command stops with an error that names the options.

## Run the node as a systemd unit

`install --systemd` enrolls and then writes a unit that starts the node with its secrets loaded as systemd credentials. It needs root. It takes every flag of `enroll` plus these:

| Flag           | What it does                                                                                                                      |
| -------------- | --------------------------------------------------------------------------------------------------------------------------------- |
| `--systemd`    | Required. systemd is the only target so far.                                                                                      |
| `--unit-dir`   | Where the unit file is written. Default `/etc/systemd/system`.                                                                    |
| `--no-start`   | Write the files but do not run `systemctl`.                                                                                       |
| `--bin-path`   | Path of the binary the unit runs. Default: the running executable.                                                                |
| `--credential` | An extra `LoadCredential=` as `NAME=PATH`, for a secret enrollment does not deliver (the UI's `ui-password`). Repeat it for more. |
| `-- ...`       | Everything after `--` is passed to the binary itself, for example `-- --backends :25565`.                                         |

```sh
<node-binary> install --systemd \
  --controller-url https://controller.example:9900 --pin sha256:<hex> \
  --code-file /root/enroll-code \
  -- --backends :25565
```

A value that cannot be written safely into a unit file (for instance one with a line break) is refused. `%` and `$` in the arguments are escaped, so systemd does not expand them.

## Revoke a code and sign a mint

A code can be revoked whether or not it was spent. Over the API it is `DELETE /api/enrollments/{id}` with the admin role. Revoking a spent code also revokes the token it produced.

A controller holds the keys of the fleet, so minting a code for the roles `proxy`, `ui` or `controller` needs the admin token **and** a **step-up assertion**, signed over the exact request with a 60 second expiry. A mint made with the UI token needs one for every role. Without `--enroll-step-up-pubkey-file` those mints cannot be made at all. The signing key is derived from a passphrase and a salt, and the salt is not a secret. `GET /enrollments/step-up` on the controller returns it, and answers `409` when the controller has no step-up key. When you send `"step_up": "<passphrase>"` in the body of `POST /api/enrollments`, the UI backend fetches the salt, signs the request and forwards it, so the passphrase never reaches the controller. `wayhouse-controller step-up sign` does the same from a shell.

## Related

- [Configuration](../../05-configuration.md) for the settings the node reads once it runs.
- [Upgrading](../../upgrading.md) for moving a fleet to a new version.
