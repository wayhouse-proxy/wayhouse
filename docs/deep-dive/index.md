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

> [!NOTE]
> This section is being written, feature by feature. The design chapters below are the
> source of truth for the design today; when a chapter and the code disagree, the chapter
> wins and one of them gets fixed. For everyday tasks see the [Manual](../manual/index.md).
