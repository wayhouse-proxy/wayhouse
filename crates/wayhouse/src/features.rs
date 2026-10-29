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

/// Reads a `GET /features/overrides` body; `None` for anything that is not that document.
fn parse_overrides(body: &[u8]) -> Option<BTreeMap<String, (bool, Option<u64>)>> {
    let doc: serde_json::Value = serde_json::from_slice(body).ok()?;
    doc["items"]
        .as_array()?
        .iter()
        .map(|i| {
            Some((
                i["name"].as_str()?.to_string(),
                (i["value"].as_bool()?, i["expires_at"].as_u64()),
            ))
        })
        .collect()
}

fn set_overrides(next: BTreeMap<String, (bool, Option<u64>)>) -> bool {
    let Ok(mut g) = OVERRIDES.write() else {
        return false;
    };
    let changed = *g != next;
    *g = next;
    changed
}

/// Polls the controller's overrides every 15 s and republishes the gauges when they change.
/// An unreachable controller keeps the last document; a 404/501 (an older controller) clears it.
pub async fn run(base_url: String, token: Option<String>, handle: wayhouse_core::RuntimeHandle) {
    let url = format!("{base_url}/features/overrides");
    let mut tick = tokio::time::interval(std::time::Duration::from_secs(15));
    loop {
        tick.tick().await;
        let mut req = wayhouse_http::client().get(&url);
        if let Some(t) = &token {
            req = req.bearer_auth(t);
        }
        let next = match req.send().await {
            Ok(r) if r.status().is_success() => match r.bytes().await.ok().and_then(|b| parse_overrides(&b)) {
                Some(n) => n,
                None => continue,
            },
            Ok(r) if matches!(r.status().as_u16(), 404 | 501) => BTreeMap::new(),
            Ok(r) => {
                tracing::debug!(status = %r.status(), "feature overrides not read");
                continue;
            }
            Err(e) => {
                tracing::debug!(error = %wayhouse_http::error_chain(&e), "feature overrides not read");
                continue;
            }
        };
        if set_overrides(next) {
            tracing::info!("feature overrides changed");
            publish(&handle.snapshot().features);
        }
    }
}

/// Sets `wayhouse_feature_enabled` and `wayhouse_feature_source` from `config`.
pub fn publish(config: &BTreeMap<String, bool>) {
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

    #[test]
    fn the_controllers_overrides_are_read_and_an_expired_one_is_ignored() {
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
