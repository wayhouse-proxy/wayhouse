//! The OpenAPI descriptions of the console's routes. The handlers are spread over many modules and
//! most only relay to the controller or the aggregator, so each route is described here, once, beside
//! the others. `the_spec_matches_the_router` (in `openapi.rs`) checks that every route listed here
//! exists with the stated role, and that every route in the sources is listed.

#[allow(unused_imports)]
use crate::openapi::{
    AddRegistryBody, FetchBody, InstallBody, LoginBody, ProcessApply, ProcessRev, PruneAnswer,
    PruneBody,
};
#[allow(unused_imports)]
use crate::openapi::{
    ErrorBody, InstallAnswer, RegistryList, ReleaseStatus, SessionStatus, WebSyncStatus,
};

#[utoipa::path(
    post, path = "/ui/login", tag = "Session",
    summary = "Sign in",
    description = "Checks the credentials and sets the session cookie `wayhouse_ui_session` (`HttpOnly`, `SameSite=Lax`, `Secure` when the console serves HTTPS itself). With `--users-file` the body needs `username` and `password` and the user's role becomes the session's role; with `--ui-password-file` only `password` is read and the session is an admin; with neither the console is open, any password is accepted and the session is an anonymous admin. Attempts are throttled per client address and per username; a wrong user and a wrong password answer alike. A browser request must be same-origin (`Sec-Fetch-Site`/`Origin`), else `403`.",
    security(()),
    request_body(content = LoginBody, content_type = "application/json", description = "The credentials."),
    responses(
        (status = 200, description = "Signed in. The `Set-Cookie` header carries the session. Body: `ok` (plain text).", body = String, content_type = "text/plain"),
        (status = 401, description = "Wrong username or password.", body = crate::openapi::ErrorBody),
        (status = 429, description = "Too many attempts. `Retry-After` says how many seconds to wait.", body = crate::openapi::ErrorBody),
        (status = 503, description = "Too many sessions of a higher role are open already; try again later (plain text).", body = String, content_type = "text/plain")
    )
)]
#[allow(dead_code)]
async fn post_ui_login() {}

#[utoipa::path(
    post, path = "/ui/logout", tag = "Session",
    summary = "Sign out",
    description = "Revokes the session named by the cookie, if there is one, and expires the cookie. Signing out without a valid session is not an error.",
    security(()),
    responses(
        (status = 200, description = "Signed out. Body: `ok` (plain text).", body = String, content_type = "text/plain")
    )
)]
#[allow(dead_code)]
async fn post_ui_logout() {}

#[utoipa::path(
    get, path = "/ui/session", tag = "Session",
    summary = "Who am I",
    description = "Answers for any valid session, so a page can check on load whether it is signed in and what it may do. With no login configured every request is an anonymous admin.",
    security(("session" = [])),
    responses(
        (status = 200, description = "The session.", body = SessionStatus, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain")
    ),
    extensions(("x-wayhouse-role" = json!("viewer")))
)]
#[allow(dead_code)]
async fn get_ui_session() {}

#[utoipa::path(
    get, path = "/api/release", tag = "Session",
    summary = "The running version and the newest release",
    description = "The version this console runs and, from the signed release feed, the newest stable release above it. The feed is checked once a day (plus up to an hour of spread) and only a feed whose signature verifies is believed. `enabled` is `false` when the feed is switched off (`--release-feed-url \"\"`).",
    security(("session" = [])),
    responses(
        (status = 200, description = "The release status.", body = ReleaseStatus, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain")
    ),
    extensions(("x-wayhouse-role" = json!("viewer")))
)]
#[allow(dead_code)]
async fn get_api_release() {}

#[utoipa::path(
    get, path = "/ws/fleet", tag = "Session",
    summary = "Live fleet updates (WebSocket)",
    description = "Upgrades to a WebSocket that carries the live fleet view: the first text message is the current view and another follows each update the aggregator reports. Each message is a JSON array of the objects the aggregator's `GET /fleet/subscribe` sends (see the [Aggregator API](/reference/aggregator-api/)). All sockets share one subscription, so the aggregator sees a single subscriber. The socket closes with code 1008 when its session ends (sign-out, idle timeout, maximum age or eviction); the session is re-checked every 30 seconds and before each message. With no aggregator configured the console sends the text `no aggregator configured` and closes. At most 256 sockets are open at once; another upgrade gets `503`. Over HTTP/2 the browser opens the socket as an extended `CONNECT`. A cross-site upgrade is refused with `403`.",
    security(("session" = [])),
    responses(
        (status = 101, description = "Switching protocols to WebSocket."),
        (status = 503, description = "Too many sockets are open (plain text).", body = String, content_type = "text/plain"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain")
    ),
    extensions(("x-wayhouse-role" = json!("viewer")))
)]
#[allow(dead_code)]
async fn get_ws_fleet() {}

#[utoipa::path(
    get, path = "/healthz", tag = "Service",
    summary = "Liveness",
    description = "Answers `ok` while the process is up. It needs no session.",
    security(()),
    responses(
        (status = 200, description = "The text `ok`.", body = String, content_type = "text/plain")
    )
)]
#[allow(dead_code)]
async fn get_healthz() {}

#[utoipa::path(
    get, path = "/metrics", tag = "Service",
    summary = "Prometheus metrics",
    description = "The console's metrics in the Prometheus text format. The route is served only when `--metrics-token-file` is set (the token is then required) or when no login is configured (then it is open); with a login and no metrics token it does not exist.",
    security(()),
    responses(
        (status = 200, description = "Prometheus text format.", body = String, content_type = "text/plain"),
        (status = 401, description = "The metrics token is missing or wrong (plain text).", body = String, content_type = "text/plain")
    )
)]
#[allow(dead_code)]
async fn get_metrics() {}

#[utoipa::path(
    get, path = "/openapi.json", tag = "Service",
    summary = "This document",
    description = "The OpenAPI description of this API, as JSON.",
    security(("session" = [])),
    responses(
        (status = 200, description = "The OpenAPI document.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain")
    ),
    extensions(("x-wayhouse-role" = json!("viewer")))
)]
#[allow(dead_code)]
async fn get_openapi_json() {}

#[utoipa::path(
    get, path = "/api/fleet/pools", tag = "Fleet",
    summary = "Pools and backends of every instance",
    description = "Forwards to the aggregator's `GET /fleet/pools` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Aggregator API](/reference/aggregator-api/).",
    security(("session" = [])),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The aggregator could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No aggregator is configured (`--aggregator-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("viewer")))
)]
#[allow(dead_code)]
async fn get_api_fleet_pools() {}

#[utoipa::path(
    get, path = "/api/fleet/sessions", tag = "Fleet",
    summary = "Session counts of every instance",
    description = "Forwards to the aggregator's `GET /fleet/sessions` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Aggregator API](/reference/aggregator-api/).",
    security(("session" = [])),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The aggregator could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No aggregator is configured (`--aggregator-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("viewer")))
)]
#[allow(dead_code)]
async fn get_api_fleet_sessions() {}

#[utoipa::path(
    get, path = "/api/fleet/healthz", tag = "Fleet",
    summary = "When each instance was last heard from",
    description = "Forwards to the aggregator's `GET /fleet/healthz` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Aggregator API](/reference/aggregator-api/).",
    security(("session" = [])),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The aggregator could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No aggregator is configured (`--aggregator-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("viewer")))
)]
#[allow(dead_code)]
async fn get_api_fleet_healthz() {}

#[utoipa::path(
    get, path = "/api/fleet/features", tag = "Fleet",
    summary = "How each feature flag stands across the fleet",
    description = "Forwards to the aggregator's `GET /fleet/features` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Aggregator API](/reference/aggregator-api/).",
    security(("session" = [])),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The aggregator could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No aggregator is configured (`--aggregator-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("viewer")))
)]
#[allow(dead_code)]
async fn get_api_fleet_features() {}

#[utoipa::path(
    get, path = "/api/fleet/stats", tag = "Fleet",
    summary = "Rates and gauges over time",
    description = "Forwards to the aggregator's `GET /fleet/stats` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Aggregator API](/reference/aggregator-api/). The `range` query goes along unchanged and the aggregator validates it.",
    security(("session" = [])),
    params(
        ("range" = Option<String>, Query, description = "`5m`, `15m` (default) or `1h`.", example = "15m")
    ),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The aggregator could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No aggregator is configured (`--aggregator-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("viewer")))
)]
#[allow(dead_code)]
async fn get_api_fleet_stats() {}

#[utoipa::path(
    get, path = "/api/fleet/nodes", tag = "Fleet",
    summary = "The node list",
    description = "The aggregator's paged node list (`GET /fleet/nodes`, see the [Aggregator API](/reference/aggregator-api/) for every query key), with the tags operators have set laid over what each node declared. Operator tags live in the controller; the console reads them (`GET /ui/node-tags`) and asks the aggregator for the list with them (`POST /fleet/nodes/query`). Without a controller, or with no tags set, it is a plain `GET /fleet/nodes`. When the controller cannot answer, or the aggregator is older than the overlay, the list comes without the overlay and the response carries `x-operator-tags: unavailable`.",
    security(("session" = [])),
    params(
        ("page" = Option<usize>, Query, description = "The page, from 1.", example = 1),
        ("size" = Option<usize>, Query, description = "Rows per page, 1 to 200.", example = 25),
        ("sort" = Option<String>, Query, description = "`instance`, `state`, `region`, `sessions` or `version`."),
        ("dir" = Option<String>, Query, description = "`asc` or `desc`."),
        ("q" = Option<String>, Query, description = "Free-text filter."),
        ("state" = Option<String>, Query, description = "`ok`, `degraded` or `stale`."),
        ("region" = Option<String>, Query, description = "Only this region."),
        ("tag" = Option<Vec<String>>, Query, description = "`key` or `key=value`; repeat to require several."),
        ("group_by" = Option<String>, Query, description = "`region` to group by region.")
    ),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The aggregator could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No aggregator is configured (`--aggregator-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("viewer")))
)]
#[allow(dead_code)]
async fn get_api_fleet_nodes() {}

