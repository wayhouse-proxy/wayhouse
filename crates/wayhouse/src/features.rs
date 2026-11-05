//! The proxy's view of the feature-flag registry (`wayhouse_config::features`): which flags
//! this build contains, their effective values, and the metrics that expose them.

use std::collections::BTreeMap;

use wayhouse_config::features::{view, FeatureView, Scope, Source};
use wayhouse_core::metrics_defs as m;

/// Whether this binary contains the code behind `flag`. A flag with no entry here always does.
fn available(flag: &str) -> bool {
    [
        ("gossip_verdicts", cfg!(feature = "gossip")),
        ("sniffers", cfg!(feature = "wasm-sniffers")),
    ]
    .iter()
    .all(|(name, built)| *name != flag || *built)
}

/// Every proxy flag with its effective value; `config` is the live `features:` section.
pub fn effective(config: &BTreeMap<String, bool>) -> Vec<FeatureView> {
    view(Scope::Proxy, config, &overrides(), available)
}

/// Flag name to `(value, expires_at)`, as the controller last served it.
static OVERRIDES: std::sync::RwLock<BTreeMap<String, (bool, Option<u64>)>> =
    std::sync::RwLock::new(BTreeMap::new());

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The controller's overrides still in force (expiry is judged here, so a controller that
/// goes away cannot hold a protection flag off past its time).
pub fn overrides() -> BTreeMap<String, bool> {
    let now = unix_now();
    OVERRIDES
        .read()
        .map(|g| {
            g.iter()
                .filter(|(_, (_, exp))| exp.is_none_or(|e| e > now))
                .map(|(k, (v, _))| (k.clone(), *v))
                .collect()
        })
        .unwrap_or_default()
}

type Overrides = BTreeMap<String, (bool, Option<u64>)>;

/// Reads a `GET /features/overrides` body; `None` for anything that is not that document.
/// An item without a name or a boolean value is skipped (and logged), not the whole document.
fn parse_overrides(body: &[u8]) -> Option<Overrides> {
    let doc: serde_json::Value = serde_json::from_slice(body).ok()?;
    Some(
        doc["items"]
            .as_array()?
            .iter()
            .filter_map(|i| {
                let item = i["name"].as_str().zip(i["value"].as_bool());
                if item.is_none() {
                    tracing::warn!(item = %i, "feature override skipped: it needs a name and a value");
                }
                let (name, value) = item?;
                Some((name.to_string(), (value, i["expires_at"].as_u64())))
            })
            .collect(),
    )
}

/// Writes the overrides in the shape the controller serves them, so a restart without a
/// reachable controller keeps a kill switch where it was. A failure is logged, never fatal.
fn save(path: &std::path::Path, overrides: &Overrides) {
    let items: Vec<_> = overrides
        .iter()
        .map(|(name, (value, exp))| serde_json::json!({"name": name, "value": value, "expires_at": exp}))
        .collect();
    let tmp = path.with_extension("tmp");
    let doc = serde_json::json!({ "items": items }).to_string();
    if let Err(e) = std::fs::write(&tmp, doc).and_then(|()| std::fs::rename(&tmp, path)) {
        tracing::warn!(path = %path.display(), error = %e, "feature overrides not saved");
    }
}

/// Loads the saved overrides at start; an expired one is judged out by [`overrides`].
fn restore(path: &std::path::Path) -> bool {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return false,
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "saved feature overrides not read");
            return false;
        }
    };
    let Some(saved) = parse_overrides(&bytes) else {
        tracing::warn!(path = %path.display(), "saved feature overrides are not a valid document; ignored");
        return false;
    };
    set_overrides(saved);
    true
}

fn set_overrides(next: Overrides) -> bool {
    let Ok(mut g) = OVERRIDES.write() else {
        return false;
    };
    let changed = *g != next;
    *g = next;
    changed
}

