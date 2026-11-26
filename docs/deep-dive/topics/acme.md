---
title: "Certificates: ACME issuance, renewal and distribution"
description: How the controller gets TLS certificates for published sites from an ACME CA, why only the leader orders and keys stay sealed, how DNS and HTTP challenges are proven, and how gateway hosts receive certificates without ever speaking ACME.
---

A site published on the web needs a valid certificate, and certificates expire. Doing that by hand on every gateway host does not scale, and doing it independently on each gateway means each one needs credentials for the DNS zone and each one hits the CA's rate limits. The controller therefore issues and renews certificates itself, keeps the private keys sealed, and hands them to the gateways that need them. This page is about how that works and why it is shaped the way it is. The options are in the [Manual](../../manual/index.md) and the generated [Reference](../../reference/index.md), and the full API is in `docs/acme.md`.

## The problem

1. **Many gateways, one account.** CAs limit orders per account and per name. Every gateway ordering for itself multiplies the load and the exposure.
2. **Private keys are high value.** A wildcard key is worth more than any single site's. Where it sits matters.
3. **Challenges need access to something.** DNS-01 needs to publish a TXT record in a zone, which means an API credential. HTTP-01 needs to answer on port 80 of the name.
4. **A failing CA must not become an outage,** and a failing loop must not hammer the CA into a ban.
5. **High availability.** Several controller nodes, and a leader that can change in the middle of an order.
6. **The data path stays clean.** The proxy never speaks ACME and never parses HTTP.

## The model: sites, gateways, one account

A **site** is a set of hostnames, or a `*.zone` wildcard, together with the gateways that serve them and the way to prove control: a DNS plugin for DNS-01 or HTTP-01. The controller orders **one certificate per site**. A name belongs to at most one site, and a site holds at most 100 names, which is the CA's own limit. The cluster has **one ACME account**, created when the first order is made, and its URL can be read so an operator can pin `CAA accounturi=` at the DNS provider.

A **wildcard needs at least one gateway**. The reason is the key: a wildcard key should live only on a few hardened nodes, so it cannot be issued with no say about where it goes.

Issuance is behind the `acme` cargo feature, on by default. A build without it leaves out the ACME client and its dependencies, but still **decodes, stores and replicates** ACME state, so a mixed cluster stays consistent. It refuses `--acme` at start and answers 501 on the routes.

## The loop: leader only, term tagged, backoff replicated

Only the **Raft leader**, or a standalone controller, runs the issuance and renewal loop. For each enabled site that is due, it runs one order through the site's solver, seals the new private key under the cluster keyring, and commits the certificate as **one entry tagged with the leader's term**. A deposed leader's late result carries an older term and is dropped. This is the same trick as the relay entries in [config rollout](config-rollout.md): a proposal that arrives after its author lost leadership must not win.

The failure policy is also replicated:

- **A certificate is due** when less than a third of its lifetime is left. The renewal point moves earlier by up to 5 percent of the lifetime at random, so a fleet issued together does not renew together.
- **A failure advances a replicated backoff**: 5 minutes, doubling, capped at 6 hours. A CA's own `Retry-After` is honoured for rate limits, up to 7 days. Because the state is in the log, a new leader continues the backoff instead of starting over, and does not hammer the CA.
- **At most 5 failed orders per site and hour** before the loop waits out the window, so a misconfigured DNS plugin cannot burn the CA's failed validation limit.
- An order has a ceiling of 2 hours.

The account key and every certificate key are **sealed under the same cluster keyring** that protects other secrets (see [secrets](secrets.md)), with a scope of its own. Under HA every replica needs the same keyring. A replica without it holds issuance and says so, and if it becomes leader nothing is issued until the key is there. The stored state is crash idempotent per Raft index, like the other replicated areas (see [stored UI data](ui-data.md)), and only two versions of a site's certificate are kept.

### Staging and untrusted certificates

A certificate from `letsencrypt-staging`, from any CA whose host contains `staging` or has a label starting with `test`, or from a loopback test CA, is stored with `untrusted: true`. It exists for rehearsal and must never be served to real clients. When an operator later switches the CA from staging to production, the controller creates a new account at the new CA and replaces the untrusted certificates at once, leaving trusted ones alone.

## Proving control

### DNS-01 through a plugin

The controller never holds a DNS credential in its own code. A **DNS provider plugin** with the trigger `on_acme_dns` is asked to set and remove the TXT record, and the plugin never sees an ACME key (see [plugins](plugin-host.md)). The controller looks the record up **itself** before it tells the CA to validate, by querying the zone's own name servers directly, up to 8 of them with a 3 second timeout each.

The timing has a reason that came from experience. A caching resolver remembers "does not exist" for the zone's negative TTL, 30 minutes at Cloudflare, if it is asked in the instant before the provider serves the record. So the first lookup waits **20 seconds** before it asks, and then polls every 5 seconds. A plugin call gets at least 30 seconds, queue wait included, and a site with a longer propagation timeout gives its provider that long.

