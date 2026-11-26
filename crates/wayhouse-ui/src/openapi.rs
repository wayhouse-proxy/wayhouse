//! The OpenAPI description of the console's HTTP API.
//!
//! The console has about a hundred routes in many modules and most only relay to the controller
//! or the aggregator, so the routes are described in one place, `openapi_paths.rs`, and the
//! types they share are here. `docs/reference/openapi/console.json` is the checked-in rendering
//! the docs site is built from. Two tests keep it honest: `the_checked_in_spec_is_current` fails
//! when the rendering is stale, and `the_spec_matches_the_router` fails when a route exists
//! without a description (or the other way round) or the stated role is wrong.

use utoipa::openapi::security::{ApiKey, ApiKeyValue, SecurityRequirement, SecurityScheme};
use utoipa::{Modify, OpenApi};

/// The JSON error shape the console's own routes use. Relayed answers keep whatever shape the
/// controller or aggregator sent.
#[derive(serde::Serialize, utoipa::ToSchema)]
#[allow(dead_code)] // documentation type: the handlers build `{"error": ...}` inline
pub(crate) struct ErrorBody {
    /// What went wrong, in one sentence.
    #[schema(example = "no such registry")]
    pub error: String,
}

/// The sign-in request.
#[derive(serde::Deserialize, utoipa::ToSchema)]
#[allow(dead_code)]
pub(crate) struct LoginBody {
    /// Required with `--users-file`; ignored with `--ui-password-file`.
    #[schema(example = "ada")]
    pub username: Option<String>,
    /// The password.
    pub password: String,
}

/// Who the session belongs to.
#[derive(serde::Serialize, utoipa::ToSchema)]
#[allow(dead_code)]
pub(crate) struct SessionStatus {
    /// Always `true`: a request without a valid session is answered `401` instead.
    pub authenticated: bool,
    /// The signed-in user; `null` for the anonymous admin of an open console.
    pub username: Option<String>,
    /// `viewer`, `operator` or `admin`.
    #[schema(example = "operator")]
    pub role: String,
}

/// The newest release the signed feed knows of.
#[derive(serde::Serialize, utoipa::ToSchema)]
#[allow(dead_code)]
pub(crate) struct LatestRelease {
    /// The version, without a leading `v`.
    #[schema(example = "0.4.1")]
    pub version: String,
    /// `YYYY-MM-DD` of the publish.
    #[schema(example = "2026-12-01")]
    pub date: String,
    /// Where the release notes are (https).
    pub notes_url: String,
}

/// The running version and the newest release.
#[derive(serde::Serialize, utoipa::ToSchema)]
#[allow(dead_code)]
pub(crate) struct ReleaseStatus {
    /// The version this console runs.
    #[schema(example = "0.4.0")]
    pub current: String,
    /// `false` when the feed is switched off (`--release-feed-url ""`).
    pub enabled: bool,
    /// The newest stable release above `current`, or `null`.
    pub latest: Option<LatestRelease>,
    /// Unix seconds of the last good check; `null` before the first.
    pub checked_at: Option<u64>,
}

/// One registry.
#[derive(serde::Serialize, utoipa::ToSchema)]
#[allow(dead_code)]
pub(crate) struct RegistryInfo {
    /// The id to use in the other routes.
    pub id: String,
    /// The display name.
    pub name: String,
    /// The index URL.
    pub url: String,
    /// `true` for the built-in registry.
    pub official: bool,
    /// `official` or `external`: how far the registry is trusted.
    pub risk: String,
}

/// The registries of one kind.
#[derive(serde::Serialize, utoipa::ToSchema)]
#[allow(dead_code)]
pub(crate) struct RegistryList {
    /// The registries.
    pub registries: Vec<RegistryInfo>,
    /// Whether additions survive a restart (a registries file is configured).
    pub persistent: bool,
}

/// A registry to add.
#[derive(serde::Deserialize, utoipa::ToSchema)]
#[allow(dead_code)]
pub(crate) struct AddRegistryBody {
    /// The index URL (`https`, no credentials).
    #[schema(example = "https://registry.example.com/index.json")]
    pub url: String,
    /// A display name; defaults to the host.
    pub name: Option<String>,
}

