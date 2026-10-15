# Web config slice Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans (inline; the project forbids subagents in implementation threads). Steps use checkbox syntax.

**Goal:** Add the optional `web:` config section (schema 3) with validation, resolved as `Config.web`, plus the startup/`--check` warnings. Closes #360.

**Architecture:** New `crates/wayhouse-config/src/web.rs` holds the raw and resolved types and `validate_web`; `validate()` calls it once after pools and listeners are built. No generated listeners and no data-path change (that is #368).

**Tech Stack:** Rust, serde; `wayhouse-config` takes no new dependencies.

**Spec:** `docs/superpowers/specs/2026-10-15-web-routing-design.md`

## Global Constraints

- `CONFIG_SCHEMA_VERSION` 2 to 3; `("web", 3)` in `version::FIELD_SINCE`; the two tests that use `schema_version: 3` as "too new" move to 4.
- Schema changes update four places: types + `validate()`, `config.example.yaml`, `docs/05-configuration.md`, tests.
- No `unsafe`, no new dependencies. Public text says "Wueschli".

## Review Focus

- Host collision including `*.x` against `*.x` and case differences: rejected, naming both sites.
- `trusted_proxy` entry that is not an IP/CIDR: clear error.
- `listen` non-loopback with empty `trusted_proxy`: warning, not an error.
- `internal_ports` reversed, zero, or over 1000 ports: error.
- Pins: outside range, same pool pinned to two ports, two pools pinned to the same port.

### Task 1: types, validation, warnings

**Files:** Create `crates/wayhouse-config/src/web.rs`, `web/tests.rs`; modify `schema.rs`, `lib.rs`, `validate.rs`, `resolved.rs`, `version.rs`, `crates/wayhouse-controller/src/api.rs`, `crates/wayhouse/src/reload.rs` (schema-4 tests).

**Interfaces:** Produces `WebConfig { pool, listen_http, listen_https, trusted_proxy: Vec<Cidr>, internal_bind: IpAddr, internal_ports: (u16,u16), sites }`, `WebSite { hosts, upstreams }`, `WebUpstream::{Addr(SocketAddr), Pool { name, via: WebVia, port: Option<u16> }}`, `WebVia::{Direct, Wayhouse}`, `pub(crate) fn validate_web(raw: RawWeb, pools: &[PoolConfig]) -> Result<WebConfig, ConfigError>`, and `Config::warnings` entries.

- [ ] **Step 1:** Write failing tests in `web/tests.rs` via `parse_str`, one per acceptance rule with a stable error phrase; defaults parse; `web:` under schema 2 refused; warnings for no routed listener and for non-loopback listen without trusted_proxy.
- [ ] **Step 2:** `cargo test -p wayhouse-config web` fails.
- [ ] **Step 3:** Implement; reuse `parse_host_pattern` and `Cidr::parse`.
- [ ] **Step 4:** `cargo test -p wayhouse-config` and the two schema-4 tests pass.
- [ ] **Step 5:** Commit `feat(config): web section with sites, upstreams and validation`.

### Task 2: docs and bookkeeping

- [ ] **Step 1:** Commented `web:` example in `config.example.yaml` (incl. `trusted_proxy`, non-loopback `listen`); a "web" section in `docs/05-configuration.md` (fields, defaults, errors, schema 3).
- [ ] **Step 2:** `cargo fmt --all`, `make check` green, `make docs-fmt-check docs-links` if separate. Commit, push, open PR (`Closes #360`).
