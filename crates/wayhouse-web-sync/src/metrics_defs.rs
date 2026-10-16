//! Metric names (documented in `docs/06-operations-observability.md`).

/// Gauge: unix seconds of the last cycle that left Caddy on the intended
/// configuration (loaded, or already identical). Alert on `time() - this`.
pub const LAST_OK: &str = "wayhouse_web_sync_last_ok";
/// Counter: documents Caddy refused (HTTP 4xx from `POST /load`).
pub const REJECTS: &str = "wayhouse_web_sync_rejects_total";
/// Counter, label `stage` (`read`, `render`, `send`): cycles that failed before Caddy had
/// the new document. The previous configuration keeps serving.
pub const ERRORS: &str = "wayhouse_web_sync_errors_total";
/// Counter: documents Caddy accepted.
pub const LOADS: &str = "wayhouse_web_sync_loads_total";
/// Gauge: seconds the last render took.
pub const RENDER_SECONDS: &str = "wayhouse_web_sync_last_render_seconds";
