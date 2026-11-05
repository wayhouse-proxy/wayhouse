//! The optional plugins file: the installs an operator wants, reconciled with the install
//! database (P6, #466).
//!
//! ```yaml
//! plugins:
//!   - name: pelican-main
//!     sha256: 9f2c...        # a module already stored (uploaded or fetched from a registry)
//!     config: { panel_host: panel.example.com }
//!     enabled: true
//!     approved_sha256: 41ab...  # sha-256 of the capability set the options resolve to
//! ```
//!
//! The file never carries module bytes, secrets or webhook tokens; those stay in the API.
//! Rules the reconcile keeps:
//! - It owns only the installs it created (`created_by` is [`FILE_ACTOR`]); an install made in
//!   the UI or API is never changed, adopted or removed, and a clashing name is reported.
//! - An entry is applied only when the capability set its options resolve to hashes to its
//!   `approved_sha256`; the dry run shows the hash to copy. A module that is not stored, or
//!   whose stored sha-256 differs, is refused.
//! - A file that cannot be read or parsed changes nothing. Removing an entry removes its
//!   install; changing an entry's module is done by renaming it (the old install goes, the
//!   new one starts without the old one's secrets).
//! - Managed installs are read-only in the API (`409 managed_by_file`) for config, enable,
//!   disable and delete. Under HA only the leader reconciles.
//!
//! Reconcile runs at start, on `SIGHUP` and every [`INTERVAL`], so a new leader or a module
//! uploaded after the file was written is picked up without a signal.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use wayhouse_plugin_host::{inspect, Capabilities};

use super::api::{self, PluginsState};
use super::{valid_name, InstallRecord, MAX_CONFIG_BYTES};

/// The `created_by` of installs the file owns.
pub const FILE_ACTOR: &str = "plugins-file";
/// The header the reconciler's own calls carry. Its value is a per-process secret that
/// never leaves the process (it is not among the headers a follower forwards), so an HTTP
/// caller cannot pass as the reconciler by sending `x-actor: plugins-file` or this header.
pub const FILE_HEADER: &str = "x-wayhouse-plugins-file";

fn file_token() -> &'static str {
    static TOKEN: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    TOKEN.get_or_init(|| format!("{:032x}", rand::random::<u128>()))
}

/// Whether this call is the reconciler's own.
pub fn is_file_caller(headers: &HeaderMap) -> bool {
    headers
        .get(FILE_HEADER)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v == file_token())
}
/// How often the file is reconciled without a signal.
pub const INTERVAL: Duration = Duration::from_secs(30);
const MAX_FILE_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub struct DeclaredFile {
    pub plugins: Vec<Declared>,
}

/// `plugins` is `Option` so that a missing key and a bare `plugins:` (null) can be told
/// from `plugins: []`: an empty, comment-only or half-written file must be a parse error,
/// not "remove everything". Write `plugins: []` to say that on purpose.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    plugins: Option<Vec<Declared>>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Declared {
    pub name: String,
    pub sha256: String,
    #[serde(default)]
    pub config: serde_json::Value,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Lowercase hex sha-256 of the effective capability set as JSON (the dry run prints it).
    pub approved_sha256: String,
}

fn yes() -> bool {
    true
}

