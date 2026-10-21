# Controller ACME (DNS-01) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans (native, no subagents) to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The controller issues and renews ACME certificates per site with DNS-01 through a plugin hook, stores them sealed and replicated, and serves them only to the gateways assigned to each site.

**Architecture:** A library crate `wayhouse-acme` (ACME client wrapper, `DnsSolver` trait, cert model, fake CA for tests) and a controller module `acme/` (sites, sealed cert store, leader-only issuance loop, Raft entries, admin API). Slice 2 adds the plugin trigger `on_acme_dns`; slice 3 adds gateway credentials, the bundle endpoint and `wayhouse-certsync`.

**Tech Stack:** Rust, `instant-acme` (ring feature), `rcgen`, `x509-parser`, existing `Keyring`/`Sealed`, openraft state machine, axum, sled.

**Spec:** `docs/superpowers/specs/2026-10-15-controller-acme-design.md`

## Global Constraints

- Opt-in `--acme`; off by default; no ACME route exists without it.
- `--acme-ca <letsencrypt|letsencrypt-staging|URL>` required with `--acme`; `--acme-accept-tos` required to create the account; CA URL must be `https` except `127.0.0.1`.
- ECDSA P-256, fresh key per issuance. Renew when less than one third of the lifetime remains (+ up to 5 % jitter).
- Sealing scope: `install = "acme"`, `slot = "account"` or `"cert:<site>@<version>"`.
- Wildcard names require a non-empty `gateways` list (422).
- Backoff 5 min to 6 h; at most 5 failed attempts per hour per site.
- Keys and tokens never in `Debug`/`Display`/logs/list responses; plaintext only in `zeroize` buffers.
- Bundles served only over TLS or loopback (`--allow-insecure-secrets` opts out).
- No real CA or DNS in CI. Public text says "Wueschli", never the maintainer's first name. Nothing is released.
- Commit trailers and PR body attribution as required by the harness.

## Review Focus

- Leader change during an order: no second certificate for the same names; the new leader resumes from replicated attempt state.
- A DNS plugin that traps, never answers or times out: the order fails, `cleanup` still runs, backoff advances.
- Site rename/overlapping names: a name in two sites is refused.
- Controller restart with an expired or nearly expired certificate: renewal is attempted immediately, existing cert still served.
- A follower without the keyring: serves nothing for keys, holds, never crashes.

---

## Slice 1: crate, store, issuance loop

### Task 1: `wayhouse-acme` crate skeleton and renewal arithmetic

**Files:** Create `crates/wayhouse-acme/{Cargo.toml,src/lib.rs,src/model.rs,src/renew.rs}`; modify workspace `Cargo.toml`.

**Interfaces:**
- Produces: `model::{CertBundle{chain_pem:String,key_pem:Redacted,not_after:u64,issued_at:u64,serial:String,names:Vec<String>}}`, `model::validate_names(&[String]) -> Result<(), NameError>`, `renew::due(issued_at:u64, not_after:u64, now:u64, jitter_frac:f64) -> bool`, `renew::backoff(failures:u32) -> Duration`.

- [ ] Write failing tests: `due` is false at 50 % life left, true at 33 % and below, jitter shifts the edge by at most 5 %; `backoff(0)=5min`, doubles, caps at 6 h; `validate_names` rejects uppercase, empty, `*` not leading, `a.*.b`, duplicates, and accepts `*.example.com`.
- [ ] Run `cargo test -p wayhouse-acme`; expect FAIL.
- [ ] Implement; `Redacted` prints `<redacted>` and zeroizes on drop.
- [ ] Run tests; expect PASS. Commit.

### Task 2: Fake CA (test-support) and `AcmeCa` client wrapper

**Files:** Create `crates/wayhouse-acme/src/{ca.rs,solver.rs,testing/mod.rs}`.

