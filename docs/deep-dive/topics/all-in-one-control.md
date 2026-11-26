---
title: "wayhouse-control: three programs in one process"
description: How the controller, the aggregator and the UI run as one process for a small install without being merged, how one command line is routed to the three, and what the shortcut costs in security.
---

A fleet of one proxy and a handful of friends does not want three extra services to configure, start in order and give tokens to each other. A big fleet wants exactly that separation. **`wayhouse-control`** is the answer for the small case: the same three programs, linked into one binary and started together, with the wiring done for you. This page explains how little it does and why that is the design. The flags are in the [Manual](../../manual/index.md) and the generated [Reference](../../reference/index.md). The parts are described in [the controller core](controller-core.md), [the aggregator](aggregator.md) and [the UI backend](ui-backend.md).

## The problem

1. **A small install should be one unit.** One service file, one container, one place for logs.
2. **But there must not be a second code path.** A "lite" mode that behaves differently from the split deployment would be tested less, and a bug would show only in one of them.
3. **Three programs have three flag sets,** and several flags share a name: every program has a `--listen`.
4. **The UI needs the other two's addresses and tokens,** and a typo there leaves a console that cannot reach anything.
5. **Secrets must not be shared by accident.** The three programs read secrets under the same names from the environment, so one value would silently reach all of them.
6. **If one part dies, the others must not pretend all is well.**

## Nothing is merged

The crate's own header says it plainly. The three programs are the **standalone ones**, called through the same entry points the separate binaries use. Each has its own listener, its own tokens and its own settings file. This crate does two things only: it **routes one command line to the three programs**, and it **points the UI at the other two over loopback** with the tokens a split deployment would use. The code path is the same as in a split fleet, so the in-process wiring cannot hide a bug that the split one has.

The program is small: a planner that turns an argument list into three argument lists, and a `main` that starts the three.

## One command line, three programs

Four flags belong to the control program itself: `--settings-dir`, `--tls-cert`, `--tls-key` and `--ca-file`. Everything else is **namespaced**. `--controller-X`, `--aggregator-X` and `--ui-X` reach flag `--X` of that program. A UI flag whose own name already starts with `ui-` keeps it, so the UI's password file is `--ui-ui-password-file`. The plan is a pure function of the arguments, the three programs' flag definitions and the credentials directory, so it is tested without starting anything.

The rules the planner enforces:

- **An unknown flag is an error,** and so is a flag given twice or one of the shared flags with other than one value. The message says how a program's flags are written.
- **Shared flags go where they apply.** The settings directory goes to all three. The certificate pair goes to the controller and the aggregator. The CA file is not passed on at all: it is trusted once, by the process, because the programs would each try to initialise it.
- **The UI has its own TLS,** `--ui-tls-cert` and `--ui-tls-key`, since the UI is what an operator's browser meets and its certificate is its own choice. The UI's `--controller-...` and `--aggregator-...` flags, meaning the URLs and token files it uses to reach the other two, cannot be given at all, because wayhouse-control derives them.
- **A one-shot is refused.** The password hashing mode belongs to the UI program and makes no sense inside a running trio, so the error says to run it directly.

### TLS is decided from the command line only

Whether the UI dials the others over `http` or `https` depends on whether a certificate was given. A certificate in a settings file would not change the scheme the UI uses, so the planner reads only the command line and the documentation says to give `--tls-cert` there. With TLS the UI reaches the others at `https://127.0.0.1:<port>`, so the shared certificate must be valid for `127.0.0.1`, and for `::1` when listening there.

### The derived wiring

The planner computes what the UI needs, in the same way an operator would have written it:

1. **The address of each of the other two,** from its `--listen` flag or its default, with a wildcard address replaced by loopback. A listen address that is only in a settings file would lose against the command line, so the planner also writes the listen flag explicitly. The UI is told an address, so the program must really serve it.
2. **The scheme,** as above.
3. **The token,** reading the same file that gates the target. For the controller it prefers the **scoped UI token** when one is configured and never the admin token it could have used (see [the controller core](controller-core.md)).
4. **The UI's own address is pinned too, and held to a rule:** off loopback it needs its **own certificate**, since a login over plain HTTP would send the password in the clear.

## Secrets: three rules

The three programs share secret names, which is a trap in one process. The planner and the entry point close it three ways.

- **The environment is refused.** A variable such as `WAYHOUSE_AUTH_TOKEN` has a name that all three would read, so one value would reach every program. If any variable beginning `WAYHOUSE_` and mentioning token, password, step-up or proof is set, the start fails and names it, and says to give the secret per program.
- **Credential files are mapped per program.** With systemd credentials, a file named `<program>-<flag without -file>` stands in for that flag when the command line does not give it: `controller-auth-token`, `aggregator-ingest-token` and `ui-ui-password`. The process's own credentials directory variable is then removed before anything starts, so the programs cannot read the unprefixed names.
- **A credential that maps to nothing is an error.** A file whose name looks like a secret but belongs to no flag of the three would load fine and turn nothing on, so a component could start with no login. That was a real bug, and the planner now refuses it by name. Files that merely carry a path (a TLS key handed over as a credential) are not mistaken for secrets.

The secret flags themselves are found by looking at the programs' definitions, flags ending in `-file` that mention a token, password, step-up or proof. A new secret flag in any program is therefore covered without anyone remembering to list it.

## Running and failing together

The entry point starts all three on one runtime and waits for the first to return. **If any one stops, even cleanly, the process exits with an error** naming it, so the supervisor restarts all three together. That is the honest choice for a bundle: a console that is up while its controller is dead would be a confusing half state. The controller is listed first, though start order does not matter, since the UI and aggregator retry their connections.

Logging is configured once, with the usual log filter variable. The three programs **share one metrics recorder**, so every `/metrics` page lists the series of all three (see [metrics and logs](metrics-and-logs.md)).

## What was traded away

- **The token split is nominal inside one process.** The controller, proxy, agent and ingest tokens are still checked at the HTTP layer between the listeners, but one compromised process holds every token. Fleets that care keep the split binaries, and the crate says so in its header.
- **Everything restarts together.** A crash or upgrade of one part restarts all three. That is the cost of not running a supervisor of its own.
- **No high availability.** It is a single node and is meant to be one. Replicated controllers use their own binaries (see [controller HA](controller-ha.md)).
- **Prefixed flags are longer.** `--ui-ui-password-file` is ugly, and the alternative was a flag clash.
- **Shared environment secrets are unsupported.** Giving one secret three ways is more typing, in exchange for never giving it to the wrong program.

## Where to look

- `crates/wayhouse-control/src/lib.rs`: the planner, the secret rules and its tests.
- `crates/wayhouse-control/src/main.rs`: startup and the fail together loop.
