---
title: Manual
description: Everyday use of wayhouse, task by task, with every option.
sidebar:
    label: Overview
    order: 0
---

The manual is for anyone who runs wayhouse. It starts with the basic setup most people use,
then goes theme by theme through every option: what it does, what to set, and how to check
that it worked. Each option is shown the way you will meet it: in the **config file**, in
the **admin UI**, over the **HTTP API** and as a **launch parameter** or environment
variable.

## Start here

1. [Quick start](getting-started/quick-start.md): run one proxy with the example config.
2. [Install](getting-started/install.md): containers, systemd, Kubernetes or from source.
3. [Set up a single box](getting-started/basic-setup.md): proxy, controller, UI and sealed
   secrets together.
4. [Your first listener and pool](getting-started/first-listener-and-pool.md).

## Then by theme

- **Routing and clients:** [route players by rules](routing/route-by-rules.md),
  [keep the client IP](routing/keep-client-ip.md).
- **Security and limits:** [protect a listener](security/protect-a-listener.md).
- **Operating:** [drain and maintain](operate/drain-and-maintain.md),
  [monitor](operate/monitor.md), upgrading.
- **Certificates, plugins, sniffers, web sites and the admin UI** have their own sections.

> [!NOTE]
> The manual is being written theme by theme. Where a theme has no page yet, the
> [Reference](../reference/index.md) lists every key, flag, endpoint, screen and metric, and
> the [Deep dive](../deep-dive/index.md) explains how each feature works. The coverage list
> in `docs/coverage-allowlist.txt` is the to-do list for the manual.