/// Polls the controller's overrides every 15 s and republishes the gauges when they change.
/// An unreachable controller keeps the last document; a 404/501 (an older controller) clears it.
pub async fn run(
    base_url: String,
    token: Option<String>,
    handle: wayhouse_core::RuntimeHandle,
    saved: std::path::PathBuf,
) {
    let url = format!("{base_url}/features/overrides");
    if restore(&saved) {
        tracing::info!(path = %saved.display(), "feature overrides restored");
        publish(&handle.snapshot().features, &handle);
    }
    let mut tick = tokio::time::interval(std::time::Duration::from_secs(15));
    // Warn when the failure changes (a rotated token shows up once), not every 15 s.
    let mut failing: Option<String> = None;
    let mut note = |problem: Option<String>| {
        if problem != failing {
            match &problem {
                Some(p) => {
                    tracing::warn!(problem = %p, "feature overrides not read; keeping the last ones")
                }
                None => tracing::info!("feature overrides are being read again"),
            }
            failing = problem;
        }
    };
    loop {
        tick.tick().await;
        // An override that ran out puts its flag back even if the controller says nothing new.
        publish(&handle.snapshot().features, &handle);
        let mut req = wayhouse_http::client().get(&url);
        if let Some(t) = &token {
            req = req.bearer_auth(t);
        }
        // An old controller (or an ingress hiccup) answering 404/501 clears the live set but
        // must not wipe the saved copy a later restart relies on.
        let mut cleared = false;
        let next = match req.send().await {
            Ok(r) if r.status().is_success() => {
                match r.bytes().await.ok().and_then(|b| parse_overrides(&b)) {
                    Some(n) => n,
                    None => continue,
                }
            }
            Ok(r) if matches!(r.status().as_u16(), 404 | 501) => {
                cleared = true;
                BTreeMap::new()
            }
            Ok(r) => {
                note(Some(format!("controller answered {}", r.status())));
                continue;
            }
            Err(e) => {
                note(Some(wayhouse_http::error_chain(&e)));
                continue;
            }
        };
        note(None);
        if set_overrides(next.clone()) {
            if !cleared {
                save(&saved, &next);
            }
            tracing::info!("feature overrides changed");
            publish(&handle.snapshot().features, &handle);
        }
    }
}

/// The `port_routes` value the plugin overlay was last merged with.
static PORT_ROUTES_APPLIED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(true);

/// Hands the effective values to the data path (`wayhouse_core::flags`). Returns whether
/// `port_routes` differs from what the last config build merged, which asks for a rebuild.
fn apply(config: &BTreeMap<String, bool>, applied: &std::sync::atomic::AtomicBool) -> bool {
    use std::sync::atomic::Ordering;
    let rows = effective(config);
    let value = |name: &str| rows.iter().find(|r| r.name == name).is_none_or(|r| r.value);
    wayhouse_core::flags::set_all(value);
    let ports = value("port_routes");
    applied.swap(ports, Ordering::SeqCst) != ports
}

/// Applies `config` and the live overrides to the data path, then sets
/// `wayhouse_feature_enabled` and `wayhouse_feature_source`. A change of `port_routes` asks
/// for a rebuild, since the plugin entry listeners are part of the built configuration.
pub fn publish(config: &BTreeMap<String, bool>, handle: &wayhouse_core::RuntimeHandle) {
    publish_with(config, &PORT_ROUTES_APPLIED, handle);
}