#[utoipa::path(
    post, path = "/api/fleet/nodes/resolve", tag = "Fleet",
    summary = "Which instances a filter selects",
    description = "The instances a filter selects, with the operator's tags counted, so a bulk action shows the count the list showed. It reads and changes nothing, so a viewer may ask. Takes the filter keys of the node list in the query; paging keys are ignored. If the operator tags cannot be read from the controller it answers `502` instead of guessing, because a bulk action must not act on a different set than the one displayed.",
    security(("session" = [])),
    params(
        ("q" = Option<String>, Query, description = "Free-text filter."),
        ("state" = Option<String>, Query, description = "`ok`, `degraded` or `stale`."),
        ("region" = Option<String>, Query, description = "Only this region."),
        ("tag" = Option<Vec<String>>, Query, description = "`key` or `key=value`; repeat to require several.")
    ),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 502, description = "The operator tags could not be read from the controller, or the aggregator could not be reached.", body = crate::openapi::ErrorBody),
        (status = 503, description = "No aggregator is configured (`--aggregator-url`).", body = crate::openapi::ErrorBody),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The aggregator could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No aggregator is configured (`--aggregator-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("viewer")))
)]
#[allow(dead_code)]
async fn post_api_fleet_nodes_resolve() {}

#[utoipa::path(
    get, path = "/api/fleet/nodes/{instance}/pools", tag = "Fleet",
    summary = "Pools and backends of one instance",
    description = "Forwards to the aggregator's `GET /fleet/nodes/{instance}/pools` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Aggregator API](/reference/aggregator-api/).",
    security(("session" = [])),
    params(
        ("instance" = String, Path, description = "The instance's name, as it reports it.", example = "edge-fra-1")
    ),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The aggregator could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No aggregator is configured (`--aggregator-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("viewer")))
)]
#[allow(dead_code)]
async fn get_api_fleet_nodes_by_instance_pools() {}

#[utoipa::path(
    get, path = "/api/fleet/instances/{instance}/sniffers", tag = "Fleet",
    summary = "List one instance's sniffer modules",
    description = "Forwards to the aggregator's `GET /fleet/instances/{instance}/sniffers` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Aggregator API](/reference/aggregator-api/).",
    security(("session" = [])),
    params(
        ("instance" = String, Path, description = "The instance's name, as it reports it.", example = "edge-fra-1")
    ),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The aggregator could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No aggregator is configured (`--aggregator-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("viewer")))
)]
#[allow(dead_code)]
async fn get_api_fleet_instances_by_instance_sniffers() {}

#[utoipa::path(
    post, path = "/api/fleet/instances/{instance}/drain", tag = "Fleet",
    summary = "Take an instance out of rotation",
    description = "Forwards to the aggregator's `POST /fleet/instances/{instance}/drain` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Aggregator API](/reference/aggregator-api/). The signed-in user's name goes along as `X-Actor`.",
    security(("session" = [])),
    params(
        ("instance" = String, Path, description = "The instance's name, as it reports it.", example = "edge-fra-1")
    ),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The aggregator could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No aggregator is configured (`--aggregator-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("operator")))
)]
#[allow(dead_code)]
async fn post_api_fleet_instances_by_instance_drain() {}

#[utoipa::path(
    post, path = "/api/fleet/instances/{instance}/undrain", tag = "Fleet",
    summary = "Put an instance back into rotation",
    description = "Forwards to the aggregator's `POST /fleet/instances/{instance}/undrain` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Aggregator API](/reference/aggregator-api/). The signed-in user's name goes along as `X-Actor`.",
    security(("session" = [])),
    params(
        ("instance" = String, Path, description = "The instance's name, as it reports it.", example = "edge-fra-1")
    ),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The aggregator could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No aggregator is configured (`--aggregator-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("operator")))
)]
#[allow(dead_code)]
async fn post_api_fleet_instances_by_instance_undrain() {}

#[utoipa::path(
    post, path = "/api/fleet/pools/{pool}/backends", tag = "Fleet",
    summary = "Add a backend to a pool everywhere",
    description = "Forwards to the aggregator's `POST /fleet/pools/{pool}/backends` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Aggregator API](/reference/aggregator-api/). The answer lists the result of every instance. The signed-in user's name goes along as `X-Actor`.",
    security(("session" = [])),
    params(
        ("pool" = String, Path, description = "The pool's name.", example = "survival")
    ),
    request_body(content = serde_json::Value, content_type = "application/json", description = "`{\"addr\": \"ip:port\"}`."),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The aggregator could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No aggregator is configured (`--aggregator-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("operator")))
)]
#[allow(dead_code)]
async fn post_api_fleet_pools_by_pool_backends() {}

#[utoipa::path(
    patch, path = "/api/fleet/pools/{pool}/backends/{addr}", tag = "Fleet",
    summary = "Set a backend's operator state everywhere",
    description = "Forwards to the aggregator's `PATCH /fleet/pools/{pool}/backends/{addr}` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Aggregator API](/reference/aggregator-api/). The answer lists the result of every instance. The signed-in user's name goes along as `X-Actor`.",
    security(("session" = [])),
    params(
        ("pool" = String, Path, description = "The pool's name.", example = "survival"),
        ("addr" = String, Path, description = "The backend's address.", example = "10.0.2.4:25565")
    ),
    request_body(content = serde_json::Value, content_type = "application/json", description = "`{\"state\": \"enabled|draining|disabled\"}`."),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The aggregator could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No aggregator is configured (`--aggregator-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("operator")))
)]
#[allow(dead_code)]
async fn patch_api_fleet_pools_by_pool_backends_by_addr() {}

#[utoipa::path(
    delete, path = "/api/fleet/pools/{pool}/backends/{addr}", tag = "Fleet",
    summary = "Remove a backend from a pool everywhere",
    description = "Forwards to the aggregator's `DELETE /fleet/pools/{pool}/backends/{addr}` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Aggregator API](/reference/aggregator-api/). The answer lists the result of every instance. The signed-in user's name goes along as `X-Actor`.",
    security(("session" = [])),
    params(
        ("pool" = String, Path, description = "The pool's name.", example = "survival"),
        ("addr" = String, Path, description = "The backend's address.", example = "10.0.2.4:25565")
    ),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The aggregator could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No aggregator is configured (`--aggregator-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("operator")))
)]
#[allow(dead_code)]
async fn delete_api_fleet_pools_by_pool_backends_by_addr() {}

#[utoipa::path(
    post, path = "/api/fleet/route-hint", tag = "Fleet",
    summary = "Push a route hint to every instance",
    description = "Forwards to the aggregator's `POST /fleet/route-hint` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Aggregator API](/reference/aggregator-api/). The answer lists the result of every instance. The signed-in user's name goes along as `X-Actor`.",
    security(("session" = [])),
    request_body(content = serde_json::Value, content_type = "application/json", description = "`{\"src_ip\", \"pool\", \"ttl_sec\"}`."),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The aggregator could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No aggregator is configured (`--aggregator-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("operator")))
)]
#[allow(dead_code)]
async fn post_api_fleet_route_hint() {}

#[utoipa::path(
    post, path = "/api/fleet/sniffers", tag = "Fleet",
    summary = "Upload a sniffer module to every instance",
    description = "Forwards to the aggregator's `POST /fleet/sniffers` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Aggregator API](/reference/aggregator-api/). The body is the raw `.wasm` module (up to 8 MiB); the answer lists the result of every instance. The signed-in user's name goes along as `X-Actor`.",
    security(("session" = [])),
    params(
        ("name" = String, Query, description = "The module's name.", example = "minecraft")
    ),
    request_body(content = Vec<u8>, content_type = "application/octet-stream", description = "The `.wasm` module."),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The aggregator could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No aggregator is configured (`--aggregator-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("operator")))
)]
#[allow(dead_code)]
async fn post_api_fleet_sniffers() {}

#[utoipa::path(
    delete, path = "/api/fleet/sniffers/{name}", tag = "Fleet",
    summary = "Remove a sniffer module everywhere",
    description = "Forwards to the aggregator's `DELETE /fleet/sniffers/{name}` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Aggregator API](/reference/aggregator-api/). The answer lists the result of every instance. The signed-in user's name goes along as `X-Actor`.",
    security(("session" = [])),
    params(
        ("name" = String, Path, description = "The name.", example = "panel")
    ),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The aggregator could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No aggregator is configured (`--aggregator-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("operator")))
)]
#[allow(dead_code)]
async fn delete_api_fleet_sniffers_by_name() {}

#[utoipa::path(
    post, path = "/api/fleet/sniffers/{name}/rollback", tag = "Fleet",
    summary = "Roll a sniffer module back everywhere",
    description = "Forwards to the aggregator's `POST /fleet/sniffers/{name}/rollback` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Aggregator API](/reference/aggregator-api/). The answer lists the result of every instance. The signed-in user's name goes along as `X-Actor`.",
    security(("session" = [])),
    params(
        ("name" = String, Path, description = "The name.", example = "panel")
    ),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The aggregator could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No aggregator is configured (`--aggregator-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("operator")))
)]
#[allow(dead_code)]
async fn post_api_fleet_sniffers_by_name_rollback() {}