/// A sniffer to install.
#[derive(serde::Deserialize, utoipa::ToSchema)]
#[allow(dead_code)]
pub(crate) struct InstallBody {
    /// The sniffer's name.
    #[schema(example = "minecraft")]
    pub name: String,
    /// A version; the newest compatible release when omitted.
    #[schema(example = "1.2.0")]
    pub version: Option<String>,
}

/// What happened on one instance.
#[derive(serde::Serialize, utoipa::ToSchema)]
#[allow(dead_code)]
pub(crate) struct InstallResult {
    /// The instance's name.
    pub instance: String,
    /// Whether the instance took the module.
    pub ok: bool,
    /// Whether the instance pins module hashes and needs the pin line added by hand.
    pub pinned: bool,
    /// Why it failed, when it did.
    pub error: Option<String>,
    /// More detail from the instance.
    pub detail: Option<serde_json::Value>,
    /// The instance's HTTP status.
    pub status: Option<u16>,
}

/// The outcome of an install.
#[derive(serde::Serialize, utoipa::ToSchema)]
#[allow(dead_code)]
pub(crate) struct InstallAnswer {
    /// The sniffer's name.
    pub sniffer: String,
    /// The version installed.
    pub version: String,
    /// Whether a signature was verified.
    pub signed: bool,
    /// `official` or `external`.
    pub risk: String,
    /// The module's SHA-256, hex.
    pub sha256: String,
    /// `false` when some proxy's version is unknown, so `min_proxy` was checked only against the rest.
    pub min_proxy_checked: bool,
    /// Why `min_proxy_checked` is false.
    pub min_proxy_note: Option<String>,
    /// One result per instance.
    pub results: Vec<InstallResult>,
    /// For each instance that pins hashes, the pin line to add.
    pub pinned_instances: Vec<serde_json::Value>,
}

/// A plugin to fetch.
#[derive(serde::Deserialize, utoipa::ToSchema)]
#[allow(dead_code)]
pub(crate) struct FetchBody {
    /// The plugin's name.
    pub name: String,
    /// A version; the newest compatible release when omitted.
    pub version: Option<String>,
    /// Fetch a module whose signature could not be verified.
    #[serde(default)]
    pub allow_unsigned: bool,
}

/// The new values of a program's settings file.
#[derive(serde::Deserialize, utoipa::ToSchema)]
#[allow(dead_code)]
pub(crate) struct ProcessApply {
    /// Flag name to value; the flags are the ones the program published.
    pub values: serde_json::Value,
    /// The revision of the file the page showed.
    pub rev: u64,
}

/// A revision to settle.
#[derive(serde::Deserialize, utoipa::ToSchema)]
#[allow(dead_code)]
pub(crate) struct ProcessRev {
    /// The revision the page showed, so a stale click cannot settle another trial.
    pub rev: u64,
}

/// The names to prune.
#[derive(serde::Deserialize, utoipa::ToSchema)]
#[allow(dead_code)]
pub(crate) struct PruneBody {
    /// Names an earlier call listed as unlisted.
    #[serde(default)]
    pub names: Vec<String>,
}

/// What a prune found and did.
#[derive(serde::Serialize, utoipa::ToSchema)]
#[allow(dead_code)]
pub(crate) struct PruneAnswer {
    /// Nodes with operator tags that the aggregator no longer lists.
    pub unlisted: Vec<String>,
    /// The names whose tags were removed.
    pub removed: Vec<String>,
}

/// The web-sync sidecar's state.
#[derive(serde::Serialize, utoipa::ToSchema)]
#[allow(dead_code)]
pub(crate) struct WebSyncStatus {
    /// Whether `--web-sync-url` is set. When false nothing else is present.
    pub configured: bool,
    /// Whether the sidecar answered.
    pub reachable: Option<bool>,
    /// Why it did not answer.
    pub error: Option<String>,
    /// Seconds since the last good sync; `null` before the first.
    pub last_ok_age_secs: Option<u64>,
    /// Configurations loaded.
    pub loads: Option<u64>,
    /// Configurations rejected.
    pub rejects: Option<u64>,
    /// Errors by stage: `read`, `render`, `send`.
    pub errors: Option<serde_json::Value>,
}

