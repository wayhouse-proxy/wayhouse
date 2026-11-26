//! `POST /ingest` (slice 6) — a proxy pushes its own [`IngestPayload`]
//! periodically; the aggregator stores latest-write-wins per `instance`.
//!
//! `GET /fleet/pools` / `/fleet/sessions` / `/fleet/healthz` (slice 8) serve
//! a merged fleet view straight off the same [`IngestStore`] — no fan-out
//! call to any instance, the data already arrived. Each entry carries
//! `last_seen_ms_ago` (the aggregator's own clock, not the proxy's — see
//! [`crate::ingest`]'s note on why); `/fleet/healthz` additionally flags an
//! instance `stale` past [`STALE_AFTER_MS`], which is where "stale" first
//! gets an actual threshold (`IngestStore` itself has none).
//!
//! **No `GET /fleet/config`** in this release: `IngestPayload` carries
//! structural/health *state*, not config *content* — the controller (docs/10
//! "The controller") is the one authoritative source for config, via its own
//! `GET /config`/`/config/revisions`. A fleet-wide "which revision is each
//! instance actually running" view is a real, useful, and still-open
//! addition (an optional `config_revision` field on `IngestPayload`,
//! self-reported by `wayhouse` when in `--controller` mode) — deferred because it
//! needs `controller_client` and `aggregator_client` to share state inside
//! `wayhouse` that today are deliberately independent ("unrelated axes", slice
//! 7's doc comment), not because it doesn't matter.
//!
//! `GET /fleet/subscribe` (slice 11a) is the machine-to-machine feed
//! `wayhouse-ui` subscribes to for live updates: current merged fleet state (the
//! same shape as combining `/fleet/pools` + `/fleet/sessions` + a `stale`
//! flag) on connect, then again on every accepted `POST /ingest` — debounced
//! against a burst of pushes landing close together. Reuses the exact
//! catch-up-then-broadcast SSE shape `wayhouse-controller`'s `/config/subscribe`
//! already proved out, applied to *state* instead of a revision log, so
//! there's no cursor/replay semantics to get right: a lagged subscriber just
//! gets the *current* view on its next tick, same as a fresh connect would.

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{ConnectInfo, DefaultBodyLimit, Extension, Path, Query, RawQuery, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;
use tokio::sync::{broadcast, mpsc};
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::{Stream, StreamExt};

use crate::ingest::{IngestPayload, IngestStore, PoolSummary, SessionCounts};
use crate::nodes;
use crate::skew::{classify, Reported, Skew};
use crate::trust::AdminUrlPolicy;
use crate::util::unix_ms;

/// How long since an instance's last push before `/fleet/healthz` calls it
/// `stale` — roughly 3x `wayhouse`'s default `--aggregator-interval-sec` (10s),
/// generous enough to absorb one missed tick without false-flagging a
/// perfectly healthy instance.
pub(crate) const STALE_AFTER_MS: u64 = 30_000;

/// How long `/fleet/subscribe` waits after a signal before rebuilding and
/// sending the fleet view, draining any further signals that land in that
/// window — coalesces a burst of near-simultaneous pushes (a whole fleet's
/// push interval lining up) into one rebuild instead of one per instance.
const SUBSCRIBE_DEBOUNCE: Duration = Duration::from_millis(150);

/// Capacity of the ingest-update broadcast — generous for a control plane
/// (pushes are seconds apart, not a hot path).
const UPDATES_CAPACITY: usize = 64;

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<IngestStore>,
    /// Shared client for `crate::fanout`'s calls out to each instance's own
    /// admin API. `reqwest::Client` is cheap to clone (an `Arc` internally)
    /// and reuses connections, so one lives on `AppState` rather than being
    /// built per call.
    pub http: reqwest::Client,
    /// Bearer token every request except `GET /healthz` must present
    /// (`--auth-token`); `None` leaves this aggregator's own API open.
    pub auth_token: Option<String>,
    /// Bearer token `POST /ingest` requires (`--ingest-token`), so the proxies
    /// that push telemetry do not hold a credential that also unlocks
    /// `/fleet/*`. `main` insists it is set whenever `auth_token` is; `None`
    /// leaves `/ingest` open (the all-open, loopback case).
    pub ingest_token: Option<String>,
    /// Which `admin_url`s an ingested payload may carry (see [`crate::trust`]).
    pub admin_url_policy: AdminUrlPolicy,
    /// Bearer token `crate::fanout` presents to every instance's admin API
    /// (`--instance-token`) — a separate secret from `auth_token`: one gates
    /// calls *into* this aggregator, the other is what this aggregator
    /// presents *out* to instances that require `settings.admin.auth_token`.
    pub instance_token: Option<String>,
    /// Most bytes of one instance's reply the fan-out buffers; more is a 502.
    pub max_instance_response_bytes: usize,
    /// Fires (with no payload — subscribers rebuild the view themselves from
    /// `store`) on every accepted `POST /ingest`, for `/fleet/subscribe`.
    pub updates: broadcast::Sender<()>,
}

/// Fan-out client deadlines, and the most an instance's reply may weigh.
const FANOUT_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
const FANOUT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
const MAX_INSTANCE_RESPONSE_BYTES: usize = 8 << 20;

impl AppState {
    pub fn new(store: Arc<IngestStore>) -> Self {
        let (updates, _rx) = broadcast::channel(UPDATES_CAPACITY);
        AppState {
            store,
            // No redirects: a followed 3xx would carry the instance token to
            // wherever an instance's admin API points it.
            http: wayhouse_http::builder()
                .redirect(reqwest::redirect::Policy::none())
                // An instance that never answers (or never ends) must not hang a
                // broadcast verb: `join` waits for every instance.
                .connect_timeout(FANOUT_CONNECT_TIMEOUT)
                .timeout(FANOUT_TIMEOUT)
                .build()
                .expect("the extra roots were validated by init_ca_file, so the client builds"),
            auth_token: None,
            ingest_token: None,
            admin_url_policy: AdminUrlPolicy::default(),
            instance_token: None,
            max_instance_response_bytes: MAX_INSTANCE_RESPONSE_BYTES,
            updates,
        }
    }

    pub fn with_max_instance_response_bytes(mut self, max: usize) -> Self {
        self.max_instance_response_bytes = max;
        self
    }

    pub fn with_auth_token(mut self, auth_token: Option<String>) -> Self {
        self.auth_token = auth_token;
        self
    }

    pub fn with_ingest_token(mut self, ingest_token: Option<String>) -> Self {
        self.ingest_token = ingest_token;
        self
    }

    pub fn with_admin_url_policy(mut self, policy: AdminUrlPolicy) -> Self {
        self.admin_url_policy = policy;
        self
    }

    pub fn with_instance_token(mut self, instance_token: Option<String>) -> Self {
        self.instance_token = instance_token;
        self
    }
}

#[utoipa::path(
    get, path = "/openapi.json", tag = "Service",
    summary = "This document",
    description = "The OpenAPI description of this API, as JSON.",
    responses((status = 200, description = "The OpenAPI document.", body = serde_json::Value, content_type = "application/json"))
)]
async fn openapi_spec() -> Json<serde_json::Value> {
    Json(crate::openapi::openapi_json())
}

