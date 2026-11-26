---
title: Turn a capability off in an emergency
description: Runtime feature flags, the override that switches one off for an hour or until cleared, and the console and API calls that set, read and clear it.
sidebar:
    order: 7
---

A runtime **feature flag** is a switch for one capability, such as the source ACL, the geo filter, sniffers, plugin webhooks or certificate issuance. The configuration sets it in the `features:` section. An **override** is the emergency exception on top: it beats the config until it is cleared or its time ends, and it never restarts anything.

Use an override when a capability is hurting players right now and you do not want to push a new config under pressure. A bad ACL that blocks real players is the typical case.

## Which flag for which symptom

| Symptom                                                          | Flip                                                         | Cost                                                              | Ends                                    |
| ---------------------------------------------------------------- | ------------------------------------------------------------ | ----------------------------------------------------------------- | --------------------------------------- |
| Players land on the wrong pool after a sniffer change            | `sniffers` off                                               | Routes with `sniffer:` never match; sniffer rejects stop dropping | Always has an end time (at most 7 days) |
| A bad ACL, country list or limit locks real players out          | `acl`, `geo`, `rate_limit` or `per_source` off               | That protection is gone for new connections                       | Always has an end time (at most 7 days) |
| A plugin's ports must stop taking customers                      | `port_routes` off                                            | Those listeners are removed; established TCP sessions continue    | Until cleared                           |
| Backend verdicts from other regions are wrong                    | `gossip_verdicts` off                                        | Each proxy judges backends by its own checks                      | Until cleared                           |
| A plugin webhook is being abused                                 | `plugin_webhooks` off                                        | The webhook listener answers 503; plugin ticks go on              | Until cleared                           |
| Certificate orders keep failing                                  | `acme_issuance` off                                          | No new orders or renewals; served certificates keep working       | Until cleared                           |
| Nobody should change backends, sniffers or route hints right now | `admin_write.backends`, `.sniffer_upload`, `.route_hint` off | Those write routes answer 403; reads and drain stay open          | Until cleared                           |

## In the console

Open `/features`. It lists every flag the build enforces with its value, where the value comes from (default, config, override or "not in this build"), and the risk of switching it off. A banner shows while a protection flag is off. An admin sets an override with a **reason**, which is required, and an **expiry**, which a protection flag must have. **Clear** removes it.

## Over the API

The UI relays the controller's routes. Reads need the viewer role and writes the admin role.

| Request                                 | What it does                                                                                                                       |
| --------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------- |
| `GET /api/features`                     | Every flag with its value and its source.                                                                                          |
| `GET /api/features/overrides`           | The active overrides with the reason, who set them and when they end.                                                              |
| `PUT /api/features/overrides/{name}`    | `{"value": false, "reason": "...", "ttl_secs": 3600}` sets one. `ttl_secs` is optional for a flag that may stay off until cleared. |
| `DELETE /api/features/overrides/{name}` | Clears it, and the config value applies again.                                                                                     |

Against the controller directly the same routes are `GET /features`, `GET /features/overrides`, `PUT /features/overrides/{name}` and `DELETE /features/overrides/{name}`. For a view across the fleet, `GET /api/fleet/features` on the aggregator says how many nodes have each flag on, off or without the code (see [the fleet API](fleet-api.md)).

```sh
# Turn the ACL off for an hour, with a reason that is recorded next to your name.
curl -sS -X PUT "$CTL/features/overrides/acl" \
  -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
  -d '{"value": false, "reason": "false positives after the 14:00 deploy", "ttl_secs": 3600}'

# See what is overridden, then clear it.
curl -sS "$CTL/features/overrides" -H "Authorization: Bearer $TOKEN"
curl -sS -X DELETE "$CTL/features/overrides/acl" -H "Authorization: Bearer $TOKEN"
```

## Things to know

- Proxies pick an override up within about 15 seconds and keep the last one if the controller is unreachable. A proxy drops an expired one itself.
- Turning a flag off does not touch connections that are already open, except where the table says so.
- An override survives a config change. Clear it, or let it end, to go back to the config value.
- A node whose build lacks the code behind a flag reports it as not available, and no override can switch it on.
- Every set and clear writes a `warn` audit line on the controller with the flag, value, reason, end time and actor.

How the flags are registered and enforced is in [runtime feature flags](../../deep-dive/topics/runtime-feature-flags.md).
