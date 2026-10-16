//! One sync cycle: read `wayhouse`, render, and load into Caddy only when that is safe.
//!
//! The rule everything here serves: **never hand Caddy an empty or partial document.**
//! `POST /load` with `{}` answers 200 and removes every listener (spike #355), so a
//! failed read, a missing certificate or an unknown pool must leave Caddy on the
//! configuration it has.

use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use wayhouse_config::{WebUpstreamView, WebVia, WebView};

use crate::admin_addr::AdminAddr;
use crate::caddy::{CaddyClient, LoadError};
use crate::certs;
use crate::metrics_defs as m;
use crate::pools::PoolBackends;
use crate::render::{render, RenderInput};
use crate::source::Source;

pub trait Loader {
    fn load(&self, doc: &Value) -> impl std::future::Future<Output = Result<(), LoadError>>;
}

impl Loader for CaddyClient {
    async fn load(&self, doc: &Value) -> Result<(), LoadError> {
        CaddyClient::load(self, doc).await
    }
}

/// What a cycle did, for logs and tests.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// `wayhouse` has no `web:` section (or has no sites yet), and nothing was ever
    /// loaded, so Caddy is left alone.
    Idle,
    /// Caddy already has this document.
    Unchanged,
    Loaded,
    /// Caddy refused the document; it keeps its previous configuration.
    Rejected,
    /// Failed before Caddy had a new document: `stage` is `read`, `render` or `send`.
    Failed(&'static str),
}

pub struct Syncer<S, L> {
    source: S,
    loader: L,
    admin: AdminAddr,
    /// What Caddy has, as far as we know.
    loaded: Option<Value>,
    /// The last section we rendered from, to empty it if `web:` is later removed.
    last_web: Option<WebView>,
    /// A document Caddy refused, so an unchanged config is not re-sent every cycle.
    rejected: Option<Value>,
}

impl<S: Source, L: Loader> Syncer<S, L> {
    pub fn new(source: S, loader: L, admin: AdminAddr) -> Self {
        Self {
            source,
            loader,
            admin,
            loaded: None,
            last_web: None,
            rejected: None,
        }
    }

    /// `resend`: send even if the document equals the last one loaded. The caller
    /// sets it periodically so a Caddy that restarted (and lost its configuration)
    /// is brought back; an identical load is a no-op for a Caddy that kept it.
    pub async fn run_once(&mut self, resend: bool) -> Outcome {
        let web = match self.source.web().await {
            Ok(Some(w)) => w,
            Ok(None) => match self.last_web.clone() {
                // The section was removed after we served it: withdraw the sites.
                Some(mut w) => {
                    w.sites.clear();
                    w
                }
                None => return Outcome::Idle,
            },
            Err(e) => {
                tracing::warn!(error = %e, "cannot read wayhouse; leaving Caddy as it is");
                return fail("read");
            }
        };
        // Before anything was loaded an empty site list means "nothing to serve yet",
        // and sending it would replace Caddy's base configuration.
        if web.sites.is_empty() && self.loaded.is_none() {
            return Outcome::Idle;
        }

        let pools = if needs_pools(&web) {
            match self.source.pools().await {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!(error = %e, "cannot read pools; leaving Caddy as it is");
                    return fail("read");
                }
            }
        } else {
            PoolBackends::new()
        };
        let site_certs = match certs::load(&web).await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = %e, "certificate files unusable; leaving Caddy as it is");
                return fail("render");
            }
        };

        let started = Instant::now();
        let doc = match render(&RenderInput {
            web: &web,
            pools: &pools,
            certs: &site_certs,
            admin: &self.admin,
        }) {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!(error = %e, "cannot render the web section; leaving Caddy as it is");
                return fail("render");
            }
        };
        metrics::gauge!(m::RENDER_SECONDS).set(started.elapsed().as_secs_f64());
        self.last_web = Some(web);

        if self.loaded.as_ref() == Some(&doc) && !resend {
            mark_ok();
            return Outcome::Unchanged;
        }
        if self.rejected.as_ref() == Some(&doc) {
            return Outcome::Rejected;
        }
        match self.loader.load(&doc).await {
            Ok(()) => {
                if self.loaded.as_ref() != Some(&doc) {
                    tracing::info!("loaded a new configuration into Caddy");
                }
                self.loaded = Some(doc);
                self.rejected = None;
                metrics::counter!(m::LOADS).increment(1);
                mark_ok();
                Outcome::Loaded
            }
            Err(LoadError::Rejected { status, body }) => {
                tracing::error!(status, %body, "Caddy rejected the configuration; it keeps its previous one");
                metrics::counter!(m::REJECTS).increment(1);
                self.rejected = Some(doc);
                Outcome::Rejected
            }
            Err(e) => {
                tracing::warn!(error = %e, "cannot load into Caddy; will retry");
                fail("send")
            }
        }
    }
}

fn needs_pools(web: &WebView) -> bool {
    web.sites.iter().any(|s| {
        s.upstreams.iter().any(|u| {
            matches!(
                u,
                WebUpstreamView::Pool {
                    via: WebVia::Direct,
                    ..
                }
            )
        })
    })
}

fn fail(stage: &'static str) -> Outcome {
    metrics::counter!(m::ERRORS, "stage" => stage).increment(1);
    Outcome::Failed(stage)
}

fn mark_ok() {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs() as f64);
    metrics::gauge!(m::LAST_OK).set(now);
}

#[cfg(test)]
mod tests;
