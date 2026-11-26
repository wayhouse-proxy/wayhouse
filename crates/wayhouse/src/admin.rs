//! The admin / observability HTTP API. Bound to an internal address only.

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::time::Duration;

use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::StatusCode,
    middleware,
    response::{IntoResponse, Response},
    routing::{delete, get, patch, post},
    Json, Router,
};
use metrics_exporter_prometheus::PrometheusHandle;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use wayhouse_config::features::{
    ADMIN_WRITE_BACKENDS, ADMIN_WRITE_ROUTE_HINT, ADMIN_WRITE_SNIFFER_UPLOAD,
};
use wayhouse_core::pool::AdminState as BackendState;
use wayhouse_core::sniff::Sniffers;
use wayhouse_core::RuntimeHandle;

use crate::admin_openapi::{ConfigSaved, ErrorBody, FeaturesReply, ValidateOk, WebReply};
use crate::config_file::ConfigFile;

/// The OpenAPI description of the admin API. Its paths are the handlers' own
/// `#[utoipa::path]` attributes, so the description lives next to the code it describes;
/// `docs/reference/openapi/proxy-admin.json` is a checked-in rendering of it (see the test
/// `the_checked_in_spec_is_current`).
#[derive(utoipa::OpenApi)]
#[openapi(
    info(
        title = "wayhouse proxy admin API",
        description = "The HTTP API every `wayhouse` proxy serves on `settings.admin.listen` \
        (default `127.0.0.1:9900`). It reads the live state, changes backends, drains the instance, \
        and reads or replaces the config file.",
        license(name = "MIT OR Apache-2.0"),
    ),
    paths(
        healthz, readyz, metrics, openapi_spec, pools, add_backend, patch_backend, delete_backend, config,
        config_schema, validate_config, get_config_file, put_config_file, web, features, sessions,
        drain, undrain, route_hint, list_sniffers, upload_sniffer, delete_sniffer, rollback_sniffer
    ),
    components(schemas(ErrorBody)),
    modifiers(&BearerAuth),
    tags(
        (name = "Health", description = "Liveness and readiness, for load balancers."),
        (name = "Observability", description = "Metrics."),
        (name = "Pools", description = "Read pools and change backends at runtime."),
        (name = "Configuration", description = "Read, validate and replace the running config."),
        (name = "Web proxy", description = "What `wayhouse-web-sync` reads."),
        (name = "Feature flags", description = "The proxy's feature flags and their effective values."),
        (name = "Sessions", description = "Live connections and UDP sessions."),
        (name = "Lifecycle", description = "Take the instance out of rotation and back."),
        (name = "Routing", description = "Push route hints."),
        (name = "Sniffers", description = "Manage WASM sniffer modules."),
    ),
)]
pub struct AdminApi;

/// Adds the bearer scheme every route except `/healthz` is behind (the token is
/// `settings.admin.auth_token`; with none set the API is open).
struct BearerAuth;

impl utoipa::Modify for BearerAuth {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        use utoipa::openapi::security::{
            HttpAuthScheme, HttpBuilder, SecurityRequirement, SecurityScheme,
        };
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "bearer",
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .description(Some(
                        "`Authorization: Bearer <token>`, the value of `settings.admin.auth_token`. \
                         With no token configured the API is open.",
                    ))
                    .build(),
            ),
        );
        openapi.security = Some(vec![SecurityRequirement::new(
            "bearer",
            Vec::<String>::new(),
        )]);
    }
}

/// The OpenAPI description as JSON.
pub(crate) fn openapi_json() -> serde_json::Value {
    use utoipa::OpenApi;
    serde_json::to_value(AdminApi::openapi()).expect("the spec serializes")
}

#[utoipa::path(
    get, path = "/openapi.json", tag = "Observability",
    summary = "This API's OpenAPI description",
    description = "The OpenAPI 3.1 document these pages are generated from, as served by the build you are talking to. \
Use it to generate a client or to check what a particular version supports.",
    responses((status = 200, description = "An OpenAPI 3.1 document.", content_type = "application/json", body = serde_json::Value))
)]
async fn openapi_spec() -> Json<serde_json::Value> {
    Json(openapi_json())
}

#[derive(Clone)]
struct AdminState {
    runtime: RuntimeHandle,
    prometheus: PrometheusHandle,
    /// Bearer token every request except `GET /healthz` must present;
    /// `None` leaves the API open (`settings.admin.auth_token`, phase 10+11
    /// slice 10 — see `docs/05` "Admin API auth").
    auth_token: Option<std::sync::Arc<wayhouse_http::rotating::RotatingSecret>>,
    /// `settings.sniffers.dir`, if configured — where `/admin/sniffers`
    /// writes/removes `.wasm` files. `None` when this instance has no
    /// `settings.sniffers` block at all (turning sniffing on from nothing is
    /// still startup-only; see `sniffers_disabled`).
    sniffers_dir: Option<PathBuf>,
    sniffers: std::sync::Arc<Sniffers>,
    /// The live `settings.sniffers.modules` pins (follows reloads), so an
    /// upload the next rescan would reject never reaches the disk.
    sniffer_pins: SnifferPins,
    /// Vets an uploaded module before it is written. `None` when this build or
    /// instance cannot load sniffers at all.
    sniffer_validator: Option<SnifferValidator>,
    /// The config file this proxy runs from (`None` under `--controller`), for `/config/file`.
    config_file: Option<ConfigFile>,
}

pub type SnifferPins =
    std::sync::Arc<dyn Fn() -> Vec<wayhouse_config::SnifferModulePin> + Send + Sync>;
pub type SnifferValidator = std::sync::Arc<dyn Fn(&[u8]) -> Result<(), String> + Send + Sync>;

/// The admin API route table. Split out from [`serve`] so integration tests can
/// mount it on their own ephemeral listener. `/healthz` alone stays outside
/// the [`wayhouse_http::server::require_bearer`] gate — plain liveness-probe convention, same
/// choice `wayhouse-controller`/`wayhouse-aggregator` made for their own `/healthz`.
fn router(state: AdminState) -> Router {
    let gated = Router::new()
        .route("/readyz", get(readyz))
        .route("/metrics", get(metrics))
        .route("/openapi.json", get(openapi_spec))
        .route("/pools", get(pools))
        .route(
            "/pools/{pool}/backends",
            post(add_backend).layer(middleware::from_fn_with_state(
                ADMIN_WRITE_BACKENDS,
                write_gate,
            )),
        )
        .route(
            "/pools/{pool}/backends/{addr}",
            patch(patch_backend)
                .delete(delete_backend)
                .layer(middleware::from_fn_with_state(
                    ADMIN_WRITE_BACKENDS,
                    write_gate,
                )),
        )
        .route("/config", get(config))
        .route("/config/schema", get(config_schema))
        .route("/config/validate", post(validate_config))
        .route(
            "/config/file",
            get(get_config_file)
                .put(put_config_file)
                .layer(DefaultBodyLimit::max(1 << 20)),
        )
        .route("/web", get(web))
        .route("/features", get(features))
        .route("/sessions", get(sessions))
        .route("/admin/drain", post(drain))
        .route("/admin/undrain", post(undrain))
        .route(
            "/route-hint",
            post(route_hint).layer(middleware::from_fn_with_state(
                ADMIN_WRITE_ROUTE_HINT,
                write_gate,
            )),
        )
        .route(
            "/admin/sniffers",
            get(list_sniffers).merge(
                post(upload_sniffer)
                    .layer(DefaultBodyLimit::max(
                        crate::sniffer_loader::MAX_MODULE_BYTES,
                    ))
                    .layer(middleware::from_fn_with_state(
                        ADMIN_WRITE_SNIFFER_UPLOAD,
                        write_gate,
                    )),
            ),
        )
        .route(
            "/admin/sniffers/{name}",
            delete(delete_sniffer).layer(middleware::from_fn_with_state(
                ADMIN_WRITE_SNIFFER_UPLOAD,
                write_gate,
            )),
        )
        .route(
            "/admin/sniffers/{name}/rollback",
            post(rollback_sniffer).layer(middleware::from_fn_with_state(
                ADMIN_WRITE_SNIFFER_UPLOAD,
                write_gate,
            )),
        );

    // The aggregator's fan-out (and the controller's tooling) call these with the
    // protocol header; operators also call them by hand, so a missing header is
    // fine but another major is refused. The bearer check is the outer layer: the
    // gate counts mismatches into the fleet view, so an unauthenticated request
    // must not reach it.
    let gated = wayhouse_http::protocol::gate_lenient(gated, "proxy").route_layer(
        middleware::from_fn_with_state(
            state
                .auth_token
                .clone()
                .map_or_else(
                    || wayhouse_http::server::BearerAuth::new(None),
                    wayhouse_http::server::BearerAuth::rotating,
                )
                .with_body("unauthorized\n"),
            wayhouse_http::server::require_bearer,
        ),
    );
    Router::new()
        .route("/healthz", get(healthz))
        .merge(gated)
        .with_state(state)
}

/// A write route behind one of the `admin_write.*` kill switches: while the flag is off the
/// answer is `403`, before the handler or the body is read. Only the routes that change a pool,
/// a sniffer module or a route hint carry it; reads, drain and the flag endpoints never do, so
/// switching these off cannot lock an operator out of the way back in.
async fn write_gate(
    State(flag): State<usize>,
    req: axum::extract::Request,
    next: middleware::Next,
) -> Response {
    if wayhouse_core::flags::on(flag) {
        return next.run(req).await;
    }
    (
        StatusCode::FORBIDDEN,
        "this write is switched off by a feature flag (admin_write.*); GET /features shows which\n",
    )
        .into_response()
}

/// The admin API's TLS handshake limits: the defaults, with each
/// `settings.admin.tls` limit that is set on top.
pub fn handshake_limits(
    tls: Option<&wayhouse_config::AdminTls>,
) -> wayhouse_http::tls::HandshakeLimits {
    let d = wayhouse_http::tls::HandshakeLimits::default();
    let Some(t) = tls else { return d };
    wayhouse_http::tls::HandshakeLimits {
        max_pending: t.max_pending.unwrap_or(d.max_pending),
        max_pending_per_source: t.max_pending_per_source.unwrap_or(d.max_pending_per_source),
        new_per_source_per_sec: t.new_per_source_per_sec.unwrap_or(d.new_per_source_per_sec),
        new_per_source_burst: t.new_per_source_burst.unwrap_or(d.new_per_source_burst),
        ..d
    }
}

#[allow(clippy::too_many_arguments)] // flat wiring of independent `main.rs` inputs
pub async fn serve(
    addr: SocketAddr,
    tls: Option<(
        std::sync::Arc<wayhouse_http::tls::ReloadingCert>,
        wayhouse_http::tls::HandshakeLimits,
    )>,
    runtime: RuntimeHandle,
    prometheus: PrometheusHandle,
    auth_token: Option<std::sync::Arc<wayhouse_http::rotating::RotatingSecret>>,
    sniffers_dir: Option<PathBuf>,
    sniffers: std::sync::Arc<Sniffers>,
    sniffer_pins: SnifferPins,
    sniffer_validator: Option<SnifferValidator>,
    config_file: Option<ConfigFile>,
) {
    let app = router(AdminState {
        runtime,
        prometheus,
        auth_token,
        sniffers_dir,
        sniffers,
        sniffer_pins,
        sniffer_validator,
        config_file,
    });

    // HTTPS with `settings.admin.tls`, plain HTTP otherwise. A bind failure
    // lands here too and, as before, ends only this task (non-fatal).
    let (cert, limits) = match tls {
        Some((cert, limits)) => (Some(cert), limits),
        None => (None, wayhouse_http::tls::HandshakeLimits::default()),
    };
    if let Err(e) = wayhouse_http::tls::serve(addr, app, cert, limits, "admin API").await {
        tracing::error!(%addr, error = %e, "admin API server error");
    }
}

#[utoipa::path(
    get, path = "/healthz", tag = "Health",
    summary = "Liveness probe",
    description = "Answers `ok` while the process runs. It is the only route that needs no bearer \
token, so a load balancer or a container runtime can probe it.",
    security(()),
    responses((status = 200, description = "The process is alive.", body = String, content_type = "text/plain", example = "ok"))
)]
async fn healthz() -> &'static str {
    "ok"
}

#[utoipa::path(
    get, path = "/readyz", tag = "Health",
    summary = "Readiness probe",
    description = "`200` when at least one listener is configured and the instance is not draining; \
`503` otherwise. Point a load balancer's health check here: `POST /admin/drain` makes it fail on purpose.",
    responses(
        (status = 200, description = "Ready to take new connections.", body = String, content_type = "text/plain", example = "ready"),
        (status = 503, description = "Draining (`draining`) or no listener configured (`not ready`).", body = String, content_type = "text/plain", example = "draining"),
    )
)]
async fn readyz(State(s): State<AdminState>) -> impl IntoResponse {
    if s.runtime.ready() {
        (StatusCode::OK, "ready")
    } else if s.runtime.is_draining() {
        (StatusCode::SERVICE_UNAVAILABLE, "draining")
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, "not ready")
    }
}

/// `POST /admin/drain` — take this instance out of LB rotation (`readyz` starts
/// failing) without stopping the data path. In-flight sessions keep running;
/// operators wait for `active_conns` to fall, then send `SIGTERM`.
#[utoipa::path(
    post, path = "/admin/drain", tag = "Lifecycle",
    summary = "Take the instance out of rotation",
    description = "Marks the instance draining: `/readyz` starts failing so a load balancer stops sending new \
clients, while the data path keeps serving. Sessions in flight are never closed by this. Wait for `active_conns` \
to fall, then stop the process (`SIGTERM`). Undo it with `POST /admin/undrain`.",
    responses(
        (status = 200, description = "Draining.", body = String, content_type = "text/plain", example = "draining; active_conns=42\n"),
    )
)]
async fn drain(State(s): State<AdminState>) -> (StatusCode, String) {
    s.runtime.set_draining(true);
    let n = s.runtime.active_conns();
    tracing::info!(active_conns = n, "instance marked draining via admin API");
    (StatusCode::OK, format!("draining; active_conns={n}\n"))
}