#[utoipa::path(
    put, path = "/api/fleet/nodes/{instance}/tags", tag = "Fleet",
    summary = "Replace the operator tags of a node",
    description = "Stores the tags an operator sets on one node. They live in the controller (replicated under HA), not in the aggregator, which keeps nothing of its own: the console forwards to the controller's `PUT /ui/node-tags/{instance}`, which checks the tags. They are laid over the tags the node declares in `GET /api/fleet/nodes`. The signed-in user's name goes along as `X-Actor`.",
    security(("session" = [])),
    params(
        ("instance" = String, Path, description = "The instance's name, as it reports it.", example = "edge-fra-1")
    ),
    request_body(content = serde_json::Value, content_type = "application/json", description = "`{\"tags\": {\"key\": \"value\"}}`."),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("operator")))
)]
#[allow(dead_code)]
async fn put_api_fleet_nodes_by_instance_tags() {}

#[utoipa::path(
    delete, path = "/api/fleet/nodes/{instance}/tags", tag = "Fleet",
    summary = "Drop the operator tags of a node",
    description = "Removes the operator's tags of one node: the console forwards to the controller's `DELETE /ui/node-tags/{instance}`. The signed-in user's name goes along as `X-Actor`.",
    security(("session" = [])),
    params(
        ("instance" = String, Path, description = "The instance's name, as it reports it.", example = "edge-fra-1")
    ),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("operator")))
)]
#[allow(dead_code)]
async fn delete_api_fleet_nodes_by_instance_tags() {}

#[utoipa::path(
    post, path = "/api/fleet/node-tags/prune", tag = "Fleet",
    summary = "Clean up tags of nodes that are gone",
    description = "Removes the operator tags of nodes the aggregator no longer lists. Nothing is deleted unless the body names what to delete, as the operator saw it in the answer to a call without names: the aggregator forgets everything on a restart, so right after one it lists only the few nodes that have pushed so far and \\\"unlisted\\\" is not yet \\\"retired\\\". Only names that are both confirmed and still unlisted are removed. With `dry_run=true` nothing is removed whatever the body says. It refuses (`409`) when the aggregator lists no node at all.",
    security(("session" = [])),
    params(
        ("dry_run" = Option<bool>, Query, description = "`true` only reports; `false` (the default) removes the confirmed names. Anything else is `400`.")
    ),
    request_body(content = PruneBody, content_type = "application/json", description = "The names to remove, as listed by an earlier call. May be empty."),
    responses(
        (status = 200, description = "What is unlisted and what was removed.", body = PruneAnswer, content_type = "application/json"),
        (status = 400, description = "`dry_run` is not `true` or `false`, or the body is not `{\"names\": [...]}`.", body = crate::openapi::ErrorBody),
        (status = 409, description = "The aggregator lists no nodes, so nothing is pruned.", body = crate::openapi::ErrorBody),
        (status = 502, description = "A delete failed (the body then also carries `removed`, what went before it), or the controller or aggregator answered something unexpected.", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller or no aggregator is configured.", body = crate::openapi::ErrorBody),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain")
    ),
    extensions(("x-wayhouse-role" = json!("operator")))
)]
#[allow(dead_code)]
async fn post_api_fleet_node_tags_prune() {}

#[utoipa::path(
    get, path = "/api/config", tag = "Configuration",
    summary = "The current config",
    description = "Forwards to the controller's `GET /config` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/). The config is read from the controller with the inline secrets scrubbed (`settings.admin.auth_token`, `settings.gossip.psk`) for a viewer or operator; an admin gets them. With no controller but a proxy configured (`--proxy-url`) the console talks to that proxy's config file instead (`GET /config/file`, `PUT /config/file`).",
    security(("session" = [])),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("viewer")))
)]
#[allow(dead_code)]
async fn get_api_config() {}

#[utoipa::path(
    get, path = "/api/config/schema", tag = "Configuration",
    summary = "The JSON Schema of the config",
    description = "Forwards to the controller's `GET /config/schema` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/). The schema the config editor validates against. With no controller but a proxy configured (`--proxy-url`) the console talks to that proxy's config file instead (`GET /config/file`, `PUT /config/file`).",
    security(("session" = [])),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("viewer")))
)]
#[allow(dead_code)]
async fn get_api_config_schema() {}

#[utoipa::path(
    get, path = "/api/config/revisions", tag = "Configuration",
    summary = "List config revisions",
    description = "Forwards to the controller's `GET /config/revisions` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/).",
    security(("session" = [])),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("viewer")))
)]
#[allow(dead_code)]
async fn get_api_config_revisions() {}

#[utoipa::path(
    get, path = "/api/config/revisions/{revision}", tag = "Configuration",
    summary = "Read one config revision",
    description = "Forwards to the controller's `GET /config/revisions/{revision}` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/). The inline secrets are scrubbed for a viewer or operator, as for the current config.",
    security(("session" = [])),
    params(
        ("revision" = String, Path, description = "The revision number.", example = "7")
    ),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("viewer")))
)]
#[allow(dead_code)]
async fn get_api_config_revisions_by_revision() {}

#[utoipa::path(
    get, path = "/api/config/revisions/{revision}/diff", tag = "Configuration",
    summary = "Diff a revision against another",
    description = "Forwards to the controller's `GET /config/revisions/{revision}/diff` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/). The inline secrets are scrubbed for a viewer or operator. Only `against` is forwarded from the query.",
    security(("session" = [])),
    params(
        ("revision" = String, Path, description = "The revision number.", example = "7"),
        ("against" = Option<u64>, Query, description = "The revision to compare with.", example = 6)
    ),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("viewer")))
)]
#[allow(dead_code)]
async fn get_api_config_revisions_by_revision_diff() {}

#[utoipa::path(
    get, path = "/api/config/pending", tag = "Configuration",
    summary = "The revision on trial",
    description = "Forwards to the controller's `GET /config/pending` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/). The revision awaiting confirmation, with its remaining time.",
    security(("session" = [])),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("viewer")))
)]
#[allow(dead_code)]
async fn get_api_config_pending() {}

#[utoipa::path(
    post, path = "/api/config", tag = "Configuration",
    summary = "Submit a config",
    description = "Forwards to the controller's `POST /config` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/). The body is the config as raw YAML text and is forwarded byte for byte. Of the query only `base`, `stage`, `group` and `confirm_within` are forwarded; the controller's `409` for a stale `base` passes through untouched. With no controller but a proxy configured (`--proxy-url`) the console talks to that proxy's config file instead (`GET /config/file`, `PUT /config/file`). The signed-in user's name goes along as `X-Actor`, so the controller records who acted.",
    security(("session" = [])),
    params(
        ("base" = Option<u64>, Query, description = "The revision the edit is based on; a newer head is refused.", example = 6),
        ("stage" = Option<String>, Query, description = "Stage for canary groups instead of promoting."),
        ("group" = Option<String>, Query, description = "A canary group to stage for; repeat for several."),
        ("confirm_within" = Option<u64>, Query, description = "Seconds to confirm the trial before it reverts.", example = 120)
    ),
    request_body(content = String, content_type = "text/plain", description = "The config as YAML."),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("admin")))
)]
#[allow(dead_code)]
async fn post_api_config() {}

#[utoipa::path(
    post, path = "/api/config/validate", tag = "Configuration",
    summary = "Validate a config",
    description = "Forwards to the controller's `POST /config/validate` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/). Nothing is stored. It is a read on the controller but admin-only here, because it is the editor's save-time check. With no controller but a proxy configured (`--proxy-url`) the console talks to that proxy's config file instead (`GET /config/file`, `PUT /config/file`).",
    security(("session" = [])),
    request_body(content = String, content_type = "text/plain", description = "The config as YAML."),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("admin")))
)]
#[allow(dead_code)]
async fn post_api_config_validate() {}

#[utoipa::path(
    post, path = "/api/config/rollback/{revision}", tag = "Configuration",
    summary = "Roll back to a revision",
    description = "Forwards to the controller's `POST /config/rollback/{revision}` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/). The signed-in user's name goes along as `X-Actor`, so the controller records who acted.",
    security(("session" = [])),
    params(
        ("revision" = String, Path, description = "The revision number.", example = "7")
    ),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("admin")))
)]
#[allow(dead_code)]
async fn post_api_config_rollback_by_revision() {}

#[utoipa::path(
    post, path = "/api/config/promote/{revision}", tag = "Configuration",
    summary = "Promote a staged revision",
    description = "Forwards to the controller's `POST /config/promote/{revision}` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/). The signed-in user's name goes along as `X-Actor`, so the controller records who acted.",
    security(("session" = [])),
    params(
        ("revision" = String, Path, description = "The revision number.", example = "7")
    ),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("admin")))
)]
#[allow(dead_code)]
async fn post_api_config_promote_by_revision() {}

#[utoipa::path(
    post, path = "/api/config/confirm/{revision}", tag = "Configuration",
    summary = "Confirm a revision on trial",
    description = "Forwards to the controller's `POST /config/confirm/{revision}` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/). The signed-in user's name goes along as `X-Actor`, so the controller records who acted.",
    security(("session" = [])),
    params(
        ("revision" = String, Path, description = "The revision number.", example = "7")
    ),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("admin")))
)]
#[allow(dead_code)]
async fn post_api_config_confirm_by_revision() {}