/// The OpenAPI paths of the routes in this module.
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(ingest, fleet_pools, fleet_node_pools, fleet_sessions, fleet_healthz, fleet_features, fleet_stats,
        fleet_nodes, fleet_nodes_with_tags, resolve_nodes, subscribe_fleet, openapi_spec),
    components(schemas(
        crate::ingest::FeatureReport, crate::ingest::ListenerStats, crate::ingest::BackendSummary,
        crate::ingest::SessionCounts, crate::nodes::Node, crate::nodes::NodeState, crate::nodes::Rollup,
        crate::nodes::Group, crate::skew::Skew, crate::history::Series<crate::history::FleetPoint>,
        crate::history::Named<crate::history::ListenerPoint>, crate::history::Named<crate::history::PoolPoint>,
        crate::history::Named<crate::history::RegionPoint>, crate::features::FlagRollup,
    )),
    tags(
        (name = "Ingest", description = "Where a proxy reports its state."),
        (name = "Fleet views", description = "What the fleet looks like, read from what the proxies pushed."),
        (name = "Node list", description = "The filtered, sorted and paged node list."),
        (name = "Service", description = "Liveness, metrics and this document."),
    ),
)]
pub(crate) struct ApiDoc;

pub fn router(state: AppState) -> Router {
    // Two bearer layers: `/ingest` accepts only `--ingest-token`, everything
    // under `/fleet` only `--auth-token`.
    // Only `/ingest` (a proxy pushing) is component-facing and gated on the
    // protocol header; `/fleet` serves the UI and operators.
    // The bearer check is the outer layer: an unauthenticated request must not
    // reach the protocol gate, which counts mismatches into the fleet view.
    let ingest =
        wayhouse_http::protocol::gate(Router::new().route("/ingest", post(ingest)), "aggregator")
            .route_layer(axum::middleware::from_fn_with_state(
                wayhouse_http::server::BearerAuth::new(state.ingest_token.as_deref()),
                wayhouse_http::server::require_bearer,
            ));
    let fleet = Router::new()
        .route("/fleet/pools", get(fleet_pools))
        .route("/fleet/sessions", get(fleet_sessions))
        .route("/fleet/healthz", get(fleet_healthz))
        .route("/fleet/stats", get(fleet_stats))
        .route("/fleet/nodes", get(fleet_nodes))
        .route("/fleet/nodes/{instance}/pools", get(fleet_node_pools))
        .route("/fleet/features", get(fleet_features))
        .route(
            "/fleet/nodes/query",
            post(fleet_nodes_with_tags).layer(DefaultBodyLimit::max(NODE_TAGS_BODY_LIMIT)),
        )
        .route(
            "/fleet/nodes/resolve",
            post(resolve_nodes).layer(DefaultBodyLimit::max(NODE_TAGS_BODY_LIMIT)),
        )
        .route("/fleet/subscribe", get(subscribe_fleet))
        .route("/openapi.json", get(openapi_spec))
        .merge(crate::fanout::router())
        .route_layer(axum::middleware::from_fn_with_state(
            wayhouse_http::server::BearerAuth::new(state.auth_token.as_deref()),
            wayhouse_http::server::require_bearer,
        ));
    ingest.merge(fleet).with_state(state)
}

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
}

/// `POST /ingest` — body is one [`IngestPayload`] as JSON. `instance` must
/// be non-empty (it's the key everything is stored and overwritten under);
/// anything else in the payload is accepted as-is, this is a summary a proxy
/// self-reports, not something the aggregator validates against reality —
/// except `admin_url`, which later receives the instance token and so must pass
/// the [`AdminUrlPolicy`].
#[utoipa::path(
    post, path = "/ingest", tag = "Ingest",
    summary = "Push an instance's state",
    description = "A proxy reports a summary of itself: its pools and backends, session counts, listener counters and feature flags. \
The aggregator keeps the newest push per `instance` and overwrites it on the next one, in memory only. Proxies push every \
`--aggregator-interval-sec` seconds (default 10), and the aggregator calls an instance `stale` after 30 seconds of silence.\n\n\
This route takes the **ingest token** (`--ingest-token`), not the admin token, and is gated on the `x-wayhouse-protocol` header: \
a caller on another protocol major, or with no header, gets `426`. The `admin_url` is checked against `--instance-url-allow`: \
the aggregator later sends the instance token to that URL, so a push may not name an arbitrary host.",
    security(("ingest" = [])),
    params(("x-wayhouse-protocol" = String, Header, description = "The caller's wire protocol, `major.minor`.", example = "1.3")),
    request_body(content = IngestPayload, content_type = "application/json", description = "The instance's state."),
    responses(
        (status = 200, description = "Stored."),
        (status = 400, description = "The instance name is empty, `admin_url` is not allowed, a field or list is over its limit, or the body is not JSON. (A body that is not JSON at all is answered with plain text.)", body = crate::openapi::ErrorBody),
        (status = 401, description = "No bearer token, or the wrong one. Body: `unauthorized`.", body = String, content_type = "text/plain"),
        (status = 422, description = "The body is JSON but does not have the payload's shape.", body = String, content_type = "text/plain"),
        (status = 426, description = "The caller's protocol major differs, or the header is missing or unparsable. The body names both versions.", body = String, content_type = "text/plain"),
        (status = 503, description = "The store holds the most live instances it will take (10,000); this push is refused.", body = crate::openapi::ErrorBody),
    )
)]
async fn ingest(
    State(state): State<AppState>,
    peer: Option<Extension<ConnectInfo<wayhouse_http::tls::PeerAddr>>>,
    Json(payload): Json<IngestPayload>,
) -> Response {
    if payload.instance.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "instance must not be empty".into(),
            }),
        )
            .into_response();
    }
    let peer_ip = peer.map(|Extension(ConnectInfo(p))| p.0.ip());
    if let Err(why) = state.admin_url_policy.check(&payload.admin_url, peer_ip) {
        tracing::warn!(instance = %payload.instance, ?peer_ip, admin_url = %payload.admin_url, %why, "rejected an ingest push");
        return (StatusCode::BAD_REQUEST, Json(ErrorResponse { error: why })).into_response();
    }
    if let Err(why) = payload.check_bounds() {
        return (StatusCode::BAD_REQUEST, Json(ErrorResponse { error: why })).into_response();
    }
    let instance = payload.instance.clone();
    if state.store.try_ingest(payload).is_err() {
        tracing::warn!(
            instance,
            "ingest store is full of live instances, push refused"
        );
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ErrorResponse {
                error: "too many instances".into(),
            }),
        )
            .into_response();
    }
    // No subscribers connected right now is not an error — the state is
    // durably held in `store` regardless; a subscriber that connects later
    // just gets it in its initial view.
    let _ = state.updates.send(());
    tracing::debug!(instance, "ingested a push from a proxy instance");
    StatusCode::OK.into_response()
}

/// One instance's pools and backends.
#[derive(Serialize, utoipa::ToSchema)]
struct FleetPools {
    /// The instance's name.
    #[schema(example = "edge-fra-1")]
    instance: String,
    /// Milliseconds since its last push, by the aggregator's own clock.
    last_seen_ms_ago: u64,
    /// Its pools.
    pools: Vec<PoolSummary>,
    /// Its fleet group path, when it reports one.
    #[serde(skip_serializing_if = "Option::is_none")]
    group: Option<String>,
}

/// `GET /fleet/pools` — every known instance's pool/backend summary, oldest-
/// pushed-first is not the order here; [`IngestStore::snapshot`] already
/// sorts by instance name for a stable table.
#[utoipa::path(
    get, path = "/fleet/pools", tag = "Fleet views",
    summary = "Pools and backends of every instance",
    description = "Every known instance's pools and backends, as of its last push, sorted by instance name. Nothing is fetched from the instances: \
the data already arrived with their pushes. `last_seen_ms_ago` is on the aggregator's own clock.",
    responses(
        (status = 200, description = "One entry per instance.", body = Vec<FleetPools>),
    )
)]
async fn fleet_pools(State(state): State<AppState>) -> Response {
    let now = unix_ms();
    let out: Vec<FleetPools> = state
        .store
        .snapshot()
        .into_iter()
        .map(|s| FleetPools {
            instance: s.payload.instance,
            last_seen_ms_ago: now.saturating_sub(s.received_at_ms),
            pools: s.payload.pools,
            group: s.payload.group,
        })
        .collect();
    (StatusCode::OK, Json(out)).into_response()
}

