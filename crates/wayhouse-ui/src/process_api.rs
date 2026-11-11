//! Process flags of the fleet's own programs (`/api/process/*`, G1).
//!
//! The UI backend reads and writes the settings files of `wayhouse-settings` in `--settings-dir`;
//! the programs notice a change, restart, and undo it unless it is confirmed
//! (`docs/12-deployment.md`). The flag list of each program is the one it published when it last
//! started, so a program that never ran with this directory shows as "not connected".
//!
//! Reads need `Viewer` (and show a Viewer no values); applying, confirming and undoing need `Admin`, the level that can already
//! change the fleet's configuration. Credentials never pass through here: the settings crate
//! refuses token, password and PSK flags, and the files hold no secret.

use std::path::PathBuf;

use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use wayhouse_settings::{ApplyError, File, Values};

use crate::api::AppState;
use crate::auth::Actor;
use crate::auth::ActorRole;

/// The programs whose flags can be edited, with the name their settings file carries.
const PROGRAMS: &[(&str, &str)] = &[
    ("wayhouse", "Proxy"),
    ("controller", "Controller"),
    ("aggregator", "Aggregator"),
    ("ui", "UI"),
];

pub fn viewer_router() -> Router<AppState> {
    Router::new().route("/api/process", get(list))
}

pub fn admin_router() -> Router<AppState> {
    Router::new()
        .route("/api/process/{program}", put(apply))
        .route("/api/process/{program}/confirm", post(confirm))
        .route("/api/process/{program}/undo", post(undo))
}

fn error(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({ "error": msg.into() }))).into_response()
}

fn refusal(e: ApplyError) -> Response {
    match e {
        ApplyError::Invalid(m) => error(StatusCode::BAD_REQUEST, m),
        ApplyError::Conflict(m) => error(StatusCode::CONFLICT, m),
        ApplyError::Io(m) => {
            tracing::error!(error = %m, "settings file");
            error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "the settings file could not be written",
            )
        }
    }
}

/// The settings directory and the program name, or the error answer.
fn target(state: &AppState, program: &str) -> Result<(PathBuf, &'static str), Box<Response>> {
    let Some(dir) = state.settings_dir.as_deref() else {
        return Err(Box::new(error(
            StatusCode::NOT_FOUND,
            "this UI was started without --settings-dir",
        )));
    };
    let Some((name, _)) = PROGRAMS.iter().find(|(n, _)| *n == program) else {
        return Err(Box::new(error(StatusCode::NOT_FOUND, "no such program")));
    };
    Ok((dir.clone(), name))
}

/// What the page shows of a settings file. `redact` hides the values (a data directory, a
/// listen address, a tunnel endpoint are the operator's to see, not a Viewer's); the
/// revision and the pending trial stay, so the page still shows that a change is waiting.
fn file_json(f: &File, redact: bool) -> serde_json::Value {
    let (values, previous) = if redact {
        (json!({}), json!({}))
    } else {
        (json!(f.values), json!(f.previous))
    };
    json!({
        "rev": f.rev,
        "values": values,
        "previous": previous,
        "redacted": redact,
        "pending": f.pending.as_ref().map(|p| json!({
            "applied_at": p.applied_at,
            "attempts": p.attempts,
            "confirm_secs": wayhouse_settings::CONFIRM_SECS,
        })),
        // The program's refusal text can quote the rejected value (`invalid value 'x'`).
        "error": if redact { f.error.as_ref().map(|_| "the program refused the last change".to_string()) } else { f.error.clone() },
    })
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
) -> Result<T, Box<Response>> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|_| Box::new(error(StatusCode::INTERNAL_SERVER_ERROR, "internal error")))
}

