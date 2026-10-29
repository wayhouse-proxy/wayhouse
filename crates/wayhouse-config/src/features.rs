//! The feature-flag registry and the `features:` section (config schema 4).
//!
//! Design: `design-feature-flags-0.4.0.md`. A flag is a named on/off switch for one
//! operator-visible capability. Its value comes from, in order of precedence: a controller
//! override (F4), the `features:` section of the config, the registry default. A flag the
//! serving binary was built without reports `available: false` and source `build`.
//!
//! Unknown names are an error everywhere (a kill switch an older node silently ignores is
//! worse than a rejected config), and a flag that is new after schema 4 adds a
//! `features.<name>` entry to [`crate::version::FIELD_SINCE`].
//!
//! `wired` says whether the runtime consults the flag yet. The registry lands before the code
//! that reads each flag (F3), so an unwired flag is accepted and reported but changes nothing;
//! the UI hides it.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::ConfigError;

/// Which process reads the flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Proxy,
    Controller,
}

/// When a changed value takes effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Apply {
    Live,
    Startup,
}

/// Which way an override may flip a flag. Flags that open new network exposure are
/// `OffOnly`, so a stolen token cannot switch them on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OverrideMode {
    Both,
    OffOnly,
}

/// One registry entry.
#[derive(Debug, Clone, Copy)]
pub struct Flag {
    pub name: &'static str,
    pub default: bool,
    pub scope: Scope,
    pub apply: Apply,
    pub override_mode: OverrideMode,
    /// Turning it off removes protection (rate limits, ACLs): an override needs an expiry.
    pub protection: bool,
    /// Whether the runtime consults it yet (see the module doc).
    pub wired: bool,
    /// The config schema version that introduced it.
    pub since: u32,
    /// What turning it off costs, in one sentence.
    pub risk: &'static str,
    /// What happens to connections already open.
    pub impact: &'static str,
}

const fn flag(
    name: &'static str,
    scope: Scope,
    override_mode: OverrideMode,
    risk: &'static str,
    impact: &'static str,
) -> Flag {
    Flag {
        name,
        default: true,
        scope,
        apply: Apply::Live,
        override_mode,
        protection: false,
        wired: false,
        since: 4,
        risk,
        impact,
    }
}

const fn protection(mut f: Flag) -> Flag {
    f.protection = true;
    f
}

use OverrideMode::{Both, OffOnly};
use Scope::{Controller, Proxy};

/// Every flag, in the order the UI lists them.
pub const REGISTRY: &[Flag] = &[
    flag(
        "gossip_verdicts",
        Proxy,
        Both,
        "Backends are judged by this proxy's own health checks only; regional correlation is lost.",
        "The mesh keeps running; established connections are untouched.",
    ),
    flag(
        "sniffers",
        Proxy,
        Both,
        "Routes with `sniffer:` never match, so players can land on the default or another pool.",
        "Connections already routed are untouched; sniffing only reads the first bytes.",
    ),
    flag(
        "port_routes",
        Proxy,
        OffOnly,
        "Plugin entry listeners are removed; customers lose those ports (they stay reserved).",
        "Established TCP sessions continue; UDP sessions drain.",
    ),
    flag(
        "admin_write.backends",
        Proxy,
        Both,
        "Adding, patching and removing backends through the admin API answers 403.",
        "No connection is touched; reads, drain and the flag endpoints stay open.",
    ),
    flag(
        "admin_write.sniffer_upload",
        Proxy,
        Both,
        "Uploading, deleting and rolling back sniffer modules answers 403.",
        "No connection is touched; reads, drain and the flag endpoints stay open.",
    ),
    flag(
        "admin_write.route_hint",
        Proxy,
        Both,
        "Route hints answer 403.",
        "No connection is touched; reads, drain and the flag endpoints stay open.",
    ),
    protection(flag(
        "rate_limit",
        Proxy,
        Both,
        "New-connection rate limits are bypassed: less protection against floods.",
        "New connections only.",
    )),
    protection(flag(
        "per_source",
        Proxy,
        Both,
        "Per-source connection limits are bypassed: less protection against one noisy client.",
        "New connections only.",
    )),
    protection(flag(
        "acl",
        Proxy,
        Both,
        "Client address allow and deny lists are bypassed.",
        "New connections only.",
    )),
    protection(flag(
        "geo",
        Proxy,
        Both,
        "Country allow and deny lists are bypassed.",
        "New connections only.",
    )),
    flag(
        "plugin_webhooks",
        Controller,
        OffOnly,
        "Every plugin webhook call answers 503; the listener stays up.",
        "No plugin tick is affected.",
    ),
    flag(
        "registry_install",
        Controller,
        OffOnly,
        "Installing or updating from a registry is refused.",
        "Installed plugins and sniffers keep running.",
    ),
    flag(
        "acme_issuance",
        Controller,
        Both,
        "No new orders or renewals; served certificates keep working until they expire.",
        "No connection is touched.",
    ),
    flag(
        "tunnel_registration",
        Controller,
        Both,
        "New origin and proxy registrations are refused.",
        "Existing leases and tunnels are untouched.",
    ),
];