/// The head of the document and the cookie scheme.
#[derive(OpenApi)]
#[openapi(
    info(
        title = "wayhouse console API",
        description = "The HTTP API of `wayhouse-ui` (default `:9903`), the backend of the web console. It signs users in, \
        enforces their role, and relays to the controller and the aggregator, so most routes document only what the console adds \
        and point to the controller or aggregator API for the request and answer.",
        license(name = "MIT OR Apache-2.0"),
    ),
    components(schemas(ErrorBody, LoginBody, SessionStatus, ReleaseStatus, LatestRelease, RegistryInfo, RegistryList,
        AddRegistryBody, InstallBody, InstallResult, InstallAnswer, FetchBody, ProcessApply, ProcessRev, PruneBody,
        PruneAnswer, WebSyncStatus)),
    modifiers(&Schemes),
)]
struct ConsoleApi;

/// Adds the session cookie scheme and the default requirement.
struct Schemes;

impl Modify for Schemes {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "session",
            SecurityScheme::ApiKey(ApiKey::Cookie(ApiKeyValue::with_description(
                crate::api::SESSION_COOKIE,
                "The session cookie `wayhouse_ui_session`, set by `POST /ui/login`. Each route needs a role: viewer, \
                 operator or admin (`x-wayhouse-role`). With no login configured every request is an anonymous admin. \
                 Without a valid session a route answers `401`; with too low a role, `403`.",
            ))),
        );
        openapi.security = Some(vec![SecurityRequirement::new(
            "session",
            Vec::<String>::new(),
        )]);
    }
}

/// The whole document.
pub fn openapi() -> utoipa::openapi::OpenApi {
    let mut doc = ConsoleApi::openapi();
    doc.merge(crate::openapi_paths::RoutesDoc::openapi());
    doc
}

/// The document as JSON.
pub fn openapi_json() -> serde_json::Value {
    serde_json::to_value(openapi()).expect("the spec serializes")
}

