# Web sync and Caddy image Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans. Steps use checkbox syntax.

**Goal:** Render the `web:` config into Caddy's admin API with `wayhouse-web-sync` (#356) and ship a pinned `wayhouse-caddy` image with recipes and a real-Caddy CI test (#357).

**Architecture:** `wayhouse` serves the resolved section at `GET /web`; a separate `wayhouse-web-sync` process renders it (pure function) and loads it into Caddy over a unix socket. The sync ships as a second binary in the `wayhouse` image; Caddy is a separate pinned image.

**Tech Stack:** Rust (tokio, reqwest, axum metrics), Caddy 2.11.7, Docker, GitHub Actions.

**Spec:** `docs/web-proxy.md`, `docs/superpowers/specs/2026-10-15-web-routing-design.md`, spike findings on #355 (issue comments on #356, #357).

## Global Constraints

- Never send Caddy an empty or partial document (`POST /load` with `{}` removes every listener).
- `admin.config.persist: false`; admin API on a unix socket or loopback only.
- `stream_close_delay` on every `reverse_proxy`; `protocols: [h1, h2]`; `proxy_protocol` with `fallback_policy: REJECT` and an explicit `allow`.
- Certificates inline PEM (Caddy does not watch files); `via: direct` re-reads `GET /pools`.
- No releases, tags or release dispatch; the new image only joins the existing tag-driven release workflow.
- Out of scope: `wayhouse-certsync` (#363, needs the ACME work), `via: wayhouse` listeners (#368), Caddy ACME.

## Review Focus

- A failed read, a missing certificate or an unknown pool must leave Caddy untouched (tests in `sync/tests.rs`).
- A drained backend must leave Caddy within one interval (`a_drain_reaches_caddy_through_the_next_pools_read`).
- A renewed certificate must change the document (`a_changed_certificate_changes_the_document`).
- A spoofed PROXY header or forwarding header must not set the client address (`tests/web.rs`).
- Caddy restarting and losing its configuration must be repaired (`resend`).

### Task 1: Config and admin route
- [x] Tests in `wayhouse-config/src/web/tests.rs`: `tls`, `upstream_tls`, `health_path`, `WebView`.
- [x] `web.rs`, `web/view.rs`, `Cidr: Display`; `Snapshot.web`; `GET /web` in `admin.rs` with a test; protocol minor 1.2 and the `docs/upgrading.md` row.

### Task 2: The sync crate
- [x] `render.rs` golden tests, `caddy.rs` fake-server tests, `sync.rs` cycle tests, `pools.rs`, `certs.rs`, `admin_addr.rs`; `main.rs` with metrics.

### Task 3: Image, recipes, CI
- [x] `deploy/Dockerfile` (`wayhouse-web-sync` in the `wayhouse` image, `wayhouse-caddy` target), `check-caddy-image.sh`, compose and k8s recipes, lint checks.
- [x] `tests/web.rs` real-Caddy e2e; CI job `web-e2e` in `ci-ok.needs`; `release.yml` publishes the seventh image.
- [x] Docs: `web-proxy.md`, 05, 06, 12, README, AGENTS, HANDOVER.