/// `GET /fleet/nodes/{instance}/pools` — one instance's pools and backend state, so a UI that looks at
/// one node does not have to read the whole fleet. `404` for an instance that never pushed. The name
/// only keys a map lookup; it is never put into a URL.
#[utoipa::path(
    get, path = "/fleet/nodes/{instance}/pools", tag = "Fleet views",
    summary = "Pools and backends of one instance",
    description = "The same shape as one entry of `GET /fleet/pools`, for one instance, so a view of a single node need not read the whole fleet.",
    params(("instance" = String, Path, description = "The instance's name, as it reports it.", example = "edge-fra-1")),
    responses(
        (status = 200, description = "The instance's pools.", body = FleetPools),
        (status = 404, description = "No instance of that name has pushed.", body = crate::openapi::ErrorBody),
    )
)]
async fn fleet_node_pools(State(state): State<AppState>, Path(instance): Path<String>) -> Response {
    match state.store.get(&instance) {
        Some(s) => (
            StatusCode::OK,
            Json(FleetPools {
                last_seen_ms_ago: unix_ms().saturating_sub(s.received_at_ms),
                instance: s.payload.instance,
                pools: s.payload.pools,
                group: s.payload.group,
            }),
        )
            .into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "no such instance".into(),
            }),
        )
            .into_response(),
    }
}

/// One instance's session counts.
#[derive(Serialize, utoipa::ToSchema)]
struct FleetSessions {
    /// The instance's name.
    #[schema(example = "edge-fra-1")]
    instance: String,
    /// Milliseconds since its last push.
    last_seen_ms_ago: u64,
    /// Open TCP sessions.
    tcp: usize,
    /// Open UDP sessions.
    udp: usize,
}

/// `GET /fleet/sessions` — every known instance's session *counts* (not the
/// full live registry — see the module doc's note on `IngestPayload` being a
/// summary; an instance's own `GET /sessions` still has the detail).
#[utoipa::path(
    get, path = "/fleet/sessions", tag = "Fleet views",
    summary = "Session counts of every instance",
    description = "Each instance's open TCP and UDP session counts, as of its last push. These are counts only; an instance's own `GET /sessions` has the detail.",
    responses(
        (status = 200, description = "One entry per instance.", body = Vec<FleetSessions>),
    )
)]
async fn fleet_sessions(State(state): State<AppState>) -> Response {
    let now = unix_ms();
    let out: Vec<FleetSessions> = state
        .store
        .snapshot()
        .into_iter()
        .map(|s| FleetSessions {
            instance: s.payload.instance,
            last_seen_ms_ago: now.saturating_sub(s.received_at_ms),
            tcp: s.payload.sessions.tcp,
            udp: s.payload.sessions.udp,
        })
        .collect();
    (StatusCode::OK, Json(out)).into_response()
}

/// How recently one instance was heard from.
#[derive(Serialize, utoipa::ToSchema)]
struct FleetHealth {
    /// The instance's name.
    #[schema(example = "edge-fra-1")]
    instance: String,
    /// Milliseconds since its last push.
    last_seen_ms_ago: u64,
    /// More than 30 seconds since the last push. Advisory: an operator signal, not a promise
    /// that the instance is down (it could be a slow link to the aggregator).
    stale: bool,
    /// The product version the instance runs; empty from an older build.
    version: String,
}

/// `GET /fleet/healthz` — every known instance's push recency. This is
/// distinct from the aggregator's own `GET /healthz` (its own liveness) and
/// from any one instance's `GET /healthz` (that instance's own liveness) —
/// it answers "when did *we* last hear from each instance."
#[utoipa::path(
    get, path = "/fleet/healthz", tag = "Fleet views",
    summary = "When each instance was last heard from",
    description = "How recent each instance's last push is, and whether that is `stale` (more than 30 seconds). This is not the aggregator's own `GET /healthz`, and not an instance's: it answers \"when did the aggregator last hear from each one\". `stale` is advisory: it can mean a slow link rather than a dead instance.",
    responses(
        (status = 200, description = "One entry per instance.", body = Vec<FleetHealth>),
    )
)]
async fn fleet_healthz(State(state): State<AppState>) -> Response {
    let now = unix_ms();
    let out: Vec<FleetHealth> = state
        .store
        .snapshot()
        .into_iter()
        .map(|s| {
            let age = now.saturating_sub(s.received_at_ms);
            FleetHealth {
                instance: s.payload.instance,
                last_seen_ms_ago: age,
                stale: age > STALE_AFTER_MS,
                version: s.payload.version,
            }
        })
        .collect();
    (StatusCode::OK, Json(out)).into_response()
}

#[derive(serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct StatsQuery {
    /// How far back to look: `5m`, `15m` (the default) or `1h`.
    #[param(example = "15m")]
    range: Option<String>,
}

/// The query of the node list.
#[derive(utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
#[allow(dead_code)] // documentation type: the query is parsed from the raw string
struct NodeQuery {
    /// The page, from 1.
    #[param(minimum = 1, example = 1)]
    page: Option<usize>,
    /// Rows per page, 1 to 200. Default 25.
    #[param(minimum = 1, maximum = 200, example = 25)]
    size: Option<usize>,
    /// The sort key: `instance` (default), `state`, `region`, `sessions` or `version`. Versions sort by their dotted numbers (`0.9.0` before `0.10.0`).
    sort: Option<String>,
    /// `asc` (default) or `desc`.
    dir: Option<String>,
    /// A case-insensitive substring of the instance, region, group, version or a `key=value` tag. At most 128 bytes.
    q: Option<String>,
    /// Only nodes in this state: `ok`, `degraded` or `stale`.
    state: Option<String>,
    /// Only nodes of this region.
    region: Option<String>,
    /// Only nodes that carry this tag: `key`, or `key=value` for an exact value. Repeat the parameter to require several; all must match.
    tag: Option<Vec<String>>,
    /// `region` groups the selection by region: the list is ordered by region first and the answer carries a roll-up per region.
    group_by: Option<String>,
}

/// The filter part of the node-list query.
#[derive(utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
#[allow(dead_code)] // documentation type: the query is parsed from the raw string
struct NodeFilter {
    /// A case-insensitive substring of the instance, region, group, version or a `key=value` tag.
    q: Option<String>,
    /// Only nodes in this state: `ok`, `degraded` or `stale`.
    state: Option<String>,
    /// Only nodes of this region.
    region: Option<String>,
    /// Only nodes that carry this tag: `key` or `key=value`. Repeat to require several.
    tag: Option<Vec<String>>,
}

/// The body of a node query that carries the operator's tags.
#[derive(utoipa::ToSchema)]
#[allow(dead_code)] // documentation type: the body is parsed by `nodes::parse_operator`
struct OperatorTagsBody {
    /// The operator's `key=value` tags per instance name; they win over a tag the instance declared. Unknown top-level keys are refused, and tags for an instance that is not known are ignored.
    #[schema(example = json!({"edge-fra-1": {"owner": "platform"}}))]
    operator_tags: std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>>,
}

/// The instances a filter selects.
#[derive(utoipa::ToSchema)]
#[allow(dead_code)] // documentation type: the handler builds the object inline
struct ResolveAnswer {
    /// How many instances match.
    count: usize,
    /// Their names, ascending.
    instances: Vec<String>,
}

/// `GET /fleet/features`: the per-flag roll-up of what the proxies report ([`crate::features`]).
#[utoipa::path(
    get, path = "/fleet/features",
    tag = "Fleet views",
    summary = "How each feature flag stands across the fleet",
    description = "For each flag the proxies report: how many have it on, off, or lack the code behind it, with the names of the instances running it off or overridden (up to 50 each).",
    responses(
        (status = 200, description = "The roll-up.", body = crate::features::FeatureRollup),
    )
)]
async fn fleet_features(State(state): State<AppState>) -> Response {
    Json(crate::features::rollup(&state.store.snapshot())).into_response()
}