**CNAME delegation** covers the case where the zone's provider has no plugin or the operator does not want to give one access: the challenge name is pointed with a CNAME at a zone that can be automated, and the plugin sets the TXT record there. The controller still waits until the **original** name shows the value, so a missing or wrong CNAME fails the order with a message naming the CNAME it expected.

### HTTP-01 and the replicated token

HTTP-01 proves control by answering a path on port 80, which suits names reachable from the internet and cannot cover a wildcard. The problem is that the CA's request lands on a gateway, and the order is made by the leader. The solution is to make the answer **a replicated entry**: the leader commits the token and the key authorization as one entry, so every controller node, followers included, can answer. The gateway forwards exactly the challenge path prefix to a controller node, and the controller route needs no token because the body is what the CA is meant to read.

Two details come from failure modes. A follower that has not applied the entry yet answers 404, and the CA does not retry a failed validation, so the leader **waits two seconds** after publishing. And expiry is set by the leader's clock and checked by each serving node's, which the slack of five minutes covers if the nodes run NTP. The answer is withdrawn when the order ends, and if the leader dies mid order the next pass drops what is left once it has expired.

### External account binding

Some CAs tie an ACME account to a customer. The controller takes the binding identifier and the HMAC key from flags, uses it **once**, when the cluster's account is created, and does not store the binding key. The key file must not be readable by group or others.

## Distribution: gateways never talk to the CA

A gateway gets its certificates from the controller. Each registered gateway has a **token of its own**, and it reads only the sites that list its name. The controller stores only the SHA-256 of a token and replicates only that. A bundle is the certificate chain, the private key, its expiry and serial, and whether it is untrusted, and **any controller node serves bundles**, followers included, so a gateway can use the nearest replica. A node without the cluster secret key answers 503 instead of a key. Each served bundle writes one log line with the site, gateway and version, never the key.

Revoking a gateway makes its token fail at once on every replica and removes its name from every site, so registering the same name later does not give the new holder the old sites. A revoked gateway may have copied the keys it could read, so the next renewal creates a new key for every site, and for a wildcard an operator should renew early. The key travels in a bundle response in plaintext, so the controller should be served over TLS, and `--acme` refuses a non loopback plain HTTP listener unless explicitly overridden.

The bundle routes are rate limited per source address, an IPv6 source counted by its `/64`, valid requests or not. After 20 requests a source that asks faster than 5 per second gets 429 with a retry hint. A gateway that polls once a minute never notices.

### `wayhouse-certsync`

On the gateway host, a small program, `wayhouse-certsync`, polls the controllers and installs the files. It takes several controller URLs and tries the one that answered last first. Every interval it lists the readable sites and fetches those whose version differs from the one recorded on disk.

The installation is designed so that a reader never sees half a file. Each file is written to a temporary file and renamed. The chain is replaced first, the key second and the metadata file **last**, so the metadata names a version only when both files of it are in place. A reader that opens the chain and the key between the two renames can still briefly see a new chain with the old key, so a reload after the sync, or a retry on mismatch, is the documented answer.

Other rules guard the host. Nothing is rewritten when the version is unchanged and both files are intact, and a deleted or truncated file is fetched again. A site is **never downgraded**: a controller that lists a lower version with an expiry no later than the installed one, a follower that has not applied the newest issuance, is ignored, while a recreated site with a later expiry is installed. When no controller answers, the files on disk stay as they are. The reload command runs after a change and on the first pass after start, without a shell, for at most 30 seconds, and is retried if it fails. A rejected token is logged with a hint and nothing is deleted.

## What was traded away

- **Keys in plaintext in memory.** The private key sits in the heap of the serving node while a response is written, and in certsync while it writes the file. Sealing protects storage, not the moments of use.
- **A poll, not a push.** Changes arrive within one interval. There is no push channel yet.
- **A leaked gateway leaks keys.** A revoked gateway keeps whatever it already copied, and the answer is renewal.
- **The failure cap.** Five failures an hour may delay a site that would have succeeded on the sixth try, in exchange for never being banned.
- **Rolling upgrades pause ACME writes** until every member can decode the newest entries, and a downgrade past an HTTP-01 site is not possible.

## Where to look

- `crates/wayhouse-acme/src/`: the client, the models and the renewal rules.
- `crates/wayhouse-controller/src/acme/runner.rs`: the leader loop, term tagging and the failure window.
- `crates/wayhouse-controller/src/acme/challenge.rs` and `dns_plugin.rs`: HTTP-01 answers and the DNS plugin solver.
- `crates/wayhouse-controller/src/acme/store.rs` and `bundles.rs`: the replicated store and the bundle routes.
- `crates/wayhouse-certsync/src/`: the gateway side installer.
- `docs/acme.md`: the operator guide and the route tables.