async fn list(
    State(state): State<AppState>,
    Extension(ActorRole(role)): Extension<ActorRole>,
) -> Response {
    let redact = role == crate::role::Role::Viewer;
    let Some(dir) = state.settings_dir.as_deref().cloned() else {
        return Json(json!({ "dir_configured": false, "programs": [] })).into_response();
    };
    let programs = blocking(move || {
        PROGRAMS
            .iter()
            .map(|(name, label)| {
                let flags = wayhouse_settings::read_schema(&dir, name);
                let file = wayhouse_settings::read(&dir, name).ok().flatten();
                json!({
                    "program": name,
                    "label": label,
                    "connected": flags.is_some(),
                    "flags": flags.unwrap_or_default(),
                    "file": file.as_ref().map(|f| file_json(f, redact)),
                })
            })
            .collect::<Vec<_>>()
    })
    .await;
    match programs {
        Ok(p) => Json(json!({ "dir_configured": true, "programs": p })).into_response(),
        Err(r) => *r,
    }
}

#[derive(Deserialize)]
struct ApplyBody {
    values: Values,
    /// The revision of the file the page showed.
    rev: u64,
}

async fn apply(
    State(state): State<AppState>,
    Extension(actor): Extension<Actor>,
    Path(program): Path<String>,
    Json(body): Json<ApplyBody>,
) -> Response {
    let (dir, name) = match target(&state, &program) {
        Ok(t) => t,
        Err(r) => return *r,
    };
    let result = blocking(move || {
        let Some(flags) = wayhouse_settings::read_schema(&dir, name) else {
            return Err(ApplyError::Conflict(
                "this program has not started with --settings-dir yet, so its flags are unknown"
                    .into(),
            ));
        };
        wayhouse_settings::apply_flags(
            &dir,
            name,
            &flags,
            body.values,
            body.rev,
            wayhouse_settings::now(),
        )
    })
    .await;
    match result {
        Ok(Ok(file)) => {
            // Names only: a value could be a path or an address the operator keeps private.
            tracing::info!(?actor, program = name, keys = ?file.values.keys().collect::<Vec<_>>(),
                "process flags applied as a trial; waiting for confirmation");
            Json(file_json(&file, false)).into_response()
        }
        Ok(Err(e)) => refusal(e),
        Err(r) => *r,
    }
}

async fn confirm(
    State(state): State<AppState>,
    Extension(actor): Extension<Actor>,
    Path(program): Path<String>,
    Json(body): Json<SettleBody>,
) -> Response {
    finish(&state, &program, &actor, true, body.rev).await
}

async fn undo(
    State(state): State<AppState>,
    Extension(actor): Extension<Actor>,
    Path(program): Path<String>,
    Json(body): Json<SettleBody>,
) -> Response {
    finish(&state, &program, &actor, false, body.rev).await
}

/// Confirm and undo name the revision the page showed, so a stale click cannot settle a
/// different trial than the one the operator looked at.
#[derive(Deserialize)]
struct SettleBody {
    rev: u64,
}