/// `POST /admin/undrain` — put the instance back into rotation.
#[utoipa::path(
    post, path = "/admin/undrain", tag = "Lifecycle",
    summary = "Put the instance back into rotation",
    description = "Clears the draining mark; `/readyz` passes again if a listener is configured.",
    responses(
        (status = 200, description = "Back in rotation.", body = String, content_type = "text/plain", example = "ready\n"),
    )
)]
async fn undrain(State(s): State<AdminState>) -> (StatusCode, &'static str) {
    s.runtime.set_draining(false);
    tracing::info!("instance returned to rotation via admin API");
    (StatusCode::OK, "ready\n")
}

/// `GET /web` — the resolved `web:` section as JSON, for `wayhouse-web-sync`: where
/// Caddy listens, and per site its hosts, upstreams in final form (`addr`; `pool` with
/// `via: direct`; `pool` plus the generated loopback `local` address with
/// `via: wayhouse`) and TLS settings. `404` when the config has no `web:` section.
#[utoipa::path(
    get, path = "/web", tag = "Web proxy",
    summary = "The resolved web: section",
    description = "What `wayhouse-web-sync` reads to build Caddy's configuration: where Caddy listens and, per site, \
its hosts, upstreams in final form and TLS settings. `404` when the config has no `web:` section.",
    responses(
        (status = 200, description = "The resolved section.", body = WebReply),
        (status = 404, description = "No `web:` section is configured.", body = String, content_type = "text/plain"),
    )
)]
async fn web(State(s): State<AdminState>) -> Response {
    match &s.runtime.snapshot().web {
        Some(w) => axum::Json(w.view()).into_response(),
        None => (StatusCode::NOT_FOUND, "no web section configured\n").into_response(),
    }
}

/// `GET /features`: every proxy feature flag with its effective value and where it comes from.
#[utoipa::path(
    get, path = "/features", tag = "Feature flags",
    summary = "List the feature flags",
    description = "Every proxy feature flag with its effective value and where the value comes from. \
The flags and what they do are in the Manual.",
    responses((status = 200, description = "All flags of the proxy scope.", body = FeaturesReply))
)]
async fn features(State(s): State<AdminState>) -> Response {
    let rows = crate::features::effective(&s.runtime.snapshot().features);
    axum::Json(serde_json::json!({ "flags": rows })).into_response()
}

/// `GET /config/schema`: the JSON Schema of a config document, as the controller serves it, so a
/// UI on a file-config proxy (no controller) can build its forms from the proxy it talks to.
#[utoipa::path(
    get, path = "/config/schema", tag = "Configuration",
    summary = "JSON Schema of a config document",
    description = "The JSON Schema the config is validated against, the same one the controller serves. The header \
`X-Config-Schema-Version` carries the highest `schema_version` this build accepts.",
    responses(
        (status = 200, description = "A JSON Schema document.", content_type = "application/json",
            headers(("X-Config-Schema-Version" = String, description = "Highest accepted `schema_version`.")),
            body = serde_json::Value),
    )
)]
async fn config_schema() -> Response {
    (
        [
            ("Content-Type", "application/json".to_string()),
            (
                "X-Config-Schema-Version",
                wayhouse_config::version::CONFIG_SCHEMA_VERSION.to_string(),
            ),
        ],
        wayhouse_config::CONFIG_JSON_SCHEMA,
    )
        .into_response()
}

/// `POST /config/validate`: a raw YAML config document is parsed and validated and nothing is
/// applied. `200 {"ok":true,"schema_version":N}` or `422 {"error":...}`, the controller's shapes.
#[utoipa::path(
    post, path = "/config/validate", tag = "Configuration",
    summary = "Validate a config document",
    description = "Parses and validates the YAML in the body with the same code as a reload, and applies nothing. \
Nothing is read from the running config: a pool a listener names has to be in the document itself.",
    request_body(content = String, content_type = "text/yaml", description = "A complete config document."),
    responses(
        (status = 200, description = "The document is valid.", body = ValidateOk),
        (status = 422, description = "The document does not parse or fails validation; `error` names the key and the reason.", body = ErrorBody,
            example = json!({"error": "invalid configuration: listener game: route 0: unknown pool survival"})),
    )
)]
async fn validate_config(body: String) -> Response {
    match wayhouse_config::parse_str(&body) {
        Ok(cfg) => {
            axum::Json(serde_json::json!({"ok": true, "schema_version": cfg.schema_version}))
                .into_response()
        }
        Err(e) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            axum::Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// The answer for `/config/file` on a proxy with no config file (`--controller` has none to show).
fn no_config_file() -> Response {
    (
        StatusCode::NOT_FOUND,
        axum::Json(serde_json::json!({"error": "this proxy does not run from a config file"})),
    )
        .into_response()
}

/// `GET /config/file`: the config file, with its etag (`?previous=1`: the copy a write
/// kept of the file before it). Always on. Inline secrets are masked in the body.
#[utoipa::path(
    get, path = "/config/file", tag = "Configuration",
    summary = "Read the config file",
    description = "The config file the proxy runs from, with its ETag. Inline secrets are masked in the body (a value such as `__wayhouse_redacted_admin_token__` stands in for the secret), \
and saving the shown text keeps them. A proxy started with `--controller` has no file and answers `404`.",
    params(("previous" = Option<String>, Query, description = "`1` (or `true`) returns the copy the last write kept of the file before it.")),
    responses(
        (status = 200, description = "The YAML text.", body = String, content_type = "text/yaml",
            headers(
                ("ETag" = String, description = "Send it as `If-Match` when you save an edit of this text."),
                ("X-Config-Revision" = u64, description = "The file's revision counter; the alternative to the ETag, as `?base=` on a save."),
                ("X-Config-Schema-Version" = String, description = "Highest accepted `schema_version`."),
            )),
        (status = 404, description = "This proxy has no config file, or `previous=1` and no earlier version is kept yet.", body = ErrorBody),
        (status = 422, description = "The file does not parse, so its secrets cannot be masked; fix it on the host.", body = ErrorBody),
        (status = 500, description = "The file cannot be read.", body = ErrorBody),
    )
)]
async fn get_config_file(
    State(s): State<AdminState>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Response {
    let Some(file) = s.config_file.as_ref() else {
        return no_config_file();
    };
    let previous = q.get("previous").is_some_and(|v| v == "1" || v == "true");
    match file.read(previous) {
        // The tags describe the file as it is; the body hides inline secrets (a viewer may read
        // this), and a save of the shown text keeps them.
        Ok(bytes) if crate::config_file::redact(&String::from_utf8_lossy(&bytes)).is_none() => (
            StatusCode::UNPROCESSABLE_ENTITY,
            axum::Json(serde_json::json!({"error": "the config file does not parse, so its secrets cannot be hidden; fix it on the host"})),
        )
            .into_response(),
        Ok(bytes) => (
            [
                ("Content-Type", "text/yaml; charset=utf-8".to_string()),
                ("ETag", crate::config_file::etag(&bytes)),
                (
                    "X-Config-Revision",
                    crate::config_file::revision(&bytes).to_string(),
                ),
                (
                    "X-Config-Schema-Version",
                    wayhouse_config::version::CONFIG_SCHEMA_VERSION.to_string(),
                ),
            ],
            crate::config_file::redact(&String::from_utf8_lossy(&bytes)).unwrap_or_default(),
        )
            .into_response(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && previous => (
            StatusCode::NOT_FOUND,
            axum::Json(serde_json::json!({"error": "no earlier version is kept yet"})),
        )
            .into_response(),
        Err(e) => {
            tracing::warn!(error = %e, "cannot read the config file for /config/file");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                axum::Json(serde_json::json!({"error": "cannot read the config file"})),
            )
                .into_response()
        }
    }
}

/// How long a save waits to see the reload take the new file.
#[cfg(not(test))]
const APPLY_WAIT: Duration = Duration::from_secs(5);
#[cfg(test)]
const APPLY_WAIT: Duration = Duration::from_millis(300);

/// `PUT /config/file`: replace the config file (raw YAML body) if it is still the version named by
/// `If-Match`; the proxy then reloads it like any edit. Off unless `--allow-config-write`.
#[utoipa::path(
    put, path = "/config/file", tag = "Configuration",
    summary = "Replace the config file",
    description = "Replaces the file with the YAML body if it is still the version you read, then triggers a reload. \
Off unless the proxy was started with `--allow-config-write`. Name the version you edited with `If-Match` (the ETag \
of the read) or `?base=` (its `X-Config-Revision`). The old file is kept and can be read with `GET /config/file?previous=1`. \
The body is limited to 1 MiB.",
    params(("base" = Option<u64>, Query, description = "The `X-Config-Revision` of the version you edited, instead of `If-Match`.")),
    request_body(content = String, content_type = "text/yaml", description = "The complete new config file."),
    responses(
        (status = 200, description = "Saved. `applied` says whether the reload took it within five seconds.", body = ConfigSaved),
        (status = 403, description = "Config writes are switched off: start the proxy with `--allow-config-write`.", body = ErrorBody),
        (status = 404, description = "This proxy has no config file.", body = ErrorBody),
        (status = 409, description = "The file changed since you read it (`code: stale`). Read it again and redo the edit.", body = ErrorBody),
        (status = 422, description = "The new text does not validate; the file is unchanged.", body = ErrorBody),
        (status = 428, description = "Neither `If-Match` nor `?base=` was sent.", body = ErrorBody),
        (status = 503, description = "The file or its directory is not writable.", body = ErrorBody),
    )
)]
async fn put_config_file(
    State(s): State<AdminState>,
    Query(q): Query<std::collections::HashMap<String, String>>,
    headers: axum::http::HeaderMap,
    body: String,
) -> Response {
    let Some(file) = s.config_file.as_ref() else {
        return no_config_file();
    };
    let json_error = |status: StatusCode, error: &str, code: Option<&str>| {
        let mut body = serde_json::json!({ "error": error });
        if let Some(code) = code {
            body["code"] = code.into();
        }
        (status, axum::Json(body)).into_response()
    };
    if !file.allow_write {
        return json_error(
            StatusCode::FORBIDDEN,
            "config writes are switched off: start the proxy with --allow-config-write",
            None,
        );
    }
    // The version being edited: `If-Match` with the ETag of a read, or `?base=` with its
    // `X-Config-Revision` (what the web editor keeps).
    let base = match (
        headers.get("if-match").and_then(|v| v.to_str().ok()),
        q.get("base"),
    ) {
        (Some(tag), _) => Some(tag.to_string()),
        (None, Some(n)) if n.parse::<u64>().is_ok() => None,
        _ => return json_error(
            StatusCode::PRECONDITION_REQUIRED,
            "send If-Match with the ETag of the version you edited, or ?base= with its revision",
            None,
        ),
    };
    let file = file.clone();
    let revision = q.get("base").and_then(|n| n.parse::<u64>().ok());
    let before = s.runtime.current();
    let written = tokio::task::spawn_blocking(move || {
        let base = match (&base, revision) {
            (Some(tag), _) => crate::config_file::Base::Etag(tag),
            (None, Some(n)) => crate::config_file::Base::Revision(n),
            (None, None) => return Err(crate::config_file::WriteError::Stale),
        };
        file.write(base, &body)
    })
    .await
    .unwrap_or_else(|e| Err(crate::config_file::WriteError::Io(e.to_string())));
    match written {
        Ok((etag, revision)) => {
            s.runtime.request_reload();
            // The reload swaps the live snapshot when it accepts the file and keeps the old one
            // when it does not (a bad TLS path, a port in use), so a new snapshot is the proof.
            let applied = tokio::time::timeout(APPLY_WAIT, async {
                while std::sync::Arc::ptr_eq(&before, &s.runtime.current()) {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            })
            .await
            .is_ok();
            let mut answer = serde_json::json!({
                "ok": true, "saved": true, "applied": applied, "etag": etag, "revision": revision,
            });
            if !applied {
                answer["note"] = "saved, but not confirmed applied: the proxy may still be reloading, or it refused the file and keeps the running config (see its log); GET /config/file?previous=1 returns the version before".into();
            }
            axum::Json(answer).into_response()
        }
        Err(crate::config_file::WriteError::Stale) => json_error(
            StatusCode::CONFLICT,
            "the config file changed since you read it",
            Some("stale"),
        ),
        Err(crate::config_file::WriteError::Invalid(e)) => {
            json_error(StatusCode::UNPROCESSABLE_ENTITY, &e, None)
        }
        Err(crate::config_file::WriteError::NotWritable) => json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "the config file or its directory is not writable (a read-only mount, or the proxy's user lacks permission)",
            None,
        ),
        Err(crate::config_file::WriteError::Io(e)) => {
            tracing::warn!(error = %e, "config file write failed");
            json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "the config file could not be written",
                None,
            )
        }
    }
}

