# Certificates from the controller (ACME)

The controller can issue and renew TLS certificates from an ACME CA (Let's Encrypt or any
other) for the sites you publish, and keep their private keys sealed. It is **off by
default**; start it with `--acme`. Design and reasoning:
[`superpowers/specs/2026-10-15-controller-acme-design.md`](superpowers/specs/2026-10-15-controller-acme-design.md).

Status: **slice 1** is built (sites, the sealed store, replication, the issuance and
renewal loop, the admin API). Still to come, in this order: the DNS-provider plugin hook
(`on_acme_dns`, [#362](https://github.com/wayhouse-proxy/wayhouse/issues/362)), then
distribution of certificates to gateways ([#363](https://github.com/wayhouse-proxy/wayhouse/issues/363)).
Until the plugin hook exists no order can complete: a site's order fails with "the DNS
plugin is not available on this node", visible in the site's status.

## What it does

You describe **sites**: a set of hostnames (or a `*.zone` wildcard), the gateways that
serve them and the DNS plugin that can publish `_acme-challenge` TXT records. The leader
of the controller tier orders one certificate per site, proves control with the DNS-01
challenge, seals the new private key under the cluster secret key and stores certificate
and sealed key (replicated under HA). It renews when less than a third of the lifetime is
left, with backoff after failures.

The data path never speaks ACME and never parses HTTP.

## Turning it on

```
wayhouse-controller --plugins --secret-key-file /etc/wayhouse/secret.keys \
  --acme --acme-ca letsencrypt-staging --acme-accept-tos
```

- `--acme-ca` is required and has no default: `letsencrypt`, `letsencrypt-staging` or an
  `https://` ACME directory URL. Try `letsencrypt-staging` first; its certificates are not
  trusted by browsers but its rate limits are generous.
- `--secret-key-file` is the same keyring plugin secrets use (`--plugin-secret-key-file`
  is the old name and still works; so does `WAYHOUSE_PLUGIN_SECRET_KEY`). `--acme` refuses
  to start without it.
- `--acme-accept-tos` agrees to the CA's terms of service when the cluster's one ACME
  account is created.
- Not available on a `--role slave` controller.

Under HA every replica needs the same keyring. A replica without it holds issuance and says
so; if it becomes leader nothing is issued until the key is there.

## Sites

All routes need the admin bearer token like the rest of the controller API.

| Route                       | Does                                                                                   |
| --------------------------- | -------------------------------------------------------------------------------------- |
| `PUT /acme/sites/{name}`    | Create or replace a site. Body: `names`, `dns_plugin`, optional `gateways`, `enabled`. |
| `GET /acme/sites`           | Every site with its certificate status and last error.                                 |
| `GET /acme/sites/{name}`    | One site.                                                                              |
| `DELETE /acme/sites/{name}` | Remove the site, its certificates and its retry state.                                 |
| `GET /acme/account`         | The account URL, for pinning `CAA accounturi=` at your DNS provider. No key.           |

Rules: names are lowercase hostnames, optionally `*.zone`; a name belongs to one site
(`409` otherwise); **a wildcard needs at least one gateway** (`422`), because a wildcard key
should live only on a few hardened nodes. No route returns a private key, a certificate
chain or ciphertext.

## Failure behaviour

- A failed order backs off from 5 minutes up to 6 hours and stops after 5 failures in an
  hour, so a broken DNS plugin cannot use up the CA's failed-validation limit. The count and
  last error are in the site's status and survive a leader change.
- If the controller is down gateways keep serving the last certificate until it expires.
  That is why renewal starts at a third of the lifetime and not near the end: alert on
  `wayhouse_acme_cert_expiry_seconds` dropping below a third.
- A restart with an expired or nearly expired certificate renews at once.

## Metrics

`GET /metrics` on the controller: `wayhouse_acme_cert_expiry_seconds{site}` (gauge, seconds
until `notAfter`, negative once expired), `wayhouse_acme_issue_total{result="ok|error"}`
(counter) and `wayhouse_acme_held` (gauge, `1` while this node has no usable secret key).

## Security notes

Keys are sealed with XChaCha20-Poly1305 under the cluster keyring, bound to their site and
version, replicated only as ciphertext, and appear in plaintext only in the issuing leader's
memory. Losing every copy of the keyring loses the sealed keys: certificates are then
reissued (the CA does not care), so keep the key backup the secret-storage design asks for
anyway. Rotation works as for plugin secrets
([`superpowers/specs/2026-10-08-plugin-secret-storage-design.md`](superpowers/specs/2026-10-08-plugin-secret-storage-design.md)).