fn is_hex64(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl DeclaredFile {
    pub fn parse(text: &str) -> Result<Self, String> {
        if text.len() > MAX_FILE_BYTES {
            return Err(format!("the file is over {MAX_FILE_BYTES} bytes"));
        }
        let raw: Raw = serde_norway::from_str(text).map_err(|e| e.to_string())?;
        let mut file = Self {
            plugins: raw.plugins.ok_or(
                "the file needs a `plugins:` list (`plugins: []` removes every managed install)",
            )?,
        };
        let mut seen = std::collections::HashSet::new();
        for d in &mut file.plugins {
            if !valid_name(&d.name) {
                return Err(format!("{:?} is not a valid install name", d.name));
            }
            if !seen.insert(d.name.clone()) {
                return Err(format!("{:?} is listed twice", d.name));
            }
            if !is_hex64(&d.sha256) || !is_hex64(&d.approved_sha256) {
                return Err(format!(
                    "{}: sha256 and approved_sha256 must be 64 lowercase hex characters",
                    d.name
                ));
            }
            if d.config.is_null() {
                d.config = serde_json::json!({});
            }
            if serde_json::to_vec(&d.config).map_or(true, |b| b.len() > MAX_CONFIG_BYTES) {
                return Err(format!(
                    "{}: config is over {MAX_CONFIG_BYTES} bytes",
                    d.name
                ));
            }
        }
        Ok(file)
    }
}

/// Hash of a capability set, as the file states it.
pub fn caps_hash(caps: &Capabilities) -> String {
    let bytes = serde_json::to_vec(caps).expect("capabilities serialize");
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// What one entry (or one owned install no entry names) comes to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Row {
    pub name: String,
    pub id: Option<String>,
    /// `create`, `update`, `remove`, `unchanged` or `refused`.
    pub action: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// For a refusal over the hash: the set's real hash, to paste into the file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_sha256: Option<String>,
    /// Set after an apply that failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Op {
    Create {
        d: Declared,
        approved: Capabilities,
    },
    SetConfig {
        id: String,
        config: serde_json::Value,
        approved: Capabilities,
    },
    SetEnabled {
        id: String,
        enabled: bool,
    },
    Remove {
        id: String,
    },
}

pub struct Planned {
    pub rows: Vec<Row>,
    /// Row index and the operation, in the order to run them (removals last).
    pub ops: Vec<(usize, Op)>,
}

/// The capability set `config` resolves to for the stored module `sha256`, or why not.
pub type Resolve<'a> = &'a dyn Fn(&str, &serde_json::Value) -> Result<Capabilities, String>;

fn refused(name: &str, id: Option<String>, reason: impl Into<String>) -> Row {
    Row {
        name: name.into(),
        id,
        action: "refused",
        reason: Some(reason.into()),
        effective_sha256: None,
        error: None,
    }
}

/// Pure: compare the file with the installs. Never calls the API.
pub fn plan(file: &DeclaredFile, installs: &[InstallRecord], resolve: Resolve<'_>) -> Planned {
    let mut rows = Vec::new();
    let mut ops = Vec::new();
    let mut removals = Vec::new();
    let owned = |r: &&InstallRecord| r.created_by.as_deref() == Some(FILE_ACTOR);
    // Resolve and check the approval hash; on failure the row says why.
    let approve = |d: &Declared, id: Option<String>| -> Result<Capabilities, Box<Row>> {
        let caps =
            resolve(&d.sha256, &d.config).map_err(|why| refused(&d.name, id.clone(), why))?;
        let hash = caps_hash(&caps);
        if hash == d.approved_sha256 {
            Ok(caps)
        } else {
            let mut row = refused(
                &d.name,
                id,
                "approved_sha256 does not match the capabilities these options resolve to",
            );
            row.effective_sha256 = Some(hash);
            Err(Box::new(row))
        }
    };
    for d in &file.plugins {
        let same: Vec<&InstallRecord> = installs.iter().filter(|r| r.name == d.name).collect();
        let row_ix = rows.len();
        match same.as_slice() {
            [] => match approve(d, None) {
                Ok(approved) => {
                    rows.push(Row {
                        name: d.name.clone(),
                        id: None,
                        action: "create",
                        reason: None,
                        effective_sha256: None,
                        error: None,
                    });
                    ops.push((
                        row_ix,
                        Op::Create {
                            d: d.clone(),
                            approved,
                        },
                    ));
                }
                Err(row) => rows.push(*row),
            },
            [rec] if owned(rec) => {
                let id = Some(rec.id.clone());
                if rec.sha256 != d.sha256 {
                    rows.push(refused(
                        &d.name,
                        id,
                        "the module differs from the installed one; rename the entry to replace the install",
                    ));
                    continue;
                }
                let mut step = Vec::new();
                if rec.config != d.config {
                    match approve(d, id.clone()) {
                        Ok(approved) => step.push(Op::SetConfig {
                            id: rec.id.clone(),
                            config: d.config.clone(),
                            approved,
                        }),
                        Err(row) => {
                            rows.push(*row);
                            continue;
                        }
                    }
                }
                if rec.enabled != d.enabled {
                    step.push(Op::SetEnabled {
                        id: rec.id.clone(),
                        enabled: d.enabled,
                    });
                }
                rows.push(Row {
                    name: d.name.clone(),
                    id,
                    action: if step.is_empty() {
                        "unchanged"
                    } else {
                        "update"
                    },
                    reason: None,
                    effective_sha256: None,
                    error: None,
                });
                ops.extend(step.into_iter().map(|op| (row_ix, op)));
            }
            [_] => rows.push(refused(
                &d.name,
                None,
                "an install with this name exists and the file does not own it",
            )),
            many => rows.push(refused(
                &d.name,
                None,
                format!(
                    "{} installs share this name ({}); delete the extra ones",
                    many.len(),
                    many.iter()
                        .map(|r| r.id.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            )),
        }
    }
    let named: std::collections::HashSet<&str> =
        file.plugins.iter().map(|d| d.name.as_str()).collect();
    for rec in installs.iter().filter(owned) {
        if !named.contains(rec.name.as_str()) {
            removals.push((rows.len(), Op::Remove { id: rec.id.clone() }));
            rows.push(Row {
                name: rec.name.clone(),
                id: Some(rec.id.clone()),
                action: "remove",
                reason: Some("no longer in the file".into()),
                effective_sha256: None,
                error: None,
            });
        }
    }
    ops.extend(removals);
    Planned { rows, ops }
}

/// Reads the file and the store, and plans; nothing is changed.
#[derive(Clone)]
pub struct Reconciler {
    st: PluginsState,
    path: Arc<PathBuf>,
}

impl Reconciler {
    pub fn new(st: PluginsState, path: PathBuf) -> Self {
        Self {
            st,
            path: Arc::new(path),
        }
    }

    fn resolve(&self, sha: &str, config: &serde_json::Value) -> Result<Capabilities, String> {
        let bytes = self
            .st
            .store
            .get_blob(sha)
            .map_err(|e| e.to_string())?
            .ok_or(
                "no module with this sha256 is stored: upload it or fetch it from a registry first",
            )?;
        let info = inspect(&bytes).map_err(|e| e.to_string())?;
        info.effective_caps(config).map_err(|e| e.to_string())
    }

    /// Standalone: always. Under HA: this node is the leader and has applied every committed
    /// entry, so a fresh leader does not plan from a store that is behind (and create a
    /// duplicate of what the old leader just created).
    async fn leader_caught_up(&self) -> bool {
        match &self.st.ha {
            None => true,
            Some(ha) => ha.is_leader() && ha.raft.ensure_linearizable().await.is_ok(),
        }
    }

    pub async fn plan(&self) -> Result<Planned, String> {
        let read = |e: std::io::Error| format!("cannot read {}: {e}", self.path.display());
        let len = tokio::fs::metadata(&*self.path).await.map_err(read)?.len();
        if len > MAX_FILE_BYTES as u64 {
            return Err(format!("the file is over {MAX_FILE_BYTES} bytes"));
        }
        let text = tokio::fs::read_to_string(&*self.path).await.map_err(read)?;
        let file = DeclaredFile::parse(&text)?;
        let installs = self.st.store.list().map_err(|e| e.to_string())?;
        let this = self.clone();
        let resolve = move |sha: &str, c: &serde_json::Value| this.resolve(sha, c);
        // Compiling and inspecting modules is CPU work; keep it off the async workers.
        tokio::task::spawn_blocking(move || plan(&file, &installs, &resolve))
            .await
            .map_err(|e| e.to_string())
    }

    /// Plans and runs. A failed operation is recorded on its row and the rest still run.
    pub async fn reconcile(&self) -> Result<Vec<Row>, String> {
        let Planned { mut rows, ops } = self.plan().await?;
        let mut headers = HeaderMap::new();
        headers.insert(
            FILE_HEADER,
            HeaderValue::from_str(file_token()).expect("hex is a header value"),
        );
        for (ix, op) in ops {
            // A failed config step leaves the rest of that entry's steps for the next pass.
            if rows[ix].error.is_some() {
                continue;
            }
            // A forwarded call would lose the reconciler's identity, so never forward.
            if self.st.ha.as_ref().is_some_and(|h| !h.is_leader()) {
                rows[ix].error = Some("not the leader any more".into());
                continue;
            }
            let resp = self.run(&headers, op).await;
            if !resp.status().is_success() {
                let status = resp.status();
                let body = axum::body::to_bytes(resp.into_body(), 64 * 1024)
                    .await
                    .unwrap_or_default();
                let why = serde_json::from_slice::<serde_json::Value>(&body)
                    .ok()
                    .and_then(|v| v["error"].as_str().map(str::to_owned))
                    .unwrap_or_else(|| status.to_string());
                rows[ix].error = Some(format!("{status}: {why}"));
            }
        }
        Ok(rows)
    }

    async fn run(&self, headers: &HeaderMap, op: Op) -> Response {
        let st = &self.st;
        match op {
            Op::Create { d, approved } => {
                let body = serde_json::json!({
                    "name": d.name, "sha256": d.sha256, "config": d.config,
                    "approved": approved, "enabled": d.enabled,
                });
                let body = axum::body::Bytes::from(body.to_string());
                api::install_inner(st, headers, body)
                    .await
                    .unwrap_or_else(|r| r)
            }
            Op::SetConfig {
                id,
                config,
                approved,
            } => {
                let body = axum::body::Bytes::from(
                    serde_json::json!({ "config": config, "approved": approved }).to_string(),
                );
                api::put_config_inner(st, headers, &id, body)
                    .await
                    .unwrap_or_else(|r| r)
            }
            Op::SetEnabled { id, enabled } => api::set_enabled(st, headers, id, enabled).await,
            Op::Remove { id } => api::remove(State(st.clone()), headers.clone(), Path(id)).await,
        }
    }
}

/// Runs the reconcile loop; returns at once when `reconciler` is `None`.
pub fn spawn(reconciler: Reconciler) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        #[cfg(unix)]
        let mut hup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup()).ok();
        let mut last = String::new();
        loop {
            // Only the leader writes; a standalone controller is its own leader.
            if reconciler.leader_caught_up().await {
                let summary = match reconciler.reconcile().await {
                    Ok(rows) => {
                        let changed: Vec<String> = rows
                            .iter()
                            .filter(|r| r.action != "unchanged")
                            .map(|r| match (&r.error, &r.reason) {
                                (Some(e), _) => format!("{} {}: failed: {e}", r.action, r.name),
                                (None, Some(why)) => format!("{} {}: {why}", r.action, r.name),
                                _ => format!("{} {}", r.action, r.name),
                            })
                            .collect();
                        changed.join("; ")
                    }
                    Err(why) => format!("not reconciled: {why}"),
                };
                // Log a state once, not every interval.
                if summary != last {
                    if !summary.is_empty() {
                        tracing::info!(target: "wayhouse_controller::plugins::audit", what = "plugins file", %summary, "plugins file reconciled");
                    }
                    last = summary;
                }
            }
            #[cfg(unix)]
            match hup.as_mut() {
                Some(h) => tokio::select! {
                    _ = h.recv() => {}
                    _ = tokio::time::sleep(INTERVAL) => {}
                },
                None => tokio::time::sleep(INTERVAL).await,
            }
            #[cfg(not(unix))]
            tokio::time::sleep(INTERVAL).await;
        }
    })
}