/// `GET /config` — a plaintext view of the active snapshot (listeners + pools).
#[utoipa::path(
    get, path = "/config", tag = "Configuration",
    summary = "Summary of the running configuration",
    description = "A plain-text view of the live snapshot: the global limits, each listener with its bind address, \
protocol, route count and the options it has switched on, and each pool with its backends. It is a summary for \
people; use `GET /config/file` for the file itself.",
    responses(
        (status = 200, description = "Plain text.", body = String, content_type = "text/plain"),
    )
)]
async fn config(State(s): State<AdminState>) -> impl IntoResponse {
    fn opt(v: Option<impl std::fmt::Display>) -> String {
        v.map_or_else(|| "-".to_string(), |n| n.to_string())
    }
    let snap = s.runtime.snapshot();
    let mut out = String::new();
    let lim = &snap.limits;
    out.push_str(&format!(
        "draining={}\tactive_conns={}\tlimits=conn:{},udp:{},new_rate:{}\tgeo_db={}\n\nlisteners:\n",
        s.runtime.is_draining(),
        s.runtime.active_conns(),
        opt(lim.max_connections),
        opt(lim.max_udp_sessions),
        opt(lim.max_new_sessions_per_sec),
        snap.geo_db.as_deref().unwrap_or("-"),
    ));
    for l in &snap.listeners {
        let bind_display = if l.extra_binds.is_empty() {
            l.bind.to_string()
        } else {
            // Port-range bind (F1.4): one real socket per port, shown compactly
            // rather than listing every address.
            format!("{} (+{} ports)", l.bind, l.extra_binds.len())
        };
        out.push_str(&format!(
            "  {}\tbind={}\tproto={:?}\troutes={}{}{}{}{}{}{}{}{}{}{}\n",
            l.name,
            bind_display,
            l.protocol,
            l.routes.len(),
            if l.route_hint { "\troute_hint" } else { "" },
            if l.freebind { "\tfreebind" } else { "" },
            if l.transparent { "\ttransparent" } else { "" },
            if l.first_packet_gate {
                "\tfirst_packet_gate"
            } else {
                ""
            },
            if l.acl.is_empty() {
                String::new()
            } else {
                format!("\tacl=+{}/-{}", l.acl.allow.len(), l.acl.deny.len())
            },
            match &l.geo {
                Some(g) => format!("\tgeo=+{}/-{}", g.allow.len(), g.deny.len()),
                None => String::new(),
            },
            match &l.rate_limit {
                Some(rl) => {
                    let f = |b: Option<wayhouse_config::TokenBucket>| match b {
                        Some(b) => format!("{}/{}", b.rate, b.burst),
                        None => "-".to_string(),
                    };
                    format!("\trate_limit=ip:{},net:{}", f(rl.per_ip), f(rl.per_net))
                }
                None => String::new(),
            },
            match &l.per_source {
                Some(ps) => {
                    let g = |v: Option<usize>| v.map_or("-".to_string(), |n| n.to_string());
                    format!(
                        "\tper_source=ip:{},net:{}",
                        g(ps.max_per_ip),
                        g(ps.max_per_net)
                    )
                }
                None => String::new(),
            },
            match &l.prefix {
                Some(p) => format!("\tprefix={p:?}"),
                None => String::new(),
            },
            if l.sniffers.is_empty() {
                String::new()
            } else {
                format!("\tsniffers={}", l.sniffers.join(","))
            },
        ));
    }
    out.push_str("\npools:\n");
    for (name, pool) in &snap.pools {
        let (added, removed) = s.runtime.backend_overlay().pending(name);
        let ov = if added.is_empty() && removed.is_empty() {
            String::new()
        } else {
            format!("\toverlay=+{}/-{}", added.len(), removed.len())
        };
        out.push_str(&format!("  {name}\tbalancer={:?}{ov}\n", pool.balancer));
        for b in pool.backends() {
            out.push_str(&format!(
                "    {}\t{}\tstate={}\tactive={}\n",
                b.addr,
                if b.is_healthy() {
                    "healthy"
                } else {
                    "unhealthy"
                },
                b.admin_state().as_str(),
                b.active(),
            ));
        }
    }
    out
}

/// `GET /sessions[?listener=&pool=&proto=tcp|udp&src=<ip>]` — a plaintext list
/// of every live proxied connection / UDP session (id, transport, listener,
/// client / local address, chosen pool and backend, age). The query params
/// filter the list (exact match on `listener` / `pool`, transport on `proto`,
/// source IP on `src`). Point-in-time; a session already gone by the time you
/// read this is simply absent.
#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct SessionsQuery {
    /// Only sessions of this listener (exact name).
    #[param(example = "game")]
    listener: Option<String>,
    /// Only sessions routed to this pool (exact name).
    pool: Option<String>,
    /// Only this transport: `tcp` or `udp`.
    #[param(example = "udp")]
    proto: Option<String>,
    /// Only sessions from this client IP address.
    #[param(example = "203.0.113.9")]
    src: Option<String>,
}

#[utoipa::path(
    get, path = "/sessions", tag = "Sessions",
    summary = "List live sessions",
    description = "Every live proxied TCP connection and UDP session, oldest id first: id, transport, listener, \
client and local address, the pool and backend chosen, and the age. A point-in-time list; a session that has \
ended is absent. The filters match exactly.",
    params(SessionsQuery),
    responses(
        (status = 200, description = "Plain text. The first line is `sessions=<count>`; each session follows on its own line, tab separated.", body = String, content_type = "text/plain",
            example = "sessions=1\n  7\ttcp\tgame\tpeer=203.0.113.9:51234\tlocal=198.51.100.1:7777\tpool=game\tbackend=10.0.0.5:7777\tage=12.3s\n"),
    )
)]
async fn sessions(
    State(s): State<AdminState>,
    Query(q): Query<SessionsQuery>,
) -> impl IntoResponse {
    let src = q
        .src
        .as_deref()
        .and_then(|s| s.parse::<IpAddr>().ok())
        .map(|ip| ip.to_canonical());
    let mut list = s.runtime.sessions();
    list.retain(|e| {
        q.listener.as_deref().is_none_or(|l| l == e.listener)
            && q.pool
                .as_deref()
                .is_none_or(|p| Some(p) == e.pool.as_deref())
            && q.proto.as_deref().is_none_or(|p| p == e.proto.as_str())
            && src.is_none_or(|ip| ip == e.peer.ip())
    });
    list.sort_by_key(|e| e.id);
    let mut out = format!("sessions={}\n", list.len());
    for e in &list {
        out.push_str(&format!(
            "  {}\t{}\t{}\tpeer={}\tlocal={}\tpool={}\tbackend={}\tage={:.1}s\n",
            e.id,
            e.proto.as_str(),
            e.listener,
            e.peer,
            e.local,
            e.pool.as_deref().unwrap_or("-"),
            e.backend.map_or_else(|| "-".to_string(), |a| a.to_string()),
            e.age.as_secs_f64(),
        ));
    }
    out
}

#[utoipa::path(
    get, path = "/metrics", tag = "Observability",
    summary = "Prometheus metrics",
    description = "Every `wayhouse_*` series in the Prometheus text exposition format. The series are listed in the Reference.",
    responses(
        (status = 200, description = "Prometheus text format.", body = String, content_type = "text/plain; version=0.0.4"),
    )
)]
async fn metrics(State(s): State<AdminState>) -> impl IntoResponse {
    s.prometheus.render()
}

/// The body of `POST /route-hint`: a short-lived `src_ip` to pool mapping that listeners with
/// `route_hint: true` consult before their route list.
#[derive(Deserialize, utoipa::ToSchema)]
struct RouteHintReq {
    /// The client address the hint is for.
    #[schema(example = "203.0.113.9")]
    src_ip: String,
    /// The pool to send that client's next connection to.
    #[schema(example = "survival")]
    pool: String,
    /// How long the hint lives, 1 to 3600 seconds.
    #[serde(default = "default_ttl_sec")]
    #[schema(example = 30, default = 30, minimum = 1, maximum = 3600)]
    ttl_sec: u64,
}

fn default_ttl_sec() -> u64 {
    30
}

#[utoipa::path(
    post, path = "/route-hint", tag = "Routing",
    summary = "Push a route hint for a client address",
    description = "Records a short-lived `src_ip` to pool mapping, which listeners with `route_hint: true` consult before \
their route list. A launcher or control plane calls it just before it sends a player to the proxy. The hint expires on its own. \
Needs the feature flag `admin_write.route_hint`.",
    request_body(content = RouteHintReq, description = "The client address, the pool, and how long the hint lives."),
    responses(
        (status = 200, description = "Recorded.", body = String, content_type = "text/plain", example = "ok\n"),
        (status = 400, description = "`src_ip` is not an IP address, `ttl_sec` is outside 1 to 3600, or the pool does not exist.", body = String, content_type = "text/plain"),
        (status = 403, description = "Writes are switched off by `admin_write.route_hint`.", body = String, content_type = "text/plain"),
    )
)]
async fn route_hint(
    State(s): State<AdminState>,
    Json(req): Json<RouteHintReq>,
) -> (StatusCode, &'static str) {
    let Ok(ip) = req.src_ip.parse::<IpAddr>() else {
        return (
            StatusCode::BAD_REQUEST,
            "src_ip is not a valid IP address\n",
        );
    };
    if req.ttl_sec == 0 || req.ttl_sec > 3600 {
        return (
            StatusCode::BAD_REQUEST,
            "ttl_sec must be between 1 and 3600\n",
        );
    }
    if s.runtime.snapshot().pool(&req.pool).is_none() {
        return (StatusCode::BAD_REQUEST, "unknown pool\n");
    }
    s.runtime
        .route_hints()
        .set(ip, req.pool, Duration::from_secs(req.ttl_sec));
    (StatusCode::OK, "ok\n")
}

#[utoipa::path(
    get, path = "/pools", tag = "Pools",
    summary = "List pools and backends",
    description = "One block per pool: the pool name and balancer, then each backend with its health, \
operator state (`enabled`, `draining`, `disabled`) and the number of live sessions.",
    responses(
        (status = 200, description = "Plain text, one line per pool and per backend, tab separated.", body = String, content_type = "text/plain",
            example = "game\tbalancer=LeastConn\n  10.0.0.5:7777\thealthy\tstate=enabled\tactive=12\n"),
    )
)]
async fn pools(State(s): State<AdminState>) -> impl IntoResponse {
    let snap = s.runtime.snapshot();
    let mut out = String::new();
    for (name, pool) in &snap.pools {
        out.push_str(&format!("{name}\tbalancer={:?}\n", pool.balancer));
        for b in pool.backends() {
            out.push_str(&format!(
                "  {}\t{}\tstate={}\tactive={}\n",
                b.addr,
                if b.is_healthy() {
                    "healthy"
                } else {
                    "unhealthy"
                },
                b.admin_state().as_str(),
                b.active(),
            ));
        }
    }
    out
}

/// The body of `PATCH /pools/{pool}/backends/{addr}`: the new operator state, one of `enabled`,
/// `draining` or `disabled`. `draining` and `disabled` stop new-session selection while existing
/// sessions keep running.
#[derive(Deserialize, utoipa::ToSchema)]
struct BackendPatch {
    /// The new operator state.
    #[schema(example = "draining")]
    state: String,
}

#[utoipa::path(
    patch, path = "/pools/{pool}/backends/{addr}", tag = "Pools",
    summary = "Set a backend's operator state",
    description = "`draining` and `disabled` stop new sessions from choosing the backend; sessions already on it keep running. \
`enabled` puts it back. Needs the feature flag `admin_write.backends`.",
    params(
        ("pool" = String, Path, description = "The pool name."),
        ("addr" = String, Path, description = "The backend address as `ip:port` (URL-encode an IPv6 address's brackets and colons).", example = "10.0.0.5:7777"),
    ),
    request_body(content = BackendPatch, description = "The new state."),
    responses(
        (status = 200, description = "State changed.", body = String, content_type = "text/plain", example = "10.0.0.5:7777 -> draining\n"),
        (status = 400, description = "`state` is not one of enabled, draining, disabled, or `addr` is not an `ip:port`.", body = String, content_type = "text/plain"),
        (status = 403, description = "Writes are switched off by `admin_write.backends`.", body = String, content_type = "text/plain"),
        (status = 404, description = "Unknown pool, or the backend is not in the pool.", body = String, content_type = "text/plain"),
    )
)]
async fn patch_backend(
    State(s): State<AdminState>,
    Path((pool, addr)): Path<(String, String)>,
    Json(req): Json<BackendPatch>,
) -> (StatusCode, String) {
    let Some(state) = BackendState::parse(&req.state) else {
        return (
            StatusCode::BAD_REQUEST,
            "state must be one of: enabled, draining, disabled\n".into(),
        );
    };
    let Ok(addr) = addr.parse::<SocketAddr>() else {
        return (
            StatusCode::BAD_REQUEST,
            "backend address is not a valid ip:port\n".into(),
        );
    };
    let snap = s.runtime.snapshot();
    let Some(pool) = snap.pool(&pool) else {
        return (StatusCode::NOT_FOUND, "unknown pool\n".into());
    };
    let Some(backend) = pool.backend(addr) else {
        return (StatusCode::NOT_FOUND, "unknown backend in pool\n".into());
    };
    backend.set_admin_state(state);
    tracing::info!(pool = %pool.name, %addr, state = state.as_str(), "backend admin state changed");
    (StatusCode::OK, format!("{addr} -> {}\n", state.as_str()))
}

/// The body of `POST /pools/{pool}/backends`: the backend to add to the pool at runtime.
#[derive(Deserialize, utoipa::ToSchema)]
struct BackendAdd {
    /// The backend address as `ip:port`.
    #[schema(example = "10.0.0.5:7777")]
    addr: String,
}