#[utoipa::path(
    post, path = "/api/config/revert", tag = "Configuration",
    summary = "Revert the revision on trial now",
    description = "Forwards to the controller's `POST /config/revert` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/). Only the `revision` query value is forwarded. The signed-in user's name goes along as `X-Actor`, so the controller records who acted.",
    security(("session" = [])),
    params(
        ("revision" = Option<u64>, Query, description = "The trial the operator saw; the revert applies only to it.", example = 7)
    ),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("admin")))
)]
#[allow(dead_code)]
async fn post_api_config_revert() {}

#[utoipa::path(
    get, path = "/api/tunnel/addresses", tag = "Tunnel and enrollment",
    summary = "The tunnel address table",
    description = "Forwards to the controller's `GET /tunnel/addresses` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/).",
    security(("session" = [])),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("viewer")))
)]
#[allow(dead_code)]
async fn get_api_tunnel_addresses() {}

#[utoipa::path(
    delete, path = "/api/tunnel/origins/{name}", tag = "Tunnel and enrollment",
    summary = "Release an origin's tunnel address",
    description = "Forwards to the controller's `DELETE /peers/{name}` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/). The signed-in user's name goes along as `X-Actor`, so the controller records who acted.",
    security(("session" = [])),
    params(
        ("name" = String, Path, description = "The name.", example = "panel")
    ),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("admin")))
)]
#[allow(dead_code)]
async fn delete_api_tunnel_origins_by_name() {}

#[utoipa::path(
    delete, path = "/api/tunnel/proxies/{name}", tag = "Tunnel and enrollment",
    summary = "Release a proxy's tunnel address",
    description = "Forwards to the controller's `DELETE /proxy-peers/{name}` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/). The signed-in user's name goes along as `X-Actor`, so the controller records who acted.",
    security(("session" = [])),
    params(
        ("name" = String, Path, description = "The name.", example = "panel")
    ),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("admin")))
)]
#[allow(dead_code)]
async fn delete_api_tunnel_proxies_by_name() {}

#[utoipa::path(
    post, path = "/api/tunnel/origins/{name}/psk/rotate", tag = "Tunnel and enrollment",
    summary = "Rotate an origin's tunnel key",
    description = "Forwards to the controller's `POST /peers/{name}/psk/rotate` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/). The signed-in user's name goes along as `X-Actor`, so the controller records who acted.",
    security(("session" = [])),
    params(
        ("name" = String, Path, description = "The name.", example = "panel")
    ),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("admin")))
)]
#[allow(dead_code)]
async fn post_api_tunnel_origins_by_name_psk_rotate() {}

#[utoipa::path(
    post, path = "/api/enrollments", tag = "Tunnel and enrollment",
    summary = "Mint an enrollment code",
    description = "Forwards to the controller's `POST /enrollments` (see the [Controller API](/reference/controller-api/)). Where the controller needs a step-up proof, the body may carry the step-up **passphrase** as a string `step_up`: the console fetches the controller's salt (`GET /enrollments/step-up`), derives the key, signs this exact request with a short expiry and forwards the signed assertion instead. The passphrase is never logged or stored. The answer carries the code, so it is sent `Cache-Control: no-store`. A controller with no step-up key answers `409` to a body with a passphrase. At most 4 KiB.",
    security(("session" = [])),
    request_body(content = serde_json::Value, content_type = "application/json", description = "The mint request (`role`, `name`, ...), with an optional `step_up` passphrase."),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 400, description = "The step-up request could not be signed.", body = crate::openapi::ErrorBody),
        (status = 409, description = "This controller has no step-up key (`--enroll-step-up-pubkey-file`).", body = crate::openapi::ErrorBody),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("admin")))
)]
#[allow(dead_code)]
async fn post_api_enrollments() {}

#[utoipa::path(
    get, path = "/api/enrollments", tag = "Tunnel and enrollment",
    summary = "List enrollment codes",
    description = "Forwards to the controller's `GET /enrollments` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/). Codes with their state, never the code itself.",
    security(("session" = [])),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("admin")))
)]
#[allow(dead_code)]
async fn get_api_enrollments() {}

#[utoipa::path(
    delete, path = "/api/enrollments/{id}", tag = "Tunnel and enrollment",
    summary = "Revoke an enrollment code",
    description = "Forwards to the controller's `DELETE /enrollments/{id}` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/). Revokes a code, spent or not. The signed-in user's name goes along as `X-Actor`, so the controller records who acted.",
    security(("session" = [])),
    params(
        ("id" = String, Path, description = "The code's id.", example = "0123abcd")
    ),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("admin")))
)]
#[allow(dead_code)]
async fn delete_api_enrollments_by_id() {}

#[utoipa::path(
    get, path = "/api/plugins", tag = "Plugins",
    summary = "List plugin installs",
    description = "Forwards to the controller's `GET /plugins` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/).",
    security(("session" = [])),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("viewer")))
)]
#[allow(dead_code)]
async fn get_api_plugins() {}

#[utoipa::path(
    get, path = "/api/plugin-routes", tag = "Plugins",
    summary = "The routes plugins have set",
    description = "Forwards to the controller's `GET /plugin-routes` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/).",
    security(("session" = [])),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("viewer")))
)]
#[allow(dead_code)]
async fn get_api_plugin_routes() {}

#[utoipa::path(
    get, path = "/api/plugins/{id}", tag = "Plugins",
    summary = "Read one plugin install",
    description = "Forwards to the controller's `GET /plugins/{id}` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/).",
    security(("session" = [])),
    params(
        ("id" = String, Path, description = "The id.", example = "0123abcd")
    ),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("viewer")))
)]
#[allow(dead_code)]
async fn get_api_plugins_by_id() {}

#[utoipa::path(
    get, path = "/api/plugins/{id}/status", tag = "Plugins",
    summary = "A plugin's status",
    description = "Forwards to the controller's `GET /plugins/{id}/status` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/).",
    security(("session" = [])),
    params(
        ("id" = String, Path, description = "The id.", example = "0123abcd")
    ),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("viewer")))
)]
#[allow(dead_code)]
async fn get_api_plugins_by_id_status() {}

#[utoipa::path(
    post, path = "/api/plugins/modules", tag = "Plugins",
    summary = "Upload a plugin module",
    description = "Forwards to the controller's `POST /plugins/modules` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/). The module bytes (up to 8 MiB) go through untouched; the controller inspects and stores them. The signed-in user's name goes along as `X-Actor`, so the controller records who acted.",
    security(("session" = [])),
    request_body(content = Vec<u8>, content_type = "application/octet-stream", description = "The `.wasm` module."),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("admin")))
)]
#[allow(dead_code)]
async fn post_api_plugins_modules() {}

#[utoipa::path(
    post, path = "/api/plugins", tag = "Plugins",
    summary = "Install a plugin",
    description = "Forwards to the controller's `POST /plugins` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/). The signed-in user's name goes along as `X-Actor`, so the controller records who acted.",
    security(("session" = [])),
    request_body(content = serde_json::Value, content_type = "application/json", description = "The install request."),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("admin")))
)]
#[allow(dead_code)]
async fn post_api_plugins() {}

#[utoipa::path(
    delete, path = "/api/plugins/{id}", tag = "Plugins",
    summary = "Remove a plugin install",
    description = "Forwards to the controller's `DELETE /plugins/{id}` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/). The signed-in user's name goes along as `X-Actor`, so the controller records who acted.",
    security(("session" = [])),
    params(
        ("id" = String, Path, description = "The id.", example = "0123abcd")
    ),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("admin")))
)]
#[allow(dead_code)]
async fn delete_api_plugins_by_id() {}

#[utoipa::path(
    post, path = "/api/plugins/{id}/enable", tag = "Plugins",
    summary = "Enable a plugin",
    description = "Forwards to the controller's `POST /plugins/{id}/enable` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/). The signed-in user's name goes along as `X-Actor`, so the controller records who acted.",
    security(("session" = [])),
    params(
        ("id" = String, Path, description = "The id.", example = "0123abcd")
    ),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("admin")))
)]
#[allow(dead_code)]
async fn post_api_plugins_by_id_enable() {}

#[utoipa::path(
    post, path = "/api/plugins/{id}/disable", tag = "Plugins",
    summary = "Disable a plugin",
    description = "Forwards to the controller's `POST /plugins/{id}/disable` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/). The signed-in user's name goes along as `X-Actor`, so the controller records who acted.",
    security(("session" = [])),
    params(
        ("id" = String, Path, description = "The id.", example = "0123abcd")
    ),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("admin")))
)]
#[allow(dead_code)]
async fn post_api_plugins_by_id_disable() {}

#[utoipa::path(
    put, path = "/api/plugins/{id}/config", tag = "Plugins",
    summary = "Replace a plugin's options",
    description = "Forwards to the controller's `PUT /plugins/{id}/config` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/). At most 80 KiB (the controller caps the config at 64 KiB). The signed-in user's name goes along as `X-Actor`, so the controller records who acted.",
    security(("session" = [])),
    params(
        ("id" = String, Path, description = "The id.", example = "0123abcd")
    ),
    request_body(content = serde_json::Value, content_type = "application/json", description = "The options."),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("admin")))
)]
#[allow(dead_code)]
async fn put_api_plugins_by_id_config() {}

#[utoipa::path(
    get, path = "/api/plugins/file/plan", tag = "Plugins",
    summary = "Dry-run the plugins file",
    description = "Forwards to the controller's `GET /plugins/file/plan` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/).",
    security(("session" = [])),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("admin")))
)]
#[allow(dead_code)]
async fn get_api_plugins_file_plan() {}

