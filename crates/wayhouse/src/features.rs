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
    view(Scope::Proxy, config, available)
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
