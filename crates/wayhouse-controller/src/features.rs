//! The controller's view of the feature-flag registry (`wayhouse_config::features`): the
//! flags it reads, their effective values and the metrics that expose them. Values are the
//! registry defaults until the override layer (F4) lands; the proxy-scoped flags are served
//! by each proxy.

use std::collections::BTreeMap;

use wayhouse_config::features::{view, FeatureView, Scope, Source};

/// Whether this binary contains the code behind `flag`.
fn available(flag: &str) -> bool {
    flag != "acme_issuance" || cfg!(feature = "acme")
}

/// Every controller flag with its effective value.
pub fn effective(overrides: &BTreeMap<String, bool>) -> Vec<FeatureView> {
    view(Scope::Controller, &BTreeMap::new(), overrides, available)
}

/// Every proxy flag with the default or override that applies fleet-wide. The `features:`
/// section is per config revision and per node, so it is not folded in here.
pub fn fleet_view(overrides: &BTreeMap<String, bool>) -> Vec<FeatureView> {
    view(Scope::Proxy, &BTreeMap::new(), overrides, |_| true)
}

/// Sets `wayhouse_feature_enabled` and `wayhouse_feature_source`.
pub fn publish(overrides: &BTreeMap<String, bool>) {
    for row in effective(overrides) {
        metrics::gauge!("wayhouse_feature_enabled", "flag" => row.name)
            .set(f64::from(u8::from(row.value)));
        for source in Source::ALL {
            metrics::gauge!("wayhouse_feature_source", "flag" => row.name, "source" => source.as_str())
                .set(f64::from(u8::from(source == row.source)));
        }
    }
}

/// The overrides in force at `now` (unix seconds): stored ones that have not expired.
pub fn active(
    store: &crate::ui_data::UiStore,
    now: u64,
) -> Result<BTreeMap<String, bool>, crate::ui_data::UiStoreError> {
    Ok(store
        .list(crate::ui_data::Kind::FeatureOverride)?
        .into_iter()
        .filter_map(|(name, v)| {
            let live = v["expires_at"].as_u64().is_none_or(|e| e > now);
            live.then_some((name, v["value"].as_bool()?))
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_controller_flag_starts_on_from_its_default() {
        let rows = effective(&BTreeMap::new());
        assert_eq!(rows.len(), 4);
        for r in &rows {
            // A flag whose code is compiled out reports itself off, from the build.
            let (value, source) = if r.available {
                (true, Source::Default)
            } else {
                (false, Source::Build)
            };
            assert_eq!((r.value, r.source), (value, source), "{}", r.name);
        }
    }
}
