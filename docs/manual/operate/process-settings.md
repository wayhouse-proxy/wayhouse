---
title: Change launch flags from the console
description: Edit the command line flags of the proxy, the controller, the aggregator and the UI from the Process flags page, with an automatic undo unless you confirm.
sidebar:
    order: 6
---

The configuration file says what the fleet does. The **launch flags** of each program, such as listen addresses, limits and timeouts, normally live in a service unit. With a settings directory you can change many of them from the console, and every change is a **trial**: the program restarts with the new value, and goes back to the old one by itself unless you confirm.

## Turn it on

Give every program the same directory with `--settings-dir DIR`, and the UI as well, so it can read and write the files.

```sh
wayhouse --config wayhouse.yaml --settings-dir /var/lib/wayhouse/settings
wayhouse-ui --settings-dir /var/lib/wayhouse/settings ...
```

Each program keeps its files in `DIR/<program>/`. The program writes the list of flags it offers there every time it starts, and reads `settings.json` for values. **The command line always wins:** a flag you give on the command line cannot be overridden by the file. A program that has never started with the directory shows as "not connected". Run it with a restart policy (systemd `Restart=on-failure` or a container restart policy), because a change works by restarting the program.

If the programs run as different users, create each `DIR/<program>` beforehand, owned by that program's user, with a group the UI's user belongs to. A group writable directory gives the UI access to one program's files without giving any program access to the rest. With `wayhouse-control` the directory is shared by the three programs.

## Change a flag

Open `/settings/process`, which needs the admin role to change anything. A viewer sees the flags and whether a change is waiting, and no values, because a listen address or a data directory is for operators to see.

1. Edit the value in the program's section. Changed flags are marked.
2. Apply. The UI checks the value's shape, the program runs its own parser on it, and the program restarts.
3. **Confirm within 60 seconds** of the restart. Confirming makes the values final.
4. If you do nothing, or the program cannot start with the value, the previous values are put back and the program restarts again. After three failed starts, or five minutes, the old values are restored before anything else reads them.

Only one trial can run per program at a time, and a change is refused if the file moved since you opened the page. If the program itself refuses the new values it keeps running, puts the old values back and shows the reason on the page.

## What you cannot set here

Some flags are never read from the file, and the page says why next to each:

- flags that act once, such as `--check` and `--hash-password`,
- **credentials**: tokens, token files, passwords and pre shared keys,
- **destinations**: URLs, files, certificates and keys, the config path and the peers of a replicated controller, because they say where tokens go or what is trusted,
- **safety overrides**: anything starting `insecure-` or `allow-insecure-`.

Set those on the command line or in the service unit, where a person chose them.

## The HTTP API

The UI exposes the same operations. Reads need the viewer role. The rest need admin.

| Request                               | What it does                                                                                                                                                              |
| ------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `GET /api/process`                    | Every program with its published flags, its file (revision, values, previous values and any pending trial) and whether it is connected.                                   |
| `PUT /api/process/{program}`          | Start a trial. Body `{"values": {...}, "rev": N}` where `rev` is the revision you read. `400` for an invalid value, `409` on a stale revision or a trial already pending. |
| `POST /api/process/{program}/confirm` | Make the pending change final. Body `{"rev": N}`.                                                                                                                         |
| `POST /api/process/{program}/undo`    | Put the previous values back now. Body `{"rev": N}`.                                                                                                                      |

`{program}` is `wayhouse`, `controller`, `aggregator` or `ui`. Without `--settings-dir` the UI answers `404`.

How the trial and the guard work is in [the deep dive](../../deep-dive/topics/process-settings.md).
