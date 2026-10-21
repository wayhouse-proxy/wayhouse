//! One pass of the sync, and the reload command that follows a change.

use std::path::PathBuf;
use std::time::Duration;

use crate::client::{Controller, FetchError, Fetched};
use crate::install;

/// How long a `--reload-cmd` may run.
const RELOAD_LIMIT: Duration = Duration::from_secs(30);

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Sites whose files were replaced this pass.
    pub updated: Vec<String>,
    /// Sites that could not be fetched or written, with the reason.
    pub failed: Vec<(String, String)>,
}

pub struct Syncer {
    controller: Controller,
    dir: PathBuf,
    /// The command and its arguments; run without a shell.
    reload: Option<Vec<String>>,
    /// Files changed but the reload has not succeeded yet.
    reload_pending: bool,
}

impl Syncer {
    pub fn new(controller: Controller, dir: PathBuf, reload: Option<Vec<String>>) -> Self {
        Self {
            controller,
            dir,
            reload,
            reload_pending: false,
        }
    }

    /// Lists the readable sites, fetches those whose version differs from the one on disk and
    /// reloads once if anything changed (or an earlier reload failed). Files of a site that
    /// is no longer listed, or any file when the controller is down, stay as they are.
    pub async fn run_once(&mut self) -> Result<Report, FetchError> {
        let entries = self.controller.list().await?;
        let mut report = Report::default();
        for entry in entries {
            let site = entry.site;
            if !install::valid_site(&site) {
                report
                    .failed
                    .push((site, "not a valid site name; skipped".into()));
                continue;
            }
            let local = install::local_version(&self.dir, &site);
            if local == Some(entry.version) {
                continue;
            }
            match self.controller.fetch(&site, local).await {
                Ok(Fetched::NotModified | Fetched::Gone) => {}
                Ok(Fetched::Bundle(bundle)) => match install::install(&self.dir, &site, &bundle) {
                    Ok(()) => {
                        tracing::info!(
                            site = %site,
                            version = bundle.version,
                            not_after = bundle.not_after,
                            untrusted = bundle.untrusted,
                            "certificate written"
                        );
                        if bundle.untrusted {
                            tracing::warn!(
                                site = %site,
                                "this certificate comes from a staging or test CA; clients will not trust it"
                            );
                        }
                        report.updated.push(site);
                    }
                    Err(e) => report
                        .failed
                        .push((site, format!("writing the files: {e}"))),
                },
                // A bad token or an unreachable controller fails every site alike.
                Err(e @ (FetchError::Unauthorized(_) | FetchError::Unreachable(_))) => {
                    return Err(e)
                }
                Err(e) => report.failed.push((site, e.to_string())),
            }
        }
        if !report.updated.is_empty() {
            self.reload_pending = true;
        }
        if self.reload_pending {
            if let Some(cmd) = &self.reload {
                match run_reload(cmd).await {
                    Ok(()) => self.reload_pending = false,
                    Err(why) => {
                        tracing::warn!(%why, "reload command failed; trying again next pass")
                    }
                }
            } else {
                self.reload_pending = false;
            }
        }
        Ok(report)
    }
}

