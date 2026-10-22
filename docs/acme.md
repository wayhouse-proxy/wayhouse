# Certificates from the controller (ACME)

The controller can issue and renew TLS certificates from an ACME CA (Let's Encrypt or any
other) for the sites you publish, and keep their private keys sealed. It is **off by
default**; start it with `--acme`. Design and reasoning:
[`superpowers/specs/2026-10-15-controller-acme-design.md`](superpowers/specs/2026-10-15-controller-acme-design.md).

Status: built are the sites, the sealed store, replication, the issuance and renewal loop,
the admin API, HTTP-01, external account binding, CNAME-delegated challenge zones, the
Certificates page of the web UI, the DNS-provider plugin hook (`on_acme_dns`,
[#362](https://github.com/wayhouse-proxy/wayhouse/issues/362)) and the distribution of
certificates to gateways ([#363](https://github.com/wayhouse-proxy/wayhouse/issues/363),
[Distribution](#distribution-to-gateways)). The first DNS-provider plugin,
[`acme-dns-cloudflare`](https://github.com/wayhouse-proxy/plugins/tree/main/plugins/acme-dns-cloudflare),
is in the plugins repository ([below](#the-dns-plugin)); for another provider, write one
against the hook in [`plugins.md`](plugins.md#acme-dns-01-on_acme_dns).

## Building without ACME

Issuance sits behind the `acme` cargo feature of `wayhouse-controller`, **on by default**. A
build with `--no-default-features` leaves out the ACME client, the runner and the DNS plugin
solver (and their dependencies: `instant-acme`, `rcgen`, `x509-parser`, `hickory-resolver`).
It still decodes, stores and replicates ACME state, so a mixed cluster stays consistent, but
`--acme` refuses to start and the `/acme` routes answer that ACME is not enabled.

## What it does

You describe **sites**: a set of hostnames (or a `*.zone` wildcard), the gateways that
serve them and either the DNS plugin that can publish `_acme-challenge` TXT records (DNS-01)
or nothing at all (HTTP-01, [below](#http-01)). The leader of the controller tier orders one
certificate per site, proves control with the site's challenge, seals the new private key under the cluster secret key and stores certificate
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
  trusted by browsers but its rate limits are generous. When you switch `--acme-ca` later (staging to
  production), the controller creates a new account at the new CA and replaces the
  `untrusted` rehearsal certificates at once; certificates already trusted are left alone.
- `--secret-key-file` is the same keyring plugin secrets use (`--plugin-secret-key-file`
  is the old name and still works; so does `WAYHOUSE_PLUGIN_SECRET_KEY`). `--acme` refuses
  to start without it.
- `--acme-accept-tos` agrees to the CA's terms of service when the cluster's one ACME
  account is created.
- Not available on a `--role slave` controller.
- On a plain-HTTP listener that is not loopback the controller **refuses to start** with
  `--acme` (the admin token and site changes would cross the network in clear text): serve TLS
  (`--tls-cert/--tls-key`), listen on loopback behind a TLS terminator, or pass
  `--allow-insecure-secrets`.
- Certificates from `letsencrypt-staging`, any CA whose host contains `staging` or has a label
  starting with `test` (such as `test-api.pki.goog`), or whose path says `staging`, and a
  loopback test CA are stored with `"untrusted": true` (shown in the site's status). They are
  for rehearsal only and must never be served to real clients. A private CA whose host has a
  label starting with `test` (such as `testca.corp`) is stored as untrusted too, and there is no
  override.

Under HA every replica needs the same keyring. A replica without it holds issuance and says
so; if it becomes leader nothing is issued until the key is there.

## Sites

All routes need the admin bearer token like the rest of the controller API.

| Route                       | Does                                                                                                                  |
| --------------------------- | --------------------------------------------------------------------------------------------------------------------- |
| `PUT /acme/sites/{name}`    | Create or replace a site. Body: `names`, `dns_plugin`, optional `challenge`, `gateways`, `challenge_zone`, `enabled`. |
| `GET /acme/sites`           | Every site with its certificate status and last error.                                                                |
| `GET /acme/sites/{name}`    | One site.                                                                                                             |
| `DELETE /acme/sites/{name}` | Remove the site, its certificates and its retry state.                                                                |
| `GET /acme/account`         | The account URL, for pinning `CAA accounturi=` at your DNS provider. No key.                                          |

Rules: names are lowercase hostnames, optionally `*.zone`, at most 100 per site (the CA limit);
a name belongs to one site
(`409` otherwise); **a wildcard needs at least one gateway** (`422`), because a wildcard key
should live only on a few hardened nodes. No route returns a private key, a certificate
chain or ciphertext.

## HTTP-01

`"challenge": "http-01"` proves control by answering `GET /.well-known/acme-challenge/<token>`
on port 80 of each name instead of publishing a TXT record. Use it when no DNS plugin fits
and the names are reachable from the internet; it cannot cover a wildcard (`422`), and
`dns_plugin` and `challenge_zone` are refused on such a site (`400`). The default is `dns-01`.

```sh
curl -X PUT -H "Authorization: Bearer $ADMIN" https://controller:7000/acme/sites/panel \
  -d '{"names":["panel.example.com"],"challenge":"http-01","gateways":["edge-1"]}'
```

How the answer reaches the CA:

1. The leader gets the token and key authorization from the CA and commits them as one
   replicated entry, so **every controller node**, followers included, can answer.
2. The CA fetches `http://panel.example.com/.well-known/acme-challenge/<token>`. Your
   gateway forwards exactly that path prefix to a controller node. With
   [`wayhouse-web-sync`](web-proxy.md#http-01-challenges) that is one flag,
   `--acme-challenge-upstream https://controller-1:7000,https://controller-2:7000`; with a
   Caddy you configure yourself:

    ```text
    handle /.well-known/acme-challenge/* {
        reverse_proxy https://controller-1:7000 https://controller-2:7000
    }
    ```

3. `GET /.well-known/acme-challenge/{token}` on the controller needs no token: the body is
   what the CA is meant to read, the token is chosen by the CA per order, and anything unknown
   or expired is a plain `404`. The answer is keyed by the token alone; the `Host` header that
   `wayhouse-web-sync` forwards (the site's name) is ignored. The answer is withdrawn when the order ends. If the leader
   dies mid-order, the next pass drops what is left once it has expired (the order time plus
   the site's `propagation_timeout_secs`).

List **every** controller node (as above) so the forward survives a node going down, or point it at the leader: a follower
that has not applied the entry yet answers `404`, and the CA does not retry a failed
validation. The leader waits two seconds after publishing so a follower has applied the entry.
The expiry is set by the leader's clock and checked on each serving node's, which the five
minutes of slack covers as long as the nodes run NTP.

Rolling upgrade: the cluster-wide ACME level is now 4, so while any member still runs an older
build, **all** ACME writes and issuance (DNS-01 renewals and gateway changes included) pause,
as for the earlier levels. Once an HTTP-01 site exists, an older build cannot decode its
entries, so do not downgrade past this release.

A site whose gateways do not forward the path fails the order with the CA's own message (usually
`unauthorized` or `connection`) in the site's `last_error`, and it counts toward the hourly
failure cap like any other failure. Holding port 80 open to the internet is the price of
HTTP-01; with DNS-01 nothing needs to be reachable.

## CNAME-delegated challenge zones

If the DNS provider that holds your zone has no plugin, or you do not want to give a plugin
access to it, point the challenge names at a zone you can automate. Create once, per name:

```text
_acme-challenge.panel.example.com.  CNAME  _acme-challenge.panel.example.com.acme.other.net.
```

and set `"challenge_zone": "acme.other.net"` on the site. The DNS plugin is then asked to set
and remove the TXT record at `_acme-challenge.panel.example.com.acme.other.net`
(`zone_hint` is `panel.example.com.acme.other.net`), and `example.com` itself is never
changed. The controller still waits until the _original_ name shows the value, so a missing
or wrong CNAME fails the order with a message that names the CNAME it expected. A wildcard
`*.example.com` uses `_acme-challenge.example.com`. `challenge_zone` is a plain hostname, not
a wildcard.

## External account binding

Some CAs (ZeroSSL, Google Trust Services, ...) tie an ACME account to a customer. Give the
controller the binding the CA shows you when it starts:

```sh
wayhouse-controller --acme --acme-ca https://acme.example/directory --acme-accept-tos \
  --acme-eab-kid KID --acme-eab-hmac-key-file /etc/wayhouse/eab.key
```

The key file holds the HMAC key as the CA prints it (base64url) and must not be readable by
group or others. The binding is used once, when the cluster's account is created; the account
key is sealed like every other ACME secret, and the binding key is not stored. Every replica
needs the same flags, as with `--acme-ca`. If you change the CA, the account for the new CA is
created with whatever binding the flags carry at that time.

## Distribution to gateways

A gateway host gets its certificates from the controller, not from the CA. Each gateway has
its own **token**; a token reads only the sites that list that gateway's name in `gateways`.

| Route                          | Auth          | Does                                                                                   |
| ------------------------------ | ------------- | -------------------------------------------------------------------------------------- |
| `POST /acme/gateways`          | admin token   | Register a gateway. Body `{"name": "edge-1"}`. Answers `201` with the token, **once**. |
| `GET /acme/gateways`           | admin token   | The gateways and the sites each may read. No token, no hash.                           |
| `DELETE /acme/gateways/{name}` | admin token   | Revoke: its token answers `401` at once on every replica.                              |
| `GET /acme/bundles`            | gateway token | The sites this gateway may read that have a certificate: `{site, version, not_after}`. |
| `GET /acme/bundles/{site}`     | gateway token | `{version, chain_pem, key_pem, not_after, serial, untrusted}`.                         |

A site's `gateways` list is not checked against the registered gateways. A name that is not
registered (yet) fetches nothing, and a site with an empty list is issued and renewed but no
gateway can read it. Deleting a gateway leaves its name in the sites that list it: whoever
registers that name later can read those sites, so remove the name from the sites too unless
you mean to re-register the same gateway.

The controller stores only the SHA-256 of a token and replicates only that. To rotate a
token, delete the gateway and register it again (a second `POST` of a taken name is `409`).
`GET /acme/bundles/{site}` answers `ETag: "<version>"` and `304` to a matching
`If-None-Match`; a site the token may not read answers `404`, the same as a site that does not
exist. **Any controller node serves bundles**, followers included, so a gateway can use the
nearest replica; a node without the cluster secret key answers `503` instead of a key. Every
served bundle writes one log line with the site, gateway and version, never the key.
Both bundle routes count requests per source address (an IPv6 source by its /64), valid or
not, and answer `429` once a source has used 20 requests and then asks faster than 5 per
second. A gateway polling every minute never notices.

The key travels in the bundle response in plaintext, so serve the controller over TLS (or
reach it on loopback). `--acme` already refuses a plain-HTTP listener on a non-loopback
address unless `--allow-insecure-secrets` is given. A deleted gateway may have copied the
keys it could read: the next renewal creates a new key for every site, so remove the gateway
from the sites it should no longer serve and, for a wildcard, renew early.

### `wayhouse-certsync`

`wayhouse-certsync` runs on the gateway host (it ships in the `wayhouse` image next to
`wayhouse-web-sync`: start it with `entrypoint: /usr/local/bin/wayhouse-certsync`). Create the
gateway, put its token in a file only the sync user can read, and point the sync at every
controller replica:

```sh
curl -sS -H "Authorization: Bearer $ADMIN" -d '{"name":"edge-1"}' \
  https://ctl-1:9443/acme/gateways          # copy "token" into /etc/wayhouse/gw.token
chmod 600 /etc/wayhouse/gw.token
wayhouse-certsync --controller https://ctl-1:9443,https://ctl-2:9443 \
  --token-file /etc/wayhouse/gw.token --dir /var/lib/wayhouse/certs \
  --interval 60s --reload-cmd "caddy reload --config /etc/caddy/Caddyfile"
```

Every interval it lists the readable sites and fetches those whose version differs from
`<dir>/<site>/meta.json`. Files per site: `fullchain.pem`, `privkey.pem` (mode `0600`) and
`meta.json` (`version`, `not_after`, `serial`, `untrusted`). Each file is written to a temp
file and renamed; the chain is replaced first, the key second and `meta.json` last, so
`meta.json` names a version only when both files of it are in place. A reader that opens the
chain and the key between the two renames can briefly see a new chain with the old key;
reload after the sync (`--reload-cmd`), or retry on a mismatch.

- Nothing is rewritten when the version is unchanged and both PEM files are intact; a
  deleted or truncated file is fetched again. A site is never downgraded: a controller that
  lists a lower version with an expiry no later than the installed one (a follower that has
  not applied the newest issuance) is ignored, while a recreated site with a later expiry is
  installed.
- `--reload-cmd` runs after a change and on the first successful pass after startup, `--once` runs included (a crash between the rename and the
  reload would otherwise leave the proxy on the old certificate). It runs without a shell
  (the command is split on spaces, so a path with spaces does not work; wrap it in a
  script), at most 30 seconds, and is retried on the next pass if it fails.
- Gateway registration: if a forwarded `POST /acme/gateways` answers `504`, the write may
  have been applied while the token never reached you. Delete the gateway and register it
  again. Over plain-http `--ha-peers` the freshly minted token crosses the peer link in clear
  text during the forward, like other forwarded tokens; use https peers.
- The private key sits in plaintext in the heap of the serving controller node for the length
  of a response, and in the `wayhouse-certsync` process while it writes the file.
- When no controller answers, the files on disk stay as they are and the sync retries every
  interval. `--controller` takes several URLs and tries the one that answered last first.
  A refused token (`401`) is logged with the hint to check the token file; nothing is
  deleted.
- Files of a site that is no longer listed are left in place.
- `untrusted: true` marks a certificate from a staging or test CA; the sync logs a warning.
- `--once` runs a single pass and exits non-zero on failure, for cron or a systemd timer.
  `--ca-file` trusts an extra CA for an https controller. Plain http is accepted only for a
  loopback controller, or with `--allow-insecure-secrets`.
- There is no push channel yet: changes arrive within one interval.

The synced files can also feed wayhouse's own native TLS (the controller, aggregator and UI
`--tls-cert`/`--tls-key`, and `settings.admin.tls`): point them at
`<dir>/<site>/fullchain.pem` and `<dir>/<site>/privkey.pem`. Those servers re-read the PEM
files every 30 s when their mtime changes, so a renewal needs no `--reload-cmd` for them (see
[12 Deployment](12-deployment.md), "Native TLS"). Run the sync as a user that can read the key.

## The DNS plugin

A site's `dns_plugin` is the id of a plugin install that is **enabled** and **approved for the
`on_acme_dns` trigger** ([`plugins.md`](plugins.md#acme-dns-01-on_acme_dns)); anything else is
refused with `400`, and so is any site while the controller runs without `--plugins`. For each
name of an order the leader asks the plugin to set the TXT record, then looks the record up
itself (the host's resolver, every 5 seconds, up to the site's `propagation_timeout_secs`,
default 120) before it tells the CA to validate, and always asks the plugin to remove it
afterwards, also when the order failed. The whole order may take `propagation_timeout_secs` per
name plus five minutes, and never more than two hours. A plugin that does not answer, traps, refuses or is
disabled mid-order fails that order; the reason is the site's `last_error` and the usual
backoff applies. If you disable the plugin or revoke the approval, orders fail with "not
available" until you restore it.

The first lookup comes 5 seconds after the plugin answered, so a caching resolver is not asked
before the provider serves the record. If an order still fails with "not visible to the
resolver", a caching resolver (Unbound, systemd-resolved) may be holding the answer "does not
exist" for the zone's negative TTL, up to 30 minutes at Cloudflare, and retries within that time
get the same answer. Flush that resolver's cache for the name, or point the controller host's
`/etc/resolv.conf` at a resolver that does not cache. Using a site name nobody has queried
yet also works, because there is nothing negative to cache for it.

Known limitation: the 5 second delay makes this unlikely but does not remove it. A provider
that needs longer than that to serve the record can still prime the cache of a caching
resolver. Checking the zone's authoritative nameservers instead is tracked in
[#408](https://github.com/wayhouse-proxy/wayhouse/issues/408), scheduled after 0.3.0.

The plugin only ever sees the record name and value. ACME keys stay in the controller.

### Cloudflare

`acme-dns-cloudflare` in [`wayhouse-proxy/plugins`](https://github.com/wayhouse-proxy/plugins)
sets and removes the TXT record through the Cloudflare API. Create an API token with
`Zone:Read` and `DNS:Edit` on the zones your sites use, upload and install the module with the
capabilities in its `caps.json` (the `on_acme_dns` trigger, the `http` host `api.cloudflare.com`
and the secret slot `CLOUDFLARE_API_TOKEN`), set the slot with `PUT
/plugins/{id}/secrets/CLOUDFLARE_API_TOKEN`, and give the site `"dns_plugin": "<id>"`. The
plugin finds the zone by trying the host and its parent names, adds the record with a 60 s TTL
and removes only the record that carries the challenge value. It talks to no other host.

## Failure behaviour

- A failed order backs off from 5 minutes up to 6 hours and stops after 5 failures in an
  hour, so a broken DNS plugin cannot use up the CA's failed-validation limit. The count and
  last error are in the site's status and survive a leader change.
- Renewal starts at a third of the lifetime and not near the end, so there is room for
  retries: alert on `wayhouse_acme_cert_expiry_seconds` dropping below a third. A gateway
  whose controller is down keeps serving the files it has ([below](#distribution-to-gateways)).
- A CA rate limit (`429`/`rateLimited`) pauses the site for the CA's `Retry-After` (at most 7
  days) instead of the normal backoff when that is longer.
- A restart with an expired or nearly expired certificate renews at once.

## Metrics

`GET /metrics` on the **current leader** (a follower does not issue and emits neither the
expiry nor the held series): `wayhouse_acme_cert_expiry_seconds{site}` (gauge, seconds
until `notAfter`, negative once expired), `wayhouse_acme_issue_total{result="ok|error"}`
(counter) and `wayhouse_acme_held` (gauge, `1` while this node has no usable secret key).

## Security notes

Keys are sealed with XChaCha20-Poly1305 under the cluster keyring, bound to their site and
version, replicated only as ciphertext, and appear in plaintext only in the issuing leader's
memory. Losing every copy of the keyring loses the sealed keys: certificates are then
reissued (the CA does not care), so keep the key backup the secret-storage design asks for
anyway.

**Rotating the key.** The steps are those of plugin secrets
([`superpowers/specs/2026-10-08-plugin-secret-storage-design.md`](superpowers/specs/2026-10-08-plugin-secret-storage-design.md),
[`plugins.md`](plugins.md)), but the rewrap is a separate call for the ACME keys: (1) append the new key to the
keyring file on every controller, (2) `POST /admin/plugins/secrets/rewrap` for the plugin secrets (if plugins are on) **and**
`POST /admin/acme/rewrap` for the account key and every certificate key, (3) remove the old key. The ACME call refuses until
every member reports the active key, is idempotent, and answers `{"rewrapped", "skipped", "unreadable", "active_key_id"}`;
`GET /acme/account` and `GET /acme/sites` show the `key_id` each secret is under. Removing the old key before step 2 stops
issuance (the account cannot be opened) and makes the certificate keys under it unservable.