#[cfg(test)]
mod tests {
    /// `docs/reference/openapi/console.json` is the rendering the docs site is built from.
    /// Regenerate it with `WAYHOUSE_UPDATE_OPENAPI=1 cargo test -p wayhouse-ui --lib openapi`.
    #[test]
    fn the_checked_in_spec_is_current() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/reference/openapi/console.json");
        let mut want = serde_json::to_string_pretty(&super::openapi_json()).unwrap();
        want.push('\n');
        if std::env::var_os("WAYHOUSE_UPDATE_OPENAPI").is_some() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &want).unwrap();
            return;
        }
        let have = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(
            have == want,
            "{} is out of date; run `WAYHOUSE_UPDATE_OPENAPI=1 cargo test -p wayhouse-ui --lib openapi` and commit it",
            path.display()
        );
    }

    use axum::body::Body;
    use axum::http::{Method, Request, StatusCode};
    use tower::ServiceExt;

    use crate::role::Role;

    /// A concrete URL for a templated path: every `{param}` becomes `x`.
    fn fill(path: &str) -> String {
        let mut out = String::new();
        let mut skip = false;
        for c in path.chars() {
            match c {
                '{' => {
                    skip = true;
                    out.push('x');
                }
                '}' => skip = false,
                _ if !skip => out.push(c),
                _ => {}
            }
        }
        out
    }

    /// The router and the description agree: every documented route exists with the documented
    /// role, and no route exists that is not documented.
    #[tokio::test]
    async fn the_spec_matches_the_router() {
        let users = [
            ("viv", Role::Viewer),
            ("olly", Role::Operator),
            ("ada", Role::Admin),
        ];
        let state = crate::api::AppState::new(None).with_users(
            users
                .map(|(name, role)| {
                    let record = crate::users::UserRecord {
                        username: name.into(),
                        password_hash: String::new(),
                        role,
                    };
                    (name.to_string(), record)
                })
                .into(),
        );
        let cookie = |name: &str, role: Role| {
            let id = state.sessions.create(crate::session::Session {
                role,
                username: Some(name.into()),
            });
            format!("{}={id}", crate::api::SESSION_COOKIE)
        };
        let cookies: Vec<(Role, String)> = users.iter().map(|(n, r)| (*r, cookie(n, *r))).collect();
        let app = crate::api::router(state);
        let send = |method: Method, path: String, cookie: Option<String>| {
            let app = app.clone();
            async move {
                let mut req = Request::builder().method(method).uri(path);
                if let Some(c) = cookie {
                    req = req.header("cookie", c);
                }
                let resp = app.oneshot(req.body(Body::empty()).unwrap()).await.unwrap();
                let status = resp.status();
                let body = axum::body::to_bytes(resp.into_body(), 1 << 20)
                    .await
                    .unwrap();
                (status, body.is_empty())
            }
        };
        // An unmatched route answers 404 with no body; a handler's own 404 carries one.
        let missing = |s: (StatusCode, bool)| {
            s.0 == StatusCode::METHOD_NOT_ALLOWED || (s.0 == StatusCode::NOT_FOUND && s.1)
        };

        let doc = super::openapi_json();
        let mut documented: Vec<(String, String)> = Vec::new();
        for (path, item) in doc["paths"].as_object().unwrap() {
            for (method, op) in item.as_object().unwrap() {
                documented.push((method.to_uppercase(), path.clone()));
                // Served by `serve.rs`, outside `api::router`.
                if path == "/healthz" || path == "/metrics" {
                    continue;
                }
                let url = fill(path);
                let m: Method = method.to_uppercase().parse().unwrap();
                let role = op.get("x-wayhouse-role").and_then(|r| r.as_str());
                let admin = cookies
                    .iter()
                    .find(|(r, _)| *r == Role::Admin)
                    .unwrap()
                    .1
                    .clone();
                // Signing out with the admin cookie would end the session the next requests use.
                let who = if role.is_some() { Some(admin) } else { None };
                let got = send(m.clone(), url.clone(), who).await;
                assert!(
                    !missing(got),
                    "{method} {path} is documented but the router has no such route ({:?})",
                    got.0
                );
                let Some(role) = role else { continue };
                let need = match role {
                    "viewer" => Role::Viewer,
                    "operator" => Role::Operator,
                    "admin" => Role::Admin,
                    other => panic!("{method} {path}: unknown x-wayhouse-role {other}"),
                };
                let (st, _) = send(m.clone(), url.clone(), None).await;
                assert_eq!(
                    st,
                    StatusCode::UNAUTHORIZED,
                    "{method} {path} without a session"
                );
                for (r, c) in &cookies {
                    let (st, _) = send(m.clone(), url.clone(), Some(c.clone())).await;
                    if *r < need {
                        assert_eq!(
                            st,
                            StatusCode::FORBIDDEN,
                            "{method} {path} as {r:?}: documented role is {role}"
                        );
                    } else {
                        assert_ne!(
                            st,
                            StatusCode::FORBIDDEN,
                            "{method} {path} as {r:?}: documented role is {role}"
                        );
                        assert_ne!(st, StatusCode::UNAUTHORIZED, "{method} {path} as {r:?}");
                    }
                }
            }
        }
        // The other direction: no method on a documented path is served without being described.
        let admin = cookies
            .iter()
            .find(|(r, _)| *r == Role::Admin)
            .unwrap()
            .1
            .clone();
        for (_, path) in documented.clone() {
            if path == "/healthz" || path == "/metrics" {
                continue;
            }
            let url = fill(&path);
            for m in ["GET", "POST", "PUT", "PATCH", "DELETE"] {
                let who = if path.starts_with("/ui/log") {
                    None
                } else {
                    Some(admin.clone())
                };
                let got = send(m.parse().unwrap(), url.clone(), who).await;
                if !missing(got) {
                    assert!(
                        documented.contains(&(m.to_string(), path.clone())),
                        "{m} {path} is served but not documented"
                    );
                }
            }
        }
        // And no route path in the sources is left out.
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        for entry in std::fs::read_dir(dir).unwrap() {
            let file = entry.unwrap().path();
            let name = file.file_name().unwrap().to_string_lossy().to_string();
            if !name.ends_with(".rs") || name.starts_with("openapi") {
                continue;
            }
            let src = std::fs::read_to_string(&file).unwrap();
            let code = src.split("#[cfg(test)]").next().unwrap();
            for part in code.split(".route(").skip(1) {
                let part = part.trim_start();
                let Some(rest) = part.strip_prefix('"') else {
                    continue;
                };
                let path = &rest[..rest.find('"').unwrap()];
                assert!(
                    doc["paths"].get(path).is_some(),
                    "{name} serves {path}, which the spec does not describe"
                );
            }
        }
    }
}
