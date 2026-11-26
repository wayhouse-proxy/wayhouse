---
title: HTTP API basics
description: How wayhouse's HTTP APIs are addressed, authenticated and versioned, and what their errors look like.
sidebar:
    order: 6
---

Every wayhouse program that you can reach over the network speaks plain HTTP (or HTTPS) with a bearer token. This page is what they have in common. The per-route detail, with request and response shapes and a `curl` example for each route, is in the Reference: the proxy admin API (Reference, "Proxy admin API", see the [Reference overview](../../reference/index.md)) and the Controller API are written that way; the aggregator and console routes are described in [Config revisions](config-revisions.md) and [Operate the whole fleet](fleet-api.md).

## Who serves what

| Program               | Default address  | What the API is for                                                                                 |
| --------------------- | ---------------- | --------------------------------------------------------------------------------------------------- |
| `wayhouse` (proxy)    | `127.0.0.1:9900` | The proxy's own state: pools, sessions, drain, its config file. `settings.admin.listen` changes it. |
| `wayhouse-controller` | `:9901`          | Config revisions, secrets, plugins, certificates, enrollment, the HA cluster.                       |
| `wayhouse-aggregator` | `:9902`          | The fleet view: what every proxy reports, and commands fanned out to all of them.                   |
| `wayhouse-ui`         | `:9903`          | The console's backend, `/api/...`, behind a login.                                                  |

The addresses are the defaults of the solo bundle; every program takes `--listen`, and the proxy takes `settings.admin.listen`.

## Authentication

The proxy, the controller and the aggregator use **bearer tokens**: send `Authorization: Bearer <token>` on every request. A request with no header, a wrong token, or a header that does not start with `Bearer ` (capital B, one space) is answered `401` with the body `unauthorized`.

- The proxy admin API checks one token, `settings.admin.auth_token`. `GET /healthz` is the only route that needs no token, so a probe can reach it.
- The controller separates its credentials (admin, proxy, agent and an optional UI token), so an edge box cannot administer the fleet. Which token unlocks which route is in [Secrets and tokens](../security/secrets-and-tokens.md).
- The UI is the exception: a person logs in, the browser holds a session cookie, and the UI's role (viewer, operator, admin) decides what a route allows. It forwards to the controller and aggregator with its own tokens, so the browser session never becomes a bearer token. See [The console](console.md).

A program started without a token leaves its API open. It refuses to do that on a non-loopback address unless you pass `--insecure-no-auth`, so an open API is a decision, not an accident. Tokens are at least 16 bytes. Treat them like passwords: use HTTPS (`--tls-cert` and `--tls-key`, or `settings.admin.tls` on the proxy), and pass them as files, never on a command line.

## Content types

Routes answer in one of two styles, and the reference says which.

- **Plain text** for the proxy's read-outs (`/pools`, `/sessions`, `/config`). They are meant for a person at a terminal; a script that parses them splits on tabs, and the layout is not a promise the way a JSON body's keys are.
- **JSON** for everything a program is meant to parse: the config validation result, the sniffer list, feature flags, and almost everything on the controller and aggregator. A JSON body is sent with `Content-Type: application/json`.

A config document is sent as raw YAML (`Content-Type: text/yaml`), not wrapped in JSON, and a module upload is raw bytes.

## Errors

The status code says what kind of failure it is; the body says why.

| Status | Meaning                                                                                                                                                                |
| ------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `400`  | The request is malformed: a bad address, an unknown state, a missing field.                                                                                            |
| `401`  | No token, or the wrong one.                                                                                                                                            |
| `403`  | Authenticated, but the write is switched off. On the proxy that is an `admin_write.*` [feature flag](feature-flags.md), or `--allow-config-write` for the config file. |
| `404`  | The pool, backend, module or route does not exist.                                                                                                                     |
| `409`  | The request is fine but conflicts with the state: a changed config file (`code: stale`), a pinned sniffer module, an instance with no `settings.sniffers`.             |
| `413`  | The body is larger than the route allows (1 MiB for the config file, 8 MiB for a sniffer module).                                                                      |
| `422`  | The document is well formed but fails validation. The error names the key and the reason.                                                                              |
| `426`  | The caller speaks an incompatible protocol major (see below).                                                                                                          |
| `428`  | The route needs a precondition header you did not send (`If-Match` on `PUT /config/file`).                                                                             |
| `503`  | The instance cannot do it right now: draining, not ready, or its file is not writable.                                                                                 |

Plain-text routes put the reason in the body. JSON routes use `{"error": "..."}`, plus `"code"` where a client should branch on it (today only `"stale"`). Match on the status first and the code second; the sentence is for people and may be reworded.

## Versioning

Three numbers are independent of each other and of the product version.

- **The config `schema_version`** is the version of the YAML document. A program accepts every version up to its own maximum. `GET /config/schema` returns the JSON Schema and an `X-Config-Schema-Version` header with the highest version this build accepts.
- **The wire protocol** (`x-wayhouse-protocol: <major>.<minor>`) is how wayhouse programs talk to each other. A minor bump is additive and always accepted; a major bump is an incompatible change, and a program answers `426 Upgrade Required` to a peer on another major. Responses from the routes behind that gate carry the server's own version in the same header. The proxy admin API is lenient: a request with no header is accepted, so `curl` works, but an other-major header is refused.
- **The product version** follows the [upgrade rules](../../upgrading.md): two adjacent minor versions can run side by side while you upgrade a fleet. One exception goes the other way round: upgrade every proxy before you use a config field a newer version introduced.

A client that parses a JSON response should ignore keys it does not know: a protocol minor is additive, and receivers ignore unknown fields.

## Streams

A few controller routes are long-lived server-sent-event streams (`/config/subscribe` and the other `.../subscribe` routes). A reverse proxy in front of them must not buffer the response and must allow a long read timeout; the [deployment chapter](../../12-deployment.md#proxy-configuration) has working Caddy and nginx settings.
