# Controller-managed ACME certificates (DNS-01 first)

Status: design (2026-10-15). Decisions marked **[decided]** come from Wueschli's direction for the web front door ([#297](https://github.com/wayhouse-proxy/wayhouse/issues/297)); the rest is the recommendation the build plan follows unless the maintainer changes it. This **reverses #297's "ACME / certificate issuance is out of scope"** line: the controller issues and renews certificates; Wayhouse's data path still never speaks ACME or parses HTTP.

## Goal

An operator lists the sites they publish (hostnames, optionally wildcards) and which gateway nodes serve them. The controller obtains one certificate per site from an ACME CA, renews it before it expires, and hands it, with its private key, only to the gateways assigned to that site. Consumers (a packaged Caddy, or Wayhouse's own native TLS) read plain files, so neither needs to know ACME exists.

Success: with `--acme` on, a site in the API gets a valid chain and key on its gateways' disks, renewals happen without operator action, a controller restart or leader change loses nothing, and the private keys exist in plaintext only in the issuing leader's memory and on the assigned gateways.

Non-goals: an HTTP reverse proxy or HTTP-01 serving in Wayhouse (HTTP-01 is a later slice, see below), TLS-ALPN-01, certificates for the fleet's own admin/peer TLS (they keep their operator-supplied files), a general PKI, OCSP stapling.

## Decisions

- **[decided]** Opt-in `--acme` on the controller, off by default (like `--plugins`). Without it, no ACME code runs and none of the routes below exist.
- **[decided]** DNS-01 first. The controller need not be reachable from the internet and wildcards become possible.
- **[decided]** The DNS provider is a plugin hook: the plugin only sets and removes a TXT record. ACME crypto, the account key and certificate keys stay in the controller; a plugin never sees them.
- **[decided]** Certificates are per site and distributed only to the gateways assigned to that site. Wildcards only inside gateways: a wildcard site must name at least one gateway, and the API refuses to assign it to "all".
- **[decided]** Private keys are sealed with the secret-storage machinery of [#235](2026-10-08-plugin-secret-storage-design.md) (built in plugin slice 8): XChaCha20-Poly1305 under the cluster keyring, replicated as ciphertext, never returned by an unscoped API.
- **[recommended]** ACME client: the `instant-acme` crate (Apache-2.0, pure Rust, async, `ring` feature matches our TLS provider), with `rcgen` for keys and CSRs and `x509-parser` for `notAfter`. Licenses are compatible with the workspace; a maintenance check is part of slice 1 (last release, open issues).
- **[recommended]** No default CA. `--acme-ca <letsencrypt|letsencrypt-staging|URL>` is required with `--acme` so nobody gets production rate limits, or untrusted staging certificates, by accident. `--acme-accept-tos` is required for the account to be created. External account binding (ZeroSSL and similar) is not built; the account flow leaves room for it.
- **[recommended]** Key algorithm ECDSA P-256, a fresh key per issuance (a leaked key has a short useful life; there is no key reuse across renewals).

## Components

```
operator ──API──► controller ──(leader only)──► ACME CA (instant-acme)
                    │   ▲                          ▲
                    │   └─ on_acme_dns hook ──► DNS-provider plugin ──http──► DNS API
                    │ Raft: sealed account key, sealed cert keys, chains, status
                    ▼
        gateway credential ──GET /acme/bundles/{site}──► certsync ──files──► Caddy / wayhouse
```

1. **`wayhouse-acme` (new library crate).** Wraps the ACME client behind `AcmeCa` (account, order, challenge, finalize) and defines `DnsSolver` (`present(fqdn, value)`, `cleanup(fqdn, value)`, `wait_visible`). Contains the cert model (`CertBundle { chain_pem, key, not_after, serial, names }`), the renewal arithmetic and an in-process **fake CA** under a `test-support` feature (see Testing). It has no Raft, no HTTP server and no plugin dependency, so it is testable alone.
2. **Controller module `acme/`.** Site store, the sealed cert store, the leader-only issuance loop, replicated entries, admin API, metrics. Uses the existing `Keyring` and `Sealed`.
3. **Plugin trigger `on_acme_dns`.** The `DnsSolver` the controller uses calls the site's DNS plugin through the existing hook queue (below).
4. **Gateway side.** Gateway credentials, a bundle endpoint, and `wayhouse-certsync`, a small binary that materializes bundles as files (below).

## Data model

All of it is controller-owned state, like plugin installs. None of it is part of the operator's Wayhouse YAML or its revisions, and the data path never sees it.

- **Site** `{name, names[], wildcard_ok, gateways[], dns_plugin, propagation_timeout, enabled}` set with `PUT /acme/sites/{name}`. `names` are lowercase hostnames or `*.zone`; a name may belong to one site only. `dns_plugin` is an install id that must have approved the `on_acme_dns` trigger (400 otherwise). Wildcards need `gateways` non-empty (422).
- **Account** one per cluster `{directory_url, account_url, sealed_key}`. Created lazily by the leader on the first order. `AcmeAccountSet` applies only if none exists, so two racing leaders converge on the first; the loser's account is orphaned at the CA, which is harmless.
- **Cert version** per site `{version, chain_pem, sealed_key, not_after, issued_at, serial}`; the newest version wins and at most two are kept (the previous one stays until gateways have picked up the new one, see Distribution).
- **Attempt state** per site `{failures, next_attempt_at, last_error}`, replicated so a new leader continues the backoff instead of hammering the CA.

Raft entries (new, gated for rolling upgrades like the plugin entries): `AcmeSiteSet`, `AcmeSiteDelete`, `AcmeAccountSet`, `AcmeCertIssued` (term-tagged like `PluginState`, so a deposed leader's late result is dropped), `AcmeAttempt`. Encryption happens on the leader before proposing; plaintext keys never cross the peer link or reach the log. The sealing scope is `install = "acme"`, `slot = "account"` or `"cert:<site>@<version>"`, so a ciphertext copied to another site or version fails authentication. Snapshots carry the same ciphertext. Standalone controllers use the same code with the local store.

## Issuance and renewal

- Only the Raft leader (or the standalone node) runs the loop, as the plugin tick runner does. A node without the keyring, or a leader whose keyring lacks the active key id, **holds** issuance, raises the same visible alert as a held plugin, and keeps serving what it has.
- A site is due when it has no certificate, its names changed, or less than **one third of the lifetime** remains (`not_after`, `issued_at`; for a 90-day certificate that is 30 days, for a 6-day certificate 2 days), plus up to 5 % jitter. The loop wakes on a timer and on site changes.
- One order per site covering all its names. For each DNS-01 authorization: `present` through the plugin, **wait until the TXT is visible** by querying DNS ourselves (a hickory-resolver lookup, bounded by `propagation_timeout`, default 120 s), tell the CA to validate, poll, then `cleanup` always, including on failure. A CNAME-delegated `_acme-challenge` is out of scope for slice 1.
- Failure: exponential backoff from 5 min to 6 h with the failure count and error replicated; a failed-authorization limit at the CA is respected by capping at 5 failures per hour per site. `last_error` is shown in the API with provider secrets scrubbed by the same function the plugin host uses.
- **Controller down or no leader:** gateways keep serving the last certificate until it expires, which is why renewal starts at one third of the lifetime and not near the end. Metrics and the API expose the time to expiry per site; docs recommend alerting below one third.
- CAA: the API shows the account URI, so operators can pin `CAA ... accounturi=`.

## DNS-01 plugin hook

A plugin declares `"triggers": {"on_acme_dns": true}` (needs `on_timer`, like the other triggers). The host calls the export `on_acme_dns(op_ptr, op_len)` with `{"op":"present"|"cleanup","fqdn":"_acme-challenge.example.com","value":"<txt>","zone_hint":"example.com"}`. The plugin does the provider call through its approved `http` capability and `${secret:NAME}` slots, and reports through a new import `acme_dns_result(ok, message)`; no answer or a trap is a failure. The call goes through the existing bounded hook queue, with a timeout from the site's `propagation_timeout`, so a slow provider fails one issuance and never builds a backlog. Only the leader calls it.

ABI: the trigger is additive (modules that do not declare it are unaffected), so this is **proposed without a bump** of the plugin ABI minor, the same call the host:port secret slot made. If the maintainer prefers a bump while the major is 0, every official plugin must be rebuilt; flagged as an open decision. The `selftest` plugin in the plugins repo gains the hook so CI can drive it with a fake provider; no real provider plugin is part of this work.

## Distribution

**Contract: one directory layout and one endpoint.**

- Layout (what Caddy's `load_files`/`tls { }` and Wayhouse's `ReloadingCert` already read): `<dir>/<site>/fullchain.pem`, `<dir>/<site>/privkey.pem` (mode 0600, written to a temp file and renamed, chain before key so a reader never sees a new key with an old chain), `<dir>/<site>/meta.json` (`version`, `not_after`, `serial`).
- Endpoint: `GET /acme/bundles` lists the sites assigned to the caller with `{version, not_after}`; `GET /acme/bundles/{site}` returns `{version, chain_pem, key_pem, not_after}`, honors `If-None-Match` (the version as ETag) and returns `304`. Any controller node can answer: every node holds the keyring, decrypts locally and so a follower serves gateways, and gateways list several controller URLs.
- **Gateway credential:** `POST /acme/gateways {name}` (admin token) returns a random token once; the controller stores only its hash. A gateway token reaches only its own `/acme/bundles` and only the sites that list its name in `gateways`. It cannot read the admin API, and the admin token is never used by gateways. Deleting a gateway revokes it; the site's key is then reissued on the next renewal (fresh key), and docs say revocation of an already-served key is the CA's and browsers' job.
- Bundles are served only over the controller's TLS listener or a loopback bind; the controller refuses to start `--acme` with a plain-HTTP non-loopback listener unless `--allow-insecure-secrets` is given, the same rule as plugin secrets. Response bodies are never logged. Every key fetch writes an audit line (`site`, `gateway`, `version`, never the key).
- **`wayhouse-certsync`**: a small binary (own crate, `wayhouse-http` client only) that polls the list endpoint on a timer plus on SSE hint, fetches changed bundles, writes the layout above atomically and optionally runs a reload command or POSTs to a local URL (Caddy's admin `/load` is the slice-2 web-sync's job; certsync only writes files). It keeps the last good files when the controller is unreachable. It ships as an extra binary in the existing `wayhouse` image (no new GHCR image), matching the web-sync recommendation; to be confirmed by Wueschli.

Caddy v1 consumers point their `tls` at the layout (operator-supplied-paths already planned in the Caddy design); Wayhouse's own native-TLS terminators point `--tls-cert/--tls-key` at the same files and reload on change.

## Security notes

- The controller leader holds every fleet private key in memory during issuance and the DNS credentials only inside the plugin host (expanded host-side). Compromise of a controller node is compromise of the keyring and so of all sealed keys, as already stated in the secret-storage threat model. Bundles scoped per gateway limit what a compromised *gateway* gets.
- The key hits plaintext in three places only: leader memory in `zeroize` buffers while generating and sealing, the gateway's files (0600), and the TLS response to an authenticated gateway.
- `Debug`/`Display` of keys and tokens print `<redacted>`; a raw-bytes scan test (as the secret-storage gate requires) proves a sentinel key never appears in the log, snapshot, sled directory or any list/status response.
- Rotation: `POST /admin/acme/rewrap` re-encrypts the account and certificate keys (one `AcmeOp::Rewrap` entry; items carry the nonce they were derived from and are skipped if it changed, like `RewrapItem`), as step 2 of the secret-storage rotation next to `/admin/plugins/secrets/rewrap`.
- The fleet channel to the CA uses the system roots through `reqwest`/`rustls`, like the rest of the controller; the CA directory URL must be `https` except for `127.0.0.1` in tests.

## Testing

No real CA or DNS in CI.

- **Fake CA** in `wayhouse-acme` (`test-support` feature): an in-process axum server implementing the RFC 8555 subset the client uses (directory, newNonce, newAccount with JWS verification, newOrder, authorization, DNS-01 challenge, finalize, certificate download). It **actually validates** the challenge by looking the TXT up in the test's fake provider, and signs the CSR with a test CA, so renewals, failures (`urn:ietf:params:acme:error:dns`), rate-limit replies and slow validation can be scripted. The tests do not need Pebble, a container or network.
- Unit: renewal arithmetic, name validation, backoff, scrub function.
- Controller (standalone and HA via the existing `test_support` harness): issue, renew on advancing time, leader change mid-order resumes without a duplicate certificate, restart keeps everything, keyring missing holds, raw-bytes scan, AAD binding, rewrap.
- Gateway: scoping (token for site A cannot read site B), 304 handling, atomic write order, last-good kept when the controller is down.
- Plugin hook: the `selftest` plugin and a fake provider drive `present`/`cleanup`; a trapping or silent plugin fails the order and still triggers cleanup.

## Slices

| # | Slice | Closes |
|---|---|---|
| 1 | `wayhouse-acme` crate, fake CA, keyring scope generalization, controller `acme/` (sites API, store, replicated entries, issuance and renewal loop with a built-in test solver, metrics), `--acme` flags | part of the umbrella |
| 2 | `on_acme_dns` trigger in the plugin host, controller `DnsSolver` via the hook queue, `selftest` update, `docs/plugins.md` | DNS hook issue |
| 3 | Gateway credentials, bundle endpoint, `wayhouse-certsync`, docs chapter `docs/acme.md`, #297 revision | distribution issue |
| later | HTTP-01 (controller publishes a token, gateways serve `/.well-known/acme-challenge/`); UI page for sites and expiry; CNAME-delegated challenge zones; EAB; a real provider plugin in the plugins repo | separate issues, p3 |

Slices 1 to 3 bundle into three PRs (this spec and plan ride with the first). Nothing here is released by this work; release remains the maintainer's decision.

## Open decisions for Wueschli

1. Plugin ABI: additive trigger without a minor bump (recommended) or a bump.
2. `wayhouse-certsync` as an extra binary in the `wayhouse` image (recommended) or its own image.
3. Flag name for the keyring: keep `--plugin-secret-key-file`, and add the neutral alias `--secret-key-file` (recommended; same file and env var), because ACME now uses it too.
