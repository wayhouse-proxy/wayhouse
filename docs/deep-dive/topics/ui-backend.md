---
title: "The UI backend: sessions, auth and proxying"
description: Why the operator console has a backend of its own that holds every machine token, how its sessions, roles and login limits work, which browser attacks it defends against, and how it relays the controller and the aggregator without becoming a third authority.
---

The operator console runs in a browser, and a browser is the most exposed place in the fleet. It runs other people's pages in the same process, it keeps credentials in memory and storage, and anyone who can make an operator open a link has a foothold. The console therefore never talks to the controller or the aggregator itself. It talks to **`wayhouse-ui`**, a small server whose only job is to be the browser's one trusted peer. This page explains how it is built. The options are in the [Manual](../../manual/index.md) and the generated [Reference](../../reference/index.md). The browser side is in [the UI frontend](ui-frontend.md).

## The problem

1. **A browser must not hold fleet credentials.** A token in a browser is a token in every script that page ever loads.
2. **Two services, one console.** Reads and operational verbs live in the aggregator, config in the controller, each with its own token.
3. **More than one operator.** Different people should be able to read, operate or change the fleet, and an action should be attributable.
4. **The login is an attack surface.** Password checking is expensive on purpose, so unlimited attempts are both a guessing target and a cheap way to burn CPU.
5. **A browser can be steered by another site.** A page the operator happens to visit can try to call a local server.
6. **Updates must be live,** without every browser tab holding its own connection to the aggregator.

## A backend for the frontend

`wayhouse-ui` is a **dedicated process**, not part of the controller or the aggregator. It holds both services' machine bearer tokens itself, and calls each on the operator's behalf. **The browser never holds a bearer token for anything.** It holds only a session cookie for `wayhouse-ui`.

It is also **not a third authority**. It has no store and no fleet data of its own, and it decides nothing beyond "is this a valid session, and is the role high enough". Nothing in it outlives a restart except the fact that sessions are gone, the same ephemeral posture as [the aggregator](aggregator.md). It is one process on one port: the API, the WebSocket and the built frontend all come from it.

## Sessions

A login creates a session in an in memory store. The session id is 32 random bytes, 256 bits, hex encoded. Its length is chosen to resist being _guessed_, which matters here more than for the fleet's other bearer tokens, because those are operator chosen secrets and this one is generated and only has to be unguessable, not memorable. Sessions have **both an idle and an absolute timeout**, and the store is **capped**, so a stolen cookie has a bounded life and `POST /ui/login` cannot grow the map without limit. A restart logs everyone out, and nothing durable is lost.

The cookie is `SameSite=Lax`, which is one of the defences described below.

## Roles

There are three roles, declared from low to high so that "at least this role" is a comparison:

- **viewer**: every read.
- **operator**: plus the intent verbs that the aggregator fans out (backends, route hints, drain).
- **admin**: plus config submit, rollback and promote, adoption, and the records and settings that change the fleet.

The design is explicit that this is not a general permission matrix. Every route the process serves already falls into one of these three buckets, and a richer model would solve a problem this surface does not have. A gate takes a minimum role. A missing or invalid session is a **401** and a valid session with too low a role is a **403**, so the frontend can tell "not logged in" from "logged in, not allowed". On success, the username is placed in the request as the **actor**, and the proxies forward it as `X-Actor`, which the controller records per revision. That is the durable half of the audit trail.

Accounts come from a users file with Argon2id password hashes and a role each. `--hash-password` produces a hash so plaintext never sits in a file. The older single shared `--ui-password` is kept as a legacy mode that is implicitly admin, which the module notes as the one place the project's usual "clean break over compatibility shim" stance did not apply. Open mode, with no password and no users file, is loopback only by policy.

## Login limits

Every login attempt in users file mode costs an Argon2 verification, so an unthrottled login is a brute force target and a cheap CPU attack at once. Three limits apply to each attempt:

- **A token bucket per client address**, an IPv6 address by its `/64`.
- **A growing delay per username and client address.** Each attempt by a client at a name pushes that client's next admission out, 100 ms after the first and doubling up to a 5 second cap. A successful login clears the client's delay. The delay is per _client_, not per account, on purpose: a delay shared by every client would let one tight loop hold a known name locked forever.
- **A looser token bucket per username across all clients,** so a botnet cannot spread its guesses over one account.

Names hash into a **fixed set of 1,024 slots**, real or not, so the limits reveal nothing about which accounts exist and the memory is bounded. The known limits are written in the module. Behind a reverse proxy every client shares the proxy's address, so the per client delay degrades to a per name delay and the per address bucket to a global one, though the per name ceiling still holds. A botnet that out runs the per name ceiling can still crowd the owner out for as long as it keeps it up. The state is per replica and in memory.

## Cross site defences