#[utoipa::path(
    post, path = "/pools/{pool}/backends", tag = "Pools",
    summary = "Add a backend to a pool",
    description = "Adds the address to the pool at runtime. The edit lives in the runtime overlay, so it \
survives a config reload. The call requests a reload and returns; the backend joins when that reload completes \
(normally within a moment, so an immediate `PATCH` of it can still answer `404`), and the health checker corrects its state within one interval. Needs the feature flag `admin_write.backends` (on by default).",
    params(("pool" = String, Path, description = "The pool name, as in the config.")),
    request_body(content = BackendAdd, description = "The backend to add."),
    responses(
        (status = 200, description = "Added.", body = String, content_type = "text/plain", example = "added 10.0.0.5:7777 to game\n"),
        (status = 400, description = "`addr` is not a valid `ip:port`.", body = String, content_type = "text/plain"),
        (status = 403, description = "Writes are switched off by `admin_write.backends`.", body = String, content_type = "text/plain"),
        (status = 404, description = "No such pool.", body = String, content_type = "text/plain"),
    )
)]
async fn add_backend(
    State(s): State<AdminState>,
    Path(pool): Path<String>,
    Json(req): Json<BackendAdd>,
) -> (StatusCode, String) {
    let Ok(addr) = req.addr.parse::<SocketAddr>() else {
        return (
            StatusCode::BAD_REQUEST,
            "addr is not a valid ip:port\n".into(),
        );
    };
    if s.runtime.snapshot().pool(&pool).is_none() {
        return (StatusCode::NOT_FOUND, "unknown pool\n".into());
    }
    s.runtime.backend_overlay().add(&pool, addr);
    s.runtime.request_reload();
    tracing::info!(%pool, %addr, "backend added via admin API");
    (StatusCode::OK, format!("added {addr} to {pool}\n"))
}

/// `DELETE /pools/{pool}/backends/{addr}` — remove a backend from a pool at
/// runtime (works whether it came from the file or a prior `POST`). Existing
/// sessions on it keep running; it just stops being selected.
#[utoipa::path(
    delete, path = "/pools/{pool}/backends/{addr}", tag = "Pools",
    summary = "Remove a backend from a pool",
    description = "Works whether the backend came from the config file or from an earlier `POST`. Sessions already \
on it keep running; it just stops being chosen. Needs the feature flag `admin_write.backends`.",
    params(
        ("pool" = String, Path, description = "The pool name."),
        ("addr" = String, Path, description = "The backend address as `ip:port`.", example = "10.0.0.5:7777"),
    ),
    responses(
        (status = 200, description = "Removed.", body = String, content_type = "text/plain", example = "removed 10.0.0.5:7777 from game\n"),
        (status = 400, description = "`addr` is not an `ip:port`.", body = String, content_type = "text/plain"),
        (status = 403, description = "Writes are switched off by `admin_write.backends`.", body = String, content_type = "text/plain"),
        (status = 404, description = "No such pool.", body = String, content_type = "text/plain"),
    )
)]
async fn delete_backend(
    State(s): State<AdminState>,
    Path((pool, addr)): Path<(String, String)>,
) -> (StatusCode, String) {
    let Ok(addr) = addr.parse::<SocketAddr>() else {
        return (
            StatusCode::BAD_REQUEST,
            "backend address is not a valid ip:port\n".into(),
        );
    };
    if s.runtime.snapshot().pool(&pool).is_none() {
        return (StatusCode::NOT_FOUND, "unknown pool\n".into());
    }
    s.runtime.backend_overlay().remove(&pool, addr);
    s.runtime.request_reload();
    tracing::info!(%pool, %addr, "backend removed via admin API");
    (StatusCode::OK, format!("removed {addr} from {pool}\n"))
}

/// A module name safe to join onto `sniffers_dir` — no path separators, no
/// `..`, non-empty. Rejects anything else rather than trying to sanitize it,
/// since this name becomes a filename on disk from an admin-API caller.
fn valid_module_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

#[derive(Serialize, Deserialize, utoipa::ToSchema)]
struct SnifferInfo {
    /// The module name (the file stem).
    #[schema(example = "a2s")]
    name: String,
    /// SHA-256 of the file, lowercase hex.
    sha256: String,
    /// File size in bytes.
    size_bytes: u64,
    /// Whether this module is actually loaded into the live registry right
    /// now (a file can exist on disk but have failed its last scan — e.g. a
    /// hash-pin mismatch — in which case this is `false`).
    loaded: bool,
    /// A previous version is kept (`POST /admin/sniffers/{name}/rollback`).
    #[serde(default)]
    has_previous: bool,
    /// The live instance runs the previous version because the current file
    /// failed validation at the last scan.
    #[serde(default)]
    fallback: bool,
}

/// `GET /admin/sniffers` — lists every `.wasm` file in `settings.sniffers.dir`
/// (name, sha256, size), plus whether it's currently loaded into the live
/// registry. `409` if this instance has no `settings.sniffers` configured at
/// all.
#[utoipa::path(
    get, path = "/admin/sniffers", tag = "Sniffers",
    summary = "List the sniffer modules",
    description = "Every `.wasm` file in `settings.sniffers.dir` with its SHA-256, size, and whether it is loaded now. \
A file can exist but not be loaded, for example after a hash-pin mismatch.",
    responses(
        (status = 200, description = "The modules, sorted by name.", body = Vec<SnifferInfo>),
        (status = 409, description = "This instance has no `settings.sniffers` block; turning sniffing on from nothing needs a restart.", body = String, content_type = "text/plain"),
        (status = 500, description = "The directory cannot be read.", body = String, content_type = "text/plain"),
    )
)]
async fn list_sniffers(State(s): State<AdminState>) -> Response {
    let Some(dir) = &s.sniffers_dir else {
        return sniffers_disabled();
    };
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("reading sniffers.dir: {e}\n"),
            )
                .into_response()
        }
    };
    let loaded = s.sniffers.names();
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("wasm") {
            continue;
        }
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        out.push(SnifferInfo {
            sha256: format!("{:x}", Sha256::digest(&bytes)),
            size_bytes: bytes.len() as u64,
            loaded: loaded.contains(&name),
            has_previous: prev_path(dir, &name).is_file(),
            fallback: s.sniffers.get(&name).is_some_and(|x| x.is_fallback()),
            name,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Json(out).into_response()
}

/// `POST /admin/sniffers?name=<module>` — validates a `.wasm` module's raw
/// bytes (a loadable sniffer module within `MAX_MODULE_BYTES`; else `400`, or
/// `413` past the body limit), checks them against `settings.sniffers.modules`
/// pins when the instance has any (else `409`, because the next rescan would
/// reject the file and a stray unpinned file makes the next startup fatal),
/// then writes `settings.sniffers.dir/<name>.wasm` atomically (temp file +
/// rename) and requests a reload so `SnifferLoader::scan` picks it up — the
/// same hot mechanism `POST /pools/{pool}/backends` uses for the runtime
/// overlay. Nothing is written when any check fails. Updating the pin list
/// itself is a config change, out of scope for this endpoint.
#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct SnifferUploadQuery {
    /// The module name: letters, digits, `-` and `_`. The file is stored as `<name>.wasm`.
    #[param(example = "a2s")]
    name: String,
}

/// The kept previous version of `<name>.wasm`: a dotfile, so the `*.wasm` scan
/// and the listing never treat it as a module of its own.
pub(crate) fn prev_path(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
    dir.join(format!(".{name}.wasm.prev"))
}

/// Serialises every swap of `<name>.wasm` / `.prev` (upload, rollback, delete).
static SWAP_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn sha_of_file(path: &std::path::Path) -> Option<String> {
    std::fs::read(path)
        .ok()
        .map(|b| format!("{:x}", Sha256::digest(&b)))
}

/// Uniquifies upload temp files so concurrent uploads never share one.
static TMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[utoipa::path(
    post, path = "/admin/sniffers", tag = "Sniffers",
    summary = "Upload a sniffer module",
    description = "The body is the raw bytes of a `.wasm` module, at most 8 MiB. It is validated, checked against the \
`settings.sniffers.modules` pins when the instance has any, written atomically to `settings.sniffers.dir/<name>.wasm`, \
and a reload picks it up. The module it replaces is kept for `rollback`. Nothing is written when a check fails. \
Needs the feature flag `admin_write.sniffer_upload`.",
    params(SnifferUploadQuery),
    request_body(content = Vec<u8>, content_type = "application/octet-stream", description = "The module bytes."),
    responses(
        (status = 200, description = "Stored.", body = String, content_type = "text/plain", example = "uploaded a2s.wasm (24576 bytes)\n"),
        (status = 400, description = "The name is invalid, or the bytes are not a loadable sniffer module.", body = String, content_type = "text/plain"),
        (status = 403, description = "Writes are switched off by `admin_write.sniffer_upload`.", body = String, content_type = "text/plain"),
        (status = 409, description = "No `settings.sniffers` block, or the instance pins its modules and this name or hash is not listed (`pinned:`).", body = String, content_type = "text/plain"),
        (status = 413, description = "The body is larger than the module limit.", body = String, content_type = "text/plain"),
        (status = 500, description = "The file could not be written.", body = String, content_type = "text/plain"),
    )
)]
async fn upload_sniffer(
    State(s): State<AdminState>,
    Query(q): Query<SnifferUploadQuery>,
    body: Bytes,
) -> Response {
    let Some(dir) = &s.sniffers_dir else {
        return sniffers_disabled();
    };
    if !valid_module_name(&q.name) {
        return (
            StatusCode::BAD_REQUEST,
            "name must be non-empty and contain only [A-Za-z0-9_-]\n".to_string(),
        )
            .into_response();
    }
    let Some(validator) = s.sniffer_validator.clone() else {
        return sniffers_disabled();
    };
    let bytes = body.clone();
    // Compiling a module is CPU work; keep it off the async workers.
    let v0 = validator.clone();
    let verdict = tokio::task::spawn_blocking(move || v0(&bytes)).await;
    match verdict {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            return (
                StatusCode::BAD_REQUEST,
                format!("invalid sniffer module: {e}\n"),
            )
                .into_response()
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("validating the module: {e}\n"),
            )
                .into_response()
        }
    }
    let pins = (s.sniffer_pins)();
    if !pins.is_empty() {
        let digest = format!("{:x}", Sha256::digest(&body));
        let refusal = match pins.iter().find(|p| p.name == q.name) {
            Some(p) if p.sha256 == digest => None,
            Some(p) => Some(format!(
                "pinned: {} must have sha256 {}, the upload has {digest}\n",
                q.name, p.sha256
            )),
            None => Some(format!(
                "pinned: {} is not listed in settings.sniffers.modules (upload sha256 {digest})\n",
                q.name
            )),
        };
        if let Some(msg) = refusal {
            return (StatusCode::CONFLICT, msg).into_response();
        }
    }
    let tmp = dir.join(format!(
        ".{}.{}.tmp",
        q.name,
        TMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let path = dir.join(format!("{}.wasm", q.name));
    let new_sha = format!("{:x}", Sha256::digest(&body));
    let prev = prev_path(dir, &q.name);
    // The whole swap, including the decision to keep the current file, runs
    // under SWAP_LOCK on a blocking thread (it validates a module), so
    // concurrent uploads each see the file the other installed.
    let written = {
        let (tmp, path, prev, dir, name, body, validator) = (
            tmp.clone(),
            path.clone(),
            prev,
            dir.clone(),
            q.name.clone(),
            body.clone(),
            validator.clone(),
        );
        tokio::task::spawn_blocking(move || {
            let _guard = SWAP_LOCK
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            std::fs::write(&tmp, &body).and_then(|()| {
                // Keep the module being replaced when it is itself a loadable
                // module (a rejected one must not displace the known-good
                // previous), not the same build, and, on a pinned instance,
                // one a pin for this name matches (anything else could never
                // be rolled back to, so it would only occupy the slot), via a hard link so
                // `<name>.wasm` is never missing: link the current file to a
                // temp name, rename that over `.prev`, then rename the new
                // file over the current one.
                let keep_current = std::fs::read(&path).is_ok_and(|cur| {
                    let cur_sha = format!("{:x}", Sha256::digest(&cur));
                    cur_sha != new_sha
                        && (pins.is_empty()
                            // The loader judges a name by its first pin.
                            || pins
                                .iter()
                                .find(|p| p.name == name)
                                .is_some_and(|p| p.sha256 == cur_sha))
                        && validator(&cur).is_ok()
                });
                if keep_current {
                    let keep = dir.join(format!(
                        ".{name}.{}.keep",
                        TMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                    ));
                    let kept = std::fs::hard_link(&path, &keep)
                        .or_else(|_| std::fs::copy(&path, &keep).map(|_| ()))
                        .and_then(|()| std::fs::rename(&keep, &prev));
                    if let Err(e) = kept {
                        let _ = std::fs::remove_file(&keep);
                        return Err(e);
                    }
                }
                std::fs::rename(&tmp, &path)
            })
        })
        .await
        .unwrap_or_else(|e| Err(std::io::Error::other(e)))
    };
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("writing {}: {e}\n", path.display()),
        )
            .into_response();
    }
    s.runtime.request_reload();
    tracing::info!(name = %q.name, size = body.len(), "sniffer module uploaded via admin API");
    (
        StatusCode::OK,
        format!("uploaded {}.wasm ({} bytes)\n", q.name, body.len()),
    )
        .into_response()
}

