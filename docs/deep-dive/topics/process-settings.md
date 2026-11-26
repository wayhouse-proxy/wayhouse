---
title: "Process settings: editing flags from the console with a safety net"
description: How an operator changes a program's launch flags from the UI without a shell, why every change is a trial that undoes itself unless confirmed, and which flags the mechanism refuses to touch.
---

The configuration file says what the fleet should do. The **launch flags** of the programs, such as listen addresses, limits and timeouts, say how each program itself runs, and they normally live in a service unit that only someone with a shell can edit. The settings mechanism lets the console change them, and it is built around one fear: **a wrong value can stop the very program that would be used to fix it.** This page explains the file, the trial, and the guard. The screen is in the [Manual](../../manual/index.md) and the flags are in the generated [Reference](../../reference/index.md). The relay that the console uses is described in [the UI backend](ui-backend.md).

## The problem

1. **A flag change needs a restart,** and a restart with a bad value can leave nothing running.
2. **Nobody should keep a second list of flags.** A list in the UI would go out of date the day a flag is added.
3. **The editor and the edited are different programs,** possibly different users, and must not tear each other's writes.
4. **Some flags must never be reachable this way:** credentials, and anything that says where a credential is sent.
5. **There is no supervisor to build.** The project already relies on systemd or a container restart policy, and a second watchdog would be a second thing to fail.

## One file per program

Every program, meaning the proxy, the controller, the aggregator and the UI, can be started with `--settings-dir DIR`. It then reads `DIR/<program>/settings.json` and adds the values it holds as flags **that the command line did not give**, so **the command line always wins**. An operator who wants a flag fixed puts it on the command line, and the file can never override it.

The file holds a **revision** that counts every write, the current values, and, only while a change is on trial, the previous values and a pending mark with the time it was applied and the number of starts since. A value is a boolean, a string or a list of strings. A refused change leaves a short error behind, which the next write clears.

Each program has a directory of its own, `0700` by default and `0770` when the settings directory is itself group-writable, with files owned by the writer in the same mode. That is the switch for giving the UI backend write access to one program's files without giving any program access to the rest. A writer and a program either run as one user or share that one directory's group.

Writes are **atomic**: a temporary file in the program's directory, synced, then renamed over the old one. The temporary name is the writer's own process, so two writers cannot tear one another's file. A read of a settings or schema file is capped at 1 MiB, since a file the UI backend reads was written by another user. Read-modify-write sequences hold an exclusive lock on a `.lock` file in that directory, so the UI backend's apply, confirm or undo and the program's own guard cannot interleave and lose an update.

## No second list of flags

The flag list the console offers comes from **each program's own command line definition**. Each program writes `schema.json` at every start, listing the flags it knows, with their shape and help text. The UI backend does not link the other programs, so it reads that file to draw the form and to check a value's shape.

A new value is checked by running the **same parser** that the program uses. The check refuses an unknown flag, a value of the wrong shape, a bad number and a conflict between flags, **here and not at the restart**. Required flags are not demanded, since they come from the command line. A file written by a newer build and read by an older one does not stop it from starting: flags it does not know are skipped with a warning.

### What cannot be set from the file

The list of exclusions is the security story. A flag is refused if it is:

- **The file's own location** or something that acts once and exits: the settings directory, help, version, `--check`, the password hashing mode and the secret management modes. A dry run like these does not even use up a trial start.
- **A safety override,** any flag beginning `insecure-` or `allow-insecure-`. These belong on the command line or in the unit, where a person chose them.
- **A credential:** a token, a token file, a password or a pre shared key. They are the bootstrap of the thing that edits the settings, and a credential in an editable file would let the editor raise its own privileges.
- **A destination:** URLs, file paths, certificates and keys, the config path and the peers of a replicated controller. These say where tokens go or what is trusted, and another program sharing the directory could otherwise **redirect them**.

The list is a function of flag names, so a new credential flag that follows the naming convention is refused without anyone updating a table. The console shows the reason next to each refused flag.

## Trial applies

Changing a flag restarts the program through its normal restart policy. **A change is never final until someone confirms it.** The apply step does this:

1. The caller names the revision it read. A file that moved since is refused, and so is a change while another is still pending.
2. The new values become the file's values, the old ones are kept as `previous`, and the change is marked pending with the time.

The program notices the change and restarts. Then three independent timers can end the trial.

- **Confirm.** The operator confirms in the console, which clears the mark and makes the values final. Confirm and undo name the revision they were meant for, so one meant for an earlier trial cannot act on a newer one.
- **Time.** If the mark is not cleared within **60 seconds** of the program's start, the guard puts the previous values back and restarts again.
- **Crash loop.** A value that stops the program from starting at all cannot run a guard, so the check happens at the **next start**: after **3 attempts**, or **300 seconds** after the apply whatever the count, the previous values are restored before anything else reads them.

**No supervisor process is involved.** The file and the restart policy are enough, and the restart is a plain exit with the conventional temporary failure status that service managers treat as "start me again".

### The guard

A running program watches its own file once a second. It compares the file's values with the ones it booted with and returns one of four verdicts:

- **Nothing.** The values match and either nothing is pending or the minute has not passed.
- **Restart.** The values differ and the parser accepts them.
- **Reject.** The values differ and the parser refuses them. The previous values are put back, the reason is stored in the file, and the program **keeps running**, since nothing about its current run is wrong. If the refused file has no pending mark to undo, it is logged once per revision.
- **Undo.** Pending and unconfirmed past the deadline.

Every undo reads the file again **under the lock** right before writing. If the revision changed or the pending mark is gone, a confirm landed in the meantime, and it wins. If the undo cannot be written, the guard says so and stops: restarting would only find the same values again and loop.

### When the file cannot be written

A trial needs to be countable and undoable on disk. If a change is pending but the directory is not writable, nothing could ever end the trial, so the program **uses the previous values and watches nothing**, with a warning, instead of restarting forever against a file it cannot change. A file that cannot be read at all starts the program without it.

An old flat layout file, `DIR/<program>.json`, is ignored with a warning that says where settings live now.

## Who may do it

The UI backend exposes the settings through a proxy with the **viewer** role allowed to read, though a viewer sees the flags and no values (a listen address or a tunnel endpoint is the operator's to see), and the **admin** role needed to apply, confirm and undo (see [the UI backend](ui-backend.md)). The backend only checks shapes using the published flag list. The full parse happens inside the program that will run the values, so a console that is newer or older than a program cannot approve what the program would refuse.

## What was traded away

- **A change takes a restart.** Connections of the program end, and the proxy drains as it does on any restart.
- **One trial at a time per program,** so a second change waits for the first to be confirmed or undone.
- **A restart policy is required.** Without systemd, a container policy or the like, the program would exit and stay down. The mechanism is for deployments that already have one.
- **Many flags are not editable here.** Anything that is a credential or a destination is deliberately left to the command line, so some changes still need a shell.
- **One minute is a guess.** It is long enough for a program to come up and an operator to look, and short enough that a wrong value does not stay long.
- **The deadline is measured from the program's start,** not from the apply, so a slow restart does not eat the operator's time to confirm.

## Where to look

- `crates/wayhouse-settings/src/lib.rs`: the file, locking, the schema, the exclusion rules, layering at start, the guard, and apply, confirm and undo.
- `crates/wayhouse-ui/src/process_api.rs`: the console's API over these files.