/// `GET /fleet/stats?range=5m|15m|1h` (default `15m`) — fleet-wide rates and
/// gauges per minute from the rolling history ([`crate::history`]); aggregates
/// only, never a per-instance series.
#[utoipa::path(
    get, path = "/fleet/stats", tag = "Fleet views",
    summary = "Rates and gauges over time",
    description = "Fleet-wide rates and gauges per minute, from a rolling history of about an hour that every push feeds. Proxies push cumulative counters; the aggregator turns the differences into rates. \
A counter that went down means the instance restarted. A gap between pushes spreads the increase over the minutes it spans, so a silent instance never shows as a spike. There is no per-node series: only the fleet total and the series per listener, pool and region. \
The history is in memory: a restart of the aggregator starts the charts empty.",
    params(StatsQuery),
    responses(
        (status = 200, description = "The series.", body = crate::history::Stats),
        (status = 400, description = "`range` is not one of the three values.", body = crate::openapi::ErrorBody),
    )
)]
async fn fleet_stats(State(state): State<AppState>, Query(q): Query<StatsQuery>) -> Response {
    let range_sec = match q.range.as_deref() {
        None | Some("15m") => 900,
        Some("5m") => 300,
        Some("1h") => 3600,
        Some(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: "range must be 5m, 15m or 1h".into(),
                }),
            )
                .into_response()
        }
    };
    (
        StatusCode::OK,
        Json(state.store.history().stats(range_sec, unix_ms())),
    )
        .into_response()
}

fn bad_request(error: String) -> Response {
    (StatusCode::BAD_REQUEST, Json(ErrorResponse { error })).into_response()
}

/// `GET /fleet/nodes?page=&size=&sort=&dir=&q=&state=&region=&tag=&group_by=region` — the
/// filtered, sorted and paged node list with rollups ([`crate::nodes`]).
#[utoipa::path(
    get, path = "/fleet/nodes", tag = "Node list",
    summary = "The node list",
    description = "Every node, filtered, sorted and paged on the server so a large fleet never travels to the browser whole. A node is `stale` after 30 seconds without a push, `degraded` when it pushes and at least one backend is unhealthy, and `ok` otherwise. \
Region and tags are what an instance reports about itself (`--region`, `--tag key=value`); the aggregator stores nothing of its own. Query keys it does not know are ignored.",
    params(NodeQuery),
    responses(
        (status = 200, description = "One page, with a roll-up over everything the filter selects.", body = crate::nodes::Page),
        (status = 400, description = "A query value is invalid.", body = crate::openapi::ErrorBody),
    )
)]
async fn fleet_nodes(State(state): State<AppState>, RawQuery(query): RawQuery) -> Response {
    node_page(&state, query.as_deref(), &[])
}

/// Largest body of a query that carries operator tags: about 1000 nodes of 16 tags each.
const NODE_TAGS_BODY_LIMIT: usize = 4 << 20;

/// `POST /fleet/nodes/query?<same query as the list>` with `{"operator_tags": {...}}`: the list
/// with the operator's tags laid over what the instances declared (see [`nodes::parse_operator`]).
/// A POST only because the tags do not fit a URL.
#[utoipa::path(
    post, path = "/fleet/nodes/query", tag = "Node list",
    summary = "The node list with operator tags",
    description = "The same list as `GET /fleet/nodes`, with the operator's tags laid over the tags the instances declared. Operator tags live in the controller; the console sends them with each query, and they win over a declared tag of the same key. It is a `POST` only because the tags do not fit a URL. Nothing is changed. The body may be empty, which means no operator tags; it is at most 4 MiB.",
    params(NodeQuery),
    request_body(content = OperatorTagsBody, content_type = "application/json", description = "Operator tags by instance. May be empty."),
    responses(
        (status = 200, description = "One page, as for `GET /fleet/nodes`.", body = crate::nodes::Page),
        (status = 400, description = "A query value or the body is invalid (an unknown key, or a tag that breaks the label rules).", body = crate::openapi::ErrorBody),
        (status = 413, description = "The body is over 4 MiB."),
    )
)]
async fn fleet_nodes_with_tags(
    State(state): State<AppState>,
    RawQuery(query): RawQuery,
    body: Bytes,
) -> Response {
    node_page(&state, query.as_deref(), &body)
}

fn node_page(state: &AppState, query: Option<&str>, body: &[u8]) -> Response {
    let q = match nodes::Query::parse(query.unwrap_or_default()) {
        Ok(q) => q,
        Err(e) => return bad_request(e),
    };
    let mut all = nodes::nodes(&state.store.snapshot(), unix_ms(), STALE_AFTER_MS);
    match nodes::parse_operator(body) {
        Ok(op) => nodes::overlay(&mut all, &op),
        Err(e) => return bad_request(e),
    }
    (StatusCode::OK, Json(nodes::page(all, &q))).into_response()
}

/// `POST /fleet/nodes/resolve?<filter>` — the instances a filter selects and how many, for a bulk
/// action to show the count before the operator confirms. Takes the query a list view sent
/// (paging keys are ignored) and, like [`fleet_nodes_with_tags`], the operator's tags; reads,
/// changes nothing.
#[utoipa::path(
    post, path = "/fleet/nodes/resolve", tag = "Node list",
    summary = "Which instances a filter selects",
    description = "The instances a filter selects and how many, so a bulk action can show the count before the operator confirms. It takes the query a list view sent (paging keys are ignored) and, like `POST /fleet/nodes/query`, the operator's tags in the body. Nothing is changed.",
    params(NodeFilter),
    request_body(content = OperatorTagsBody, content_type = "application/json", description = "Operator tags by instance. May be empty."),
    responses(
        (status = 200, description = "The selection.", body = ResolveAnswer),
        (status = 400, description = "A query value or the body is invalid.", body = crate::openapi::ErrorBody),
        (status = 413, description = "The body is over 4 MiB."),
    )
)]
async fn resolve_nodes(
    State(state): State<AppState>,
    RawQuery(query): RawQuery,
    body: Bytes,
) -> Response {
    let filter = match nodes::Filter::parse(query.as_deref().unwrap_or_default()) {
        Ok(f) => f,
        Err(e) => return bad_request(e),
    };
    let mut all = nodes::nodes(&state.store.snapshot(), unix_ms(), STALE_AFTER_MS);
    match nodes::parse_operator(&body) {
        Ok(op) => nodes::overlay(&mut all, &op),
        Err(e) => return bad_request(e),
    }
    let instances: Vec<String> = nodes::select(all, &filter)
        .into_iter()
        .map(|n| n.instance)
        .collect();
    Json(serde_json::json!({ "count": instances.len(), "instances": instances })).into_response()
}

/// One instance in the live fleet view.
#[derive(Serialize, utoipa::ToSchema)]
struct FleetInstanceView {
    /// The instance's name.
    #[schema(example = "edge-fra-1")]
    instance: String,
    /// Milliseconds since its last push.
    last_seen_ms_ago: u64,
    /// More than 30 seconds since the last push.
    stale: bool,
    /// Its pools.
    pools: Vec<PoolSummary>,
    /// Its session counts.
    sessions: SessionCounts,
    /// Its fleet group path, when it reports one.
    #[serde(skip_serializing_if = "Option::is_none")]
    group: Option<String>,
    /// The product version it runs; empty from an older build.
    version: String,
    /// The wire protocol `major.minor` it speaks; empty from an older build.
    protocol: String,
    /// The controller revision the instance runs, if it reports one.
    #[serde(skip_serializing_if = "Option::is_none")]
    config_revision: Option<u64>,
    /// How its version compares with the newest in the fleet.
    skew: Skew,
}