#[utoipa::path(
    post, path = "/api/plugins/{id}/webhook", tag = "Plugins",
    summary = "Enable a plugin's webhook",
    description = "Forwards to the controller's `POST /plugins/{id}/webhook` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/). The signed-in user's name goes along as `X-Actor`, so the controller records who acted.",
    security(("session" = [])),
    params(
        ("id" = String, Path, description = "The id.", example = "0123abcd")
    ),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("admin")))
)]
#[allow(dead_code)]
async fn post_api_plugins_by_id_webhook() {}

#[utoipa::path(
    delete, path = "/api/plugins/{id}/webhook", tag = "Plugins",
    summary = "Revoke a plugin's webhook",
    description = "Forwards to the controller's `DELETE /plugins/{id}/webhook` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/). The signed-in user's name goes along as `X-Actor`, so the controller records who acted.",
    security(("session" = [])),
    params(
        ("id" = String, Path, description = "The id.", example = "0123abcd")
    ),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("admin")))
)]
#[allow(dead_code)]
async fn delete_api_plugins_by_id_webhook() {}

#[utoipa::path(
    get, path = "/api/secrets", tag = "Secrets and keys",
    summary = "List cluster secrets",
    description = "Forwards to the controller's `GET /secrets` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/). Names only, never values.",
    security(("session" = [])),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("admin")))
)]
#[allow(dead_code)]
async fn get_api_secrets() {}

#[utoipa::path(
    put, path = "/api/secrets/{name}", tag = "Secrets and keys",
    summary = "Set a cluster secret",
    description = "Forwards to the controller's `PUT /secrets/{name}` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/). The value rides in the body to the controller, which seals it; the console never logs or keeps it. At most 16 KiB. The signed-in user's name goes along as `X-Actor`, so the controller records who acted.",
    security(("session" = [])),
    params(
        ("name" = String, Path, description = "The name.", example = "panel")
    ),
    request_body(content = serde_json::Value, content_type = "application/json", description = "The secret."),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("admin")))
)]
#[allow(dead_code)]
async fn put_api_secrets_by_name() {}

#[utoipa::path(
    delete, path = "/api/secrets/{name}", tag = "Secrets and keys",
    summary = "Delete a cluster secret",
    description = "Forwards to the controller's `DELETE /secrets/{name}` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/). The signed-in user's name goes along as `X-Actor`, so the controller records who acted.",
    security(("session" = [])),
    params(
        ("name" = String, Path, description = "The name.", example = "panel")
    ),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("admin")))
)]
#[allow(dead_code)]
async fn delete_api_secrets_by_name() {}

#[utoipa::path(
    get, path = "/api/plugins/{id}/secrets", tag = "Secrets and keys",
    summary = "List a plugin's secret slots",
    description = "Forwards to the controller's `GET /plugins/{id}/secrets` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/).",
    security(("session" = [])),
    params(
        ("id" = String, Path, description = "The id.", example = "0123abcd")
    ),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("admin")))
)]
#[allow(dead_code)]
async fn get_api_plugins_by_id_secrets() {}

#[utoipa::path(
    put, path = "/api/plugins/{id}/secrets/{slot}", tag = "Secrets and keys",
    summary = "Set a plugin secret",
    description = "Forwards to the controller's `PUT /plugins/{id}/secrets/{slot}` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/). The value is forwarded and never logged or echoed. At most 16 KiB. The signed-in user's name goes along as `X-Actor`, so the controller records who acted.",
    security(("session" = [])),
    params(
        ("id" = String, Path, description = "The id.", example = "0123abcd"),
        ("slot" = String, Path, description = "The secret slot name.", example = "TOKEN")
    ),
    request_body(content = serde_json::Value, content_type = "application/json", description = "The secret."),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("admin")))
)]
#[allow(dead_code)]
async fn put_api_plugins_by_id_secrets_by_slot() {}

#[utoipa::path(
    delete, path = "/api/plugins/{id}/secrets/{slot}", tag = "Secrets and keys",
    summary = "Delete a plugin secret",
    description = "Forwards to the controller's `DELETE /plugins/{id}/secrets/{slot}` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/). The signed-in user's name goes along as `X-Actor`, so the controller records who acted.",
    security(("session" = [])),
    params(
        ("id" = String, Path, description = "The id.", example = "0123abcd"),
        ("slot" = String, Path, description = "The secret slot name.", example = "TOKEN")
    ),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("admin")))
)]
#[allow(dead_code)]
async fn delete_api_plugins_by_id_secrets_by_slot() {}

#[utoipa::path(
    get, path = "/api/plugins/secrets/keys", tag = "Secrets and keys",
    summary = "The secret key ids each node holds",
    description = "Forwards to the controller's `GET /plugins/secrets/keys` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/). Key ids only, never key material.",
    security(("session" = [])),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("admin")))
)]
#[allow(dead_code)]
async fn get_api_plugins_secrets_keys() {}

#[utoipa::path(
    post, path = "/api/plugins/secrets/rewrap", tag = "Secrets and keys",
    summary = "Re-encrypt plugin secrets under the active key",
    description = "Forwards to the controller's `POST /plugins/secrets/rewrap` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/). Step two of a key rotation. The signed-in user's name goes along as `X-Actor`, so the controller records who acted.",
    security(("session" = [])),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("admin")))
)]
#[allow(dead_code)]
async fn post_api_plugins_secrets_rewrap() {}

#[utoipa::path(
    post, path = "/api/acme/rewrap", tag = "Secrets and keys",
    summary = "Re-encrypt the ACME keys under the active key",
    description = "Forwards to the controller's `POST /acme/rewrap` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/). Step two of a key rotation. A controller without ACME answers `404`. The signed-in user's name goes along as `X-Actor`, so the controller records who acted.",
    security(("session" = [])),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("admin")))
)]
#[allow(dead_code)]
async fn post_api_acme_rewrap() {}

#[utoipa::path(
    post, path = "/api/secrets/rewrap", tag = "Secrets and keys",
    summary = "Re-encrypt cluster secrets under the active key",
    description = "Forwards to the controller's `POST /secrets/rewrap` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/). Step two of a key rotation. The signed-in user's name goes along as `X-Actor`, so the controller records who acted.",
    security(("session" = [])),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("admin")))
)]
#[allow(dead_code)]
async fn post_api_secrets_rewrap() {}

#[utoipa::path(
    get, path = "/api/acme/sites", tag = "Certificates",
    summary = "List ACME sites",
    description = "Forwards to the controller's `GET /acme/sites` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/).",
    security(("session" = [])),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("viewer")))
)]
#[allow(dead_code)]
async fn get_api_acme_sites() {}

#[utoipa::path(
    get, path = "/api/acme/gateways", tag = "Certificates",
    summary = "List ACME gateways",
    description = "Forwards to the controller's `GET /acme/gateways` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/).",
    security(("session" = [])),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("viewer")))
)]
#[allow(dead_code)]
async fn get_api_acme_gateways() {}

#[utoipa::path(
    get, path = "/api/acme/account", tag = "Certificates",
    summary = "The ACME account",
    description = "Forwards to the controller's `GET /acme/account` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/).",
    security(("session" = [])),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("viewer")))
)]
#[allow(dead_code)]
async fn get_api_acme_account() {}

#[utoipa::path(
    put, path = "/api/acme/sites/{name}", tag = "Certificates",
    summary = "Create or replace an ACME site",
    description = "Forwards to the controller's `PUT /acme/sites/{name}` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/). At most 64 KiB. The console logs one line per ACME write, naming the user and the site. The signed-in user's name goes along as `X-Actor`, so the controller records who acted.",
    security(("session" = [])),
    params(
        ("name" = String, Path, description = "The name.", example = "panel")
    ),
    request_body(content = serde_json::Value, content_type = "application/json", description = "The site."),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("admin")))
)]
#[allow(dead_code)]
async fn put_api_acme_sites_by_name() {}

#[utoipa::path(
    delete, path = "/api/acme/sites/{name}", tag = "Certificates",
    summary = "Remove an ACME site",
    description = "Forwards to the controller's `DELETE /acme/sites/{name}` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/). Removes the site, its certificates and its retry state. The signed-in user's name goes along as `X-Actor`, so the controller records who acted.",
    security(("session" = [])),
    params(
        ("name" = String, Path, description = "The name.", example = "panel")
    ),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("admin")))
)]
#[allow(dead_code)]
async fn delete_api_acme_sites_by_name() {}

#[utoipa::path(
    post, path = "/api/acme/gateways", tag = "Certificates",
    summary = "Register an ACME gateway",
    description = "Forwards to the controller's `POST /acme/gateways` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/). The answer carries the gateway's token, once, and is passed through without being logged. At most 1 KiB. The signed-in user's name goes along as `X-Actor`, so the controller records who acted.",
    security(("session" = [])),
    request_body(content = serde_json::Value, content_type = "application/json", description = "The gateway's name."),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("admin")))
)]
#[allow(dead_code)]
async fn post_api_acme_gateways() {}

#[utoipa::path(
    delete, path = "/api/acme/gateways/{name}", tag = "Certificates",
    summary = "Revoke an ACME gateway",
    description = "Forwards to the controller's `DELETE /acme/gateways/{name}` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/). Its token answers `401` at once. The signed-in user's name goes along as `X-Actor`, so the controller records who acted.",
    security(("session" = [])),
    params(
        ("name" = String, Path, description = "The name.", example = "panel")
    ),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("admin")))
)]
#[allow(dead_code)]
async fn delete_api_acme_gateways_by_name() {}

