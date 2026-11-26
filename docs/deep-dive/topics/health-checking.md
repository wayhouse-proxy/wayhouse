---
title: Active and passive health checks
description: How wayhouse decides a backend is down, why it looks at both probes and real traffic, and how the checks stay cheap with thousands of backends.
---

A load balancer is only as good as its idea of which backends are alive. This page explains how wayhouse forms that idea, and the choices behind it. The settings you write in a pool's `health_check` block are in the [Manual](../../manual/pools/pools.md).

## The problem

Three requirements fight each other.

1. **Notice a dead backend quickly, but do not flap.** One lost packet must not take a game server out of rotation. A backend that dies mid-evening should stop getting new players within seconds, not a probe interval later.
2. **Probing must not become the load.** A fleet can hold thousands of backends. If every check opened a socket at the same moment, the probe burst would itself look like an attack, and a pool that appears at once (startup, reload, discovery) would probe everything in the same instant.
3. **A failed check must mean the backend failed.** If the proxy runs out of file descriptors or ports, every probe fails. Treating that as "all backends are down" would turn a local shortage into a full outage.

## Two sources of evidence

A backend's health flag moves on two kinds of observation, both fed into the same counters.

**Active checks** are probes the proxy sends on a timer. There are two kinds. `tcp_connect` opens a connection and counts a completed handshake as success. `udp_probe` binds a fresh socket, sends a configured payload and succeeds if a reply arrives that starts with the configured prefix (any reply when the prefix is empty). A third setting, `none`, runs no probe at all.

**Passive observations** come from real traffic. When the proxy connects to a backend for a client and the connect succeeds, that is one success. A refused or timed out connect, or a failure to write the PROXY protocol header to the backend, is one failure. For UDP, the first reply of a session is a success, and a `ConnectionRefused` on the connected upstream socket (on Linux, the ICMP port unreachable the backend's host sends when nothing listens) is a failure. Passive failures are faster than a probe interval, because a client is already waiting.

Both feed `Backend::observe`, which keeps a run of consecutive successes and a run of consecutive failures:

- a healthy backend turns unhealthy after `fall` failures in a row,
- an unhealthy backend turns healthy after `rise` successes in a row,
- any result of the opposite kind resets the other run.

The asymmetry is deliberate. Marking a backend down is cheap to undo. Sending players to a half dead backend is not, so coming back needs a streak, and a single success proves nothing.

## The checker

One task, `health::run`, sweeps every backend of every pool. It wakes every **500 ms**, and only probes backends that are **due**. Each backend carries the interval and timeout of its own pool, and a `next_check_ms` timestamp. The sweep period only bounds the resolution. It is not the probe frequency.

```text
every 500 ms:
  snapshot = current snapshot            (so reloads are picked up at once)
  for each backend:
      not due                → skip
      no permit free         → skip, stays due, goes in a later sweep
      already being probed   → skip
      otherwise              → claim it, spawn a probe task, set next due = now + interval
```

Several choices keep this cheap and honest:

- **Probes run in the background and the sweep never waits for them.** A slow probe delays nobody else's check.
- **At most 256 probes are in flight.** Each holds a socket while it runs. When the permits are out, a due backend simply waits for a later sweep.
- **A backend is probed by one task at a time.** A compare and swap on a `probing` flag claims it, and a guard clears the flag when the task ends however it ends, so a panicking or cancelled probe cannot leave a backend stuck "in flight".
- **The first check is spread out.** A new backend's first probe is placed at an offset derived from its address, anywhere inside one interval. A pool with a thousand backends does not probe them all at the same moment.
- **The sweep reads the current snapshot every time.** After a [hot reload](snapshot-and-hot-reload.md) the checker follows the new pools without being told, and backend state survives the reload because it is carried by address.

### A local failure is not the backend's fault

A probe has three results, not two: `ok`, `fail` and `unknown`. If a socket call fails for a local reason (out of file descriptors, ports or buffers), or the proxy cannot even bind the probe socket, the result is `unknown`. It is counted in the metrics but it feeds neither run, so a proxy under local pressure does not conclude that its backends died. The same rule applies to passive connects: running out of descriptors or ports is not reported as a backend failure.

### `health_check: none`

Some backends have nothing to probe. With `none`, an unhealthy backend gets a fresh chance each interval (it is observed as healthy once), and only a passive failure can take it down again. This is a retry loop, not a check, and it exists so that a backend marked down by passive evidence does not stay down forever when no probe could ever bring it back.

## Beyond one proxy

The local verdict is not the only input. With the gossip fabric turned on, every active result is also published to the mesh as this proxy's own opinion, and the checker applies the mesh's **quorum verdict** to each backend as a separate `domain_down` flag. A backend counts as healthy only when its local flag says so and the regional view is not overriding it. The override can only push a backend **down**. It never touches the local flag, so a backend returns to rotation only after this proxy's own `rise` streak. The runtime feature flag for gossip verdicts switches the whole behavior off without a restart. See [Regional health](../../10-distributed-control-plane.md).

Each pool's backend counts by state (healthy, unhealthy, draining, disabled) are refreshed as gauges at the end of every sweep. An operator's `draining` or `disabled` state is independent of health: such a backend takes no new sessions whatever its probes say.

Separately, `health_report.rs` sends the proxy's backend health to the controller. It checks every 5 seconds, sends a changed picture at once and an unchanged one every 60 seconds, because the controller forgets a proxy after three minutes of silence. A controller without plugins answers 501 or 404 and is left alone for five minutes.

## What was traded away

- **No HTTP check.** Only a connect and a UDP exchange exist. A server that accepts connections but is stuck inside its game loop looks healthy. Passive evidence helps a little, but a deeper application probe would need per game protocol knowledge.
- **A probe interval of latency.** An idle backend that dies is noticed when the next probe or the next real connect fails. The passive path closes most of that gap for busy pools.
- **Probes see the network from the proxy, not from the player.** A backend can be reachable from the proxy and not from a region of the internet. The regional gossip view exists for that case, and only partly solves it.

## Alternatives that were not taken

- **One task per backend.** Simple, but thousands of timers and tasks cost more than one sweep that skips backends that are not due.
- **Probe on every sweep and let intervals be a multiple.** It would tie every pool to one cadence.
- **Treat any local error as a failure.** It fails closed in the worst possible way: a descriptor shortage would empty every pool.

## Where to look

- `crates/wayhouse-core/src/health.rs`: the sweep, the probes and the semaphore.
- `crates/wayhouse-core/src/pool.rs`: `Backend::observe`, `is_due`, `begin_probe` and the shared backend state.
- `crates/wayhouse-core/src/proxy.rs` and `listener_udp.rs`: the passive observations.
- `crates/wayhouse/src/health_report.rs`: the report to the controller.
- Manual: [Pools: every option](../../manual/pools/pools.md) for the `health_check` settings.