/// `DELETE /admin/sniffers/{name}` — removes a `.wasm` module from
/// `settings.sniffers.dir` and requests a reload so the next rescan drops it
/// from the live registry.
#[utoipa::path(
    delete, path = "/admin/sniffers/{name}", tag = "Sniffers",
    summary = "Remove a sniffer module",
    description = "Removes `<name>.wasm` and the kept previous version, then a reload drops it from the live registry. \
Needs the feature flag `admin_write.sniffer_upload`.",
    params(("name" = String, Path, description = "The module name (letters, digits, `-` and `_`).")),
    responses(
        (status = 200, description = "Removed.", body = String, content_type = "text/plain", example = "removed a2s.wasm\n"),
        (status = 400, description = "Invalid module name.", body = String, content_type = "text/plain"),
        (status = 403, description = "Writes are switched off by `admin_write.sniffer_upload`.", body = String, content_type = "text/plain"),
        (status = 404, description = "No such module.", body = String, content_type = "text/plain"),
        (status = 409, description = "No `settings.sniffers` block on this instance.", body = String, content_type = "text/plain"),
        (status = 500, description = "The file could not be removed.", body = String, content_type = "text/plain"),
    )
)]
async fn delete_sniffer(State(s): State<AdminState>, Path(name): Path<String>) -> Response {
    let Some(dir) = &s.sniffers_dir else {
        return sniffers_disabled();
    };
    if !valid_module_name(&name) {
        return (StatusCode::BAD_REQUEST, "invalid module name\n".to_string()).into_response();
    }
    let path = dir.join(format!("{name}.wasm"));
    let removed = {
        let _guard = SWAP_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let r = std::fs::remove_file(&path);
        // The kept previous version goes too, also when the current file was
        // already gone (an orphan would resurface under a later install).
        // A current file that failed to go for another reason keeps its `.prev`.
        let current_gone = r
            .as_ref()
            .err()
            .is_none_or(|e| e.kind() == std::io::ErrorKind::NotFound);
        let prev_removed = if !current_gone {
            false
        } else {
            match std::fs::remove_file(prev_path(dir, &name)) {
                Ok(()) => true,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
                Err(e) => {
                    s.runtime.request_reload();
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!(
                            "removed {name}.wasm but not its previous version: {e}; retry the delete\n"
                        ),
                    )
                        .into_response();
                }
            }
        };
        match r {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && prev_removed => Ok(()),
            r => r,
        }
    };
    match removed {
        Ok(()) => {
            s.runtime.request_reload();
            tracing::info!(%name, "sniffer module removed via admin API");
            (StatusCode::OK, format!("removed {name}.wasm\n")).into_response()
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            (StatusCode::NOT_FOUND, "no such module\n".to_string()).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("removing {}: {e}\n", path.display()),
        )
            .into_response(),
    }
}

/// `POST /admin/sniffers/{name}/rollback` — swaps `<name>.wasm` with the kept
/// previous version (so a second rollback undoes the first; exactly one
/// previous is ever kept). `404` without a previous version. On a pinned
/// instance only when the previous version's sha256 equals the pin (`409
/// pinned:`), because anything else would make the next rescan reject the file.
#[utoipa::path(
    post, path = "/admin/sniffers/{name}/rollback", tag = "Sniffers",
    summary = "Roll a sniffer module back",
    description = "Swaps `<name>.wasm` with the kept previous version, so a second rollback undoes the first. Exactly one \
previous version is kept. On an instance that pins its modules it only works when the previous version's SHA-256 is pinned. \
Needs the feature flag `admin_write.sniffer_upload`.",
    params(("name" = String, Path, description = "The module name.")),
    responses(
        (status = 200, description = "Rolled back.", body = String, content_type = "text/plain", example = "rolled back a2s.wasm\n"),
        (status = 400, description = "Invalid module name.", body = String, content_type = "text/plain"),
        (status = 403, description = "Writes are switched off by `admin_write.sniffer_upload`.", body = String, content_type = "text/plain"),
        (status = 404, description = "No previous version is kept.", body = String, content_type = "text/plain"),
        (status = 409, description = "No `settings.sniffers` block, or the previous version's hash is not pinned (`pinned:`).", body = String, content_type = "text/plain"),
        (status = 500, description = "The swap failed.", body = String, content_type = "text/plain"),
    )
)]
async fn rollback_sniffer(State(s): State<AdminState>, Path(name): Path<String>) -> Response {
    let Some(dir) = &s.sniffers_dir else {
        return sniffers_disabled();
    };
    if !valid_module_name(&name) {
        return (StatusCode::BAD_REQUEST, "invalid module name\n".to_string()).into_response();
    }
    let path = dir.join(format!("{name}.wasm"));
    let prev = prev_path(dir, &name);
    let swapped = {
        let _guard = SWAP_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(prev_sha) = sha_of_file(&prev) else {
            return (
                StatusCode::NOT_FOUND,
                "no previous version kept\n".to_string(),
            )
                .into_response();
        };
        let pins = (s.sniffer_pins)();
        if !pins.is_empty() && !pins.iter().any(|p| p.name == name && p.sha256 == prev_sha) {
            return (
                StatusCode::CONFLICT,
                format!("pinned: the previous version of {name} (sha256 {prev_sha}) does not match its pin\n"),
            )
                .into_response();
        }
        // current -> temp link, prev -> current, temp -> prev; the current
        // file is replaced atomically and never missing.
        let tmp = dir.join(format!(
            ".{name}.{}.swap",
            TMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let had_current = path.is_file();
        let r = (|| {
            if had_current {
                std::fs::hard_link(&path, &tmp)
                    .or_else(|_| std::fs::copy(&path, &tmp).map(|_| ()))?;
            }
            std::fs::rename(&prev, &path)?;
            if had_current {
                std::fs::rename(&tmp, &prev)?;
            }
            Ok::<(), std::io::Error>(())
        })();
        match r {
            Err(_) if tmp.exists() && !prev.exists() => {
                // Only the last rename failed: retry it. If it lands the swap
                // is complete (report success); otherwise the staged former
                // current is dropped rather than left behind.
                let r = std::fs::rename(&tmp, &prev);
                if r.is_err() {
                    let _ = std::fs::remove_file(&tmp);
                }
                r
            }
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                Err(e)
            }
            ok => ok,
        }
    };
    match swapped {
        Ok(()) => {
            s.runtime.request_reload();
            tracing::info!(%name, "sniffer module rolled back via admin API");
            (StatusCode::OK, format!("rolled back {name}.wasm\n")).into_response()
        }
        Err(e) => {
            // The files may have changed before the failure; let the next scan see them.
            s.runtime.request_reload();
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("rolling back {}: {e}\n", path.display()),
            )
                .into_response()
        }
    }
}

