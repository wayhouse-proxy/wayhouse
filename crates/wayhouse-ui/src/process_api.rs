//! Process flags of the fleet's own programs (`/api/process/*`, G1).
//!
//! The UI backend reads and writes the settings files of `wayhouse-settings` in `--settings-dir`;
//! the programs notice a change, restart, and undo it unless it is confirmed
//! (`docs/12-deployment.md`). The flag list of each program is the one it published when it last
//! started, so a program that never ran with this directory shows as "not connected".
//!
//! Reads need `Viewer`; applying, confirming and undoing need `Admin`, the level that can already
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

/// What the page shows of a settings file.
fn file_json(f: &File) -> serde_json::Value {
    json!({
        "rev": f.rev,
        "values": f.values,
        "previous": f.previous,
        "pending": f.pending.as_ref().map(|p| json!({
            "applied_at": p.applied_at,
            "attempts": p.attempts,
            "confirm_secs": wayhouse_settings::CONFIRM_SECS,
        })),
        "error": f.error,
    })
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
) -> Result<T, Box<Response>> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|_| Box::new(error(StatusCode::INTERNAL_SERVER_ERROR, "internal error")))
}

async fn list(State(state): State<AppState>) -> Response {
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
                    "file": file.as_ref().map(file_json),
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
            Json(file_json(&file)).into_response()
        }
        Ok(Err(e)) => refusal(e),
        Err(r) => *r,
    }
}

async fn confirm(
    State(state): State<AppState>,
    Extension(actor): Extension<Actor>,
    Path(program): Path<String>,
) -> Response {
    finish(&state, &program, &actor, true).await
}

async fn undo(
    State(state): State<AppState>,
    Extension(actor): Extension<Actor>,
    Path(program): Path<String>,
) -> Response {
    finish(&state, &program, &actor, false).await
}

async fn finish(state: &AppState, program: &str, actor: &Actor, confirm: bool) -> Response {
    let (dir, name) = match target(state, program) {
        Ok(t) => t,
        Err(r) => return *r,
    };
    let result = blocking(move || {
        if confirm {
            wayhouse_settings::confirm(&dir, name)
        } else {
            wayhouse_settings::undo(&dir, name)
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
            Json(file_json(&file)).into_response()
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
        let (status, undone) =
            call(&app, "POST", "/api/process/controller/undo", json!(null)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(undone["values"], json!({}));
        assert_eq!(undone["pending"], json!(null));
        let (status, _) = put(json!({"listen": "0.0.0.0:9"}), 1).await;
        assert_eq!(status, StatusCode::CONFLICT, "stale revision");
        let (status, file) = put(json!({"listen": "0.0.0.0:9"}), 2).await;
        assert_eq!(status, StatusCode::OK, "{file}");
        let (status, done) =
            call(&app, "POST", "/api/process/controller/confirm", json!(null)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(done["pending"], json!(null));
        assert_eq!(done["values"], json!({"listen": "0.0.0.0:9"}));
        let (status, _) = call(&app, "POST", "/api/process/controller/confirm", json!(null)).await;
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