async fn finish(
    state: &AppState,
    program: &str,
    actor: &Actor,
    confirm: bool,
    rev: u64,
) -> Response {
    let (dir, name) = match target(state, program) {
        Ok(t) => t,
        Err(r) => return *r,
    };
    let result = blocking(move || {
        if confirm {
            wayhouse_settings::confirm(&dir, name, rev)
        } else {
            wayhouse_settings::undo(&dir, name, rev)
        }
    })
    .await;
    match result {
        Ok(Ok(file)) => {
            tracing::info!(
                ?actor,
                program = name,
                confirmed = confirm,
                "process flags trial settled"
            );
            Json(file_json(&file, false)).into_response()
        }
        Ok(Err(e)) => refusal(e),
        Err(r) => *r,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;
    use wayhouse_settings::{Kind, Value};

    fn flag(name: &str, kind: Kind, reason: Option<&str>) -> wayhouse_settings::Flag {
        wayhouse_settings::Flag {
            name: name.into(),
            help: String::new(),
            kind,
            value_name: None,
            default: None,
            editable: reason.is_none(),
            reason: reason.map(Into::into),
        }
    }

    /// A settings dir where the controller has started once (it published its flags).
    fn published() -> (tempfile::TempDir, Router) {
        let dir = tempfile::tempdir().unwrap();
        let flags = vec![
            flag("listen", Kind::Text, None),
            flag("plugins", Kind::Bool, None),
            flag("auth-token", Kind::Text, Some("a credential")),
        ];
        let body = serde_json::to_vec(&json!({ "flags": flags })).unwrap();
        std::fs::write(
            wayhouse_settings::schema_path(dir.path(), "controller"),
            body,
        )
        .unwrap();
        let state = AppState::new(None).with_settings_dir(dir.path().to_path_buf());
        (dir, crate::api::router(state))
    }

    async fn call(
        app: &Router,
        method: &str,
        path: &str,
        body: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        let req = Request::builder()
            .method(method)
            .uri(path)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(json!(null)),
        )
    }

    #[tokio::test]
    async fn without_a_settings_dir_the_page_is_told_and_writes_are_refused() {
        let app = crate::api::router(AppState::new(None));
        let (status, body) = call(&app, "GET", "/api/process", json!(null)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, json!({ "dir_configured": false, "programs": [] }));
        let (status, _) = call(
            &app,
            "PUT",
            "/api/process/controller",
            json!({"values":{},"rev":0}),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn the_list_shows_which_programs_published_their_flags() {
        let (_dir, app) = published();
        let (_, body) = call(&app, "GET", "/api/process", json!(null)).await;
        let progs = body["programs"].as_array().unwrap();
        assert_eq!(progs.len(), 4);
        let by = |n: &str| progs.iter().find(|p| p["program"] == n).unwrap();
        assert_eq!(by("controller")["connected"], true);
        assert_eq!(by("controller")["flags"].as_array().unwrap().len(), 3);
        assert_eq!(by("aggregator")["connected"], false);
        assert_eq!(by("controller")["file"], json!(null));
    }

    #[test]
    fn a_redacted_file_does_not_quote_the_programs_refusal() {
        let f = File {
            rev: 3,
            error: Some("invalid value '10.9.8.7:9000' for '--listen'".into()),
            ..File::default()
        };
        let seen = file_json(&f, true).to_string();
        assert!(!seen.contains("10.9.8.7"), "{seen}");
        assert!(seen.contains("refused the last change"));
        assert!(file_json(&f, false).to_string().contains("10.9.8.7"));
    }

    #[tokio::test]
    async fn a_viewer_sees_that_a_change_is_waiting_but_not_the_values() {
        let dir = tempfile::tempdir().unwrap();
        let flags = vec![flag("listen", Kind::Text, None)];
        std::fs::write(
            wayhouse_settings::schema_path(dir.path(), "controller"),
            serde_json::to_vec(&json!({ "flags": flags })).unwrap(),
        )
        .unwrap();
        let mut users = std::collections::HashMap::new();
        users.insert(
            "carol".to_string(),
            crate::users::UserRecord {
                username: "carol".into(),
                password_hash: crate::users::hash_password("carol-pass"),
                role: crate::role::Role::Viewer,
            },
        );
        let state = AppState::new(None)
            .with_users(users)
            .with_settings_dir(dir.path().to_path_buf());
        let viewer = state.sessions.create(crate::session::Session {
            role: crate::role::Role::Viewer,
            username: Some("carol".into()),
        });
        let admin = state.sessions.create(crate::session::Session {
            role: crate::role::Role::Admin,
            username: None,
        });
        let app = crate::api::router(state);
        let send = |method: &str, id: &str, body: Body| {
            let req = Request::builder()
                .method(method)
                .uri(if method == "GET" {
                    "/api/process"
                } else {
                    "/api/process/controller"
                })
                .header("content-type", "application/json")
                .header(
                    axum::http::header::COOKIE,
                    format!("{}={id}", crate::api::SESSION_COOKIE),
                )
                .body(body)
                .unwrap();
            let app = app.clone();
            async move {
                let resp = app.oneshot(req).await.unwrap();
                let status = resp.status();
                let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
                    .await
                    .unwrap();
                (
                    status,
                    serde_json::from_slice::<serde_json::Value>(&bytes).unwrap_or(json!(null)),
                )
            }
        };
        let put = json!({"values":{"listen":"10.9.8.7:9000"},"rev":0}).to_string();
        assert_eq!(send("PUT", &admin, Body::from(put)).await.0, StatusCode::OK);

        let controller = |body: &serde_json::Value| {
            body["programs"]
                .as_array()
                .unwrap()
                .iter()
                .find(|p| p["program"] == "controller")
                .unwrap()["file"]
                .clone()
        };
        let (_, as_admin) = send("GET", &admin, Body::empty()).await;
        assert_eq!(controller(&as_admin)["values"]["listen"], "10.9.8.7:9000");
        let (status, as_viewer) = send("GET", &viewer, Body::empty()).await;
        assert_eq!(status, StatusCode::OK);
        let file = controller(&as_viewer);
        assert_eq!(file["redacted"], true);
        assert_eq!(file["values"], json!({}));
        assert!(
            file["pending"].is_object(),
            "the pending trial is still shown"
        );
        assert!(!as_viewer.to_string().contains("10.9.8.7"));
    }

    #[tokio::test]
    async fn a_trial_is_applied_confirmed_and_undone_through_the_api() {
        let (dir, app) = published();
        let put = |values: serde_json::Value, rev: u64| {
            call(
                &app,
                "PUT",
                "/api/process/controller",
                json!({ "values": values, "rev": rev }),
            )
        };
        let (status, file) = put(json!({"listen": "0.0.0.0:9"}), 0).await;
        assert_eq!(status, StatusCode::OK, "{file}");
        assert_eq!(file["rev"], 1);
        assert_eq!(file["pending"]["confirm_secs"], 60);
        // On disk the previous values wait for an undo.
        let on_disk = wayhouse_settings::read(dir.path(), "controller")
            .unwrap()
            .unwrap();
        assert_eq!(
            on_disk.values.get("listen"),
            Some(&Value::One("0.0.0.0:9".into()))
        );
        assert!(on_disk.pending.is_some());
        // A second change cannot be stacked, and a stale page is refused.
        let (status, _) = put(json!({"listen": "0.0.0.0:10"}), 1).await;
        assert_eq!(status, StatusCode::CONFLICT);
        let (status, _) = call(
            &app,
            "POST",
            "/api/process/controller/undo",
            json!({"rev": 7}),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "a stale undo");
        let (status, undone) = call(
            &app,
            "POST",
            "/api/process/controller/undo",
            json!({"rev": 1}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(undone["values"], json!({}));
        assert_eq!(undone["pending"], json!(null));
        let (status, _) = put(json!({"listen": "0.0.0.0:9"}), 1).await;
        assert_eq!(status, StatusCode::CONFLICT, "stale revision");
        let (status, file) = put(json!({"listen": "0.0.0.0:9"}), 2).await;
        assert_eq!(status, StatusCode::OK, "{file}");
        let (status, done) = call(
            &app,
            "POST",
            "/api/process/controller/confirm",
            json!({"rev": 3}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(done["pending"], json!(null));
        assert_eq!(done["values"], json!({"listen": "0.0.0.0:9"}));
        let (status, _) = call(
            &app,
            "POST",
            "/api/process/controller/confirm",
            json!({"rev": 4}),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "nothing is waiting");
    }

    #[tokio::test]
    async fn credentials_unknown_flags_unknown_programs_and_unpublished_programs_are_refused() {
        let (_dir, app) = published();
        let put = |program: &str, values: serde_json::Value| {
            let path = format!("/api/process/{program}");
            let app = app.clone();
            async move { call(&app, "PUT", &path, json!({ "values": values, "rev": 0 })).await }
        };
        let (status, body) = put("controller", json!({"auth-token": "x"})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(
            body["error"].as_str().unwrap().contains("credential"),
            "{body}"
        );
        let (status, _) = put("controller", json!({"nope": "x"})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (status, _) = put("controller", json!({"plugins": "yes"})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (status, _) = put("../etc", json!({})).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = put("aggregator", json!({"listen": "x"})).await;
        assert_eq!(
            status,
            StatusCode::CONFLICT,
            "never started with this directory"
        );
    }
}