/// The merged view `/fleet/subscribe` sends — everything the three plain
/// `GET /fleet/*` endpoints report, combined into one payload so `wayhouse-ui`
/// doesn't need three separate subscriptions.
fn fleet_view(store: &IngestStore) -> Vec<FleetInstanceView> {
    let now = unix_ms();
    let states = store.snapshot();
    let skews = classify(
        &states
            .iter()
            .map(|s| Reported {
                version: &s.payload.version,
                protocol: &s.payload.protocol,
                mismatch_rose_ms_ago: s.mismatch_rose_at_ms.map(|t| now.saturating_sub(t)),
                fresh: now.saturating_sub(s.received_at_ms) <= STALE_AFTER_MS,
            })
            .collect::<Vec<_>>(),
    );
    states
        .into_iter()
        .zip(skews)
        .map(|(s, skew)| {
            let age = now.saturating_sub(s.received_at_ms);
            FleetInstanceView {
                instance: s.payload.instance,
                last_seen_ms_ago: age,
                stale: age > STALE_AFTER_MS,
                pools: s.payload.pools,
                sessions: s.payload.sessions,
                group: s.payload.group,
                version: s.payload.version,
                protocol: s.payload.protocol,
                config_revision: s.payload.config_revision,
                skew,
            }
        })
        .collect()
}

/// `GET /fleet/subscribe` — see the module doc. Spawns
/// [`subscribe_fleet_worker`] to do the actual send-then-tail work and turns
/// its output into SSE `Event`s, keeping the worker's logic (the part worth
/// testing) free of any HTTP/SSE framing — same split `wayhouse-controller`'s
/// `subscribe`/`subscribe_worker` uses.
#[utoipa::path(
    get, path = "/fleet/subscribe", tag = "Fleet views",
    summary = "Live fleet view (server-sent events)",
    description = "A server-sent-event stream of the merged fleet state: every instance with its pools, sessions, version, protocol, config revision and skew. \
The first event is the current view; another follows every accepted push, debounced by 150 ms so a burst of pushes lands as one event. There is no replay: a subscriber that falls behind just gets the current view next. Each event's `data` is a JSON array of the objects below. \
A reverse proxy in front of this route must not buffer the response and must allow a long read timeout.",
    responses(
        (status = 200, description = "The event stream; each event's data is a JSON array.", body = Vec<FleetInstanceView>, content_type = "text/event-stream",
            example = json!([{"instance": "edge-fra-1", "last_seen_ms_ago": 3200, "stale": false, "pools": [], "sessions": {"tcp": 12, "udp": 0}, "version": "0.4.0", "protocol": "1.3", "config_revision": 41, "skew": "none"}])),
    )
)]
async fn subscribe_fleet(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let (tx, rx) = mpsc::channel(4);
    let updates = state.updates.subscribe();
    tokio::spawn(subscribe_fleet_worker(state.store, updates, tx));

    let events = ReceiverStream::new(rx)
        .map(|view| Ok(Event::default().data(serde_json::to_string(&view).unwrap_or_default())));
    Sse::new(events).keep_alive(KeepAlive::default())
}

