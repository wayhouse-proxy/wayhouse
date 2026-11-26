---
title: The admin console, screen by screen
description: What each page of the web console shows, which role it needs, and the HTTP routes behind the stored records (dashboards, themes, layout and defaults).
sidebar:
    order: 1
---

The console is the browser front end of `wayhouse-ui`. It never holds a fleet token: you sign in to the UI, and the UI calls the controller and the aggregator for you (see [tokens and secrets](../security/secrets-and-tokens.md)). Everything you can do here you can also do over the HTTP API, and the console edits the same configuration file the proxies read.

Three roles decide what you may do. A **viewer** reads everything. An **operator** also changes backends, route hints and drain state, and saves dashboards and filters. An **admin** also changes configuration, themes, defaults and secrets. A button your role cannot use is hidden or disabled. The server decides, so a hidden button is a convenience and not a lock.

## Watching the fleet

| Screen         | What it shows                                                                                                                                                                                                                                     |
| -------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `/overview`    | Traffic across every instance: connections, throughput and sessions as charts. Rates are per second, averaged over each minute, and a marker shows when the numbers are stale.                                                                    |
| `/dashboard`   | Your widgets on a 12 column grid. The first time you see the default; **Customize** lets you add, remove and resize widgets, and **Done** stores the board for everyone. Needs the operator role to change.                                       |
| `/listeners`   | Traffic per listener, summed over every instance that runs it.                                                                                                                                                                                    |
| `/fleet`       | Every node that reports to the aggregator, by region. Filter, then act on one node or on everything the filter selects. This page also adds a backend or a route hint to every instance (see [the fleet API](fleet-api.md)).                      |
| `/tunnel`      | The addresses the controller assigned to origins and proxies on the WireGuard network. An admin can release an address, mint a new enrolment code for an origin, and **rotate** an origin's key (see [the tunnel](../pools/wireguard-tunnel.md)). |
| `/nodes/add`   | Creates a one time code and shows the command that sets a new node up. The code works once and expires, and nothing long lived goes on a command line (see [enrol a node](enroll-a-node.md)).                                                     |
| `/sniffers`    | The WASM modules that recognise a game protocol or hostname. Upload a module (operator role) and see which version runs. A file that fails validation leaves the previous version running, and the page says so.                                  |
| `/plugins/:id` | One plugin install. Its tabs are the options, the write only secrets, the webhook (the token is shown once) and the status and log. See [plugins](../../plugins.md).                                                                              |

## Changing the configuration

| Screen             | What it does                                                                                                                                                                                                         |
| ------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `/settings`        | The global settings as a form generated from the config schema. **Show advanced** reveals the rest. It is read only without the admin role, and it turns itself off when the YAML on the Source page does not parse. |
| `/settings/source` | The same configuration file as text. Comments and unknown keys are kept. This is the page to paste a whole file into.                                                                                                |
| `/config/web`      | The `web:` section: the hostnames Caddy serves and the upstreams their requests go to (see [the web front door](../../web-proxy.md)).                                                                                |
| `/config-history`  | Every submitted configuration is a revision. Compare one with the current revision, **roll back** to it, or **promote** it (admin role). See [config revisions](config-revisions.md).                                |
| `/certificates`    | The certificates the controller issues and renews, and the gateways that fetch them. An admin adds, replaces and removes sites and registers gateways (see [certificates](../../acme.md)).                           |

Edits send the revision they started from, so a change made by someone else in the meantime is refused and you are asked to reload instead of overwriting it.

## Settings of the console itself

| Screen                 | What it does                                                                                                                                                                                                           |
| ---------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `/settings/appearance` | Pick light, dark, system or a saved custom theme for this browser. An admin also sets the deployment default theme and default dashboard, and imports or deletes custom themes. A theme must pass ten contrast checks. |
| `/settings/layout`     | Edit the **form layout override** stored for this deployment: which fields are grouped, ordered, labelled or hidden as advanced. The preview shows its effect. It never changes what the YAML says.                    |
| `/settings/process`    | The launch flags of the proxy, the controller, the aggregator and the UI, with a safety net. See [process flags](process-settings.md).                                                                                 |
| `/settings/secrets`    | Write only values that a config refers to as `{ secret: name }`, such as the admin token or the gossip key, and the key rotation steps. See [tokens and secrets](../security/secrets-and-tokens.md).                   |

## The stored records behind them

Dashboards, themes, saved filters, the form layout override and the deployment defaults are kept in the controller, so every operator of a deployment sees the same thing. The UI relays them under `/api/ui/`.

| Request                                                                | Role     | What it does                                                                                                                                                |
| ---------------------------------------------------------------------- | -------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `GET /api/ui/dashboards`, `/api/ui/themes`, `/api/ui/filters`          | viewer   | The collections, and one record under `/api/ui/<kind>/{name}`.                                                                                              |
| `PUT` and `DELETE /api/ui/dashboards/{name}`, `/api/ui/filters/{name}` | operator | Save or delete a dashboard or a saved filter.                                                                                                               |
| `PUT` and `DELETE /api/ui/themes/{name}`                               | admin    | Save or delete a theme.                                                                                                                                     |
| `POST /api/ui/dashboards/{name}/rename`                                | operator | `{"to": "new name", "revision": N}` renames in one step and moves the default dashboard with it. `409` when the new name is taken or the revision is stale. |
| `GET /api/ui/layout`                                                   | viewer   | The form layout override. `404` means the shipped layout is used. `PUT` and `DELETE` need the admin role.                                                   |
| `GET /api/ui/defaults`                                                 | viewer   | `{"dashboard"?, "theme"?}`, an empty object when nothing is set. `PUT` and `DELETE` need the admin role.                                                    |

A dashboard carries a revision that counts up on every save. A save that sends the revision it was edited from is refused with `409` and `"code": "stale"` when someone saved since, which is how two editors avoid overwriting each other. Names are 1 to 64 characters without `/` or control characters. Nothing here is a secret: do not put credentials in a dashboard, filter or layout.

The formats and limits are on the [stored UI data](../../ui-data.md) page and the form layout format on [the form layout](../../ui-form-layout.md) page.
