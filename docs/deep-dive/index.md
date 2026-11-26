---
title: Deep dive
description: How every wayhouse feature works.
sidebar:
    label: Overview
    order: 0
---

The deep dive is full developer documentation, for contributors and for anyone who wants to
take inspiration: how every feature is accomplished, which libraries, protocols and
techniques it uses and why, and which trade-offs were made along the way. Each feature gets
its own explanation, from the data plane and routing to the HA control plane, the tunnel,
plugins and sniffers, secrets and certificates.

The pages are written as narrative engineering write-ups, in the spirit of an engineering
blog post: the problem, the constraints, what was tried, the design that won and why, with
diagrams and concrete numbers where we have them.

## Where to start

Read [How wayhouse fits together](design/how-it-fits-together.md) first. It is the design in one page: the rules the system follows, the path of one connection and of one config change, which program does what and what happens when each part fails. The numbered design chapters below it are the long form.

After that, pick the section that matches what you want to understand:

- **The data plane:** [listeners](topics/data-plane-listeners.md), [routing](topics/routing-and-matchers.md), [pools](topics/pools-and-load-balancing.md), [the pump](topics/connection-pump.md).
- **Configuring and operating a proxy:** [snapshots and hot reload](topics/snapshot-and-hot-reload.md), [the admin API](topics/admin-api.md), [secrets](topics/secrets.md).
- **Extending the proxy:** [sniffers](topics/sniffers.md), [plugins](topics/plugin-host.md), [registries](topics/registries-and-release-feed.md).
- **The control plane:** [the controller](topics/controller-core.md), [HA](topics/controller-ha.md), [config rollout](topics/config-rollout.md).
- **The tunnel**, **web sites and certificates**, **the console**, and **building and shipping** have their own sections in the sidebar.

When a design chapter and the code disagree, the code wins and the chapter gets fixed. For everyday tasks see the [Manual](../manual/index.md).