#[utoipa::path(
    get, path = "/api/features", tag = "Feature flags",
    summary = "The effective feature flags",
    description = "Forwards to the controller's `GET /features` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/).",
    security(("session" = [])),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("viewer")))
)]
#[allow(dead_code)]
async fn get_api_features() {}

#[utoipa::path(
    get, path = "/api/features/overrides", tag = "Feature flags",
    summary = "The feature overrides",
    description = "Forwards to the controller's `GET /features/overrides` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/).",
    security(("session" = [])),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("viewer")))
)]
#[allow(dead_code)]
async fn get_api_features_overrides() {}

#[utoipa::path(
    put, path = "/api/features/overrides/{name}", tag = "Feature flags",
    summary = "Set a feature override",
    description = "Forwards to the controller's `PUT /features/overrides/{name}` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/). At most 8 KiB. The signed-in user's name goes along as `X-Actor`, so the controller records who acted.",
    security(("session" = [])),
    params(
        ("name" = String, Path, description = "The flag's name.", example = "panel")
    ),
    request_body(content = serde_json::Value, content_type = "application/json", description = "The override: a value, a reason and an optional expiry."),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("admin")))
)]
#[allow(dead_code)]
async fn put_api_features_overrides_by_name() {}

#[utoipa::path(
    delete, path = "/api/features/overrides/{name}", tag = "Feature flags",
    summary = "Clear a feature override",
    description = "Forwards to the controller's `DELETE /features/overrides/{name}` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/). The signed-in user's name goes along as `X-Actor`, so the controller records who acted.",
    security(("session" = [])),
    params(
        ("name" = String, Path, description = "The flag's name.", example = "panel")
    ),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("admin")))
)]
#[allow(dead_code)]
async fn delete_api_features_overrides_by_name() {}

#[utoipa::path(
    get, path = "/api/ui/layout", tag = "Console records",
    summary = "Read the form layout override",
    description = "Forwards to the controller's `GET /ui/layout` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/).",
    security(("session" = [])),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("viewer")))
)]
#[allow(dead_code)]
async fn get_api_ui_layout() {}

#[utoipa::path(
    put, path = "/api/ui/layout", tag = "Console records",
    summary = "Save the form layout override",
    description = "Forwards to the controller's `PUT /ui/layout` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/). The signed-in user's name goes along as `X-Actor`, so the controller records who acted.",
    security(("session" = [])),
    request_body(content = serde_json::Value, content_type = "application/json", description = "The record."),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("admin")))
)]
#[allow(dead_code)]
async fn put_api_ui_layout() {}

#[utoipa::path(
    delete, path = "/api/ui/layout", tag = "Console records",
    summary = "Delete the form layout override",
    description = "Forwards to the controller's `DELETE /ui/layout` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/). The signed-in user's name goes along as `X-Actor`, so the controller records who acted.",
    security(("session" = [])),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("admin")))
)]
#[allow(dead_code)]
async fn delete_api_ui_layout() {}

#[utoipa::path(
    get, path = "/api/ui/defaults", tag = "Console records",
    summary = "Read the deployment defaults",
    description = "Forwards to the controller's `GET /ui/defaults` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/).",
    security(("session" = [])),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("viewer")))
)]
#[allow(dead_code)]
async fn get_api_ui_defaults() {}

#[utoipa::path(
    put, path = "/api/ui/defaults", tag = "Console records",
    summary = "Save the deployment defaults",
    description = "Forwards to the controller's `PUT /ui/defaults` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/). The signed-in user's name goes along as `X-Actor`, so the controller records who acted.",
    security(("session" = [])),
    request_body(content = serde_json::Value, content_type = "application/json", description = "The record."),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("admin")))
)]
#[allow(dead_code)]
async fn put_api_ui_defaults() {}

#[utoipa::path(
    delete, path = "/api/ui/defaults", tag = "Console records",
    summary = "Delete the deployment defaults",
    description = "Forwards to the controller's `DELETE /ui/defaults` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/). The signed-in user's name goes along as `X-Actor`, so the controller records who acted.",
    security(("session" = [])),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("admin")))
)]
#[allow(dead_code)]
async fn delete_api_ui_defaults() {}

#[utoipa::path(
    get, path = "/api/ui/dashboards", tag = "Console records",
    summary = "List the dashboards",
    description = "Forwards to the controller's `GET /ui/dashboards` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/).",
    security(("session" = [])),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("viewer")))
)]
#[allow(dead_code)]
async fn get_api_ui_dashboards() {}

#[utoipa::path(
    get, path = "/api/ui/dashboards/{name}", tag = "Console records",
    summary = "Read a dashboard",
    description = "Forwards to the controller's `GET /ui/dashboards/{name}` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/).",
    security(("session" = [])),
    params(
        ("name" = String, Path, description = "The record's name.", example = "panel")
    ),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("viewer")))
)]
#[allow(dead_code)]
async fn get_api_ui_dashboards_by_name() {}

#[utoipa::path(
    put, path = "/api/ui/dashboards/{name}", tag = "Console records",
    summary = "Save a dashboard",
    description = "Forwards to the controller's `PUT /ui/dashboards/{name}` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/). The signed-in user's name goes along as `X-Actor`, so the controller records who acted.",
    security(("session" = [])),
    params(
        ("name" = String, Path, description = "The record's name.", example = "panel")
    ),
    request_body(content = serde_json::Value, content_type = "application/json", description = "The record."),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("operator")))
)]
#[allow(dead_code)]
async fn put_api_ui_dashboards_by_name() {}

#[utoipa::path(
    delete, path = "/api/ui/dashboards/{name}", tag = "Console records",
    summary = "Delete a dashboard",
    description = "Forwards to the controller's `DELETE /ui/dashboards/{name}` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/). The signed-in user's name goes along as `X-Actor`, so the controller records who acted.",
    security(("session" = [])),
    params(
        ("name" = String, Path, description = "The record's name.", example = "panel")
    ),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("operator")))
)]
#[allow(dead_code)]
async fn delete_api_ui_dashboards_by_name() {}

#[utoipa::path(
    get, path = "/api/ui/filters", tag = "Console records",
    summary = "List the saved filters",
    description = "Forwards to the controller's `GET /ui/filters` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/).",
    security(("session" = [])),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("viewer")))
)]
#[allow(dead_code)]
async fn get_api_ui_filters() {}

#[utoipa::path(
    get, path = "/api/ui/filters/{name}", tag = "Console records",
    summary = "Read a saved filter",
    description = "Forwards to the controller's `GET /ui/filters/{name}` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/).",
    security(("session" = [])),
    params(
        ("name" = String, Path, description = "The record's name.", example = "panel")
    ),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("viewer")))
)]
#[allow(dead_code)]
async fn get_api_ui_filters_by_name() {}

#[utoipa::path(
    put, path = "/api/ui/filters/{name}", tag = "Console records",
    summary = "Save a saved filter",
    description = "Forwards to the controller's `PUT /ui/filters/{name}` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/). The signed-in user's name goes along as `X-Actor`, so the controller records who acted.",
    security(("session" = [])),
    params(
        ("name" = String, Path, description = "The record's name.", example = "panel")
    ),
    request_body(content = serde_json::Value, content_type = "application/json", description = "The record."),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("operator")))
)]
#[allow(dead_code)]
async fn put_api_ui_filters_by_name() {}

#[utoipa::path(
    delete, path = "/api/ui/filters/{name}", tag = "Console records",
    summary = "Delete a saved filter",
    description = "Forwards to the controller's `DELETE /ui/filters/{name}` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/). The signed-in user's name goes along as `X-Actor`, so the controller records who acted.",
    security(("session" = [])),
    params(
        ("name" = String, Path, description = "The record's name.", example = "panel")
    ),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("operator")))
)]
#[allow(dead_code)]
async fn delete_api_ui_filters_by_name() {}

#[utoipa::path(
    get, path = "/api/ui/themes", tag = "Console records",
    summary = "List the themes",
    description = "Forwards to the controller's `GET /ui/themes` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/).",
    security(("session" = [])),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("viewer")))
)]
#[allow(dead_code)]
async fn get_api_ui_themes() {}

#[utoipa::path(
    get, path = "/api/ui/themes/{name}", tag = "Console records",
    summary = "Read a theme",
    description = "Forwards to the controller's `GET /ui/themes/{name}` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/).",
    security(("session" = [])),
    params(
        ("name" = String, Path, description = "The record's name.", example = "panel")
    ),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("viewer")))
)]
#[allow(dead_code)]
async fn get_api_ui_themes_by_name() {}

#[utoipa::path(
    put, path = "/api/ui/themes/{name}", tag = "Console records",
    summary = "Save a theme",
    description = "Forwards to the controller's `PUT /ui/themes/{name}` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/). The signed-in user's name goes along as `X-Actor`, so the controller records who acted.",
    security(("session" = [])),
    params(
        ("name" = String, Path, description = "The record's name.", example = "panel")
    ),
    request_body(content = serde_json::Value, content_type = "application/json", description = "The record."),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("admin")))
)]
#[allow(dead_code)]
async fn put_api_ui_themes_by_name() {}

#[utoipa::path(
    delete, path = "/api/ui/themes/{name}", tag = "Console records",
    summary = "Delete a theme",
    description = "Forwards to the controller's `DELETE /ui/themes/{name}` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/). The signed-in user's name goes along as `X-Actor`, so the controller records who acted.",
    security(("session" = [])),
    params(
        ("name" = String, Path, description = "The record's name.", example = "panel")
    ),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("admin")))
)]
#[allow(dead_code)]
async fn delete_api_ui_themes_by_name() {}