fn sniffers_disabled() -> Response {
    (
        StatusCode::CONFLICT,
        "settings.sniffers is not configured on this instance; turning it on from nothing needs \
         a restart, see docs/05-configuration.md\n",
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `docs/reference/openapi/proxy-admin.json` is the rendering of [`AdminApi`] the docs site is
    /// built from. Regenerate it with `WAYHOUSE_UPDATE_OPENAPI=1 cargo test -p wayhouse openapi`.
    #[test]
    fn the_checked_in_spec_is_current() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/reference/openapi/proxy-admin.json");
        let mut want = serde_json::to_string_pretty(&openapi_json()).unwrap();
        want.push('\n');
        if std::env::var_os("WAYHOUSE_UPDATE_OPENAPI").is_some() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &want).unwrap();
            return;
        }
        let have = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(
            have == want,
            "{} is out of date; run `WAYHOUSE_UPDATE_OPENAPI=1 cargo test -p wayhouse openapi` and commit it",
            path.display()
        );
    }
    use std::time::Duration;

    use metrics_exporter_prometheus::PrometheusBuilder;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};
    use wayhouse_core::{Runtime, Snapshot};

    /// Spin up `admin::router` on an ephemeral port against a live `Runtime`.
    /// Returns the base URL and the runtime (kept alive for the test).
    async fn spawn_admin(yaml: &str) -> (String, Runtime) {
        spawn_admin_with_token(yaml, None).await
    }

    async fn spawn_admin_with_token(yaml: &str, auth_token: Option<&str>) -> (String, Runtime) {
        spawn_admin_full(yaml, auth_token, None).await
    }

    async fn spawn_admin_full(
        yaml: &str,
        auth_token: Option<&str>,
        sniffers_dir: Option<std::path::PathBuf>,
    ) -> (String, Runtime) {
        spawn_admin_config(yaml, auth_token, sniffers_dir, None).await
    }

    async fn spawn_admin_config(
        yaml: &str,
        auth_token: Option<&str>,
        sniffers_dir: Option<std::path::PathBuf>,
        config_file: Option<ConfigFile>,
    ) -> (String, Runtime) {
        let cfg = wayhouse_config::parse_str(yaml).unwrap();
        let runtime = Runtime::start(Snapshot::from_config(&cfg), std::sync::Arc::default(), 1);
        let prometheus = PrometheusBuilder::new().build_recorder().handle();
        let app = router(AdminState {
            runtime: runtime.handle(),
            prometheus,
            auth_token: auth_token
                .map(|t| std::sync::Arc::new(wayhouse_http::rotating::RotatingSecret::new(t))),
            sniffers_dir,
            sniffers: std::sync::Arc::new(wayhouse_core::sniff::Sniffers::default()),
            sniffer_pins: std::sync::Arc::new(Vec::new),
            sniffer_validator: None,
            config_file,
        });
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        tokio::time::sleep(Duration::from_millis(150)).await;
        (format!("http://{addr}"), runtime)
    }

    /// A TCP backend that echoes after a delay, so a proxied connection stays
    /// live long enough to be observed in `GET /sessions`.
    async fn slow_echo_backend() -> SocketAddr {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = l.accept().await {
                tokio::spawn(async move {
                    let mut buf = [0u8; 64];
                    while let Ok(n) = s.read(&mut buf).await {
                        if n == 0 {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(300)).await;
                        if s.write_all(&buf[..n]).await.is_err() {
                            break;
                        }
                    }
                });
            }
        });
        addr
    }

    fn free_port() -> SocketAddr {
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
    }

    #[tokio::test]
    async fn web_serves_the_resolved_section_and_404s_without_one() {
        let backend = slow_echo_backend().await;
        let proxy = free_port();
        let base_yaml = format!(
            "pools:\n  - name: web\n    targets: [\"127.0.0.1:8443\"]\n    proxy_protocol: v2\n\
             \x20 - name: p\n    targets: [\"{backend}\"]\n\
             listeners:\n  - name: l\n    bind: \"{proxy}\"\n    routes:\n\
             \x20     - match: {{ type: always }}\n        action: {{ pool: web }}\n"
        );
        let http = reqwest::Client::new();

        let (base, runtime) = spawn_admin(&base_yaml).await;
        let r = http.get(format!("{base}/web")).send().await.unwrap();
        assert_eq!(r.status(), reqwest::StatusCode::NOT_FOUND);
        runtime
            .shutdown_with_grace(Duration::from_millis(100))
            .await;

        let yaml = format!(
            "schema_version: 3\n{base_yaml}web:\n  sites:\n    - hosts: [Panel.example.com]\n      \
             upstreams: [{{ addr: \"{backend}\" }}]\n      upstream_tls: skip\n"
        );
        let (base, runtime) = spawn_admin(&yaml).await;
        let v: serde_json::Value = http
            .get(format!("{base}/web"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(v["pool"], "web");
        assert_eq!(v["sites"][0]["hosts"][0], "panel.example.com");
        assert_eq!(v["sites"][0]["upstreams"][0]["addr"], backend.to_string());
        assert_eq!(v["sites"][0]["upstream_tls"], "skip");
        runtime
            .shutdown_with_grace(Duration::from_millis(100))
            .await;
    }

    #[tokio::test]
    async fn healthz_readyz_pools_and_config_respond() {
        let backend = slow_echo_backend().await;
        let proxy = free_port();
        let yaml = format!(
            "pools:\n  - name: p\n    targets: [\"{backend}\"]\n\
             listeners:\n  - name: l\n    bind: \"{proxy}\"\n    pool: p\n"
        );
        let (base, runtime) = spawn_admin(&yaml).await;
        let http = reqwest::Client::new();

        let r = http.get(format!("{base}/healthz")).send().await.unwrap();
        assert!(r.status().is_success());
        assert_eq!(r.text().await.unwrap(), "ok");

        let r = http.get(format!("{base}/readyz")).send().await.unwrap();
        assert!(r.status().is_success());

        let body = http
            .get(format!("{base}/pools"))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert!(body.contains("p\tbalancer="), "pools body: {body}");
        assert!(body.contains(&backend.to_string()));

        let body = http
            .get(format!("{base}/config"))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert!(body.contains("listeners:"), "config body: {body}");
        assert!(body.contains("\tbind=") && body.contains("pools:"));

        runtime
            .shutdown_with_grace(Duration::from_millis(100))
            .await;
    }

    const WEB_YAML: &str = "schema_version: 3\n\
        pools:\n  - name: web\n    targets: [\"127.0.0.1:8443\"]\n    proxy_protocol: v2\n  \
        - name: origin\n    targets: [\"10.0.0.5:8080\"]\n  \
        - name: other\n    targets: [\"10.0.0.6:8080\"]\n\
        listeners:\n  - name: https\n    bind: \"127.0.0.1:0\"\n    pool: web\n\
        web:\n  trusted_proxy: [\"10.0.0.0/8\"]\n  sites:\n    - hosts: [Panel.example.com]\n      \
        upstreams:\n        - addr: 10.0.0.9:8080\n        - pool: other\n        \
        - { pool: origin, via: wayhouse, port: 19010 }\n";

    #[tokio::test]
    async fn web_endpoint_is_404_without_a_web_section() {
        let (base, runtime) = spawn_admin(
            "pools:\n  - name: p\n    targets: [\"127.0.0.1:1\"]\n\
             listeners:\n  - name: l\n    bind: \"127.0.0.1:0\"\n    pool: p\n",
        )
        .await;
        let r = reqwest::get(format!("{base}/web")).await.unwrap();
        assert_eq!(r.status(), 404);
        runtime
            .shutdown_with_grace(Duration::from_millis(100))
            .await;
    }

    #[tokio::test]
    async fn web_endpoint_returns_resolved_sites_and_follows_a_reload() {
        let (base, runtime) = spawn_admin_with_token(WEB_YAML, Some("s3cret")).await;
        let http = reqwest::Client::new();
        let url = format!("{base}/web");
        assert_eq!(http.get(&url).send().await.unwrap().status(), 401);
        let v: serde_json::Value = http
            .get(&url)
            .bearer_auth("s3cret")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(v["pool"], "web");
        assert_eq!(v["listen"]["http"], "127.0.0.1:8080");
        assert_eq!(v["listen"]["https"], "127.0.0.1:8443");
        assert_eq!(v["trusted_proxy"], serde_json::json!(["10.0.0.0/8"]));
        let site = &v["sites"][0];
        assert_eq!(site["hosts"], serde_json::json!(["panel.example.com"]));
        assert_eq!(
            site["upstreams"],
            serde_json::json!([
                { "addr": "10.0.0.9:8080" },
                { "pool": "other", "via": "direct" },
                { "pool": "origin", "via": "wayhouse", "local": "127.0.0.1:19010" },
            ])
        );

        // A reload swaps the snapshot; the endpoint follows without a restart.
        let next = WEB_YAML.replace("port: 19010", "port: 19011");
        let cfg = wayhouse_config::parse_str(&next).unwrap();
        runtime.handle().store(Snapshot::from_config(&cfg));
        let v: serde_json::Value = http
            .get(&url)
            .bearer_auth("s3cret")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(v["sites"][0]["upstreams"][2]["local"], "127.0.0.1:19011");
        runtime
            .shutdown_with_grace(Duration::from_millis(100))
            .await;
    }

    #[tokio::test]
    async fn sessions_endpoint_lists_a_live_connection_and_honours_filters() {
        let backend = slow_echo_backend().await;
        let proxy = free_port();
        let yaml = format!(
            "pools:\n  - name: p\n    targets: [\"{backend}\"]\n\
             listeners:\n  - name: l\n    bind: \"{proxy}\"\n    pool: p\n"
        );
        let (base, runtime) = spawn_admin(&yaml).await;
        let http = reqwest::Client::new();

        let empty = http
            .get(format!("{base}/sessions"))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert_eq!(empty, "sessions=0\n");

        let mut c = TcpStream::connect(proxy).await.unwrap();
        c.write_all(b"ping").await.unwrap();
        let src_port = c.local_addr().unwrap().port();
        tokio::time::sleep(Duration::from_millis(100)).await;

        let body = http
            .get(format!("{base}/sessions"))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert!(body.starts_with("sessions=1\n"), "body: {body}");
        assert!(body.contains("\ttcp\tl\t"), "body: {body}");
        assert!(body.contains("pool=p"), "body: {body}");
        assert!(body.contains(&format!("backend={backend}")), "body: {body}");
        assert!(body.contains(&format!("peer=127.0.0.1:{src_port}")));

        // Filters that should still match.
        for q in [
            "?proto=tcp",
            "?listener=l",
            "?pool=p",
            "?src=127.0.0.1",
            "?src=::ffff:127.0.0.1",
        ] {
            let b = http
                .get(format!("{base}/sessions{q}"))
                .send()
                .await
                .unwrap()
                .text()
                .await
                .unwrap();
            assert!(b.starts_with("sessions=1\n"), "{q} -> {b}");
        }
        // Filters that should exclude the session.
        for q in [
            "?proto=udp",
            "?listener=other",
            "?pool=other",
            "?src=10.0.0.1",
        ] {
            let b = http
                .get(format!("{base}/sessions{q}"))
                .send()
                .await
                .unwrap()
                .text()
                .await
                .unwrap();
            assert_eq!(b, "sessions=0\n", "{q}");
        }

        drop(c);
        runtime
            .shutdown_with_grace(Duration::from_millis(100))
            .await;
    }

    #[tokio::test]
    async fn auth_token_gates_everything_but_healthz() {
        let yaml = "pools:\n  - name: p\n    targets: [\"127.0.0.1:1\"]\n\
                    listeners:\n  - name: l\n    bind: \"127.0.0.1:0\"\n    pool: p\n";
        let (base, _runtime) = spawn_admin_with_token(yaml, Some("secret123")).await;
        let http = reqwest::Client::new();

        // healthz stays open even with a token configured.
        let r = http.get(format!("{base}/healthz")).send().await.unwrap();
        assert!(r.status().is_success());

        // Everything else is gated: no token, wrong token, right token.
        let r = http.get(format!("{base}/pools")).send().await.unwrap();
        assert_eq!(r.status(), reqwest::StatusCode::UNAUTHORIZED);

        let r = http
            .get(format!("{base}/pools"))
            .bearer_auth("wrong")
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), reqwest::StatusCode::UNAUTHORIZED);

        let r = http
            .get(format!("{base}/pools"))
            .bearer_auth("secret123")
            .send()
            .await
            .unwrap();
        assert!(r.status().is_success());
    }

    #[tokio::test]
    async fn sniffer_routes_are_409_when_settings_sniffers_is_absent() {
        let yaml = "pools:\n  - name: p\n    targets: [\"127.0.0.1:1\"]\n\
                    listeners:\n  - name: l\n    bind: \"127.0.0.1:0\"\n    pool: p\n";
        let (base, _runtime) = spawn_admin(yaml).await;
        let http = reqwest::Client::new();

        let r = http
            .get(format!("{base}/admin/sniffers"))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), reqwest::StatusCode::CONFLICT);

        let r = http
            .post(format!("{base}/admin/sniffers?name=x"))
            .body(vec![0u8])
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), reqwest::StatusCode::CONFLICT);

        let r = http
            .delete(format!("{base}/admin/sniffers/x"))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), reqwest::StatusCode::CONFLICT);
    }

    #[cfg(feature = "wasm-sniffers")]
    mod sniffer_upload {
        use super::*;
        use std::sync::{Arc, Mutex};
        use wayhouse_config::SnifferModulePin;

        const YAML: &str = "pools:\n  - name: p\n    targets: [\"127.0.0.1:1\"]\n\
                            listeners:\n  - name: l\n    bind: \"127.0.0.1:0\"\n    pool: p\n";

        const MODULE_WAT: &str = r#"(module
          (@custom "wayhouse.abi" "\00\00\01\00")
          (memory (export "memory") 1)
          (func (export "alloc") (param i32) (result i32) (i32.const 0))
          (func (export "sniff") (param i32 i32 i32 i32) (result i64) (i64.const 0)))"#;

        fn module() -> Vec<u8> {
            wat::parse_str(MODULE_WAT).unwrap()
        }

        /// A valid module padded with a custom section to `total` bytes.
        fn module_padded_to(total: usize) -> Vec<u8> {
            let mut m = module();
            let name = b"pad";
            let payload = total - m.len() - 1 - 5 - 1 - name.len();
            // section id 0, LEB128 size (5 bytes), name len, name, payload
            let size = 1 + name.len() + payload;
            m.push(0);
            let mut n = size as u32;
            for k in 0..5 {
                let mut b = (n & 0x7f) as u8;
                n >>= 7;
                if k < 4 {
                    b |= 0x80;
                }
                m.push(b);
            }
            m.push(name.len() as u8);
            m.extend_from_slice(name);
            m.extend(std::iter::repeat_n(0u8, payload));
            assert_eq!(m.len(), total);
            m
        }

        fn scratch(tag: &str) -> std::path::PathBuf {
            let dir = std::env::temp_dir().join(format!(
                "wayhouse-admin-sniffer-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            dir
        }

        async fn spawn(
            dir: &std::path::Path,
            pins: Arc<Mutex<Vec<SnifferModulePin>>>,
        ) -> (String, Runtime) {
            let cfg = wayhouse_config::parse_str(YAML).unwrap();
            let runtime = Runtime::start(Snapshot::from_config(&cfg), std::sync::Arc::default(), 1);
            let loader = Arc::new(
                crate::sniffer_loader::SnifferLoader::new(Duration::from_millis(50)).unwrap(),
            );
            let app = router(AdminState {
                runtime: runtime.handle(),
                prometheus: PrometheusBuilder::new().build_recorder().handle(),
                auth_token: None,
                sniffers_dir: Some(dir.to_path_buf()),
                sniffers: Arc::new(wayhouse_core::sniff::Sniffers::default()),
                sniffer_pins: Arc::new(move || pins.lock().unwrap().clone()),
                sniffer_validator: Some(Arc::new(move |b: &[u8]| {
                    loader.validate(b, 1 << 20).map_err(|e| e.to_string())
                })),
                config_file: None,
            });
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            });
            tokio::time::sleep(Duration::from_millis(150)).await;
            (format!("http://{addr}"), runtime)
        }

        async fn post(base: &str, name: &str, body: Vec<u8>) -> reqwest::Response {
            reqwest::Client::new()
                .post(format!("{base}/admin/sniffers?name={name}"))
                .body(body)
                .send()
                .await
                .unwrap()
        }

        fn no_pins() -> Arc<Mutex<Vec<SnifferModulePin>>> {
            Arc::default()
        }

        fn pin(name: &str, bytes: &[u8]) -> SnifferModulePin {
            SnifferModulePin {
                name: name.into(),
                sha256: format!("{:x}", Sha256::digest(bytes)),
                config: None,
            }
        }

        fn dir_is_empty(dir: &std::path::Path) -> bool {
            std::fs::read_dir(dir).unwrap().next().is_none()
        }

        #[tokio::test]
        async fn upload_list_and_delete_a_sniffer_module_round_trips() {
            let dir = scratch("roundtrip");
            let (base, _rt) = spawn(&dir, no_pins()).await;
            let http = reqwest::Client::new();
            let list = |base: String| async move {
                reqwest::get(format!("{base}/admin/sniffers"))
                    .await
                    .unwrap()
                    .json::<Vec<SnifferInfo>>()
                    .await
                    .unwrap()
            };
            assert!(list(base.clone()).await.is_empty());

            // Reject a path-traversal-shaped name before touching the filesystem.
            let r = post(&base, "..%2Fevil", module()).await;
            assert_eq!(r.status(), reqwest::StatusCode::BAD_REQUEST);

            let bytes = module();
            let r = post(&base, "demo", bytes.clone()).await;
            assert!(r.status().is_success());
            assert!(dir.join("demo.wasm").is_file());
            let l = list(base.clone()).await;
            assert_eq!(l.len(), 1);
            assert_eq!(l[0].name, "demo");
            assert_eq!(l[0].size_bytes, bytes.len() as u64);

            let r = http
                .delete(format!("{base}/admin/sniffers/demo"))
                .send()
                .await
                .unwrap();
            assert!(r.status().is_success());
            assert!(!dir.join("demo.wasm").exists());
            let r = http
                .delete(format!("{base}/admin/sniffers/demo"))
                .send()
                .await
                .unwrap();
            assert_eq!(r.status(), reqwest::StatusCode::NOT_FOUND);
            let _ = std::fs::remove_dir_all(&dir);
        }

        #[tokio::test]
        async fn upload_rejects_garbage_with_400_and_writes_nothing() {
            let dir = scratch("garbage");
            let (base, _rt) = spawn(&dir, no_pins()).await;
            let r = post(&base, "junk", b"not wasm".to_vec()).await;
            assert_eq!(r.status(), reqwest::StatusCode::BAD_REQUEST);
            assert!(r.text().await.unwrap().contains("invalid sniffer module"));
            assert!(dir_is_empty(&dir));
        }

        #[tokio::test]
        async fn upload_rejects_truncated_module_with_400() {
            let dir = scratch("truncated");
            let (base, _rt) = spawn(&dir, no_pins()).await;
            let mut m = module();
            m.truncate(m.len() - 3);
            let r = post(&base, "cut", m).await;
            assert_eq!(r.status(), reqwest::StatusCode::BAD_REQUEST);
            assert!(dir_is_empty(&dir));
        }

        #[tokio::test]
        async fn upload_rejects_empty_body() {
            let dir = scratch("empty");
            let (base, _rt) = spawn(&dir, no_pins()).await;
            let r = post(&base, "nothing", Vec::new()).await;
            assert_eq!(r.status(), reqwest::StatusCode::BAD_REQUEST);
            assert!(dir_is_empty(&dir));
        }

        #[tokio::test]
        async fn upload_rejects_oversize_with_413() {
            let dir = scratch("oversize");
            let (base, _rt) = spawn(&dir, no_pins()).await;
            let r = post(
                &base,
                "big",
                vec![0u8; crate::sniffer_loader::MAX_MODULE_BYTES + 1],
            )
            .await;
            assert_eq!(r.status(), reqwest::StatusCode::PAYLOAD_TOO_LARGE);
            assert!(dir_is_empty(&dir));
        }

        #[tokio::test]
        async fn upload_accepts_valid_module_larger_than_2mib() {
            let dir = scratch("big-ok");
            let (base, _rt) = spawn(&dir, no_pins()).await;
            let r = post(&base, "big", module_padded_to(3 << 20)).await;
            let st = r.status();
            assert!(st.is_success(), "{st} {}", r.text().await.unwrap());
            assert_eq!(
                std::fs::metadata(dir.join("big.wasm")).unwrap().len(),
                3 << 20
            );
        }

        #[tokio::test]
        async fn upload_writes_via_rename_and_leaves_no_tmp_file() {
            let dir = scratch("atomic");
            let (base, _rt) = spawn(&dir, no_pins()).await;
            let (a, b) = tokio::join!(
                post(&base, "same", module_padded_to(1 << 20)),
                post(&base, "same", module_padded_to(2 << 20)),
            );
            assert!(a.status().is_success() && b.status().is_success());
            let names: Vec<_> = std::fs::read_dir(&dir)
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            assert!(
                names
                    .iter()
                    .all(|n| n == "same.wasm" || n == ".same.wasm.prev"),
                "no leftovers: {names:?}"
            );
            assert!(names.contains(&"same.wasm".to_string()), "{names:?}");
            let len = std::fs::metadata(dir.join("same.wasm")).unwrap().len();
            assert!(len == 1 << 20 || len == 2 << 20, "whole file, got {len}");
        }

        async fn rollback(base: &str, name: &str) -> reqwest::Response {
            reqwest::Client::new()
                .post(format!("{base}/admin/sniffers/{name}/rollback"))
                .send()
                .await
                .unwrap()
        }

        async fn list(base: &str) -> Vec<SnifferInfo> {
            reqwest::get(format!("{base}/admin/sniffers"))
                .await
                .unwrap()
                .json()
                .await
                .unwrap()
        }

        fn len_of(dir: &std::path::Path, file: &str) -> u64 {
            std::fs::metadata(dir.join(file)).unwrap().len()
        }

        #[tokio::test]
        async fn upload_keeps_the_replaced_module_as_prev_and_lists_it() {
            let dir = scratch("keep-prev");
            let (base, _rt) = spawn(&dir, no_pins()).await;
            assert!(post(&base, "demo", module_padded_to(1 << 20))
                .await
                .status()
                .is_success());
            assert!(
                !list(&base).await[0].has_previous,
                "first install has no prev"
            );
            assert!(post(&base, "demo", module_padded_to(2 << 20))
                .await
                .status()
                .is_success());
            assert_eq!(len_of(&dir, ".demo.wasm.prev"), 1 << 20);
            assert_eq!(len_of(&dir, "demo.wasm"), 2 << 20);
            let l = list(&base).await;
            assert_eq!(l.len(), 1, "the .prev file is not a module of its own");
            assert!(l[0].has_previous);
            assert!(!l[0].fallback);
        }

        #[tokio::test]
        async fn invalid_current_never_replaces_the_known_good_prev() {
            let dir = scratch("bad-current");
            std::fs::write(dir.join("demo.wasm"), b"not wasm").unwrap();
            std::fs::write(dir.join(".demo.wasm.prev"), module_padded_to(1 << 20)).unwrap();
            let (base, _rt) = spawn(&dir, no_pins()).await;
            assert!(post(&base, "demo", module_padded_to(2 << 20))
                .await
                .status()
                .is_success());
            assert_eq!(len_of(&dir, ".demo.wasm.prev"), 1 << 20, "good prev kept");
            assert_eq!(len_of(&dir, "demo.wasm"), 2 << 20);
        }

        #[tokio::test]
        async fn concurrent_first_uploads_still_keep_one_as_prev() {
            let dir = scratch("concurrent-new");
            let (base, _rt) = spawn(&dir, no_pins()).await;
            let (a, b) = tokio::join!(
                post(&base, "demo", module_padded_to(1 << 20)),
                post(&base, "demo", module_padded_to(2 << 20)),
            );
            assert!(a.status().is_success() && b.status().is_success());
            assert!(
                dir.join(".demo.wasm.prev").is_file(),
                "the first install is rollbackable"
            );
        }

        #[tokio::test]
        async fn identical_reupload_does_not_overwrite_prev() {
            let dir = scratch("same-sha");
            let (base, _rt) = spawn(&dir, no_pins()).await;
            for n in [1, 2, 2] {
                assert!(post(&base, "demo", module_padded_to(n << 20))
                    .await
                    .status()
                    .is_success());
            }
            assert_eq!(len_of(&dir, ".demo.wasm.prev"), 1 << 20, "prev stays v1");
        }

        #[tokio::test]
        async fn rollback_swaps_current_and_prev() {
            let dir = scratch("rollback");
            let (base, _rt) = spawn(&dir, no_pins()).await;
            for n in [1, 2, 3] {
                assert!(post(&base, "demo", module_padded_to(n << 20))
                    .await
                    .status()
                    .is_success());
            }
            // Two updates, then a rollback: the version before the last update.
            let r = rollback(&base, "demo").await;
            assert!(r.status().is_success(), "{}", r.status());
            assert_eq!(len_of(&dir, "demo.wasm"), 2 << 20);
            assert_eq!(
                len_of(&dir, ".demo.wasm.prev"),
                3 << 20,
                "swap keeps exactly one"
            );
            let names: Vec<_> = std::fs::read_dir(&dir)
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            assert_eq!(names.len(), 2, "{names:?}");
        }

        #[tokio::test]
        async fn rollback_without_prev_is_404_and_bad_name_is_400() {
            let dir = scratch("rollback-none");
            let (base, _rt) = spawn(&dir, no_pins()).await;
            assert!(post(&base, "demo", module()).await.status().is_success());
            assert_eq!(
                rollback(&base, "demo").await.status(),
                reqwest::StatusCode::NOT_FOUND
            );
            assert_eq!(
                rollback(&base, "ghost").await.status(),
                reqwest::StatusCode::NOT_FOUND
            );
            assert_eq!(
                rollback(&base, "a.b").await.status(),
                reqwest::StatusCode::BAD_REQUEST
            );
        }

        #[tokio::test]
        async fn delete_removes_prev_too() {
            let dir = scratch("delete-prev");
            let (base, _rt) = spawn(&dir, no_pins()).await;
            for n in [1, 2] {
                assert!(post(&base, "demo", module_padded_to(n << 20))
                    .await
                    .status()
                    .is_success());
            }
            let r = reqwest::Client::new()
                .delete(format!("{base}/admin/sniffers/demo"))
                .send()
                .await
                .unwrap();
            assert!(r.status().is_success());
            assert!(dir_is_empty(&dir));
        }

        #[tokio::test]
        async fn pinned_rollback_only_when_prev_matches_the_pin() {
            let dir = scratch("rollback-pin");
            let v1 = module_padded_to(1 << 20);
            let v2 = module_padded_to(2 << 20);
            // Pinned to v2: prev (v1) differs, so rollback is refused with `pinned:`.
            std::fs::write(dir.join("demo.wasm"), &v2).unwrap();
            std::fs::write(dir.join(".demo.wasm.prev"), &v1).unwrap();
            let pins = Arc::new(Mutex::new(vec![pin("demo", &v2)]));
            let (base, _rt) = spawn(&dir, pins).await;
            let r = rollback(&base, "demo").await;
            assert_eq!(r.status(), reqwest::StatusCode::CONFLICT);
            assert!(r.text().await.unwrap().starts_with("pinned:"));
            assert_eq!(len_of(&dir, "demo.wasm"), 2 << 20, "nothing changed");
            // Pinned to v1 while v2 is current: rollback to v1 is allowed.
            let dir2 = scratch("rollback-pin-ok");
            std::fs::write(dir2.join("demo.wasm"), &v2).unwrap();
            std::fs::write(dir2.join(".demo.wasm.prev"), &v1).unwrap();
            let pins = Arc::new(Mutex::new(vec![pin("demo", &v1)]));
            let (base2, _rt2) = spawn(&dir2, pins).await;
            assert!(rollback(&base2, "demo").await.status().is_success());
            assert_eq!(len_of(&dir2, "demo.wasm"), 1 << 20);
        }

        #[tokio::test]
        async fn upload_to_pinned_instance_with_other_hash_is_409_and_writes_nothing() {
            let dir = scratch("pin-mismatch");
            let pins = Arc::new(Mutex::new(vec![pin("demo", b"something else")]));
            let (base, _rt) = spawn(&dir, pins).await;
            let r = post(&base, "demo", module()).await;
            assert_eq!(r.status(), reqwest::StatusCode::CONFLICT);
            assert!(r.text().await.unwrap().contains("pinned"));
            assert!(dir_is_empty(&dir));
        }

        #[tokio::test]
        async fn upload_unpinned_name_on_pinned_instance_is_409_and_writes_nothing() {
            let dir = scratch("pin-unlisted");
            let pins = Arc::new(Mutex::new(vec![pin("other", &module())]));
            let (base, _rt) = spawn(&dir, pins).await;
            let r = post(&base, "demo", module()).await;
            assert_eq!(r.status(), reqwest::StatusCode::CONFLICT);
            assert!(dir_is_empty(&dir));
        }

        #[tokio::test]
        async fn upload_matching_pin_is_accepted() {
            let dir = scratch("pin-ok");
            let pins = Arc::new(Mutex::new(vec![pin("demo", &module())]));
            let (base, _rt) = spawn(&dir, pins).await;
            let r = post(&base, "demo", module()).await;
            assert!(r.status().is_success());
            assert!(dir.join("demo.wasm").is_file());
        }

        #[tokio::test]
        async fn pin_check_uses_live_pins_after_reload() {
            let dir = scratch("pin-live");
            let pins = no_pins();
            let (base, _rt) = spawn(&dir, pins.clone()).await;
            assert!(post(&base, "a", module()).await.status().is_success());
            pins.lock().unwrap().push(pin("zzz", b"x"));
            let r = post(&base, "b", module()).await;
            assert_eq!(r.status(), reqwest::StatusCode::CONFLICT);
            assert!(!dir.join("b.wasm").exists());
        }

        fn dir_names(dir: &std::path::Path) -> Vec<String> {
            let mut names: Vec<_> = std::fs::read_dir(dir)
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            names
        }

        /// #246: a pinned instance keeps the replaced module as `.prev` only
        /// when it matches a pin for that name, because only such a file can
        /// ever be rolled back to.
        #[tokio::test]
        async fn pinned_upload_does_not_keep_a_current_module_no_pin_matches() {
            let dir = scratch("pin-keep-unpinned");
            let (v1, v2) = (module_padded_to(1 << 20), module_padded_to(2 << 20));
            std::fs::write(dir.join("demo.wasm"), &v1).unwrap();
            let pins = Arc::new(Mutex::new(vec![pin("demo", &v2)]));
            let (base, _rt) = spawn(&dir, pins).await;
            assert!(post(&base, "demo", v2).await.status().is_success());
            assert_eq!(len_of(&dir, "demo.wasm"), 2 << 20);
            assert_eq!(dir_names(&dir), ["demo.wasm"], "v1 matches no pin");
        }

        /// The loader judges a name by its first pin, so a later duplicate
        /// pin must not make an upload keep a module the loader would reject.
        #[tokio::test]
        async fn pinned_upload_judges_the_current_module_by_the_first_pin_of_the_name() {
            let dir = scratch("pin-keep-duplicate");
            let (v1, v2) = (module_padded_to(1 << 20), module_padded_to(2 << 20));
            std::fs::write(dir.join("demo.wasm"), &v1).unwrap();
            let pins = Arc::new(Mutex::new(vec![pin("demo", &v2), pin("demo", &v1)]));
            let (base, _rt) = spawn(&dir, pins).await;
            assert!(post(&base, "demo", v2).await.status().is_success());
            assert_eq!(dir_names(&dir), ["demo.wasm"], "v1 is not the first pin");
        }

        /// #248: a failed upload leaves no `.keep` / `.tmp` file behind.
        #[tokio::test]
        async fn failed_upload_leaves_no_temp_files() {
            let dir = scratch("upload-fails");
            std::fs::write(dir.join("demo.wasm"), module_padded_to(1 << 20)).unwrap();
            // A directory where `.prev` belongs makes the rename onto it fail
            // after the current file was linked to its `.keep` name.
            std::fs::create_dir(dir.join(".demo.wasm.prev")).unwrap();
            let (base, _rt) = spawn(&dir, no_pins()).await;
            let r = post(&base, "demo", module_padded_to(2 << 20)).await;
            assert_eq!(r.status(), reqwest::StatusCode::INTERNAL_SERVER_ERROR);
            assert_eq!(dir_names(&dir), [".demo.wasm.prev", "demo.wasm"]);
            assert_eq!(len_of(&dir, "demo.wasm"), 1 << 20, "current untouched");
        }

        /// A current file that cannot be removed leaves `.prev` alone, so the
        /// failed delete loses nothing.
        #[tokio::test]
        async fn failed_delete_keeps_prev() {
            let dir = scratch("delete-fails");
            // A directory where the module belongs makes `remove_file` fail.
            std::fs::create_dir(dir.join("demo.wasm")).unwrap();
            std::fs::write(dir.join(".demo.wasm.prev"), module()).unwrap();
            let (base, _rt) = spawn(&dir, no_pins()).await;
            let r = reqwest::Client::new()
                .delete(format!("{base}/admin/sniffers/demo"))
                .send()
                .await
                .unwrap();
            assert_eq!(r.status(), reqwest::StatusCode::INTERNAL_SERVER_ERROR);
            assert!(dir.join(".demo.wasm.prev").is_file());
        }

        /// #249: deleting a module whose current file is already gone still
        /// removes its orphaned `.prev`.
        #[tokio::test]
        async fn delete_removes_an_orphaned_prev() {
            let dir = scratch("delete-orphan");
            std::fs::write(dir.join(".demo.wasm.prev"), module()).unwrap();
            let (base, _rt) = spawn(&dir, no_pins()).await;
            let del = |name: &'static str| {
                reqwest::Client::new()
                    .delete(format!("{base}/admin/sniffers/{name}"))
                    .send()
            };
            assert!(del("demo").await.unwrap().status().is_success());
            assert!(dir_is_empty(&dir));
            assert_eq!(
                del("demo").await.unwrap().status(),
                reqwest::StatusCode::NOT_FOUND,
                "neither file existed"
            );
        }
    }

    /// The aggregator's fan-out carries the protocol header: an unknown major
    /// is refused with both versions named, while `curl` (no header) still works.
    #[tokio::test]
    async fn admin_routes_refuse_another_protocol_major_but_accept_no_header() {
        let proxy = free_port();
        let yaml = format!(
            "pools:\n  - name: p\n    targets: [\"127.0.0.1:9\"]\n    health_check:\n      type: none\n\
             listeners:\n  - name: l\n    bind: \"{proxy}\"\n    pool: p\n"
        );
        let (base, runtime) = spawn_admin(&yaml).await;
        let http = reqwest::Client::new();
        let get = |header: Option<&'static str>| {
            let mut r = http.get(format!("{base}/pools"));
            if let Some(h) = header {
                r = r.header(wayhouse_http::protocol::HEADER, h);
            }
            r.send()
        };
        assert!(get(None).await.unwrap().status().is_success());
        assert!(get(Some("1.4")).await.unwrap().status().is_success());
        let refused = get(Some("2.0")).await.unwrap();
        assert_eq!(refused.status(), reqwest::StatusCode::UPGRADE_REQUIRED);
        let body = refused.text().await.unwrap();
        let current = format!("({})", wayhouse_http::protocol::ProtocolVersion::CURRENT);
        assert!(body.contains("2.0") && body.contains(&current), "{body}");
        // Liveness is never gated.
        let health = http
            .get(format!("{base}/healthz"))
            .header(wayhouse_http::protocol::HEADER, "2.0")
            .send()
            .await
            .unwrap();
        assert!(health.status().is_success());
        runtime
            .shutdown_with_grace(std::time::Duration::from_millis(100))
            .await;
    }

    #[tokio::test]
    async fn a_tokenless_request_with_another_protocol_major_is_a_401_not_a_426() {
        let proxy = free_port();
        let yaml = format!(
            "pools:\n  - name: p\n    targets: [\"127.0.0.1:9\"]\n    health_check:\n      type: none\n\
             listeners:\n  - name: l\n    bind: \"{proxy}\"\n    pool: p\n"
        );
        let (base, runtime) = spawn_admin_with_token(&yaml, Some("tok")).await;
        // The protocol gate counts mismatches into the fleet view, so only an
        // authenticated caller may reach it.
        let resp = reqwest::Client::new()
            .get(format!("{base}/pools"))
            .header(wayhouse_http::protocol::HEADER, "2.0")
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED);
        runtime
            .shutdown_with_grace(std::time::Duration::from_millis(100))
            .await;
    }

    #[tokio::test]
    #[allow(clippy::items_after_statements)] // the alias sits next to its only users
    async fn admin_write_flags_refuse_their_writes_and_never_lock_out_reads_drain_or_the_flags() {
        use reqwest::Method as M;
        let yaml = "schema_version: 4\npools:\n  - name: p\n    targets: [\"127.0.0.1:1\"]\n\
                    listeners:\n  - name: l\n    bind: \"127.0.0.1:0\"\n    routes:\n\
                    \x20     - match: { type: always }\n        action: { pool: p }\n";
        let (base, runtime) = spawn_admin_with_token(yaml, Some("tok")).await;
        let http = reqwest::Client::new();
        let call = |m: reqwest::Method, path: &str, body: Option<serde_json::Value>| {
            let mut req = http.request(m, format!("{base}{path}")).bearer_auth("tok");
            if let Some(b) = body {
                req = req.json(&b);
            }
            async move { req.send().await.unwrap().status().as_u16() }
        };
        let writes = [
            (
                M::POST,
                "/pools/p/backends",
                Some(serde_json::json!({"addr": "127.0.0.1:2"})),
            ),
            (
                M::PATCH,
                "/pools/p/backends/127.0.0.1:1",
                Some(serde_json::json!({})),
            ),
            (M::DELETE, "/pools/p/backends/127.0.0.1:1", None),
            (M::POST, "/route-hint", Some(serde_json::json!({}))),
            (M::POST, "/admin/sniffers", None),
            (M::DELETE, "/admin/sniffers/x", None),
            (M::POST, "/admin/sniffers/x/rollback", None),
        ];
        // On, none of them is the kill switch's 403.
        for (m, path, body) in &writes {
            assert_ne!(
                call(m.clone(), path, body.clone()).await,
                403,
                "{m} {path} on"
            );
        }
        let _off = wayhouse_config::features::off_here(&[
            "admin_write.backends",
            "admin_write.sniffer_upload",
            "admin_write.route_hint",
        ]);
        for (m, path, body) in &writes {
            assert_eq!(
                call(m.clone(), path, body.clone()).await,
                403,
                "{m} {path} off"
            );
        }
        for (m, path) in [
            (M::GET, "/pools"),
            (M::GET, "/config"),
            (M::GET, "/sessions"),
            (M::GET, "/features"),
            (M::GET, "/admin/sniffers"),
            (M::POST, "/admin/drain"),
            (M::POST, "/admin/undrain"),
        ] {
            assert_ne!(
                call(m.clone(), path, None).await,
                403,
                "{m} {path} stays open"
            );
        }
        runtime
            .shutdown_with_grace(std::time::Duration::from_millis(100))
            .await;
    }

    #[tokio::test]
    async fn the_config_schema_and_validate_endpoints_answer_like_the_controllers() {
        let yaml = "pools:\n  - name: p\n    targets: [\"127.0.0.1:1\"]\n\
                    listeners:\n  - name: l\n    bind: \"127.0.0.1:0\"\n    pool: p\n";
        let (base, _runtime) = spawn_admin_with_token(yaml, Some("tok")).await;
        let http = reqwest::Client::new();
        // Behind the bearer like the rest of the gated API.
        let r = http
            .get(format!("{base}/config/schema"))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), reqwest::StatusCode::UNAUTHORIZED);
        let r = http
            .get(format!("{base}/config/schema"))
            .bearer_auth("tok")
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), reqwest::StatusCode::OK);
        assert_eq!(
            r.headers()["x-config-schema-version"],
            wayhouse_config::version::CONFIG_SCHEMA_VERSION
                .to_string()
                .as_str()
        );
        let schema: serde_json::Value = r.json().await.unwrap();
        assert!(schema.is_object());

        let validate = |body: String| {
            http.post(format!("{base}/config/validate"))
                .bearer_auth("tok")
                .body(body)
                .send()
        };
        let ok = validate(format!("schema_version: 4\n{yaml}"))
            .await
            .unwrap();
        assert_eq!(ok.status(), reqwest::StatusCode::OK);
        let v: serde_json::Value = ok.json().await.unwrap();
        assert_eq!(v, serde_json::json!({"ok": true, "schema_version": 4}));
        let bad = validate("listeners: nope".into()).await.unwrap();
        assert_eq!(bad.status(), reqwest::StatusCode::UNPROCESSABLE_ENTITY);
        let v: serde_json::Value = bad.json().await.unwrap();
        assert!(v["error"].as_str().is_some_and(|e| !e.is_empty()), "{v}");
    }

    const FILE_YAML: &str = "settings:\n  admin:\n    auth_token: \"0123456789abcdef0123\"\npools:\n  - name: p\n    targets: [\"127.0.0.1:1\"]\n\
                             listeners:\n  - name: l\n    bind: \"127.0.0.1:0\"\n    pool: p\n";

    async fn spawn_with_file(allow_write: bool) -> (String, Runtime, tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wayhouse.yaml");
        std::fs::write(&path, FILE_YAML).unwrap();
        let (base, runtime) = spawn_admin_config(
            FILE_YAML,
            Some("tok"),
            None,
            Some(ConfigFile::new(path.clone(), allow_write)),
        )
        .await;
        (base, runtime, dir, path)
    }

    #[tokio::test]
    async fn the_config_file_is_readable_with_an_etag_and_a_controller_proxy_has_none() {
        let (base, _runtime, _dir, _path) = spawn_with_file(false).await;
        let http = reqwest::Client::new();
        let r = http
            .get(format!("{base}/config/file"))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), reqwest::StatusCode::UNAUTHORIZED);
        let r = http
            .get(format!("{base}/config/file"))
            .bearer_auth("tok")
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), reqwest::StatusCode::OK);
        assert_eq!(
            r.headers()["etag"],
            crate::config_file::etag(FILE_YAML.as_bytes()).as_str()
        );
        // Inline secrets are masked in the body; the tags describe the real file.
        let body = r.text().await.unwrap();
        assert!(!body.contains("0123456789abcdef0123"), "{body}");
        assert_eq!(body, crate::config_file::redact(FILE_YAML).unwrap());
        // No earlier version yet.
        let r = http
            .get(format!("{base}/config/file?previous=1"))
            .bearer_auth("tok")
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), reqwest::StatusCode::NOT_FOUND);
        // A proxy with no config file (it follows a controller) answers 404.
        let (base, _runtime) = spawn_admin_with_token(FILE_YAML, Some("tok")).await;
        let r = http
            .get(format!("{base}/config/file"))
            .bearer_auth("tok")
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), reqwest::StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn the_config_file_cannot_be_written_unless_the_proxy_allows_it() {
        let (base, _runtime, _dir, path) = spawn_with_file(false).await;
        let http = reqwest::Client::new();
        let r = http
            .put(format!("{base}/config/file"))
            .bearer_auth("tok")
            .header("If-Match", crate::config_file::etag(FILE_YAML.as_bytes()))
            .body(FILE_YAML.replace("127.0.0.1:1", "127.0.0.1:2"))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), reqwest::StatusCode::FORBIDDEN);
        assert_eq!(std::fs::read_to_string(path).unwrap(), FILE_YAML);
    }

    #[tokio::test]
    async fn a_config_file_write_needs_the_etag_is_checked_and_keeps_the_previous_file() {
        let (base, _runtime, _dir, path) = spawn_with_file(true).await;
        let http = reqwest::Client::new();
        let put = |tag: Option<String>, body: String| {
            let mut req = http
                .put(format!("{base}/config/file"))
                .bearer_auth("tok")
                .body(body);
            if let Some(tag) = tag {
                req = req.header("If-Match", tag);
            }
            req.send()
        };
        let newer = FILE_YAML.replace("127.0.0.1:1", "127.0.0.1:2");
        // No If-Match: refused before anything is read.
        assert_eq!(
            put(None, newer.clone()).await.unwrap().status(),
            reqwest::StatusCode::PRECONDITION_REQUIRED
        );
        // A config this build refuses.
        let tag = crate::config_file::etag(FILE_YAML.as_bytes());
        let r = put(Some(tag.clone()), "listeners: nope".into())
            .await
            .unwrap();
        assert_eq!(r.status(), reqwest::StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), FILE_YAML);
        // The real write.
        let r = put(Some(tag.clone()), newer.clone()).await.unwrap();
        assert_eq!(r.status(), reqwest::StatusCode::OK);
        let v: serde_json::Value = r.json().await.unwrap();
        assert_eq!(
            v["etag"],
            crate::config_file::etag(newer.as_bytes()).as_str()
        );
        // No reload task runs in this test, so the save cannot be confirmed applied, and says so.
        assert_eq!(v["saved"], true);
        assert_eq!(v["applied"], false);
        assert!(v["note"].as_str().unwrap().contains("previous=1"), "{v}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), newer);
        // The old tag is stale now, with the code the UI looks for.
        let r = put(Some(tag), FILE_YAML.into()).await.unwrap();
        assert_eq!(r.status(), reqwest::StatusCode::CONFLICT);
        let v: serde_json::Value = r.json().await.unwrap();
        assert_eq!(v["code"], "stale");
        // The web editor names the version by its revision number instead.
        let third = newer.replace("127.0.0.1:2", "127.0.0.1:4");
        let rev = crate::config_file::revision(newer.as_bytes());
        let r = http
            .put(format!("{base}/config/file?base={}", rev + 1))
            .bearer_auth("tok")
            .body(third.clone())
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), reqwest::StatusCode::CONFLICT);
        let r = http
            .put(format!("{base}/config/file?base={rev}"))
            .bearer_auth("tok")
            .body(third.clone())
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), reqwest::StatusCode::OK);
        let v: serde_json::Value = r.json().await.unwrap();
        assert_eq!(
            v["revision"],
            crate::config_file::revision(third.as_bytes())
        );
        // The previous version can be read back (now the second one).
        let r = http
            .get(format!("{base}/config/file?previous=1"))
            .bearer_auth("tok")
            .send()
            .await
            .unwrap();
        assert_eq!(
            r.text().await.unwrap(),
            crate::config_file::redact(&newer).unwrap()
        );
    }

    #[tokio::test]
    async fn features_lists_the_proxy_flags_with_their_source() {
        let yaml = "schema_version: 4\npools:\n  - name: p\n    targets: [\"127.0.0.1:1\"]\n\
                    listeners:\n  - name: l\n    bind: \"127.0.0.1:0\"\n    routes:\n\
                    \x20     - match: { type: always }\n        action: { pool: p }\n\
                    features:\n  port_routes: false\n";
        let (base, runtime) = spawn_admin_with_token(yaml, Some("tok")).await;
        let http = reqwest::Client::new();
        let r = http.get(format!("{base}/features")).send().await.unwrap();
        assert_eq!(r.status(), reqwest::StatusCode::UNAUTHORIZED);
        let v: serde_json::Value = http
            .get(format!("{base}/features"))
            .bearer_auth("tok")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let flag = |n: &str| {
            v["flags"]
                .as_array()
                .unwrap()
                .iter()
                .find(|f| f["name"] == n)
                .unwrap_or_else(|| panic!("{n}"))
                .clone()
        };
        assert_eq!(flag("port_routes")["value"], false);
        assert_eq!(flag("port_routes")["source"], "config");
        assert_eq!(flag("admin_write.backends")["source"], "default");
        assert_eq!(flag("admin_write.backends")["available"], true);
        assert!(v["flags"]
            .as_array()
            .unwrap()
            .iter()
            .all(|f| f["scope"] == "proxy"));
        runtime
            .shutdown_with_grace(Duration::from_millis(100))
            .await;
    }
}