/// The registry entry named `name`.
#[must_use]
pub fn lookup(name: &str) -> Option<&'static Flag> {
    REGISTRY.iter().find(|f| f.name == name)
}

/// Whether an operator may override `name` to `value`: the flag exists, an `off_only` flag is
/// only turned off, and switching a protection flag off carries an expiry.
pub fn check_override(name: &str, value: bool, expires_at: Option<u64>) -> Result<(), String> {
    let f = lookup(name).ok_or_else(|| format!("unknown flag {name:?}"))?;
    if value && f.override_mode == OverrideMode::OffOnly {
        return Err(format!("{name} can only be turned off by an override"));
    }
    if !value && f.protection && expires_at.is_none() {
        return Err(format!(
            "turning {name} off removes protection; set expires_at so it comes back on"
        ));
    }
    Ok(())
}

/// Checks a `features:` section: every name is a known proxy flag. Controller flags are
/// refused here because the proxy does not read them; they are set by an override.
pub(crate) fn validate(raw: BTreeMap<String, bool>) -> Result<BTreeMap<String, bool>, ConfigError> {
    for name in raw.keys() {
        match lookup(name) {
            None => {
                let known: Vec<_> = REGISTRY
                    .iter()
                    .filter(|f| f.scope == Scope::Proxy)
                    .map(|f| f.name)
                    .collect();
                return Err(ConfigError::Invalid(format!(
                    "features.{name}: unknown flag (known: {})",
                    known.join(", ")
                )));
            }
            Some(f) if f.scope != Scope::Proxy => {
                return Err(ConfigError::Invalid(format!(
                    "features.{name}: a controller flag; set it with an override on the controller, not in the proxy config"
                )));
            }
            Some(_) => {}
        }
    }
    Ok(raw)
}

/// Where a flag's effective value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Default,
    Config,
    Override,
    /// The binary was built without the code the flag switches.
    Build,
}

impl Source {
    pub const ALL: [Source; 4] = [
        Source::Default,
        Source::Config,
        Source::Override,
        Source::Build,
    ];

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Default => "default",
            Source::Config => "config",
            Source::Override => "override",
            Source::Build => "build",
        }
    }
}

/// One row of `GET /features`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FeatureView {
    pub name: &'static str,
    pub value: bool,
    pub source: Source,
    pub default: bool,
    pub scope: Scope,
    pub apply: Apply,
    #[serde(rename = "override")]
    pub override_mode: OverrideMode,
    pub protection: bool,
    pub wired: bool,
    pub available: bool,
    pub since: u32,
    pub risk: &'static str,
    pub impact: &'static str,
}

