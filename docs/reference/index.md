---
title: Reference
description: Lookup tables for wayhouse.
sidebar:
    label: Overview
    order: 0
---

Lookup material, generated from the source on every build so it cannot drift:

- **Config keys**: every key of the YAML config.
- **Command-line flags**: every argument of every binary.
- **Metrics**: every Prometheus metric.
- **Feature flags**: runtime flags and Cargo features.
- **Proxy admin API**: every route of the proxy's HTTP API, with parameters, request and response shapes, status codes and a `curl` example. Generated from the OpenAPI document the proxy itself serves.
- **Controller API**: every route of the controller's HTTP API (config revisions, enrollment, HA, plugins, certificates, console records), with the tokens that may call it, parameters, bodies and answers. Generated from the OpenAPI document built from the controller's own route annotations.
- **Aggregator API**: every route of the aggregator's HTTP API (the push endpoint, the fleet read views, the verbs that act on every instance), with parameters, bodies and answers. Generated from the OpenAPI document built from the aggregator's own route annotations.

These pages list everything. What each thing means and when to use it is in the
[Manual](../manual/index.md) and the [Deep dive](../deep-dive/index.md). A CI check
(`make docs-coverage`) fails when a flag, key, metric or feature is missing from those
hand-written pages, apart from the known gaps in `docs/coverage-allowlist.txt`.
