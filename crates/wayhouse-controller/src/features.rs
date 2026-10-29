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
pub fn effective() -> Vec<FeatureView> {
    view(Scope::Controller, &BTreeMap::new(), available)
}

/// Sets `wayhouse_feature_enabled` and `wayhouse_feature_source`.
pub fn publish() {
    for row in effective() {
        metrics::gauge!("wayhouse_feature_enabled", "flag" => row.name)
            .set(f64::from(u8::from(row.value)));
        for source in Source::ALL {
            metrics::gauge!("wayhouse_feature_source", "flag" => row.name, "source" => source.as_str())
                .set(f64::from(u8::from(source == row.source)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_controller_flag_starts_on_from_its_default() {
        let rows = effective();
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