**Interfaces:**
- Produces: `solver::DnsSolver` (async trait: `present(&self,fqdn:&str,value:&str)->Result<(),SolverError>`, `cleanup(...)`, `wait_visible(&self,fqdn:&str,value:&str,timeout:Duration)->Result<(),SolverError>`); `ca::AcmeCa::new(directory_url:&str, accept_tos:bool, account:Option<AccountCreds>)`, `ensure_account() -> AccountCreds` (serde, key redacted), `issue(names:&[String], solver:&dyn DnsSolver, now:u64) -> Result<CertBundle, IssueError>`; `testing::FakeCa::start() -> FakeCa{url, records: Arc<Records>}`, with scripts `fail_next(kind)`, `delay_validation(d)`.

- [ ] Write failing tests with the fake CA: account create then reuse; issue single name; issue wildcard + apex (two TXT values at the same fqdn); `FakeCa::fail_next(Dns)` yields `IssueError::Validation` and `cleanup` still called; TOS not accepted refuses; fake CA really rejects when the TXT is absent.
- [ ] Run; expect FAIL.
- [ ] Implement `AcmeCa` over `instant-acme`; fake CA is an in-process axum server (directory, nonce, account with JWS ES256 verification, order, authz, challenge validating via `Records`, finalize signing the CSR with a test CA via rcgen, cert download).
- [ ] Run; expect PASS. Commit.

### Task 3: Generalize sealing scope and key flag alias

**Files:** Modify `crates/wayhouse-controller/src/plugins/secrets.rs`, `main.rs`.

**Interfaces:** Produces `plugins::secrets::{seal_acme(&Keyring, scope_slot:&str, &[u8]) -> Sealed, open_acme(...)}` thin wrappers fixing `install = "acme"`; `--secret-key-file` as alias of `--plugin-secret-key-file` (same env var).

- [ ] Failing tests: a ciphertext sealed for `cert:a@1` fails to open as `cert:b@1` and as `account`; alias flag parses.
- [ ] Implement wrappers and the clap alias. Run tests. Commit.

### Task 4: Replicated entries, state machine and snapshot

**Files:** Create `crates/wayhouse-controller/src/acme/{mod.rs,store.rs,types.rs}`; modify `ha/mod.rs` (new `WriteRequest` variants `AcmeSiteSet/AcmeSiteDelete/AcmeAccountSet/AcmeCertIssued{term}/AcmeAttempt`, `WriteResponse::AcmeApplied/AcmeRejected`), `ha/state_machine.rs` (apply + snapshot), standalone `Store` trees.

**Interfaces:** Produces `acme::AcmeStore` (sled trees `acme_sites`, `acme_account`, `acme_certs`, `acme_attempts`) with `put_site/delete_site/sites()`, `set_account_if_none`, `put_cert(site,version,bundle_sealed)` (keeps two newest), `cert(site)`, `attempt(site)/set_attempt`; `Site{name,names,gateways,dns_plugin,propagation_timeout,enabled}`.

- [ ] Failing tests: standalone store round trip; overlapping names across sites refused (`Rejected::NameTaken`); `AcmeAccountSet` first wins; `AcmeCertIssued` with wrong term rejected; replay idempotent per Raft index; snapshot restore carries sites, account, certs, attempts; raw-bytes scan finds no sentinel key in log, snapshot, sled dir.
- [ ] Implement; follow the plugin entries' patterns in `state_machine.rs`. Run `cargo test -p wayhouse-controller`. Commit.

### Task 5: Issuance loop (leader only)

**Files:** Create `crates/wayhouse-controller/src/acme/runner.rs`.

**Interfaces:** Consumes `AcmeCa`, `DnsSolver`, `AcmeStore`, `KeyringHandle`, leader/term source used by the plugin runner. Produces `acme::runner::spawn(AcmeRuntime) -> JoinHandle`, `AcmeRuntime{store, ca, solver, keyring, clock}`; metrics `wayhouse_acme_cert_expiry_seconds{site}`, `wayhouse_acme_issue_total{result}`, `wayhouse_acme_held`.

- [ ] Failing tests (fake CA, test solver backed by `Records`, injectable clock): issues missing cert; does not renew at 50 %; renews at 33 %; failure advances backoff and replicates attempt; 5 failures/hour cap; non-leader does nothing; missing keyring holds and sets `held`; leader change mid-order yields exactly one `AcmeCertIssued` for the version; restart with expired cert retries immediately.
- [ ] Implement. Run; expect PASS. Commit.