/// `GET /plugins/file/plan`: the dry run, computed now from the file as it is on disk.
pub fn router(reconciler: Reconciler, auth_token: Option<&str>) -> Router {
    async fn show(State(r): State<Reconciler>) -> Response {
        match r.plan().await {
            Ok(p) => {
                Json(serde_json::json!({ "applied": false, "plugins": p.rows })).into_response()
            }
            Err(why) => api::err(StatusCode::UNPROCESSABLE_ENTITY, why),
        }
    }
    Router::new()
        .route("/plugins/file/plan", get(show))
        .route_layer(axum::middleware::from_fn_with_state(
            wayhouse_http::server::BearerAuth::new(auth_token),
            wayhouse_http::server::require_bearer,
        ))
        .with_state(reconciler)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const SHA_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn caps() -> Capabilities {
        Capabilities::parse(br#"{"log":true}"#).unwrap()
    }

    fn entry(name: &str, sha: &str) -> Declared {
        Declared {
            name: name.into(),
            sha256: sha.into(),
            config: serde_json::json!({}),
            enabled: true,
            approved_sha256: caps_hash(&caps()),
        }
    }

    fn rec(id: &str, name: &str, sha: &str, by: &str) -> InstallRecord {
        InstallRecord {
            id: id.into(),
            name: name.into(),
            sha256: sha.into(),
            size: 1,
            approved: caps(),
            config: serde_json::json!({}),
            enabled: true,
            created_at: 0,
            created_by: Some(by.into()),
            webhook: None,
            suspended: std::collections::BTreeSet::default(),
        }
    }

    fn ok(_: &str, _: &serde_json::Value) -> Result<Capabilities, String> {
        Ok(caps())
    }

    fn file(plugins: Vec<Declared>) -> DeclaredFile {
        DeclaredFile { plugins }
    }

    fn actions(p: &Planned) -> Vec<(&str, &'static str)> {
        p.rows.iter().map(|r| (r.name.as_str(), r.action)).collect()
    }

    #[test]
    fn a_new_entry_is_created_and_a_second_run_changes_nothing() {
        let f = file(vec![entry("one", SHA_A)]);
        let p = plan(&f, &[], &ok);
        assert_eq!(actions(&p), [("one", "create")]);
        assert!(matches!(p.ops[0].1, Op::Create { .. }));
        // The install the first run made.
        let made = rec("0000000000000001", "one", SHA_A, FILE_ACTOR);
        let again = plan(&f, &[made], &ok);
        assert_eq!(actions(&again), [("one", "unchanged")]);
        assert!(again.ops.is_empty());
    }

    #[test]
    fn the_file_never_touches_or_removes_an_install_it_does_not_own() {
        let ui = rec("0000000000000002", "mine", SHA_A, "alice");
        // Same name: reported, not adopted.
        let clash = plan(
            &file(vec![entry("mine", SHA_A)]),
            std::slice::from_ref(&ui),
            &ok,
        );
        assert_eq!(actions(&clash), [("mine", "refused")]);
        assert!(clash.ops.is_empty());
        // Not named: left alone.
        let empty = plan(&file(vec![]), &[ui], &ok);
        assert!(empty.rows.is_empty() && empty.ops.is_empty());
    }

    #[test]
    fn an_owned_install_that_left_the_file_is_removed_last() {
        let gone = rec("0000000000000003", "old", SHA_A, FILE_ACTOR);
        let p = plan(&file(vec![entry("new", SHA_A)]), &[gone], &ok);
        assert_eq!(actions(&p), [("new", "create"), ("old", "remove")]);
        assert!(matches!(p.ops.last().unwrap().1, Op::Remove { .. }));
    }

    #[test]
    fn a_wrong_approval_hash_is_refused_and_shows_the_real_one() {
        let mut d = entry("one", SHA_A);
        d.approved_sha256 = SHA_B.into();
        let p = plan(&file(vec![d]), &[], &ok);
        assert_eq!(actions(&p), [("one", "refused")]);
        assert_eq!(
            p.rows[0].effective_sha256.as_deref(),
            Some(caps_hash(&caps()).as_str())
        );
        assert!(p.ops.is_empty());
    }

    #[test]
    fn two_installs_with_one_name_are_reported_with_their_ids() {
        let a = rec("000000000000000a", "one", SHA_A, FILE_ACTOR);
        let b = rec("000000000000000b", "one", SHA_A, FILE_ACTOR);
        let p = plan(&file(vec![entry("one", SHA_A)]), &[a, b], &ok);
        assert_eq!(actions(&p), [("one", "refused")]);
        let why = p.rows[0].reason.as_deref().unwrap();
        assert!(
            why.contains("000000000000000a") && why.contains("000000000000000b"),
            "{why}"
        );
        assert!(p.ops.is_empty());
    }

    #[test]
    fn a_module_that_is_not_stored_is_refused() {
        let missing = |_: &str, _: &serde_json::Value| {
            Err("no module with this sha256 is stored".to_string())
        };
        let p = plan(&file(vec![entry("one", SHA_A)]), &[], &missing);
        assert_eq!(actions(&p), [("one", "refused")]);
        assert!(p.ops.is_empty());
    }

    #[test]
    fn a_changed_module_needs_a_new_name_and_a_changed_config_or_flag_is_updated() {
        let mine = rec("0000000000000004", "one", SHA_A, FILE_ACTOR);
        let swapped = plan(
            &file(vec![entry("one", SHA_B)]),
            std::slice::from_ref(&mine),
            &ok,
        );
        assert_eq!(actions(&swapped), [("one", "refused")]);
        let mut d = entry("one", SHA_A);
        d.config = serde_json::json!({"x": 1});
        d.enabled = false;
        let p = plan(&file(vec![d]), &[mine], &ok);
        assert_eq!(actions(&p), [("one", "update")]);
        assert!(matches!(p.ops[0].1, Op::SetConfig { .. }));
        assert!(matches!(p.ops[1].1, Op::SetEnabled { enabled: false, .. }));
    }

    #[test]
    fn parsing_refuses_duplicates_bad_names_bad_hashes_and_unknown_keys() {
        let ok_entry = format!("- name: a\n  sha256: {SHA_A}\n  approved_sha256: {SHA_B}\n");
        assert!(DeclaredFile::parse(&format!("plugins:\n{}", indent(&ok_entry))).is_ok());
        assert!(DeclaredFile::parse("plugins: []")
            .unwrap()
            .plugins
            .is_empty());
        let dup = format!("plugins:\n{}{}", indent(&ok_entry), indent(&ok_entry));
        assert!(DeclaredFile::parse(&dup).unwrap_err().contains("twice"));
        let bad_name = ok_entry.replace("name: a", "name: A_B");
        assert!(DeclaredFile::parse(&format!("plugins:\n{}", indent(&bad_name))).is_err());
        let bad_sha = ok_entry.replace(SHA_A, "xyz");
        assert!(DeclaredFile::parse(&format!("plugins:\n{}", indent(&bad_sha))).is_err());
        let unknown = format!("{ok_entry}  secrets: {{}}\n");
        assert!(DeclaredFile::parse(&format!("plugins:\n{}", indent(&unknown))).is_err());
        assert!(DeclaredFile::parse("extra: 1").is_err());
        // Empty, blank, comment-only and key-without-value documents are errors, never
        // "remove everything"; an explicit empty list is the way to say that.
        for broken in ["", "   \n", "# only a comment\n", "plugins:\n"] {
            assert!(DeclaredFile::parse(broken).is_err(), "{broken:?}");
        }
    }

    fn indent(s: &str) -> String {
        s.lines().map(|l| format!("  {l}\n")).collect()
    }
}
