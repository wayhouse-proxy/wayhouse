# Web routing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans (inline; the project forbids subagents in implementation threads). Steps use checkbox syntax.

**Goal:** Add the `web:` config section and the routing it needs: sites whose upstreams are plain addresses or pools, `via: wayhouse` loopback listeners, and a proven `port`/`sni` route to a `web` pool with PROXY v2.

**Architecture:** A new `wayhouse-config/src/web.rs` holds the raw and resolved types and the validation/expansion. `validate()` calls it once, after pools and listeners are built, and appends the generated listeners to `Config::listeners`. The data path needs no change.

**Tech Stack:** Rust, serde, `sha2` (already a `wayhouse-config` dependency), tokio tests in `wayhouse-core/tests`.

**Spec:** `docs/superpowers/specs/2026-10-15-web-routing-design.md`

## Global Constraints

- Config schema bump: `CONFIG_SCHEMA_VERSION` 2 to 3; `("web", 3)` in `version::FIELD_SINCE`.
- Schema changes update four places: types + `validate()`, `config.example.yaml`, `docs/05-configuration.md`, tests (AGENTS.md).
- `wayhouse-config` takes no new dependencies. No `unsafe`. No metric strings inline.
- Public text says "Wueschli", never the owner's first name.
- `make check` before every commit; `cargo fmt --all` as its own step first.

## Review Focus

- Two sites claim the same host (including `*.x` against `*.x`): rejected, naming both sites.
- A pool used `via: wayhouse` is also used by a game listener: the generated listener must not alter it.
- Removing a pool must not move other generated ports (stability test).
- All 1000 range ports taken: clear error, not a hang or panic.
- An operator listener already bound to a candidate loopback port: skipped, not a rebind failure at runtime.

---

### Task 1: Config types, validation, expansion

**Files:**
- Create: `crates/wayhouse-config/src/web.rs` (types, `validate_web`, port allocation, tests in `web/tests.rs`)
- Modify: `crates/wayhouse-config/src/schema.rs` (`RawConfig.web: Option<RawWeb>`), `lib.rs` (mod + re-exports), `validate.rs` (one call before `Ok(Config{..})`), `resolved.rs` (`Config.web`, warnings), `version.rs` (schema 3)

**Interfaces:**
- Produces: `pub struct WebConfig { pool: String, listen_http: SocketAddr, listen_https: SocketAddr, sites: Vec<WebSite> }`, `pub struct WebSite { hosts: Vec<String>, upstreams: Vec<WebUpstream> }`, `pub enum WebUpstream { Addr(SocketAddr), Pool { name: String }, Local { pool: String, addr: SocketAddr } }`, `pub(crate) fn validate_web(raw: RawWeb, pools: &[PoolConfig], listeners: &mut Vec<ListenerConfig>) -> Result<WebConfig, ConfigError>`.

- [ ] **Step 1: Failing tests** in `web/tests.rs`, via `parse_str`: defaults parse; each validation rule in the spec returns `ConfigError::Invalid` containing a stable phrase (`must set proxy_protocol v2`, `host … in two sites`, `exactly one of addr / pool`, `must be loopback`, `outside internal_ports`, `no free port`, `listener name … already used`); `via: wayhouse` adds listener `web-up-<pool>` (tcp, bound inside the range, one `Always` route to the pool); two sites sharing a pool add one listener; pinned port honoured; removing an unrelated pool leaves the port unchanged; an operator listener bound on the hashed port pushes the generated one to the next; `web:` under `schema_version: 2` is refused.
- [ ] **Step 2:** `cargo test -p wayhouse-config web` fails to compile.
- [ ] **Step 3:** Implement the types and `validate_web`. Hash port: first 8 bytes of SHA-256 of the pool name as big-endian `u64`, modulo range size, then advance (wrapping) to the next free port. Pools processed in sorted order after pins are reserved. Hosts reuse `HostPattern` parsing from `matcher.rs`.
- [ ] **Step 4:** `cargo test -p wayhouse-config` passes.
- [ ] **Step 5:** Warning in `Config::warnings`: no TCP listener routes to `web.pool`. Test it.
- [ ] **Step 6:** Commit `feat(config): web section with pool and loopback upstreams`.

### Task 2: Data-path end-to-end tests

**Files:**
- Create: `crates/wayhouse-core/tests/web_routing.rs`

**Interfaces:** Consumes `parse_str`, `Snapshot::from_config`, `Runtime::start` as in `tcp_forward.rs`.

- [ ] **Step 1:** `entry_route_sends_a_proxy_v2_header_to_the_web_pool`: listener routes `port` (a free port) to pool `web` with `proxy_protocol: v2`; a stand-in "Caddy" backend reads the 12-byte signature, checks the v2 command/family bytes and that the source address equals the client's local address, then echoes. Assert for both a `port` route and an `sni` route (ClientHello bytes from the existing helper in `tcp_forward.rs`, copied if not shared).
- [ ] **Step 2:** `via_wayhouse_reaches_the_origin_through_the_pool`: config with `web.sites[0].upstreams: [{pool: origin, via: wayhouse}]`; read the generated address from `cfg.web`; connect to it as Caddy would; the origin echoes. Assert the origin saw no PROXY header.
- [ ] **Step 3:** `via_wayhouse_fails_over_between_backends`: pool with two backends, first stopped; the next connections still reach the second (health check `interval_sec: 1`, wait_for helper).
- [ ] **Step 4:** Run `cargo test -p wayhouse-core --test web_routing`; passes. Commit `test(core): web entry route and via-wayhouse upstream end to end`.

### Task 3: Docs and bookkeeping

**Files:** `config.example.yaml`, `docs/05-configuration.md`, `docs/03-routing.md`, `HANDOVER.md`, `docs/08-roadmap.md`, `docs/superpowers/README.md` index if present.

- [ ] **Step 1:** Commented `web:` example in `config.example.yaml` and a "web" section in `docs/05` (fields, defaults, errors, schema 3).
- [ ] **Step 2:** A short "Web sites" subsection in `docs/03-routing.md`: entry route to `web`, the two upstream modes, the client-IP note.
- [ ] **Step 3:** HANDOVER entry; roadmap line. `make docs-fmt-check docs-links` clean.
- [ ] **Step 4:** `cargo fmt --all`, `make check` green. Commit, push, open the PR (`Closes` the slice issues).