### Task 6: Admin API and flags

**Files:** Create `crates/wayhouse-controller/src/acme/api.rs`; modify `main.rs`, `api.rs` router.

**Interfaces:** Routes (admin bearer): `PUT/GET/DELETE /acme/sites/{name}`, `GET /acme/sites` (status: `not_after`, `version`, `last_error`, `held`), `GET /acme/account` (account URL only, for CAA `accounturi`). Flags: `--acme`, `--acme-ca`, `--acme-accept-tos`, `--acme-state-dir` not needed. Startup refuses `--acme` without keyring, without `--acme-ca`, on plain remote listener.

- [ ] Failing API tests: wildcard without gateways 422; unknown `dns_plugin` 400 (slice 1 allows the built-in test solver id `test`); responses never contain key material (scan); routes 404 without `--acme`; follower forwards writes.
- [ ] Implement; run `make check`. Commit; docs: `docs/acme.md` (operator view), HANDOVER entry, AGENTS.md "when you touch X" row if one applies; push, PR, `Closes` the slice issue.

## Slice 2: `on_acme_dns` hook

### Task 7: Plugin host trigger

**Files:** Modify `crates/wayhouse-plugin-host/src/{caps.rs,runtime.rs}`, `crates/wayhouse-plugin-abi/src/lib.rs` (helpers only, no version bump unless decided), `docs/plugins.md`.

**Interfaces:** Produces capability `triggers.on_acme_dns: bool` (requires `on_timer`), export `on_acme_dns(ptr:i32,len:i32)`, import `acme_dns_result(ok:i32, msg_ptr:i32, msg_len:i32)`, `Runtime::call_acme_dns(req:&AcmeDnsRequest) -> Result<AcmeDnsOutcome, CallError>`.

- [ ] Failing tests: caps validation; a wat guest answering ok; trap and silent guest map to failure; message scrubbed of expanded secrets; undeclared trigger denied.
- [ ] Implement. Commit.

### Task 8: Controller `PluginDnsSolver`

**Files:** Create `crates/wayhouse-controller/src/acme/dns_plugin.rs`; modify `plugins/runner.rs` (new `Hook::AcmeDns`).

**Interfaces:** `PluginDnsSolver{runner, resolver}` implements `DnsSolver`; `wait_visible` polls hickory-resolver every 5 s up to `propagation_timeout`.

- [ ] Failing tests: fake plugin sets the record in `Records`, order succeeds; trapping plugin fails the order and `cleanup` is still invoked; not-leader refuses; bounded queue full fails fast.
- [ ] Implement; extend `selftest` plugin in the plugins repo (separate PR there) or a wat fixture in-tree for CI. Commit, PR.

## Slice 3: distribution

### Task 9: Gateway credentials and bundle endpoint

**Files:** Create `crates/wayhouse-controller/src/acme/{gateways.rs,bundles.rs}`.

**Interfaces:** `POST /acme/gateways {name}` returns token once (hash stored, replicated entry `AcmeGatewaySet/Delete`); `GET /acme/bundles`, `GET /acme/bundles/{site}` with ETag = version, gateway-token auth only; audit line per fetch.

- [ ] Failing tests: token for A cannot read site B (404); revoked token 401; `If-None-Match` gives 304; follower serves; plaintext key only in the bundle response; log scan finds no key; refuses non-TLS non-loopback.
- [ ] Implement. Commit.

### Task 10: `wayhouse-certsync`

**Files:** Create `crates/wayhouse-certsync/`; add to image build lists (`deploy/`, Dockerfile, release workflow only where the `wayhouse` image copies binaries; no new image).

**Interfaces:** CLI `--controller URL[,URL] --token-file F --dir D [--interval 60s] [--reload-cmd C]`; writes `D/<site>/{fullchain.pem,privkey.pem,meta.json}` atomically (chain first, key 0600).

- [ ] Failing tests: writes layout, 0600 mode, rename order; unchanged version causes no rewrite; controller down keeps last good; multi-URL failover.
- [ ] Implement; docs chapter finished (`docs/acme.md`, `docs/12-deployment.md` pointer), #297 body revised, `make check`, PR.
