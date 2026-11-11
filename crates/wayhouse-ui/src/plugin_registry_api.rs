//! Browse plugin registries and fetch a plugin from one (`/api/plugin-registries/*`).
//!
//! A fetch is an upload, never an install: the verified module is handed to the controller's
//! `POST /plugins/modules`, which stores it as an unapproved blob. The operator then approves
//! its capabilities through the ordinary install call, exactly as for a module they uploaded
//! by hand. Nothing here approves, installs or enables anything.

use axum::body::Bytes;
use axum::extract::{Extension, Path, State};
use axum::http::{Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use wayhouse_registry::{
    abi_matches, verify_plugin_artifact, PluginEntry, PluginVersion, MAX_MODULE_BYTES,
};

use crate::api::AppState;
use crate::auth::Actor;
use crate::registries::{RegistriesError, RegistryKind, RegistryRef};
use crate::registry_api::{error, fetch_failure, installs_allowed, registry_json, risk};

/// The plugin ABI this release's controller speaks, `major.minor`.
fn host_plugin_abi() -> String {
    format!(
        "{}.{}",
        wayhouse_plugin_abi::ABI_MAJOR,
        wayhouse_plugin_abi::ABI_MINOR
    )
}

pub fn viewer_router() -> Router<AppState> {
    Router::new()
        .route("/api/plugin-registries", get(list_registries))
        .route("/api/plugin-registries/{id}/plugins", get(list_plugins))
}

/// Admin only: a fetch ends in the controller's module upload, which is an admin action
/// (`POST /api/plugins/modules`), and adding a registry decides where fetches come from.
pub fn admin_router() -> Router<AppState> {
    Router::new()
        .route("/api/plugin-registries", post(add_registry))
        .route("/api/plugin-registries/{id}/fetch", post(fetch))
}

async fn list_registries(State(state): State<AppState>) -> Response {
    let list: Vec<_> = state
        .registries
        .list_plugins()
        .iter()
        .map(registry_json)
        .collect();
    Json(json!({ "registries": list, "persistent": state.registries_persistent })).into_response()
}

#[derive(Deserialize)]
struct AddRegistry {
    url: String,
    #[serde(default)]
    name: Option<String>,
}

async fn add_registry(State(state): State<AppState>, Json(req): Json<AddRegistry>) -> Response {
    match state
        .registries
        .add_kind(&req.url, req.name.as_deref(), RegistryKind::Plugin)
    {
        Ok(r) => Json(registry_json(&r)).into_response(),
        Err(e @ (RegistriesError::BadUrl | RegistriesError::Credentials)) => {
            error(StatusCode::BAD_REQUEST, e.to_string())
        }
        Err(e @ RegistriesError::TooMany) => error(StatusCode::CONFLICT, e.to_string()),
        Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// The registry `id` if it is a plugin registry.
fn plugin_registry(state: &AppState, id: &str) -> Option<RegistryRef> {
    state
        .registries
        .get(id)
        .filter(|r| r.kind == RegistryKind::Plugin)
}

/// Whether a version's signature can be checked here: only the official registry, and only
/// with a configured key.
fn version_verifiable(reg: &RegistryRef, v: &PluginVersion) -> bool {
    reg.official && v.signature_url.is_some() && crate::registry_keys::official_key().is_some()
}

async fn list_plugins(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let Some(reg) = plugin_registry(&state, &id) else {
        return error(StatusCode::NOT_FOUND, "no such plugin registry");
    };
    let index = match state.registry_client.fetch_plugin_index(&reg.url).await {
        Ok(i) => i,
        Err(e) => return fetch_failure(&e),
    };
    let host_abi = host_plugin_abi();
    let plugins: Vec<_> = index
        .plugins
        .iter()
        .map(|entry| {
            let mut v = serde_json::to_value(entry).expect("index entries serialize");
            v["compatible"] = json!(entry
                .versions
                .iter()
                .any(|x| abi_matches(&x.abi, &host_abi).unwrap_or(false)));
            for (j, version) in entry.versions.iter().enumerate() {
                v["versions"][j]["compatible"] =
                    json!(abi_matches(&version.abi, &host_abi).unwrap_or(false));
                v["versions"][j]["verifiable"] = json!(version_verifiable(&reg, version));
            }
            v
        })
        .collect();
    Json(json!({
        "registry": registry_json(&reg),
        "index_name": index.name,
        "host_abi": host_abi,
        "plugins": plugins,
    }))
    .into_response()
}

#[derive(Deserialize)]
struct FetchRequest {
    name: String,
    #[serde(default)]
    version: Option<semver::Version>,
    /// The operator confirms they want a module whose signature was not verified.
    #[serde(default)]
    allow_unsigned: bool,
}

/// The newest version built for this controller's ABI, or the requested one.
#[allow(clippy::result_large_err)] // the Err is the ready-made refusal the handler returns
fn pick<'a>(
    entry: &'a PluginEntry,
    want: Option<&semver::Version>,
    host_abi: &str,
) -> Result<&'a PluginVersion, Response> {
    match want {
        Some(want) => {
            let Some(v) = entry.versions.iter().find(|v| &v.version == want) else {
                return Err(error(
                    StatusCode::NOT_FOUND,
                    "no such version of this plugin",
                ));
            };
            if !abi_matches(&v.abi, host_abi).unwrap_or(false) {
                return Err(error(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    format!(
                        "version {want} is built for plugin ABI {}, but this controller speaks ABI {host_abi}",
                        v.abi
                    ),
                ));
            }
            Ok(v)
        }
        None => entry
            .versions
            .iter()
            .filter(|v| abi_matches(&v.abi, host_abi).unwrap_or(false))
            .max_by(|a, b| a.version.cmp(&b.version))
            .ok_or_else(|| {
                error(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    format!("no version of this plugin is built for plugin ABI {host_abi}"),
                )
            }),
    }
}

async fn fetch(
    State(state): State<AppState>,
    Extension(Actor(actor)): Extension<Actor>,
    Path(id): Path<String>,
    Json(req): Json<FetchRequest>,
) -> Response {
    if let Err(refused) = installs_allowed(&state).await {
        return refused;
    }
    let Some(reg) = plugin_registry(&state, &id) else {
        return error(StatusCode::NOT_FOUND, "no such plugin registry");
    };
    let index = match state.registry_client.fetch_plugin_index(&reg.url).await {
        Ok(i) => i,
        Err(e) => return fetch_failure(&e),
    };
    let Some(entry) = index.plugins.iter().find(|p| p.name == req.name) else {
        return error(StatusCode::NOT_FOUND, "no such plugin in this registry");
    };
    let version = match pick(entry, req.version.as_ref(), &host_plugin_abi()) {
        Ok(v) => v,
        Err(refused) => return refused,
    };

    let cap = version.size.min(MAX_MODULE_BYTES);
    let bytes = match state
        .registry_client
        .fetch_artifact(&version.url, cap)
        .await
    {
        Ok(b) => b,
        Err(e) => return fetch_failure(&e),
    };
    let key = reg
        .official
        .then(crate::registry_keys::official_key)
        .flatten();
    // Only the official registry with a configured key verifies a signature, so fetch it
    // only then: a dead signature URL elsewhere must not block a fetch.
    let signature = match (&version.signature_url, &key) {
        (Some(url), Some(_)) => match state.registry_client.fetch_signature(url).await {
            Ok(s) => Some(s),
            Err(e) => return fetch_failure(&e),
        },
        _ => None,
    };
    let verified = match verify_plugin_artifact(
        &entry.name,
        version,
        &bytes,
        signature.as_deref(),
        key.as_ref(),
    ) {
        Ok(v) => v,
        Err(e) => return error(StatusCode::UNPROCESSABLE_ENTITY, e.to_string()),
    };
    if !verified.signed && !req.allow_unsigned {
        return (
            StatusCode::CONFLICT,
            Json(json!({
                "error": "this module's signature could not be verified; confirm to fetch it anyway",
                "needs_confirm": true,
                "risk": risk(&reg),
                "signed": false,
            })),
        )
            .into_response();
    }

    tracing::info!(
        ?actor, registry = %reg.id, plugin = %entry.name, version = %version.version,
        signed = verified.signed, "fetching a plugin module from a registry (upload only)"
    );
    let resp = crate::controller_proxy::proxy_typed(
        &state,
        Method::POST,
        "/plugins/modules".to_string(),
        Some(Bytes::from(bytes)),
        None,
        actor,
    )
    .await;
    let (parts, body) = resp.into_parts();
    let body = axum::body::to_bytes(body, 1024 * 1024)
        .await
        .unwrap_or_default();
    if !parts.status.is_success() {
        // The controller refused the module (or is not configured or reachable): nothing
        // was stored. Pass its answer through.
        return (parts.status, parts.headers, body).into_response();
    }
    let upload: serde_json::Value =
        serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
    (
        StatusCode::OK,
        Json(json!({
            "plugin": entry.name,
            "version": version.version.to_string(),
            "signed": verified.signed,
            "risk": risk(&reg),
            "status": "uploaded, awaiting approval",
            "upload": upload,
            "options_schema": version.options_schema,
        })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registries::Registries;
    use crate::registry_client::RegistryClient;
    use axum::body::Body;
    use axum::http::{header, Request};
    use sha2::{Digest, Sha256};
    use std::sync::{Arc, Mutex};
    use tower::ServiceExt;

    const MODULE_WAT: &str =
        r#"(module (memory (export "memory") 1) (@custom "wayhouse.plugin-abi" "\00\00\02\00"))"#;

    async fn serve(app: Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}")
    }

    /// A plugin registry serving `demo` 0.1.0 (ABI `abi`) whose module is `module`; the
    /// index claims `sha256_override` when given.
    async fn fake_registry(module: Vec<u8>, abi: &str, sha256_override: Option<&str>) -> String {
        let sha =
            sha256_override.map_or_else(|| format!("{:x}", Sha256::digest(&module)), str::to_owned);
        let index = json!({
            "schema": 1, "kind": "plugin", "name": "test plugins",
            "plugins": [{
                "name": "demo", "description": "a demo", "license": "MIT",
                "versions": [{
                    "version": "0.1.0", "abi": abi,
                    "url": "https://registry.test/demo.wasm",
                    "sha256": sha, "size": module.len()
                }]
            }]
        })
        .to_string();
        serve(
            Router::new()
                .route("/index.json", get(move || async move { index }))
                .route("/demo.wasm", get(move || async move { module })),
        )
        .await
    }

    type Calls = Arc<Mutex<Vec<(String, Vec<u8>)>>>;

    /// A stand-in controller: `GET /features` says `registry_install` is on, and every
    /// `POST` is recorded (path and body) and answered like a module upload.
    async fn fake_controller() -> (String, Calls) {
        let calls: Calls = Arc::default();
        let seen = calls.clone();
        let app = Router::new()
            .route(
                "/features",
                get(|| async {
                    Json(json!({ "flags": [{ "name": "registry_install", "value": true }] }))
                }),
            )
            .route(
                "/plugins/modules",
                post(move |body: Bytes| {
                    let seen = seen.clone();
                    async move {
                        seen.lock()
                            .unwrap()
                            .push(("/plugins/modules".into(), body.to_vec()));
                        (
                            StatusCode::CREATED,
                            Json(json!({ "sha256": "x", "capabilities": { "log": true } })),
                        )
                    }
                }),
            );
        (serve(app).await, calls)
    }

    struct Harness {
        app: Router,
        cookie: String,
        operator_cookie: String,
        registry_id: String,
    }

    async fn harness(registry_base: &str, controller: &str, official: bool) -> Harness {
        let regs = Arc::new(Registries::load(None, false).unwrap());
        let reg = regs.add_unchecked_kind(
            &format!("{registry_base}/index.json"),
            official,
            RegistryKind::Plugin,
        );
        let state = AppState::new(Some("secret".into()))
            .with_controller(controller.to_string(), None)
            .with_registries(regs, false)
            .with_registry_client(RegistryClient::for_tests().with_rewrite(registry_base));
        let session = state.sessions.create(crate::session::Session {
            role: crate::role::Role::Admin,
            username: None,
        });
        let operator = state.sessions.create(crate::session::Session {
            role: crate::role::Role::Operator,
            username: None,
        });
        Harness {
            app: crate::api::router(state),
            cookie: format!("{}={session}", crate::api::SESSION_COOKIE),
            operator_cookie: format!("{}={operator}", crate::api::SESSION_COOKIE),
            registry_id: reg.id,
        }
    }

    impl Harness {
        async fn call(
            &self,
            method: &str,
            path: &str,
            body: Option<serde_json::Value>,
        ) -> (StatusCode, serde_json::Value) {
            let mut req = Request::builder()
                .method(method)
                .uri(path)
                .header(header::COOKIE, &self.cookie);
            let body = match body {
                Some(b) => {
                    req = req.header(header::CONTENT_TYPE, "application/json");
                    Body::from(b.to_string())
                }
                None => Body::empty(),
            };
            let resp = self
                .app
                .clone()
                .oneshot(req.body(body).unwrap())
                .await
                .unwrap();
            let status = resp.status();
            let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap();
            (
                status,
                serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
            )
        }

        async fn fetch(&self, body: serde_json::Value) -> (StatusCode, serde_json::Value) {
            self.call(
                "POST",
                &format!("/api/plugin-registries/{}/fetch", self.registry_id),
                Some(body),
            )
            .await
        }
    }

    fn module() -> Vec<u8> {
        wat::parse_str(MODULE_WAT).unwrap()
    }

    #[tokio::test]
    async fn an_operator_can_neither_fetch_nor_add_a_plugin_registry() {
        let registry = fake_registry(module(), "0.2", None).await;
        let (controller, calls) = fake_controller().await;
        let mut h = harness(&registry, &controller, false).await;
        h.cookie = h.operator_cookie.clone();
        let (status, _) = h
            .fetch(json!({ "name": "demo", "allow_unsigned": true }))
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, _) = h
            .call(
                "POST",
                "/api/plugin-registries",
                Some(json!({ "url": "https://example.test/index.json" })),
            )
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(
            calls.lock().unwrap().is_empty(),
            "nothing reached the controller"
        );
        // Reading the list stays open to a viewer-level session.
        let (status, _) = h.call("GET", "/api/plugin-registries", None).await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn a_confirmed_fetch_uploads_the_verified_bytes_and_installs_nothing() {
        let m = module();
        let registry = fake_registry(m.clone(), "0.2", None).await;
        let (controller, calls) = fake_controller().await;
        let h = harness(&registry, &controller, false).await;
        let (status, body) = h
            .fetch(json!({ "name": "demo", "allow_unsigned": true }))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["status"], "uploaded, awaiting approval");
        assert_eq!(body["signed"], false);
        assert_eq!(body["risk"], "external");
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 1, "one upload and nothing else");
        assert_eq!(calls[0].0, "/plugins/modules");
        assert_eq!(calls[0].1, m);
    }

    #[tokio::test]
    async fn an_unsigned_module_needs_an_explicit_confirm_and_uploads_nothing_before_it() {
        let registry = fake_registry(module(), "0.2", None).await;
        let (controller, calls) = fake_controller().await;
        let h = harness(&registry, &controller, false).await;
        let (status, body) = h.fetch(json!({ "name": "demo" })).await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        assert_eq!(body["needs_confirm"], true);
        assert!(calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_tampered_hash_is_refused_even_when_confirmed() {
        let bad = "0".repeat(64);
        let registry = fake_registry(module(), "0.2", Some(&bad)).await;
        let (controller, calls) = fake_controller().await;
        let h = harness(&registry, &controller, false).await;
        let (status, body) = h
            .fetch(json!({ "name": "demo", "allow_unsigned": true }))
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert!(body["error"].as_str().unwrap().contains("sha256"), "{body}");
        assert!(calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn another_abi_unknown_names_and_versions_upload_nothing() {
        let registry = fake_registry(module(), "0.9", None).await;
        let (controller, calls) = fake_controller().await;
        let h = harness(&registry, &controller, false).await;
        let confirm = |extra: serde_json::Value| {
            let mut b = json!({ "name": "demo", "allow_unsigned": true });
            b.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            b
        };
        let (status, _) = h.fetch(confirm(json!({}))).await;
        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "no compatible version"
        );
        let (status, _) = h.fetch(confirm(json!({ "version": "0.1.0" }))).await;
        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "named version, wrong ABI"
        );
        let (status, _) = h.fetch(confirm(json!({ "version": "9.9.9" }))).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = h
            .fetch(json!({ "name": "nope", "allow_unsigned": true }))
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_module_whose_abi_section_disagrees_with_the_index_is_refused() {
        let wrong = wat::parse_str(
            r#"(module (memory (export "memory") 1) (@custom "wayhouse.plugin-abi" "\00\00\01\00"))"#,
        )
        .unwrap();
        let registry = fake_registry(wrong, "0.2", None).await;
        let (controller, calls) = fake_controller().await;
        let h = harness(&registry, &controller, false).await;
        let (status, _) = h
            .fetch(json!({ "name": "demo", "allow_unsigned": true }))
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn the_listing_shows_compatibility_and_a_sniffer_registry_is_not_a_plugin_one() {
        let registry = fake_registry(module(), "0.2", None).await;
        let (controller, _calls) = fake_controller().await;
        let h = harness(&registry, &controller, false).await;
        let (status, body) = h
            .call(
                "GET",
                &format!("/api/plugin-registries/{}/plugins", h.registry_id),
                None,
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["plugins"][0]["name"], "demo");
        assert_eq!(body["plugins"][0]["compatible"], true);
        assert_eq!(body["plugins"][0]["versions"][0]["verifiable"], false);
        assert_eq!(body["host_abi"], "0.2");
        let (status, body) = h.call("GET", "/api/plugin-registries", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["registries"].as_array().unwrap().len(), 1);
        // The sniffer endpoints do not serve it.
        let (status, _) = h
            .call(
                "GET",
                &format!("/api/registries/{}/sniffers", h.registry_id),
                None,
            )
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = h
            .call("GET", "/api/plugin-registries/nope/plugins", None)
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
}