#[utoipa::path(
    post, path = "/api/ui/dashboards/{name}/rename", tag = "Console records",
    summary = "Rename a dashboard",
    description = "Forwards to the controller's `POST /ui/dashboards/{name}/rename` and returns its answer, with the status and headers unchanged. The request and the answer are described under that route in the [Controller API](/reference/controller-api/). The signed-in user's name goes along as `X-Actor`, so the controller records who acted.",
    security(("session" = [])),
    params(
        ("name" = String, Path, description = "The record's name.", example = "panel")
    ),
    request_body(content = serde_json::Value, content_type = "application/json", description = "The new name."),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain"),
        (status = 502, description = "The controller could not be reached, or its answer was too large (16 MiB).", body = crate::openapi::ErrorBody),
        (status = 503, description = "No controller is configured (`--controller-url`).", body = crate::openapi::ErrorBody)
    ),
    extensions(("x-wayhouse-role" = json!("operator")))
)]
#[allow(dead_code)]
async fn post_api_ui_dashboards_by_name_rename() {}

#[utoipa::path(
    get, path = "/api/registries", tag = "Sniffer registries",
    summary = "List sniffer registries",
    description = "The registries the console draws sniffers from. `persistent` says whether additions survive a restart (a registries file is configured).",
    security(("session" = [])),
    responses(
        (status = 200, description = "The registries.", body = RegistryList, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain")
    ),
    extensions(("x-wayhouse-role" = json!("viewer")))
)]
#[allow(dead_code)]
async fn get_api_registries() {}

#[utoipa::path(
    get, path = "/api/registries/{id}/sniffers", tag = "Sniffer registries",
    summary = "List a registry's sniffers",
    description = "Fetches the registry's index and answers its sniffers.",
    security(("session" = [])),
    params(
        ("id" = String, Path, description = "The registry's id, from the registry list.", example = "0123abcd")
    ),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 404, description = "No such registry (or one of the other kind).", body = crate::openapi::ErrorBody),
        (status = 502, description = "The registry could not be reached or its index could not be read.", body = crate::openapi::ErrorBody),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain")
    ),
    extensions(("x-wayhouse-role" = json!("viewer")))
)]
#[allow(dead_code)]
async fn get_api_registries_by_id_sniffers() {}

#[utoipa::path(
    get, path = "/api/registries/{id}/sniffers/{name}", tag = "Sniffer registries",
    summary = "One sniffer with every version",
    description = "",
    security(("session" = [])),
    params(
        ("id" = String, Path, description = "The registry's id, from the registry list.", example = "0123abcd"),
        ("name" = String, Path, description = "The sniffer's name.", example = "panel")
    ),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 404, description = "No such registry, or no such sniffer in it.", body = crate::openapi::ErrorBody),
        (status = 502, description = "The registry could not be reached or its index could not be read.", body = crate::openapi::ErrorBody),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain")
    ),
    extensions(("x-wayhouse-role" = json!("viewer")))
)]
#[allow(dead_code)]
async fn get_api_registries_by_id_sniffers_by_name() {}

#[utoipa::path(
    post, path = "/api/registries", tag = "Sniffer registries",
    summary = "Add a sniffer registry",
    description = "Only an `https` URL without credentials is taken. The registry is added to the list and, with a registries file configured, saved there.",
    security(("session" = [])),
    request_body(content = AddRegistryBody, content_type = "application/json", description = "The registry."),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 400, description = "The URL is invalid or carries credentials.", body = crate::openapi::ErrorBody),
        (status = 409, description = "Too many registries, or the URL is already a registry of the other kind.", body = crate::openapi::ErrorBody),
        (status = 500, description = "The registry could not be saved.", body = crate::openapi::ErrorBody),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain")
    ),
    extensions(("x-wayhouse-role" = json!("admin")))
)]
#[allow(dead_code)]
async fn post_api_registries() {}

#[utoipa::path(
    delete, path = "/api/registries/{id}", tag = "Sniffer registries",
    summary = "Remove a sniffer registry",
    description = "The built-in official registry can be removed too.",
    security(("session" = [])),
    params(
        ("id" = String, Path, description = "The registry's id, from the registry list.", example = "0123abcd")
    ),
    responses(
        (status = 204, description = "Removed."),
        (status = 404, description = "No such registry.", body = crate::openapi::ErrorBody),
        (status = 500, description = "The change could not be saved.", body = crate::openapi::ErrorBody),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain")
    ),
    extensions(("x-wayhouse-role" = json!("admin")))
)]
#[allow(dead_code)]
async fn delete_api_registries_by_id() {}

#[utoipa::path(
    post, path = "/api/registries/{id}/install", tag = "Sniffer registries",
    summary = "Install a sniffer from a registry",
    description = "Downloads the module, checks its hash and, for the official registry, its signature, then uploads it to every instance through the aggregator. Refused with `503` while the controller's `registry_install` feature flag is off. `200` means every instance took it, `207` that some did not (or need a pin), `502` that none did.",
    security(("session" = [])),
    params(
        ("id" = String, Path, description = "The registry's id, from the registry list.", example = "0123abcd")
    ),
    request_body(content = InstallBody, content_type = "application/json", description = "What to install."),
    responses(
        (status = 200, description = "Installed everywhere.", body = InstallAnswer, content_type = "application/json"),
        (status = 207, description = "Installed on some instances only; see `results`.", body = InstallAnswer),
        (status = 404, description = "No such registry, sniffer or version.", body = crate::openapi::ErrorBody),
        (status = 422, description = "Refused: the signature or hash did not verify, the module is too large or built for another ABI.", body = crate::openapi::ErrorBody),
        (status = 502, description = "Installed nowhere, or the registry or aggregator could not be reached.", body = crate::openapi::ErrorBody),
        (status = 503, description = "Installs are switched off (`registry_install`), or no proxy is registered with the aggregator.", body = crate::openapi::ErrorBody),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain")
    ),
    extensions(("x-wayhouse-role" = json!("operator")))
)]
#[allow(dead_code)]
async fn post_api_registries_by_id_install() {}

#[utoipa::path(
    post, path = "/api/registries/updates/check", tag = "Sniffer registries",
    summary = "Check installed sniffers for updates",
    description = "Compares what the fleet runs with what the registries offer. Needs the aggregator for the fleet's side.",
    security(("session" = [])),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain")
    ),
    extensions(("x-wayhouse-role" = json!("operator")))
)]
#[allow(dead_code)]
async fn post_api_registries_updates_check() {}

#[utoipa::path(
    get, path = "/api/plugin-registries", tag = "Plugin registries",
    summary = "List plugin registries",
    description = "",
    security(("session" = [])),
    responses(
        (status = 200, description = "The registries.", body = RegistryList, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain")
    ),
    extensions(("x-wayhouse-role" = json!("viewer")))
)]
#[allow(dead_code)]
async fn get_api_plugin_registries() {}

#[utoipa::path(
    get, path = "/api/plugin-registries/{id}/plugins", tag = "Plugin registries",
    summary = "List a registry's plugins",
    description = "",
    security(("session" = [])),
    params(
        ("id" = String, Path, description = "The registry's id, from the registry list.", example = "0123abcd")
    ),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 404, description = "No such plugin registry.", body = crate::openapi::ErrorBody),
        (status = 502, description = "The registry could not be reached or its index could not be read.", body = crate::openapi::ErrorBody),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain")
    ),
    extensions(("x-wayhouse-role" = json!("viewer")))
)]
#[allow(dead_code)]
async fn get_api_plugin_registries_by_id_plugins() {}

#[utoipa::path(
    get, path = "/api/plugin-registries/{id}/plugins/{name}", tag = "Plugin registries",
    summary = "One plugin with every version",
    description = "",
    security(("session" = [])),
    params(
        ("id" = String, Path, description = "The registry's id, from the registry list.", example = "0123abcd"),
        ("name" = String, Path, description = "The plugin's name.", example = "panel")
    ),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 404, description = "No such registry, or no such plugin in it.", body = crate::openapi::ErrorBody),
        (status = 502, description = "The registry could not be reached or its index could not be read.", body = crate::openapi::ErrorBody),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain")
    ),
    extensions(("x-wayhouse-role" = json!("viewer")))
)]
#[allow(dead_code)]
async fn get_api_plugin_registries_by_id_plugins_by_name() {}

#[utoipa::path(
    post, path = "/api/plugin-registries", tag = "Plugin registries",
    summary = "Add a plugin registry",
    description = "",
    security(("session" = [])),
    request_body(content = AddRegistryBody, content_type = "application/json", description = "The registry."),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 400, description = "The URL is invalid or carries credentials.", body = crate::openapi::ErrorBody),
        (status = 409, description = "Too many registries, or the URL is already a registry of the other kind.", body = crate::openapi::ErrorBody),
        (status = 500, description = "The registry could not be saved.", body = crate::openapi::ErrorBody),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain")
    ),
    extensions(("x-wayhouse-role" = json!("admin")))
)]
#[allow(dead_code)]
async fn post_api_plugin_registries() {}