/// Sends the current fleet view immediately, then again every time `updates`
/// fires — debounced by [`SUBSCRIBE_DEBOUNCE`] so a burst of near-
/// simultaneous pushes across a fleet collapses into one rebuild. A
/// [`broadcast::error::RecvError::Lagged`] just means "rebuild now" too:
/// there's no history to replay, only ever a current view, so falling behind
/// the notification channel loses nothing a fresh rebuild doesn't already
/// fix.
async fn subscribe_fleet_worker(
    store: Arc<IngestStore>,
    mut updates: broadcast::Receiver<()>,
    tx: mpsc::Sender<Vec<FleetInstanceView>>,
) {
    if tx.send(fleet_view(&store)).await.is_err() {
        return;
    }

    loop {
        match updates.recv().await {
            Ok(()) | Err(broadcast::error::RecvError::Lagged(_)) => {
                tokio::time::sleep(SUBSCRIBE_DEBOUNCE).await;
                // Drain anything else that landed during the debounce
                // window so it doesn't trigger a second, redundant rebuild
                // right behind this one.
                while updates.try_recv().is_ok() {}
                if tx.send(fleet_view(&store)).await.is_err() {
                    return;
                }
            }
            Err(broadcast::error::RecvError::Closed) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::InstanceState;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    async fn body_json(resp: Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    /// Accepts the loopback `admin_url`s these tests push (a oneshot request
    /// has no source address, so the default rule would refuse them).
    fn test_state() -> AppState {
        AppState::new(Arc::new(IngestStore::new()))
            .with_admin_url_policy(AdminUrlPolicy::new(&["127.0.0.0/8"]).unwrap())
    }

    fn payload_json(instance: &str) -> String {
        serde_json::to_string(&IngestPayload {
            instance: instance.to_string(),
            admin_url: "http://127.0.0.1:0".to_string(),
            pools: vec![],
            sessions: SessionCounts { tcp: 3, udp: 7 },
            group: None,
            version: String::new(),
            protocol: String::new(),
            protocol_mismatches: 0,
            region: None,
            tags: std::collections::BTreeMap::default(),
            listeners: vec![],
            config_revision: None,
            features: vec![],
        })
        .unwrap()
    }

    #[tokio::test]
    async fn stats_serves_aggregates_and_rejects_an_unknown_range() {
        let state = test_state();
        let store = state.store.clone();
        store.ingest(serde_json::from_str(&payload_json("proxy-1")).unwrap());
        let app = router(state);

        let resp = app
            .clone()
            .oneshot(
                Request::get("/fleet/stats?range=5m")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp).await;
        assert_eq!(body["range_sec"], 300);
        assert_eq!(body["fleet"]["resolution_sec"], 60);
        assert_eq!(body["fleet"]["points"][0]["tcp_sessions"], 3);

        let resp = app
            .oneshot(
                Request::get("/fleet/stats?range=2d")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn a_valid_push_is_accepted_and_stored() {
        let state = test_state();
        let store = state.store.clone();
        let app = router(state);

        let resp = app
            .oneshot(
                Request::post("/ingest")
                    .header(wayhouse_http::protocol::HEADER, "1.0")
                    .header("content-type", "application/json")
                    .body(Body::from(payload_json("proxy-1")))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let stored = store.get("proxy-1").unwrap();
        assert_eq!(stored.payload.sessions.tcp, 3);
        assert_eq!(stored.payload.sessions.udp, 7);
    }

    #[tokio::test]
    async fn an_empty_instance_name_is_rejected() {
        let app = router(test_state());
        let resp = app
            .oneshot(
                Request::post("/ingest")
                    .header(wayhouse_http::protocol::HEADER, "1.0")
                    .header("content-type", "application/json")
                    .body(Body::from(payload_json("  ")))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn malformed_json_is_rejected_by_the_extractor() {
        let app = router(test_state());
        let resp = app
            .oneshot(
                Request::post("/ingest")
                    .header(wayhouse_http::protocol::HEADER, "1.0")
                    .header("content-type", "application/json")
                    .body(Body::from("not json"))
                    .unwrap(),
            )
            .await
            .unwrap();
        // axum's `Json` extractor rejects a body that doesn't even parse as
        // JSON with 400 (a 422 would be a body that parses but fails
        // `IngestPayload`'s shape) — this never reaches our own handler.
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn a_second_push_from_the_same_instance_overwrites_the_first() {
        let state = test_state();
        let store = state.store.clone();
        let app = router(state);

        app.clone()
            .oneshot(
                Request::post("/ingest")
                    .header(wayhouse_http::protocol::HEADER, "1.0")
                    .header("content-type", "application/json")
                    .body(Body::from(payload_json("proxy-1")))
                    .unwrap(),
            )
            .await
            .unwrap();

        let mut second = serde_json::from_str::<IngestPayload>(&payload_json("proxy-1")).unwrap();
        second.sessions = SessionCounts { tcp: 99, udp: 0 };
        app.oneshot(
            Request::post("/ingest")
                .header(wayhouse_http::protocol::HEADER, "1.0")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&second).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

        assert_eq!(store.snapshot().len(), 1);
        assert_eq!(store.get("proxy-1").unwrap().payload.sessions.tcp, 99);
    }

    fn full_payload(instance: &str) -> IngestPayload {
        IngestPayload {
            instance: instance.to_string(),
            admin_url: "http://127.0.0.1:0".to_string(),
            pools: vec![PoolSummary {
                name: "local".into(),
                balancer: "round_robin".into(),
                backends: vec![crate::ingest::BackendSummary {
                    addr: "127.0.0.1:9001".into(),
                    healthy: true,
                    state: "enabled".into(),
                    active: 3,
                }],
            }],
            sessions: SessionCounts { tcp: 5, udp: 10 },
            group: None,
            version: String::new(),
            protocol: String::new(),
            protocol_mismatches: 0,
            region: None,
            tags: std::collections::BTreeMap::default(),
            listeners: vec![],
            config_revision: None,
            features: vec![],
        }
    }

    #[tokio::test]
    async fn fleet_pools_reports_every_instances_pool_and_backend_state() {
        let state = test_state();
        state.store.ingest(full_payload("proxy-1"));
        let app = router(state);

        let resp = app
            .oneshot(
                Request::get("/fleet/pools")
                    .header(wayhouse_http::protocol::HEADER, "1.0")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let list = body_json(resp).await;
        assert_eq!(list[0]["instance"], "proxy-1");
        assert_eq!(list[0]["pools"][0]["name"], "local");
        assert_eq!(list[0]["pools"][0]["backends"][0]["addr"], "127.0.0.1:9001");
        assert!(list[0]["last_seen_ms_ago"].as_u64().is_some());
    }

    #[tokio::test]
    async fn fleet_node_pools_reads_one_instance_and_404s_for_an_unknown_one() {
        let state = test_state();
        state.store.ingest(full_payload("proxy-1"));
        state.store.ingest(full_payload("proxy-2"));
        let app = router(state);
        let get = |path: &'static str| {
            let app = app.clone();
            async move {
                app.oneshot(
                    Request::get(path)
                        .header(wayhouse_http::protocol::HEADER, "1.0")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap()
            }
        };
        let resp = get("/fleet/nodes/proxy-2/pools").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let one = body_json(resp).await;
        assert_eq!(one["instance"], "proxy-2");
        assert_eq!(one["pools"][0]["backends"][0]["addr"], "127.0.0.1:9001");
        assert!(one["last_seen_ms_ago"].as_u64().is_some());
        // One object, not the fleet list.
        assert!(one.is_object());
        assert_eq!(
            get("/fleet/nodes/nope/pools").await.status(),
            StatusCode::NOT_FOUND
        );
        // The literal names of the sibling routes still reach them.
        assert_eq!(
            get("/fleet/nodes/query/pools").await.status(),
            StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn fleet_sessions_reports_every_instances_counts() {
        let state = test_state();
        state.store.ingest(full_payload("proxy-1"));
        let app = router(state);

        let resp = app
            .oneshot(
                Request::get("/fleet/sessions")
                    .header(wayhouse_http::protocol::HEADER, "1.0")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let list = body_json(resp).await;
        assert_eq!(list[0]["instance"], "proxy-1");
        assert_eq!(list[0]["tcp"], 5);
        assert_eq!(list[0]["udp"], 10);
    }

    #[test]
    fn fleet_view_marks_within_and_outside_window() {
        let store = IngestStore::new();
        for (name, version) in [("a", "0.4.0"), ("b", "0.3.2"), ("c", "0.1.0")] {
            let mut p = full_payload(name);
            p.version = version.into();
            p.protocol = "1.0".into();
            store.ingest(p);
        }
        let view = serde_json::to_value(fleet_view(&store)).unwrap();
        let skew: Vec<_> = view
            .as_array()
            .unwrap()
            .iter()
            .map(|i| {
                (
                    i["instance"].as_str().unwrap(),
                    i["skew"].as_str().unwrap(),
                    i["version"].as_str().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            skew,
            [
                ("a", "none", "0.4.0"),
                ("b", "within-window", "0.3.2"),
                ("c", "outside-window", "0.1.0")
            ]
        );
    }

    #[tokio::test]
    async fn fleet_pools_and_sessions_are_empty_lists_before_any_push() {
        let app = router(test_state());
        for path in ["/fleet/pools", "/fleet/sessions", "/fleet/healthz"] {
            let resp = app
                .clone()
                .oneshot(
                    Request::get(path)
                        .header(wayhouse_http::protocol::HEADER, "1.0")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
            assert_eq!(body_json(resp).await, serde_json::json!([]));
        }
    }

    #[tokio::test]
    async fn fleet_healthz_flags_a_stale_instance_but_not_a_fresh_one() {
        let state = test_state();
        state.store.insert_state(InstanceState {
            payload: full_payload("stale-1"),
            received_at_ms: unix_ms().saturating_sub(STALE_AFTER_MS + 5_000),
            mismatch_rose_at_ms: None,
        });
        state.store.insert_state(InstanceState {
            payload: full_payload("fresh-1"),
            received_at_ms: unix_ms(),
            mismatch_rose_at_ms: None,
        });
        let app = router(state);

        let resp = app
            .oneshot(
                Request::get("/fleet/healthz")
                    .header(wayhouse_http::protocol::HEADER, "1.0")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let list = body_json(resp).await;
        let by_instance = |name: &str| {
            list.as_array()
                .unwrap()
                .iter()
                .find(|v| v["instance"] == name)
                .unwrap()
                .clone()
        };
        assert_eq!(by_instance("stale-1")["stale"], true);
        assert_eq!(by_instance("fresh-1")["stale"], false);
    }

    #[tokio::test]
    async fn fleet_healthz_reports_each_instances_version() {
        let state = test_state();
        let mut payload = full_payload("p1");
        payload.version = "0.3.1".into();
        state.store.insert_state(InstanceState {
            payload,
            received_at_ms: unix_ms(),
            mismatch_rose_at_ms: None,
        });
        let resp = router(state)
            .oneshot(
                Request::get("/fleet/healthz")
                    .header(wayhouse_http::protocol::HEADER, "1.0")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let list = body_json(resp).await;
        assert_eq!(list[0]["version"], "0.3.1");
    }

    #[tokio::test]
    async fn an_unauthenticated_push_with_another_protocol_major_is_a_401_not_a_426() {
        // The protocol gate counts a mismatch (and the fleet view then turns this
        // node red), so it must only ever see requests that passed the token.
        let state =
            AppState::new(Arc::new(IngestStore::new())).with_ingest_token(Some("ingest".into()));
        let resp = router(state)
            .oneshot(
                Request::post("/ingest")
                    .header("content-type", "application/json")
                    .header(wayhouse_http::protocol::HEADER, "2.0")
                    .body(Body::from(payload_json("proxy-1")))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn auth_token_gates_ingest_and_fleet_reads_but_not_healthz() {
        // /healthz lives outside `api::router` in `main.rs`, so it's not
        // part of what's under test here — only confirming everything
        // *inside* this router is gated when a token is set.
        let state = AppState::new(Arc::new(IngestStore::new()))
            .with_auth_token(Some("secret".into()))
            .with_ingest_token(Some("ingest".into()));
        let app = router(state);

        let resp = app
            .clone()
            .oneshot(
                Request::get("/fleet/pools")
                    .header(wayhouse_http::protocol::HEADER, "1.0")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        let resp = app
            .clone()
            .oneshot(
                Request::get("/fleet/pools")
                    .header(wayhouse_http::protocol::HEADER, "1.0")
                    .header("Authorization", "Bearer wrong")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        let resp = app
            .oneshot(
                Request::get("/fleet/pools")
                    .header(wayhouse_http::protocol::HEADER, "1.0")
                    .header("Authorization", "Bearer secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    async fn status_of(app: Router, req: Request<Body>) -> StatusCode {
        app.oneshot(req).await.unwrap().status()
    }

    fn push(token: Option<&str>, instance: &str) -> Request<Body> {
        let mut req = Request::post("/ingest")
            .header(wayhouse_http::protocol::HEADER, "1.0")
            .header("content-type", "application/json");
        if let Some(t) = token {
            req = req.header("Authorization", format!("Bearer {t}"));
        }
        req.body(Body::from(payload_json(instance))).unwrap()
    }

    fn get(path: &str, token: &str) -> Request<Body> {
        Request::get(path)
            .header(wayhouse_http::protocol::HEADER, "1.0")
            .header("Authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap()
    }

    #[tokio::test]
    async fn the_ingest_token_unlocks_only_ingest() {
        let app = router(
            test_state()
                .with_auth_token(Some("admin".into()))
                .with_ingest_token(Some("ingest".into())),
        );

        assert_eq!(
            status_of(app.clone(), push(Some("ingest"), "a")).await,
            StatusCode::OK
        );
        assert_eq!(
            status_of(app.clone(), push(Some("admin"), "a")).await,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            status_of(app.clone(), push(None, "a")).await,
            StatusCode::UNAUTHORIZED
        );

        for path in ["/fleet/pools", "/fleet/sessions", "/fleet/healthz"] {
            assert_eq!(
                status_of(app.clone(), get(path, "ingest")).await,
                StatusCode::UNAUTHORIZED,
                "{path}"
            );
            assert_eq!(
                status_of(app.clone(), get(path, "admin")).await,
                StatusCode::OK,
                "{path}"
            );
        }
        // A write verb is a /fleet route too: the ingest token cannot drain.
        let drain = |t: &str| {
            Request::post("/fleet/instances/a/drain")
                .header(wayhouse_http::protocol::HEADER, "1.0")
                .header("Authorization", format!("Bearer {t}"))
                .body(Body::empty())
                .unwrap()
        };
        assert_eq!(
            status_of(app.clone(), drain("ingest")).await,
            StatusCode::UNAUTHORIZED
        );
        assert_ne!(
            status_of(app, drain("admin")).await,
            StatusCode::UNAUTHORIZED
        );
    }

    fn push_from(peer: &str, admin_url: &str) -> Request<Body> {
        let mut req = Request::post("/ingest")
            .header(wayhouse_http::protocol::HEADER, "1.0")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_string(&IngestPayload {
                    instance: "p".into(),
                    admin_url: admin_url.into(),
                    pools: vec![],
                    sessions: SessionCounts::default(),
                    group: None,
                    version: String::new(),
                    protocol: String::new(),
                    protocol_mismatches: 0,
                    region: None,
                    tags: std::collections::BTreeMap::default(),
                    listeners: vec![],
                    config_revision: None,
                    features: vec![],
                })
                .unwrap(),
            ))
            .unwrap();
        req.extensions_mut()
            .insert(ConnectInfo(wayhouse_http::tls::PeerAddr(
                format!("{peer}:4000").parse().unwrap(),
            )));
        req
    }

    #[tokio::test]
    async fn by_default_an_instance_can_only_report_its_own_address() {
        let state = AppState::new(Arc::new(IngestStore::new()));
        let store = state.store.clone();
        let app = router(state);

        // The attack from #102: point admin_url at a host the pusher controls.
        let resp = app
            .clone()
            .oneshot(push_from("10.0.0.4", "http://evil.example:9900"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let resp = app
            .clone()
            .oneshot(push_from("10.0.0.4", "http://10.9.9.9:9900"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert!(store.get("p").is_none());

        let resp = app
            .oneshot(push_from("10.0.0.4", "http://10.0.0.4:9900"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(store.get("p").is_some());
    }

    #[tokio::test]
    async fn the_allowlist_decides_when_given() {
        let state = AppState::new(Arc::new(IngestStore::new()))
            .with_admin_url_policy(AdminUrlPolicy::new(&["*.nodes.example"]).unwrap());
        let app = router(state);
        let resp = app
            .clone()
            .oneshot(push_from("10.0.0.4", "http://n1.nodes.example:9900"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let resp = app
            .oneshot(push_from("10.0.0.4", "http://10.0.0.4:9900"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn subscribe_worker_sends_the_current_view_immediately() {
        let store = Arc::new(IngestStore::new());
        store.ingest(full_payload("proxy-1"));

        let (updates_tx, updates_rx) = broadcast::channel(8);
        let (tx, mut rx) = mpsc::channel(8);
        tokio::spawn(subscribe_fleet_worker(store, updates_rx, tx));

        let view = rx.recv().await.unwrap();
        assert_eq!(view.len(), 1);
        assert_eq!(view[0].instance, "proxy-1");
        assert_eq!(view[0].sessions.tcp, 5);
        let _ = updates_tx; // keep the sender alive for the worker's lifetime
    }

    #[tokio::test]
    async fn subscribe_worker_resends_after_an_update_once_debounced() {
        let store = Arc::new(IngestStore::new());
        store.ingest(full_payload("proxy-1"));

        let (updates_tx, updates_rx) = broadcast::channel(8);
        let (tx, mut rx) = mpsc::channel(8);
        tokio::spawn(subscribe_fleet_worker(store.clone(), updates_rx, tx));

        // Initial view.
        assert_eq!(rx.recv().await.unwrap().len(), 1);

        // A second instance pushes; the worker must pick it up after
        // debouncing, without the test needing to know the exact delay.
        store.ingest(full_payload("proxy-2"));
        updates_tx.send(()).unwrap();

        let view = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .expect("worker should resend well within the debounce + margin")
            .unwrap();
        assert_eq!(view.len(), 2);
    }

    #[tokio::test]
    async fn a_burst_of_updates_collapses_into_one_resend() {
        let store = Arc::new(IngestStore::new());
        store.ingest(full_payload("proxy-1"));

        let (updates_tx, updates_rx) = broadcast::channel(8);
        let (tx, mut rx) = mpsc::channel(8);
        tokio::spawn(subscribe_fleet_worker(store.clone(), updates_rx, tx));
        assert_eq!(rx.recv().await.unwrap().len(), 1); // initial view

        // Five near-simultaneous pushes/signals — the debounce should
        // collapse these into exactly one resend, not five.
        for i in 0..5 {
            store.ingest(full_payload(&format!("proxy-burst-{i}")));
            updates_tx.send(()).unwrap();
        }

        let view = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(view.len(), 6, "one instance plus the five burst instances");

        // Nothing else should arrive — the burst was one rebuild, not five.
        let extra = tokio::time::timeout(std::time::Duration::from_millis(300), rx.recv()).await;
        assert!(
            extra.is_err(),
            "no second resend should follow the debounced one"
        );
    }

    #[tokio::test]
    async fn ingest_rejects_other_major() {
        let state = test_state();
        let store = state.store.clone();
        let resp = router(state)
            .oneshot(
                Request::post("/ingest")
                    .header("content-type", "application/json")
                    .header(wayhouse_http::protocol::HEADER, "2.0")
                    .body(Body::from(payload_json("proxy-1")))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UPGRADE_REQUIRED);
        assert!(store.get("proxy-1").is_none());
    }

    #[tokio::test]
    async fn fleet_routes_ignore_the_header() {
        let resp = router(test_state())
            .oneshot(
                Request::get("/fleet/healthz")
                    .header(wayhouse_http::protocol::HEADER, "banana")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_ne!(resp.status(), StatusCode::UPGRADE_REQUIRED);
    }

    // --- the node list ---

    async fn get_json(app: &Router, uri: &str) -> (StatusCode, serde_json::Value) {
        let resp = app
            .clone()
            .oneshot(Request::get(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        (resp.status(), body_json(resp).await)
    }

    fn labelled(instance: &str, region: Option<&str>, tags: &[(&str, &str)]) -> IngestPayload {
        let mut p = full_payload(instance);
        p.region = region.map(String::from);
        p.tags = tags
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        p
    }

    #[tokio::test]
    async fn the_fleet_view_carries_each_instances_revision_and_flags_roll_up() {
        let state = test_state();
        let mut a = full_payload("a");
        a.config_revision = Some(7);
        a.features = vec![crate::ingest::FeatureReport {
            name: "acl".into(),
            value: false,
            source: "override".into(),
            available: true,
        }];
        state.store.ingest(a);
        state.store.ingest(full_payload("old"));
        let app = router(state);
        let (status, v) = get_json(&app, "/fleet/features").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            (v["reporting"].as_u64(), v["total"].as_u64()),
            (Some(1), Some(2))
        );
        assert_eq!(v["flags"][0]["off_instances"][0], "a");
    }

    #[test]
    fn the_instance_view_reports_the_applied_revision() {
        let store = IngestStore::default();
        let mut a = full_payload("a");
        a.config_revision = Some(7);
        store.ingest(a);
        store.ingest(full_payload("old"));
        let view = serde_json::to_value(fleet_view(&store)).unwrap();
        let rev = |n: &str| {
            view.as_array()
                .unwrap()
                .iter()
                .find(|i| i["instance"] == n)
                .unwrap()["config_revision"]
                .clone()
        };
        assert_eq!(rev("a"), 7);
        assert!(rev("old").is_null(), "an older proxy reports none");
    }

    #[tokio::test]
    async fn the_node_list_filters_pages_and_groups_what_instances_reported() {
        let state = test_state();
        for (name, region, tags) in [
            ("a", Some("eu"), vec![("env", "prod")]),
            ("b", Some("eu"), vec![("env", "stage")]),
            ("c", Some("us"), vec![("env", "prod")]),
            ("d", None, vec![]),
        ] {
            state.store.ingest(labelled(name, region, &tags));
        }
        let app = router(state);

        let (status, v) = get_json(&app, "/fleet/nodes?tag=env%3Dprod&size=1&page=2").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            (v["total"].as_u64(), v["items"][0]["instance"].as_str()),
            (Some(2), Some("c"))
        );
        assert_eq!(v["items"][0]["tags"]["env"], "prod");
        assert_eq!(v["rollup"]["nodes"], 2);

        let (_, v) = get_json(&app, "/fleet/nodes?group_by=region").await;
        let regions: Vec<_> = v["groups"]
            .as_array()
            .unwrap()
            .iter()
            .map(|g| g["region"].clone())
            .collect();
        assert_eq!(
            regions,
            [
                serde_json::json!("eu"),
                serde_json::json!("us"),
                serde_json::Value::Null
            ]
        );

        let (status, v) = get_json(&app, "/fleet/nodes?size=500").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(v["error"].as_str().unwrap().contains("size"));
    }

    #[tokio::test]
    async fn resolve_lists_the_instances_a_filter_selects_and_changes_nothing() {
        let state = test_state();
        state
            .store
            .ingest(labelled("a", Some("eu"), &[("env", "prod")]));
        state
            .store
            .ingest(labelled("b", Some("us"), &[("env", "prod")]));
        let app = router(state);
        let resp = app
            .clone()
            .oneshot(
                Request::post("/fleet/nodes/resolve?region=eu&page=3&size=1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let v = body_json(resp).await;
        assert_eq!(
            (v["count"].as_u64(), v["instances"].clone()),
            (Some(1), serde_json::json!(["a"]))
        );
        let resp = app
            .oneshot(
                Request::post("/fleet/nodes/resolve?state=sad")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn the_query_takes_a_thousand_nodes_with_sixteen_longest_tags_each() {
        // The controller keeps at most 1000 tag records of 16 tags (32-character keys,
        // 64-character values); the overlay it sends must fit `NODE_TAGS_BODY_LIMIT` (#525).
        let tags: serde_json::Map<_, _> = (0..16)
            .map(|i| (format!("{i:k<32}"), serde_json::json!("v".repeat(64))))
            .collect();
        let by_node: serde_json::Map<_, _> = (0..1000)
            .map(|i| (format!("{i:n<64}"), serde_json::Value::Object(tags.clone())))
            .collect();
        let body = serde_json::json!({ "operator_tags": by_node }).to_string();
        assert!(body.len() < NODE_TAGS_BODY_LIMIT, "{} bytes", body.len());
        let state = test_state();
        state
            .store
            .ingest(labelled("a", Some("eu"), &[("env", "prod")]));
        let resp = router(state)
            .oneshot(
                Request::post("/fleet/nodes/query")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn the_query_and_resolve_routes_take_the_operators_tags_in_the_body() {
        let state = test_state();
        state
            .store
            .ingest(labelled("a", Some("eu"), &[("env", "prod")]));
        state
            .store
            .ingest(labelled("b", Some("eu"), &[("env", "prod")]));
        let app = router(state);
        let body = r#"{"operator_tags": {"a": {"env": "stage"}}}"#;
        let post = |uri: &str, body: &str| {
            Request::post(uri)
                .body(Body::from(body.to_string()))
                .unwrap()
        };

        let resp = app
            .clone()
            .oneshot(post("/fleet/nodes/query?tag=env%3Dstage", body))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let v = body_json(resp).await;
        assert_eq!(v["total"], 1);
        assert_eq!(v["items"][0]["instance"], "a");
        assert_eq!(v["items"][0]["operator_tags"]["env"], "stage");

        let resp = app
            .clone()
            .oneshot(post("/fleet/nodes/resolve?tag=env%3Dprod", body))
            .await
            .unwrap();
        let v = body_json(resp).await;
        assert_eq!(v["instances"], serde_json::json!(["b"]));

        // No body is the plain list; a bad body is a 400, not a silent plain list.
        let resp = app
            .clone()
            .oneshot(post("/fleet/nodes/query", ""))
            .await
            .unwrap();
        assert_eq!(body_json(resp).await["total"], 2);
        let resp = app
            .oneshot(post(
                "/fleet/nodes/query",
                r#"{"operator_tags": {"a": {"Env": "x"}}}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn a_push_with_a_bad_tag_is_refused_and_a_good_one_is_listed() {
        let state = test_state();
        let app = router(state.clone());
        let push = |body: String| {
            Request::post("/ingest")
                .header("content-type", "application/json")
                .header(wayhouse_http::protocol::HEADER, "1.0")
                .body(Body::from(body))
                .unwrap()
        };
        let mut bad = labelled("bad", None, &[("Env", "prod")]);
        bad.admin_url = "http://127.0.0.1:0".into();
        let resp = app
            .clone()
            .oneshot(push(serde_json::to_string(&bad).unwrap()))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let good = labelled("good", Some("eu"), &[("env", "prod")]);
        let resp = app
            .clone()
            .oneshot(push(serde_json::to_string(&good).unwrap()))
            .await
            .unwrap();
        assert!(resp.status().is_success());
        let (_, v) = get_json(&app, "/fleet/nodes").await;
        assert_eq!(v["items"][0]["region"], "eu");
    }
}