fn publish_with(
    config: &BTreeMap<String, bool>,
    applied: &std::sync::atomic::AtomicBool,
    handle: &wayhouse_core::RuntimeHandle,
) {
    if apply(config, applied) {
        tracing::info!("port_routes changed; rebuilding the configuration");
        handle.request_reload();
    }
    for row in effective(config) {
        metrics::gauge!(m::FEATURE_ENABLED, "flag" => row.name).set(f64::from(u8::from(row.value)));
        for source in Source::ALL {
            metrics::gauge!(m::FEATURE_SOURCE, "flag" => row.name, "source" => source.as_str())
                .set(f64::from(u8::from(source == row.source)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `OVERRIDES` is process-global; tests that touch it take turns.
    static TURN: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn one_bad_item_does_not_drop_the_rest_of_the_document() {
        let body = br#"{"items":[{"name":"acl","value":false},{"value":true},{"name":"geo"},{"name":"sniffers","value":false,"expires_at":99}]}"#;
        let got = parse_overrides(body).unwrap();
        assert_eq!(
            got.keys().map(String::as_str).collect::<Vec<_>>(),
            ["acl", "sniffers"]
        );
    }

    #[test]
    fn overrides_survive_a_restart_with_the_controller_down() {
        let _t = TURN
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("overrides.json");
        assert!(!restore(&file), "no file, nothing to restore");
        let soon = unix_now() + 600;
        let live = BTreeMap::from([("acl".to_string(), (false, Some(soon)))]);
        save(&file, &live);
        set_overrides(BTreeMap::new());
        assert!(restore(&file));
        assert_eq!(overrides().get("acl"), Some(&false));
        // An override that ran out while the proxy was down is not revived.
        save(
            &file,
            &BTreeMap::from([("acl".to_string(), (false, Some(1)))]),
        );
        set_overrides(BTreeMap::new());
        restore(&file);
        assert!(overrides().is_empty());
        std::fs::write(&file, b"garbage").unwrap();
        assert!(!restore(&file), "an unreadable file is ignored");
        set_overrides(BTreeMap::new());
    }

    #[test]
    fn the_controllers_overrides_are_read_and_an_expired_one_is_ignored() {
        let _t = TURN
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let body = br#"{"items":[{"name":"sniffers","value":false,"reason":"r","set_at":1},{"name":"acl","value":false,"expires_at":2}]}"#;
        let got = parse_overrides(body).unwrap();
        assert_eq!(got["sniffers"], (false, None));
        assert!(parse_overrides(b"nope").is_none());
        assert!(set_overrides(got));
        let live = overrides();
        assert_eq!(live.get("sniffers"), Some(&false));
        assert!(!live.contains_key("acl"), "past its expiry");
        let same = OVERRIDES.read().unwrap().clone();
        assert!(!set_overrides(same));
        set_overrides(BTreeMap::new());
    }

    // The turn is held across awaits on purpose: `OVERRIDES` must stay empty until the end.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn a_port_routes_change_asks_for_a_rebuild_and_nothing_else_does() {
        let _t = TURN
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        set_overrides(BTreeMap::new());
        let cfg = wayhouse_config::parse_str(
            "pools:\n  - name: p\n    targets: [\"127.0.0.1:1\"]\nlisteners:\n  - name: l\n    bind: \"127.0.0.1:0\"\n    pool: p\n",
        )
        .unwrap();
        let runtime = wayhouse_core::Runtime::start(
            wayhouse_core::Snapshot::from_config(&cfg),
            std::sync::Arc::default(),
            1,
        );
        let handle = runtime.handle();
        let asked = || async {
            tokio::time::timeout(
                std::time::Duration::from_millis(50),
                handle.reload_requested().notified(),
            )
            .await
            .is_ok()
        };
        // Only this test's own marker is read; the shared mask is written but not asserted on.
        let applied = std::sync::atomic::AtomicBool::new(true);
        let off = BTreeMap::from([("port_routes".to_string(), false)]);
        publish_with(&BTreeMap::new(), &applied, &handle);
        assert!(!asked().await, "defaults change nothing");
        publish_with(&off, &applied, &handle);
        assert!(asked().await, "port_routes went off");
        publish_with(&off, &applied, &handle);
        assert!(!asked().await, "the same document changes nothing");
        publish_with(&BTreeMap::new(), &applied, &handle);
        assert!(asked().await, "and back on");
        runtime
            .shutdown_with_grace(std::time::Duration::from_millis(100))
            .await;
    }

    #[test]
    fn an_expired_override_no_longer_holds_a_flag_off() {
        let _t = TURN
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let value = |name: &str| {
            effective(&BTreeMap::new())
                .into_iter()
                .find(|r| r.name == name)
                .unwrap()
                .value
        };
        set_overrides(BTreeMap::from([(
            "geo".to_string(),
            (false, Some(unix_now() + 600)),
        )]));
        assert!(!value("geo"));
        set_overrides(BTreeMap::from([("geo".to_string(), (false, Some(1)))]));
        assert!(value("geo"), "past its expiry");
        set_overrides(BTreeMap::new());
    }

    #[test]
    fn a_flag_absent_from_the_config_takes_its_default_and_a_set_one_its_value() {
        let rows = effective(&BTreeMap::from([(
            "admin_write.backends".to_string(),
            false,
        )]));
        let row = |n: &str| rows.iter().find(|r| r.name == n).unwrap();
        assert_eq!(
            (
                row("admin_write.backends").value,
                row("admin_write.backends").source
            ),
            (false, Source::Config)
        );
        assert_eq!(
            (row("port_routes").value, row("port_routes").source),
            (true, Source::Default)
        );
    }
}