#[utoipa::path(
    post, path = "/api/plugin-registries/{id}/fetch", tag = "Plugin registries",
    summary = "Fetch a plugin from a registry",
    description = "Downloads the module, checks its hash and signature and uploads it to the controller's `POST /api/plugins/modules`, which stores it. It does not install or enable anything: the module waits for approval. A module whose signature cannot be verified answers `409` with `needs_confirm`; repeat with `allow_unsigned` to fetch it anyway.",
    security(("session" = [])),
    params(
        ("id" = String, Path, description = "The registry's id, from the registry list.", example = "0123abcd")
    ),
    request_body(content = FetchBody, content_type = "application/json", description = "What to fetch."),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 404, description = "No such registry, plugin or version.", body = crate::openapi::ErrorBody),
        (status = 409, description = "The signature could not be verified; `needs_confirm` is true.", body = crate::openapi::ErrorBody),
        (status = 422, description = "Refused: the hash or signature is wrong, or the version is built for another plugin ABI.", body = crate::openapi::ErrorBody),
        (status = 502, description = "The registry could not be reached.", body = crate::openapi::ErrorBody),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain")
    ),
    extensions(("x-wayhouse-role" = json!("admin")))
)]
#[allow(dead_code)]
async fn post_api_plugin_registries_by_id_fetch() {}

#[utoipa::path(
    get, path = "/api/process", tag = "Process settings",
    summary = "Launch settings of every program",
    description = "Each program's settings file (`--settings-dir`), the flags the program published when it last started, and whether it is connected. A viewer sees no values, only that a change is pending.",
    security(("session" = [])),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain")
    ),
    extensions(("x-wayhouse-role" = json!("viewer")))
)]
#[allow(dead_code)]
async fn get_api_process() {}

#[utoipa::path(
    put, path = "/api/process/{program}", tag = "Process settings",
    summary = "Change a program's launch settings",
    description = "Writes the program's settings file as a trial: the program notices it, restarts, and undoes the change unless it is confirmed within `confirm_secs` (60). `rev` must be the revision just read; a change in between answers `409`. Credential flags (tokens, passwords, PSKs) are refused.",
    security(("session" = [])),
    params(
        ("program" = String, Path, description = "The program: `wayhouse`, `controller`, `aggregator` or `ui`.", example = "controller")
    ),
    request_body(content = ProcessApply, content_type = "application/json", description = "The new values and the revision they build on."),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 400, description = "A value is invalid, unknown, or a secret.", body = crate::openapi::ErrorBody),
        (status = 404, description = "No such program.", body = crate::openapi::ErrorBody),
        (status = 409, description = "The file changed since `rev`.", body = crate::openapi::ErrorBody),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain")
    ),
    extensions(("x-wayhouse-role" = json!("admin")))
)]
#[allow(dead_code)]
async fn put_api_process_by_program() {}

#[utoipa::path(
    post, path = "/api/process/{program}/confirm", tag = "Process settings",
    summary = "Confirm the pending change",
    description = "Keeps a trial that is waiting for confirmation.",
    security(("session" = [])),
    params(
        ("program" = String, Path, description = "The program: `wayhouse`, `controller`, `aggregator` or `ui`.", example = "controller")
    ),
    request_body(content = ProcessRev, content_type = "application/json", description = "The revision to confirm."),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 409, description = "Nothing is waiting, or the revision is stale.", body = crate::openapi::ErrorBody),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain")
    ),
    extensions(("x-wayhouse-role" = json!("admin")))
)]
#[allow(dead_code)]
async fn post_api_process_by_program_confirm() {}

#[utoipa::path(
    post, path = "/api/process/{program}/undo", tag = "Process settings",
    summary = "Undo the last change",
    description = "Restores the previous settings file. Also cancels a trial.",
    security(("session" = [])),
    params(
        ("program" = String, Path, description = "The program: `wayhouse`, `controller`, `aggregator` or `ui`.", example = "controller")
    ),
    request_body(content = ProcessRev, content_type = "application/json", description = "The revision to undo."),
    responses(
        (status = 200, description = "The upstream's answer; whatever status it sent is passed through.", body = serde_json::Value, content_type = "application/json"),
        (status = 409, description = "Nothing to undo, or the revision is stale.", body = crate::openapi::ErrorBody),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain")
    ),
    extensions(("x-wayhouse-role" = json!("admin")))
)]
#[allow(dead_code)]
async fn post_api_process_by_program_undo() {}

#[utoipa::path(
    get, path = "/api/web/sync", tag = "Service",
    summary = "State of the web-sync sidecar",
    description = "The few numbers an operator wants from `wayhouse-web-sync`, read from its `/metrics` (`--web-sync-url`). Without that flag the answer is `{configured: false}`; when the sidecar does not answer, `reachable` is false and `error` says why.",
    security(("session" = [])),
    responses(
        (status = 200, description = "The sync status.", body = WebSyncStatus, content_type = "application/json"),
        (status = 401, description = "No valid session. Body: `unauthorized` (plain text).", body = String, content_type = "text/plain"),
        (status = 403, description = "The session's role is below the one this route needs (`forbidden`, plain text), or the browser marked the request cross-site.", body = String, content_type = "text/plain")
    ),
    extensions(("x-wayhouse-role" = json!("viewer")))
)]
#[allow(dead_code)]
async fn get_api_web_sync() {}

/// The paths of every console route.
#[derive(utoipa::OpenApi)]
#[openapi(paths(
    post_ui_login,
    post_ui_logout,
    get_ui_session,
    get_api_release,
    get_ws_fleet,
    get_healthz,
    get_metrics,
    get_openapi_json,
    get_api_fleet_pools,
    get_api_fleet_sessions,
    get_api_fleet_healthz,
    get_api_fleet_features,
    get_api_fleet_stats,
    get_api_fleet_nodes,
    post_api_fleet_nodes_resolve,
    get_api_fleet_nodes_by_instance_pools,
    get_api_fleet_instances_by_instance_sniffers,
    post_api_fleet_instances_by_instance_drain,
    post_api_fleet_instances_by_instance_undrain,
    post_api_fleet_pools_by_pool_backends,
    patch_api_fleet_pools_by_pool_backends_by_addr,
    delete_api_fleet_pools_by_pool_backends_by_addr,
    post_api_fleet_route_hint,
    post_api_fleet_sniffers,
    delete_api_fleet_sniffers_by_name,
    post_api_fleet_sniffers_by_name_rollback,
    put_api_fleet_nodes_by_instance_tags,
    delete_api_fleet_nodes_by_instance_tags,
    post_api_fleet_node_tags_prune,
    get_api_config,
    get_api_config_schema,
    get_api_config_revisions,
    get_api_config_revisions_by_revision,
    get_api_config_revisions_by_revision_diff,
    get_api_config_pending,
    post_api_config,
    post_api_config_validate,
    post_api_config_rollback_by_revision,
    post_api_config_promote_by_revision,
    post_api_config_confirm_by_revision,
    post_api_config_revert,
    get_api_tunnel_addresses,
    delete_api_tunnel_origins_by_name,
    delete_api_tunnel_proxies_by_name,
    post_api_tunnel_origins_by_name_psk_rotate,
    post_api_enrollments,
    get_api_enrollments,
    delete_api_enrollments_by_id,
    get_api_plugins,
    get_api_plugin_routes,
    get_api_plugins_by_id,
    get_api_plugins_by_id_status,
    post_api_plugins_modules,
    post_api_plugins,
    delete_api_plugins_by_id,
    post_api_plugins_by_id_enable,
    post_api_plugins_by_id_disable,
    put_api_plugins_by_id_config,
    get_api_plugins_file_plan,
    post_api_plugins_by_id_webhook,
    delete_api_plugins_by_id_webhook,
    get_api_secrets,
    put_api_secrets_by_name,
    delete_api_secrets_by_name,
    get_api_plugins_by_id_secrets,
    put_api_plugins_by_id_secrets_by_slot,
    delete_api_plugins_by_id_secrets_by_slot,
    get_api_plugins_secrets_keys,
    post_api_plugins_secrets_rewrap,
    post_api_acme_rewrap,
    post_api_secrets_rewrap,
    get_api_acme_sites,
    get_api_acme_gateways,
    get_api_acme_account,
    put_api_acme_sites_by_name,
    delete_api_acme_sites_by_name,
    post_api_acme_gateways,
    delete_api_acme_gateways_by_name,
    get_api_features,
    get_api_features_overrides,
    put_api_features_overrides_by_name,
    delete_api_features_overrides_by_name,
    get_api_ui_layout,
    put_api_ui_layout,
    delete_api_ui_layout,
    get_api_ui_defaults,
    put_api_ui_defaults,
    delete_api_ui_defaults,
    get_api_ui_dashboards,
    get_api_ui_dashboards_by_name,
    put_api_ui_dashboards_by_name,
    delete_api_ui_dashboards_by_name,
    get_api_ui_filters,
    get_api_ui_filters_by_name,
    put_api_ui_filters_by_name,
    delete_api_ui_filters_by_name,
    get_api_ui_themes,
    get_api_ui_themes_by_name,
    put_api_ui_themes_by_name,
    delete_api_ui_themes_by_name,
    post_api_ui_dashboards_by_name_rename,
    get_api_registries,
    get_api_registries_by_id_sniffers,
    get_api_registries_by_id_sniffers_by_name,
    post_api_registries,
    delete_api_registries_by_id,
    post_api_registries_by_id_install,
    post_api_registries_updates_check,
    get_api_plugin_registries,
    get_api_plugin_registries_by_id_plugins,
    get_api_plugin_registries_by_id_plugins_by_name,
    post_api_plugin_registries,
    post_api_plugin_registries_by_id_fetch,
    get_api_process,
    put_api_process_by_program,
    post_api_process_by_program_confirm,
    post_api_process_by_program_undo,
    get_api_web_sync,
))]
pub(crate) struct RoutesDoc;