async fn run_reload(cmd: &[String]) -> Result<(), String> {
    let (program, args) = cmd.split_first().ok_or("empty reload command")?;
    let mut child = tokio::process::Command::new(program)
        .args(args)
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("could not start {program}: {e}"))?;
    match tokio::time::timeout(RELOAD_LIMIT, child.wait()).await {
        Err(_) => Err(format!("{program} did not finish in {RELOAD_LIMIT:?}")),
        Ok(Err(e)) => Err(format!("waiting for {program}: {e}")),
        Ok(Ok(status)) if status.success() => Ok(()),
        Ok(Ok(status)) => Err(format!("{program} exited with {status}")),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    use axum::extract::{Path, State};
    use axum::http::{HeaderMap, StatusCode};
    use axum::response::{IntoResponse, Response};
    use axum::routing::get;
    use axum::{Json, Router};

    use super::*;
    use crate::install::{CHAIN_FILE, KEY_FILE};

    const TOKEN: &str = "gwt_good";

    #[derive(Default)]
    struct Fake {
        /// site -> (version, chain, key)
        sites: BTreeMap<String, u64>,
        fetches: Vec<String>,
        lists: usize,
        down: bool,
    }

    type Shared = Arc<Mutex<Fake>>;

    fn authorised(h: &HeaderMap) -> bool {
        h.get("authorization").and_then(|v| v.to_str().ok()) == Some(&format!("Bearer {TOKEN}"))
    }

    async fn list(State(f): State<Shared>, h: HeaderMap) -> Response {
        if !authorised(&h) {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        let mut f = f.lock().unwrap();
        f.lists += 1;
        if f.down {
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
        let sites: Vec<_> = f
            .sites
            .iter()
            .map(|(s, v)| serde_json::json!({"site": s, "version": v, "not_after": 2_000_000_000u64}))
            .collect();
        Json(serde_json::json!({ "sites": sites })).into_response()
    }

    async fn bundle(State(f): State<Shared>, h: HeaderMap, Path(site): Path<String>) -> Response {
        if !authorised(&h) {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        let mut f = f.lock().unwrap();
        f.fetches.push(site.clone());
        let Some(v) = f.sites.get(&site).copied() else {
            return StatusCode::NOT_FOUND.into_response();
        };
        Json(serde_json::json!({
            "version": v,
            "chain_pem": format!("-----BEGIN CERTIFICATE-----\nchain-{site}-{v}\n-----END CERTIFICATE-----\n"),
            "key_pem": format!("-----BEGIN PRIVATE KEY-----\nkey-{site}-{v}\n-----END PRIVATE KEY-----\n"),
            "not_after": 2_000_000_000u64,
            "serial": "01",
        }))
        .into_response()
    }

    struct Server {
        url: reqwest::Url,
        state: Shared,
        task: tokio::task::JoinHandle<()>,
    }

    async fn serve(sites: &[(&str, u64)]) -> Server {
        let state: Shared = Arc::new(Mutex::new(Fake {
            sites: sites.iter().map(|(s, v)| ((*s).into(), *v)).collect(),
            ..Fake::default()
        }));
        let app = Router::new()
            .route("/acme/bundles", get(list))
            .route("/acme/bundles/{site}", get(bundle))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url =
            reqwest::Url::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Server { url, state, task }
    }

    /// An address nothing listens on.
    async fn dead_url() -> reqwest::Url {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = reqwest::Url::parse(&format!("http://{}", l.local_addr().unwrap())).unwrap();
        drop(l);
        url
    }

    fn syncer(urls: Vec<reqwest::Url>, token: &str, dir: &std::path::Path) -> Syncer {
        let c = Controller::new(urls, token.into(), Duration::from_secs(5)).unwrap();
        Syncer::new(c, dir.to_path_buf(), None)
    }

    #[tokio::test]
    async fn a_pass_writes_every_listed_site_and_a_second_pass_asks_for_nothing() {
        let server = serve(&[("web", 1), ("api", 4)]).await;
        let dir = tempfile::tempdir().unwrap();
        let mut s = syncer(vec![server.url.clone()], TOKEN, dir.path());
        let report = s.run_once().await.unwrap();
        assert_eq!(report.updated, ["api", "web"]);
        assert!(
            std::fs::read_to_string(dir.path().join("web").join(KEY_FILE))
                .unwrap()
                .contains("key-web-1")
        );
        assert_eq!(install::local_version(dir.path(), "api"), Some(4));
        let inode = |p: &str| {
            use std::os::unix::fs::MetadataExt;
            std::fs::metadata(dir.path().join("web").join(p))
                .unwrap()
                .ino()
        };
        let (chain_ino, key_ino) = (inode(CHAIN_FILE), inode(KEY_FILE));
        let fetched = server.state.lock().unwrap().fetches.len();
        let again = s.run_once().await.unwrap();
        assert_eq!(again, Report::default());
        assert_eq!(
            server.state.lock().unwrap().fetches.len(),
            fetched,
            "no bundle was requested"
        );
        assert_eq!(
            (inode(CHAIN_FILE), inode(KEY_FILE)),
            (chain_ino, key_ino),
            "no rewrite"
        );
    }

    #[tokio::test]
    async fn a_new_version_replaces_the_files() {
        let server = serve(&[("web", 1)]).await;
        let dir = tempfile::tempdir().unwrap();
        let mut s = syncer(vec![server.url.clone()], TOKEN, dir.path());
        s.run_once().await.unwrap();
        server.state.lock().unwrap().sites.insert("web".into(), 2);
        let report = s.run_once().await.unwrap();
        assert_eq!(report.updated, ["web"]);
        assert!(
            std::fs::read_to_string(dir.path().join("web").join(CHAIN_FILE))
                .unwrap()
                .contains("chain-web-2")
        );
    }

    #[tokio::test]
    async fn while_the_controller_is_down_the_last_good_files_stay() {
        let server = serve(&[("web", 1)]).await;
        let dir = tempfile::tempdir().unwrap();
        let mut s = syncer(vec![server.url.clone()], TOKEN, dir.path());
        s.run_once().await.unwrap();
        // A 5xx answer, then no answer at all.
        server.state.lock().unwrap().down = true;
        assert!(matches!(
            s.run_once().await,
            Err(FetchError::Unreachable(_))
        ));
        server.task.abort();
        let _ = server.task.await;
        assert!(matches!(
            s.run_once().await,
            Err(FetchError::Unreachable(_))
        ));
        let key = std::fs::read_to_string(dir.path().join("web").join(KEY_FILE)).unwrap();
        assert!(key.contains("key-web-1"));
        assert_eq!(install::local_version(dir.path(), "web"), Some(1));
    }

    #[tokio::test]
    async fn the_next_controller_is_tried_when_the_first_is_unreachable() {
        let live = serve(&[("web", 1)]).await;
        let dir = tempfile::tempdir().unwrap();
        let mut s = syncer(vec![dead_url().await, live.url.clone()], TOKEN, dir.path());
        let report = s.run_once().await.unwrap();
        assert_eq!(report.updated, ["web"]);
        // The working one is tried first from now on.
        assert_eq!(live.state.lock().unwrap().lists, 1);
        s.run_once().await.unwrap();
        assert_eq!(live.state.lock().unwrap().lists, 2);
    }

    #[tokio::test]
    async fn a_refused_token_writes_nothing_and_names_the_token() {
        let server = serve(&[("web", 1)]).await;
        let dir = tempfile::tempdir().unwrap();
        let mut s = syncer(vec![server.url.clone()], "gwt_wrong", dir.path());
        let e = s.run_once().await.unwrap_err();
        assert!(matches!(e, FetchError::Unauthorized(401)), "{e}");
        assert!(e.to_string().contains("--token-file"));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn an_unsafe_site_name_from_the_controller_is_skipped() {
        let server = serve(&[("../evil", 1), ("web", 1)]).await;
        let dir = tempfile::tempdir().unwrap();
        let mut s = syncer(vec![server.url.clone()], TOKEN, dir.path());
        let report = s.run_once().await.unwrap();
        assert_eq!(report.updated, ["web"]);
        assert_eq!(report.failed.len(), 1);
        assert!(!dir.path().parent().unwrap().join("evil").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn reload_runs_after_a_change_and_is_retried_until_it_succeeds() {
        let server = serve(&[("web", 1)]).await;
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("reloaded");
        let ok_marker = dir.path().join("ok");
        let c = Controller::new(
            vec![server.url.clone()],
            TOKEN.into(),
            Duration::from_secs(5),
        )
        .unwrap();
        // Fails while `ok` is missing; touches `reloaded` on success.
        let script = format!(
            "test -e {ok} && echo x >> {marker}",
            ok = ok_marker.display(),
            marker = marker.display()
        );
        let mut s = Syncer::new(
            c,
            dir.path().join("certs"),
            Some(vec!["sh".into(), "-c".into(), script]),
        );
        s.run_once().await.unwrap();
        assert!(!marker.exists(), "the first reload failed");
        std::fs::write(&ok_marker, "").unwrap();
        s.run_once().await.unwrap();
        assert_eq!(
            std::fs::read_to_string(&marker).unwrap(),
            "x\n",
            "retried once the command works"
        );
        s.run_once().await.unwrap();
        assert_eq!(
            std::fs::read_to_string(&marker).unwrap(),
            "x\n",
            "an unchanged pass does not reload"
        );
    }
}