/// The effective state of every flag of `scope`. `config` is the `features:` section;
/// `overrides` holds the active (unexpired) overrides by name, which beat the config;
/// `available` says whether this build contains the code a flag switches.
#[must_use]
pub fn view(
    scope: Scope,
    config: &BTreeMap<String, bool>,
    overrides: &BTreeMap<String, bool>,
    available: impl Fn(&str) -> bool,
) -> Vec<FeatureView> {
    REGISTRY
        .iter()
        .filter(|f| f.scope == scope)
        .map(|f| {
            let (value, source) = if !available(f.name) {
                (false, Source::Build)
            } else if let Some(v) = overrides
                .get(f.name)
                .filter(|v| **v != f.default || f.override_mode == OverrideMode::Both)
            {
                (*v, Source::Override)
            } else if let Some(v) = config.get(f.name) {
                (*v, Source::Config)
            } else {
                (f.default, Source::Default)
            };
            FeatureView {
                name: f.name,
                value,
                source,
                default: f.default,
                scope: f.scope,
                apply: f.apply,
                override_mode: f.override_mode,
                protection: f.protection,
                wired: f.wired,
                available: available(f.name),
                since: f.since,
                risk: f.risk,
                impact: f.impact,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_str as parse;

    const BODY: &str = "pools:\n  - name: mc\n    targets: [\"127.0.0.1:25566\"]\nlisteners:\n  - name: main\n    bind: \"0.0.0.0:25565\"\n    protocol: tcp\n    pool: mc\n";

    fn parse_str(head: &str) -> Result<crate::Config, crate::ConfigError> {
        parse(&format!("{head}{BODY}"))
    }

    #[test]
    fn names_are_unique_and_plain() {
        let mut seen = std::collections::BTreeSet::new();
        for f in REGISTRY {
            assert!(seen.insert(f.name), "{} twice", f.name);
            assert!(
                f.name
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b == b'_' || b == b'.'),
                "{}",
                f.name
            );
            assert!(!f.risk.is_empty() && !f.impact.is_empty(), "{}", f.name);
        }
    }

    #[test]
    fn a_flag_that_opens_exposure_can_only_be_overridden_off() {
        for name in ["port_routes", "plugin_webhooks", "registry_install"] {
            let f = lookup(name).unwrap();
            assert_eq!(f.override_mode, OverrideMode::OffOnly, "{name}");
            assert!(f.default, "{name}: off_only means default on");
        }
    }

    #[test]
    fn the_features_section_takes_known_proxy_flags() {
        let cfg = parse_str("schema_version: 4\nfeatures:\n  gossip_verdicts: false\n  admin_write.backends: true\n").unwrap();
        assert!(!cfg.features["gossip_verdicts"]);
        assert_eq!(cfg.features.len(), 2);
        assert!(parse_str("").unwrap().features.is_empty());
    }

    #[test]
    fn an_unknown_flag_is_rejected_and_the_known_ones_are_listed() {
        let e = parse_str("schema_version: 4\nfeatures:\n  gossip_verdict: false\n")
            .unwrap_err()
            .to_string();
        assert!(e.contains("features.gossip_verdict: unknown flag"), "{e}");
        assert!(e.contains("gossip_verdicts"), "{e}");
    }

    #[test]
    fn a_controller_flag_is_not_a_proxy_config_key() {
        let e = parse_str("schema_version: 4\nfeatures:\n  acme_issuance: false\n")
            .unwrap_err()
            .to_string();
        assert!(e.contains("a controller flag"), "{e}");
    }

    #[test]
    fn a_flag_value_must_be_a_boolean() {
        assert!(parse_str("schema_version: 4\nfeatures:\n  sniffers: maybe\n").is_err());
    }

    #[test]
    fn the_section_needs_schema_4() {
        let e = parse_str("features:\n  sniffers: false\n")
            .unwrap_err()
            .to_string();
        assert!(e.contains("`features` needs schema_version 4"), "{e}");
        assert!(parse_str("schema_version: 3\nfeatures: {}\n").is_err());
    }

    #[test]
    fn the_view_reports_source_and_build_availability() {
        let config = BTreeMap::from([("sniffers".to_string(), false)]);
        let rows = view(Scope::Proxy, &config, &BTreeMap::new(), |n| n != "gossip_verdicts");
        let row = |n: &str| rows.iter().find(|r| r.name == n).unwrap();
        assert_eq!(
            (row("sniffers").value, row("sniffers").source),
            (false, Source::Config)
        );
        assert_eq!(
            (row("port_routes").value, row("port_routes").source),
            (true, Source::Default)
        );
        let gossip = row("gossip_verdicts");
        assert_eq!(
            (gossip.value, gossip.source, gossip.available),
            (false, Source::Build, false)
        );
        assert!(rows.iter().all(|r| r.scope == Scope::Proxy));
        let ctl = view(Scope::Controller, &BTreeMap::new(), &BTreeMap::new(), |_| true);
        assert!(ctl.iter().all(|r| r.scope == Scope::Controller) && ctl.len() == 4);
    }

    #[test]
    fn an_override_is_checked_against_the_registry() {
        let some_protection = REGISTRY.iter().find(|f| f.protection).unwrap().name;
        let off_only = REGISTRY
            .iter()
            .find(|f| f.override_mode == OverrideMode::OffOnly && !f.protection)
            .unwrap()
            .name;
        assert!(check_override("nope", false, None).unwrap_err().contains("unknown flag"));
        assert!(check_override(off_only, true, None).unwrap_err().contains("only be turned off"));
        assert!(check_override(off_only, false, None).is_ok());
        assert!(check_override(some_protection, false, None).unwrap_err().contains("expires_at"));
        assert!(check_override(some_protection, false, Some(10)).is_ok());
    }

    #[test]
    fn an_override_beats_the_config_and_an_off_only_flag_ignores_an_on_override() {
        let config = BTreeMap::from([("gossip_verdicts".to_string(), false)]);
        let overrides = BTreeMap::from([
            ("gossip_verdicts".to_string(), true),
            ("port_routes".to_string(), false),
            ("sniffers".to_string(), false),
        ]);
        let rows = view(Scope::Proxy, &config, &overrides, |n| n != "sniffers");
        let row = |n: &str| rows.iter().find(|r| r.name == n).unwrap();
        let got = |n: &str| (row(n).value, row(n).source);
        assert_eq!(got("gossip_verdicts"), (true, Source::Override));
        assert_eq!(got("port_routes"), (false, Source::Override));
        // Compiled out beats everything.
        assert_eq!(got("sniffers"), (false, Source::Build));
        // `off_only`: an override that turns it on is not obeyed (the controller refuses to
        // store one; this is the second line of defence).
        let on = BTreeMap::from([("port_routes".to_string(), true)]);
        let rows = view(Scope::Proxy, &BTreeMap::new(), &on, |_| true);
        let row = rows.iter().find(|r| r.name == "port_routes").unwrap();
        assert_eq!((row.value, row.source), (true, Source::Default));
    }

    #[test]
    fn the_view_serialises_the_shape_the_ui_reads() {
        let rows = view(Scope::Controller, &BTreeMap::new(), &BTreeMap::new(), |_| true);
        let v = serde_json::to_value(&rows[0]).unwrap();
        for key in [
            "name",
            "value",
            "source",
            "default",
            "scope",
            "apply",
            "override",
            "protection",
            "wired",
            "available",
            "since",
            "risk",
            "impact",
        ] {
            assert!(v.get(key).is_some(), "{key}");
        }
        assert_eq!(v["override"], "off_only");
        assert_eq!(v["apply"], "live");
    }
}