Open mode is loopback only, and loopback is exactly where the operator's own browser is the attacker's vehicle: any page the operator visits can `fetch("http://127.0.0.1:…/api/config/rollback/1", {method: "POST"})`, and a plain text "simple request" needs no preflight. So everything the process serves besides the static frontend passes an **origin guard**, in authenticated mode as well, where it costs nothing and also covers the WebSocket upgrade:

- For an unsafe method (anything but `GET`, `HEAD` and `OPTIONS`) or a WebSocket upgrade: when the browser sends `Sec-Fetch-Site`, it must be `same-origin` or `none`, which is a URL the user typed. `cross-site` and `same-site` are refused.
- When `Sec-Fetch-Site` is absent, a present `Origin` must name the host the request was addressed to, and `null` is refused. When `Sec-Fetch-Site` already says same origin, the browser has decided, so a `Host` rewritten by a reverse proxy cannot turn a legitimate request into a 403.
- A request with neither header, such as `curl`, a script or the end to end tests, is not a browser driven cross site request and passes.
- In open mode, **every request's `Host` must also be a loopback name or address**, which defeats DNS rebinding of an attacker's own name to 127.0.0.1.

## Relaying, thinly

The proxies to the aggregator and to the controller are **thin and stateless**. The browser's cookie never becomes a bearer token. The aggregator proxy relays fleet reads at viewer and the intent verbs at operator. The controller proxy relays config reads at viewer, and writes (submit, rollback, promote, release of a tunnel address, confirm and revert of a trial) at admin. A config submit's body is raw YAML text, so it is forwarded byte for byte with no content type forced on it, and the base revision guard and its 409 pass through untouched.

Details that matter:

- **Upstream headers are forwarded faithfully**, minus the hop by hop ones whose framing describes only that hop, so a caller sees the revision header exactly as the controller sent it.
- **A name in a path is re encoded** so it stays a single path segment, and the dot segments that a URL normaliser would resolve away from the intended route are refused.
- **The stored records,** dashboards, themes and filters, are relayed to the controller with viewers reading, operators writing dashboards and filters, and admins writing themes, the layout override and the defaults (see [stored UI data](ui-data.md)).
- **Process flags** of the fleet's own programs are read and written through settings files. The programs notice a change, restart, and **undo it unless it is confirmed**. Applying, confirming and undoing need admin. Credentials never pass through, since the settings crate refuses token, password and PSK flags.
- **Web sync** status is a read of a sidecar's metrics, showing the few numbers an operator wants.

## Live updates through one subscription

The console shows the fleet changing. Rather than one aggregator connection per browser tab, the backend keeps **one shared subscription** to the aggregator's event stream, holds the most recent view as raw text it never interprets, and **fans it out to every connected browser** over a WebSocket. The subscription reconnects with backoff from 500 ms to 30 seconds. The socket is behind the same session check as everything else, since an upgrade request is an ordinary `GET` until the handshake completes. A socket is closed with the policy violation code, 1008, when its session ends, sockets are capped at 256 by default, a message from the browser is limited to 4,096 bytes, and a slow reader is dropped after a 10 second send timeout.

## Serving the frontend

The built frontend is served as a fallback beneath every route the API does not claim, so one port is all an operator's browser talks to. A page path that no file matches gets the app, so a reload or a pasted link on a client route works. Two rules prevent confusing failures. Unknown paths below the prefixes the backend owns are **errors, never the app**. And a path that names a file, with an extension in its last segment, is a file or a 404, so a stale hashed asset after an upgrade is not answered with the app as `text/html` and status 200. Hashed build output may be cached for good, since its name changes with its content. The module notes the cost: a client route containing a dot would 404.

## What was traded away

- **Sessions do not survive a restart.** Everyone logs in again.
- **A coarse role model.** Three roles, no per route permissions.
- **Login limits are per replica,** and a reverse proxy collapses the per client signals.
- **One more process to run,** in exchange for keeping every machine token out of the browser.
- **Relaying costs a hop** and means a feature must be added to the proxy before the console can use it.

## Where to look

- `crates/wayhouse-ui/src/session.rs`, `auth.rs`, `role.rs` and `users.rs`: sessions, gates, roles and accounts.
- `crates/wayhouse-ui/src/login_limit.rs` and `origin_guard.rs`: login throttling and the cross site checks.
- `crates/wayhouse-ui/src/aggregator_proxy.rs`, `controller_proxy.rs`, `ui_data_proxy.rs` and `proxy_util.rs`: the relays.
- `crates/wayhouse-ui/src/fleet_feed.rs` and `ws.rs`: the shared subscription and the WebSocket.
- `crates/wayhouse-ui/src/process_api.rs` and `web_sync_api.rs`: process flags and web sync status.
- `crates/wayhouse-ui/src/static_files.rs`: serving the frontend.
