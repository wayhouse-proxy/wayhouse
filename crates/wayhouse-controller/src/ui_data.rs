//! Records the web UI keeps in the controller (#483): named dashboards, custom themes and saved
//! fleet filters, the single form-layout override and the deployment defaults. One replicated
//! key-value area: a standalone controller applies ops through [`UiStore::apply_local`], the HA
//! state machine through [`UiStore::apply_at`] (crash-idempotent per Raft index, like ACME).
//!
//! The controller stores JSON and checks its shape, size and names; what a widget id means is the
//! UI's business, so an unknown widget id is accepted (the UI shows a placeholder).

pub mod api;
pub mod contrast;

use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::kv::transaction::ConflictableTransactionError;
use crate::kv::Transactional;

/// Which record collection an op touches. `FormLayout` and `Defaults` are single records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    Dashboard,
    Theme,
    SavedFilter,
    FormLayout,
    Defaults,
    /// An operator's override of one feature flag (F4), named by the flag.
    FeatureOverride,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum UiOp {
    Put {
        kind: Kind,
        name: String,
        value: Value,
    },
    Delete {
        kind: Kind,
        name: String,
    },
}

/// What applying one op did; everything but `Done` and `Replayed` changed nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Applied {
    Replayed,
    Done,
    NotFound,
    /// Malformed or over a limit; the text names the path.
    Invalid(String),
    /// The collection already holds its maximum number of records.
    Full(usize),
}

/// Everything replicated, for an HA snapshot.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UiSnapshot {
    /// `(kind, name, value)`.
    pub records: Vec<(Kind, String, Value)>,
    pub applied_index: Option<u64>,
}

#[derive(Debug, thiserror::Error)]
pub enum UiStoreError {
    #[error("ui store: {0}")]
    Kv(#[from] crate::kv::Error),
    #[error("ui store record is corrupt: {0}")]
    Corrupt(#[from] serde_json::Error),
}

#[derive(Clone)]
pub struct UiStore {
    records: crate::kv::Tree,
    meta: crate::kv::Tree,
    write: Arc<Mutex<()>>,
}

/// The rolling-upgrade gate under HA: every member must decode [`UiOp`] entries. A member that
/// does not answer blocks too, like a plugin install.
pub async fn check_support(ha: &crate::ha::HaHandle) -> Result<(), String> {
    check_support_answers(&crate::plugins::peer::whoami_all(ha).await)
}

pub fn check_support_answers(
    answers: &[(
        crate::ha::NodeId,
        Result<crate::ha::routes::Whoami, crate::ha::members::WhoamiError>,
    )],
) -> Result<(), String> {
    for (id, answer) in answers {
        match answer {
            Err(why) => {
                return Err(format!(
                    "node {id} did not answer ({why}); UI data changes wait until every member reports its build"
                ))
            }
            Ok(who) if who.ui_support < crate::ha::routes::UI_SUPPORT => {
                return Err(format!(
                    "node {id} runs a build without UI data support; upgrade every replica before saving dashboards or themes"
                ))
            }
            Ok(_) => {}
        }
    }
    Ok(())
}

const THEME_TOKENS: [&str; 23] = [
    "bg",
    "surface",
    "surface-sunken",
    "ink",
    "ink-muted",
    "line",
    "control-border",
    "rail",
    "rail-ink",
    "rail-ink-muted",
    "rail-hover",
    "select",
    "select-bg",
    "primary",
    "primary-ink",
    "series-1",
    "series-2",
    "series-3",
    "series-4",
    "series-5",
    "good",
    "warn",
    "bad",
];
/// Theme names the UI reserves for its built-ins (also valid as a default theme).
const BUILTIN_THEMES: [&str; 3] = ["light", "dark", "system"];
const MAX_NAME: usize = 64;
const MAX_WIDGETS: usize = 100;

impl Kind {
    fn tag(self) -> &'static str {
        match self {
            Kind::Dashboard => "dashboards",
            Kind::Theme => "themes",
            Kind::SavedFilter => "filters",
            Kind::FormLayout => "layout",
            Kind::Defaults => "defaults",
            Kind::FeatureOverride => "feature_overrides",
        }
    }

    fn max_bytes(self) -> usize {
        match self {
            Kind::Dashboard | Kind::FormLayout => 64 << 10,
            Kind::Theme => 8 << 10,
            Kind::SavedFilter => 4 << 10,
            Kind::Defaults | Kind::FeatureOverride => 1 << 10,
        }
    }

    /// Most named records of this kind; `None` for the single records.
    fn max_records(self) -> Option<usize> {
        match self {
            Kind::Dashboard => Some(100),
            Kind::Theme => Some(50),
            Kind::SavedFilter => Some(200),
            Kind::FeatureOverride => Some(64),
            Kind::FormLayout | Kind::Defaults => None,
        }
    }

    fn is_single(self) -> bool {
        self.max_records().is_none()
    }
}

fn key(kind: Kind, name: &str) -> Vec<u8> {
    format!("{}/{name}", kind.tag()).into_bytes()
}

fn check_name(kind: Kind, name: &str) -> Result<(), String> {
    if kind.is_single() {
        return Ok(());
    }
    if kind == Kind::FeatureOverride && !name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_') {
        return Err("name must be a flag name (a-z, 0-9 and _)".into());
    }
    let n = name.chars().count();
    if n == 0 || n > MAX_NAME || name.chars().any(|c| c.is_control() || c == '/') {
        return Err(format!(
            "name must be 1 to {MAX_NAME} characters without '/' or control characters"
        ));
    }
    Ok(())
}

fn object<'a>(v: &'a Value, path: &str) -> Result<&'a serde_json::Map<String, Value>, String> {
    v.as_object()
        .ok_or_else(|| format!("{path} must be an object"))
}

fn only_keys(
    m: &serde_json::Map<String, Value>,
    path: &str,
    allowed: &[&str],
) -> Result<(), String> {
    match m.keys().find(|k| !allowed.contains(&k.as_str())) {
        Some(k) => Err(format!("{path}{k} is not a known key")),
        None => Ok(()),
    }
}

fn string_max<'a>(v: &'a Value, path: &str, max: usize) -> Result<&'a str, String> {
    match v.as_str() {
        Some(s) if !s.is_empty() && s.chars().count() <= max => Ok(s),
        _ => Err(format!("{path} must be a string of 1 to {max} characters")),
    }
}

/// Shape, size and name checks for one record. Deterministic: it runs on the proposing node and
/// again when each replica applies the entry.
pub fn validate(kind: Kind, name: &str, value: &Value) -> Result<(), String> {
    check_name(kind, name)?;
    let size = serde_json::to_vec(value).map_or(usize::MAX, |b| b.len());
    if size > kind.max_bytes() {
        return Err(format!("the record is over {} KiB", kind.max_bytes() >> 10));
    }
    let obj = object(value, "the record")?;
    match kind {
        Kind::SavedFilter => Ok(()),
        Kind::Dashboard => {
            only_keys(obj, "", &["name", "version", "widgets"])?;
            let widgets = obj
                .get("widgets")
                .and_then(Value::as_array)
                .ok_or("widgets must be an array")?;
            if widgets.len() > MAX_WIDGETS {
                return Err(format!("widgets can hold at most {MAX_WIDGETS} entries"));
            }
            for (i, w) in widgets.iter().enumerate() {
                let at = format!("widgets[{i}]");
                let w = object(w, &at)?;
                only_keys(w, &format!("{at}."), &["id", "width", "settings"])?;
                string_max(
                    w.get("id").unwrap_or(&Value::Null),
                    &format!("{at}.id"),
                    MAX_NAME,
                )?;
                if !w
                    .get("width")
                    .and_then(Value::as_u64)
                    .is_some_and(|n| (1..=12).contains(&n))
                {
                    return Err(format!("{at}.width must be an integer from 1 to 12"));
                }
                if w.get("settings").is_some_and(|s| !s.is_object()) {
                    return Err(format!("{at}.settings must be an object"));
                }
            }
            Ok(())
        }
        Kind::Theme => {
            only_keys(obj, "", &["name", "base", "tokens"])?;
            if !matches!(
                obj.get("base").and_then(Value::as_str),
                Some("light" | "dark")
            ) {
                return Err("base must be \"light\" or \"dark\"".into());
            }
            if BUILTIN_THEMES.contains(&name.to_lowercase().as_str()) {
                return Err(format!("{name:?} is a built-in theme name"));
            }
            let tokens = object(obj.get("tokens").unwrap_or(&Value::Null), "tokens")?;
            for (t, v) in tokens {
                if !THEME_TOKENS.contains(&t.as_str()) {
                    return Err(format!("tokens.{t} is not a themable token"));
                }
                let hex = v.as_str().is_some_and(|s| {
                    s.len() == 7
                        && s.starts_with('#')
                        && s[1..].bytes().all(|b| b.is_ascii_hexdigit())
                });
                if !hex {
                    return Err(format!("tokens.{t} must be a #rrggbb colour"));
                }
            }
            Ok(())
        }
        Kind::FormLayout => {
            only_keys(obj, "", &["version", "fields"])?;
            if obj.get("version").and_then(Value::as_u64) != Some(1) {
                return Err("version must be 1".into());
            }
            for (path, f) in object(obj.get("fields").unwrap_or(&Value::Null), "fields")? {
                let at = format!("fields.{path}");
                let f = object(f, &at)?;
                only_keys(
                    f,
                    &format!("{at}."),
                    &["group", "order", "widget", "advanced", "page", "help"],
                )?;
                for k in ["group", "widget", "page"] {
                    if let Some(v) = f.get(k) {
                        string_max(v, &format!("{at}.{k}"), MAX_NAME)?;
                    }
                }
                if let Some(v) = f.get("help") {
                    string_max(v, &format!("{at}.help"), 1000)?;
                }
                if f.get("order").is_some_and(|v| !v.is_i64()) {
                    return Err(format!("{at}.order must be an integer"));
                }
                if f.get("advanced").is_some_and(|v| !v.is_boolean()) {
                    return Err(format!("{at}.advanced must be true or false"));
                }
            }
            Ok(())
        }
        Kind::FeatureOverride => {
            only_keys(obj, "", &["value", "reason", "actor", "set_at", "expires_at"])?;
            if !obj.get("value").is_some_and(Value::is_boolean) {
                return Err("value must be true or false".into());
            }
            string_max(obj.get("reason").unwrap_or(&Value::Null), "reason", 500)?;
            if let Some(a) = obj.get("actor") {
                string_max(a, "actor", 128)?;
            }
            let set_at = obj.get("set_at").and_then(Value::as_u64).ok_or("set_at must be a unix time")?;
            if let Some(e) = obj.get("expires_at") {
                if !e.as_u64().is_some_and(|e| e > set_at) {
                    return Err("expires_at must be a unix time after set_at".into());
                }
            }
            Ok(())
        }
        Kind::Defaults => {
            only_keys(obj, "", &["dashboard", "theme"])?;
            for k in ["dashboard", "theme"] {
                if let Some(v) = obj.get(k) {
                    string_max(v, k, MAX_NAME)?;
                }
            }
            Ok(())
        }
    }
}

/// What an op writes, decided before the transaction from state the write lock keeps stable.
enum Write {
    Insert(Vec<u8>, Vec<u8>),
    Remove(Vec<u8>),
}

const APPLIED_KEY: &[u8] = b"applied";

impl UiStore {
    pub fn open(db: &crate::kv::Db) -> Result<Self, crate::kv::Error> {
        Ok(Self {
            records: db.open_tree("ui_records")?,
            meta: db.open_tree("ui_meta")?,
            write: Arc::default(),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ()> {
        self.write
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn get(&self, kind: Kind, name: &str) -> Result<Option<Value>, UiStoreError> {
        self.records
            .get(key(kind, name))?
            .map(|b| serde_json::from_slice(&b))
            .transpose()
            .map_err(Into::into)
    }

    /// Named records of `kind`, by name.
    pub fn list(&self, kind: Kind) -> Result<Vec<(String, Value)>, UiStoreError> {
        let prefix = key(kind, "");
        self.records
            .scan_prefix(&prefix)
            .map(|kv| {
                let (k, v) = kv?;
                let name = String::from_utf8_lossy(&k[prefix.len()..]).into_owned();
                Ok((name, serde_json::from_slice(&v)?))
            })
            .collect()
    }

    pub fn applied_index(&self) -> Result<Option<u64>, UiStoreError> {
        Ok(self
            .meta
            .get(APPLIED_KEY)?
            .and_then(|v| <[u8; 8]>::try_from(v.as_ref()).ok())
            .map(u64::from_be_bytes))
    }

    /// Applies `op` on a standalone controller, at the next local index.
    pub fn apply_local(&self, op: &UiOp) -> Result<Applied, UiStoreError> {
        let guard = self.lock();
        let next = self.applied_index()?.map_or(1, |i| i + 1);
        self.apply_locked(&guard, next, op)
    }

    /// Applies `op` at Raft `index`. An entry at or below the cursor is a replay; a rejection
    /// advances the cursor too, since every replica gives the same answer.
    pub fn apply_at(&self, index: u64, op: &UiOp) -> Result<Applied, UiStoreError> {
        let guard = self.lock();
        self.apply_locked(&guard, index, op)
    }

    fn plan(&self, op: &UiOp) -> Result<Result<Vec<Write>, Applied>, UiStoreError> {
        match op {
            UiOp::Put { kind, name, value } => {
                if let Err(why) = validate(*kind, name, value) {
                    return Ok(Err(Applied::Invalid(why)));
                }
                let k = key(*kind, name);
                if let Some(max) = kind.max_records() {
                    if self.records.get(&k)?.is_none() && self.list(*kind)?.len() >= max {
                        return Ok(Err(Applied::Full(max)));
                    }
                }
                if *kind == Kind::Defaults {
                    let exists = |kind, n: &str| -> Result<bool, UiStoreError> {
                        Ok(self.records.get(key(kind, n))?.is_some())
                    };
                    if let Some(d) = value.get("dashboard").and_then(Value::as_str) {
                        if !exists(Kind::Dashboard, d)? {
                            return Ok(Err(Applied::Invalid(format!(
                                "dashboard: no dashboard named {d:?}"
                            ))));
                        }
                    }
                    if let Some(t) = value.get("theme").and_then(Value::as_str) {
                        if !BUILTIN_THEMES.contains(&t) && !exists(Kind::Theme, t)? {
                            return Ok(Err(Applied::Invalid(format!(
                                "theme: no theme named {t:?}"
                            ))));
                        }
                    }
                }
                Ok(Ok(vec![Write::Insert(k, serde_json::to_vec(value)?)]))
            }
            UiOp::Delete { kind, name } => {
                let k = key(*kind, name);
                if self.records.get(&k)?.is_none() {
                    return Ok(Err(Applied::NotFound));
                }
                let mut writes = vec![Write::Remove(k)];
                // A deleted dashboard or theme stops being the deployment default.
                let field = match kind {
                    Kind::Dashboard => Some("dashboard"),
                    Kind::Theme => Some("theme"),
                    _ => None,
                };
                if let (Some(field), Some(Value::Object(mut d))) =
                    (field, self.get(Kind::Defaults, "")?)
                {
                    if d.get(field).and_then(Value::as_str) == Some(name) {
                        d.remove(field);
                        writes.push(Write::Insert(
                            key(Kind::Defaults, ""),
                            serde_json::to_vec(&d)?,
                        ));
                    }
                }
                Ok(Ok(writes))
            }
        }
    }

    fn apply_locked(
        &self,
        _guard: &std::sync::MutexGuard<'_, ()>,
        index: u64,
        op: &UiOp,
    ) -> Result<Applied, UiStoreError> {
        let plan = self.plan(op)?;
        let outcome = (&self.records, &self.meta)
            .transaction(
                |(records, meta)| -> Result<Applied, ConflictableTransactionError<UiStoreError>> {
                    let covered = meta
                        .get(APPLIED_KEY)?
                        .and_then(|v| <[u8; 8]>::try_from(v.as_ref()).ok())
                        .map(u64::from_be_bytes);
                    if covered.is_some_and(|c| c >= index) {
                        return Ok(Applied::Replayed);
                    }
                    meta.insert(APPLIED_KEY, index.to_be_bytes())?;
                    match &plan {
                        Err(refused) => Ok(refused.clone()),
                        Ok(writes) => {
                            for w in writes {
                                match w {
                                    Write::Insert(k, v) => {
                                        records.insert(k.as_slice(), v.as_slice())?
                                    }
                                    Write::Remove(k) => records.remove(k.as_slice())?,
                                };
                            }
                            Ok(Applied::Done)
                        }
                    }
                },
            )
            .map_err(|e| match e {
                crate::kv::transaction::TransactionError::Abort(e) => e,
                crate::kv::transaction::TransactionError::Storage(e) => e.into(),
            })?;
        self.records.flush()?;
        self.meta.flush()?;
        Ok(outcome)
    }

    /// Everything, with the cursor, for an HA snapshot.
    pub fn snapshot(&self) -> Result<UiSnapshot, UiStoreError> {
        let _guard = self.lock();
        let mut records = Vec::new();
        for kind in [
            Kind::Dashboard,
            Kind::Theme,
            Kind::SavedFilter,
            Kind::FormLayout,
            Kind::Defaults,
        ] {
            for (name, v) in self.list(kind)? {
                records.push((kind, name, v));
            }
        }
        Ok(UiSnapshot {
            records,
            applied_index: self.applied_index()?,
        })
    }

    /// Makes the store exactly `snap`, in one transaction.
    pub fn replace(&self, snap: &UiSnapshot) -> Result<(), UiStoreError> {
        let _guard = self.lock();
        let old: Vec<crate::kv::IVec> = self.records.iter().keys().collect::<Result<_, _>>()?;
        let new = snap
            .records
            .iter()
            .map(|(kind, name, v)| Ok((key(*kind, name), serde_json::to_vec(v)?)))
            .collect::<Result<Vec<_>, serde_json::Error>>()?;
        (&self.records, &self.meta)
            .transaction(|(records, meta)| {
                for k in &old {
                    records.remove(k.clone())?;
                }
                for (k, v) in &new {
                    records.insert(k.as_slice(), v.as_slice())?;
                }
                match snap.applied_index {
                    Some(i) => meta.insert(APPLIED_KEY, i.to_be_bytes())?,
                    None => meta.remove(APPLIED_KEY)?,
                };
                Ok::<(), ConflictableTransactionError<UiStoreError>>(())
            })
            .map_err(|e| match e {
                crate::kv::transaction::TransactionError::Abort(e) => e,
                crate::kv::transaction::TransactionError::Storage(e) => e.into(),
            })?;
        self.records.flush()?;
        self.meta.flush()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn store() -> (UiStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::kv::Db::open(dir.path()).unwrap();
        (UiStore::open(&db).unwrap(), dir)
    }

    fn put(kind: Kind, name: &str, value: Value) -> UiOp {
        UiOp::Put {
            kind,
            name: name.into(),
            value,
        }
    }

    fn del(kind: Kind, name: &str) -> UiOp {
        UiOp::Delete {
            kind,
            name: name.into(),
        }
    }

    fn dash() -> Value {
        json!({"name": "Ops", "widgets": [{"id": "nodes", "width": 6}, {"id": "from-the-future", "width": 12}]})
    }

    fn invalid(applied: Applied) -> String {
        match applied {
            Applied::Invalid(why) => why,
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn a_feature_override_needs_a_reason_and_a_sane_expiry() {
        let (s, _d) = store();
        let ok = json!({"value": false, "reason": "noisy", "actor": "ops", "set_at": 100});
        assert_eq!(
            s.apply_local(&put(Kind::FeatureOverride, "sniffers", ok.clone())).unwrap(),
            Applied::Done
        );
        assert_eq!(s.get(Kind::FeatureOverride, "sniffers").unwrap(), Some(ok));
        for (bad, needle) in [
            (json!({"value": "no", "reason": "r", "set_at": 1}), "value"),
            (json!({"value": false, "reason": "", "set_at": 1}), "reason"),
            (json!({"value": false, "reason": "r"}), "set_at"),
            (json!({"value": false, "reason": "r", "set_at": 5, "expires_at": 5}), "expires_at"),
            (json!({"value": false, "reason": "r", "set_at": 1, "x": 1}), "known key"),
        ] {
            let why = invalid(s.apply_local(&put(Kind::FeatureOverride, "sniffers", bad)).unwrap());
            assert!(why.contains(needle), "{why}");
        }
        let ok = json!({"value": false, "reason": "r", "set_at": 1});
        let why = invalid(s.apply_local(&put(Kind::FeatureOverride, "a/b", ok)).unwrap());
        assert!(why.contains("name"), "{why}");
    }

    #[test]
    fn a_dashboard_round_trips_and_an_unknown_widget_id_is_kept() {
        let (s, _d) = store();
        assert_eq!(
            s.apply_local(&put(Kind::Dashboard, "ops", dash())).unwrap(),
            Applied::Done
        );
        assert_eq!(s.get(Kind::Dashboard, "ops").unwrap(), Some(dash()));
        assert_eq!(
            s.list(Kind::Dashboard).unwrap(),
            vec![("ops".to_string(), dash())]
        );
    }

    #[test]
    fn a_malformed_dashboard_is_refused_naming_the_path() {
        let (s, _d) = store();
        let bad = json!({"widgets": [{"id": "nodes", "width": 13}]});
        let why = invalid(s.apply_local(&put(Kind::Dashboard, "ops", bad)).unwrap());
        assert!(why.contains("widgets[0].width"), "{why}");
        let bad = json!({"widgets": "nodes"});
        assert!(
            invalid(s.apply_local(&put(Kind::Dashboard, "ops", bad)).unwrap()).contains("widgets")
        );
        assert_eq!(s.get(Kind::Dashboard, "ops").unwrap(), None);
    }

    #[test]
    fn size_limits_apply_per_kind() {
        let (s, _d) = store();
        let big = json!({"widgets": [{"id": "x".repeat(65 * 1024), "width": 1}]});
        assert!(
            invalid(s.apply_local(&put(Kind::Dashboard, "ops", big)).unwrap()).contains("64 KiB")
        );
        let theme =
            json!({"base": "dark", "tokens": {"bg": "#101216"}, "pad": "x".repeat(9 * 1024)});
        assert!(invalid(s.apply_local(&put(Kind::Theme, "t", theme)).unwrap()).contains("8 KiB"));
    }

    #[test]
    fn names_are_one_to_sixty_four_plain_characters() {
        let (s, _d) = store();
        for bad in ["", &"n".repeat(65), "a/b", "tab\there"] {
            let why = invalid(s.apply_local(&put(Kind::Dashboard, bad, dash())).unwrap());
            assert!(why.contains("name"), "{bad:?}: {why}");
        }
    }

    #[test]
    fn a_theme_needs_a_base_known_tokens_and_hex_colours() {
        let (s, _d) = store();
        let ok = json!({"base": "dark", "tokens": {"bg": "#101216", "select": "#7fb0ff"}});
        assert_eq!(
            s.apply_local(&put(Kind::Theme, "Night shift", ok)).unwrap(),
            Applied::Done
        );
        let cases = [
            (json!({"base": "blue", "tokens": {}}), "base"),
            (
                json!({"base": "dark", "tokens": {"banner": "#101216"}}),
                "tokens.banner",
            ),
            (
                json!({"base": "dark", "tokens": {"bg": "red"}}),
                "tokens.bg",
            ),
        ];
        for (theme, path) in cases {
            assert!(invalid(s.apply_local(&put(Kind::Theme, "t", theme)).unwrap()).contains(path));
        }
        let builtin = json!({"base": "dark", "tokens": {}});
        assert!(
            invalid(s.apply_local(&put(Kind::Theme, "Light", builtin)).unwrap())
                .contains("built-in")
        );
    }

    #[test]
    fn a_form_layout_is_checked_against_the_layout_format() {
        let (s, _d) = store();
        let ok = json!({"version": 1, "fields": {"/settings/admin/listen": {"group": "Admin", "order": 2, "advanced": true}}});
        assert_eq!(
            s.apply_local(&put(Kind::FormLayout, "", ok)).unwrap(),
            Applied::Done
        );
        let bad = json!({"version": 1, "fields": {"/settings/admin/listen": {"order": "first"}}});
        let why = invalid(s.apply_local(&put(Kind::FormLayout, "", bad)).unwrap());
        assert!(why.contains("fields./settings/admin/listen.order"), "{why}");
        let bad = json!({"version": 2, "fields": {}});
        assert!(
            invalid(s.apply_local(&put(Kind::FormLayout, "", bad)).unwrap()).contains("version")
        );
        assert_eq!(
            s.apply_local(&del(Kind::FormLayout, "")).unwrap(),
            Applied::Done
        );
        assert_eq!(s.get(Kind::FormLayout, "").unwrap(), None);
    }

    #[test]
    fn defaults_must_name_something_that_exists() {
        let (s, _d) = store();
        let why = invalid(
            s.apply_local(&put(Kind::Defaults, "", json!({"dashboard": "ops"})))
                .unwrap(),
        );
        assert!(why.contains("dashboard"), "{why}");
        s.apply_local(&put(Kind::Dashboard, "ops", dash())).unwrap();
        assert_eq!(
            s.apply_local(&put(
                Kind::Defaults,
                "",
                json!({"dashboard": "ops", "theme": "dark"})
            ))
            .unwrap(),
            Applied::Done,
            "a built-in theme name is a valid default"
        );
    }

    #[test]
    fn deleting_the_default_dashboard_or_theme_resets_that_default() {
        let (s, _d) = store();
        s.apply_local(&put(Kind::Dashboard, "ops", dash())).unwrap();
        s.apply_local(&put(
            Kind::Theme,
            "night",
            json!({"base": "dark", "tokens": {}}),
        ))
        .unwrap();
        s.apply_local(&put(
            Kind::Defaults,
            "",
            json!({"dashboard": "ops", "theme": "night"}),
        ))
        .unwrap();
        assert_eq!(
            s.apply_local(&del(Kind::Theme, "night")).unwrap(),
            Applied::Done
        );
        assert_eq!(
            s.get(Kind::Defaults, "").unwrap(),
            Some(json!({"dashboard": "ops"}))
        );
        assert_eq!(
            s.apply_local(&del(Kind::Dashboard, "ops")).unwrap(),
            Applied::Done
        );
        assert_eq!(s.get(Kind::Defaults, "").unwrap(), Some(json!({})));
    }

    #[test]
    fn deleting_a_missing_record_is_not_found() {
        let (s, _d) = store();
        assert_eq!(
            s.apply_local(&del(Kind::Dashboard, "nope")).unwrap(),
            Applied::NotFound
        );
    }

    #[test]
    fn a_collection_holds_a_bounded_number_of_records() {
        let (s, _d) = store();
        for i in 0..100 {
            let op = put(Kind::Dashboard, &format!("d{i}"), dash());
            assert_eq!(s.apply_local(&op).unwrap(), Applied::Done);
        }
        assert_eq!(
            s.apply_local(&put(Kind::Dashboard, "one-more", dash()))
                .unwrap(),
            Applied::Full(100)
        );
        assert_eq!(
            s.apply_local(&put(Kind::Dashboard, "d7", dash())).unwrap(),
            Applied::Done,
            "an update still fits"
        );
    }

    #[test]
    fn an_entry_at_or_below_the_cursor_is_a_replay() {
        let (s, _d) = store();
        assert_eq!(
            s.apply_at(5, &put(Kind::Dashboard, "a", dash())).unwrap(),
            Applied::Done
        );
        assert_eq!(
            s.apply_at(5, &del(Kind::Dashboard, "a")).unwrap(),
            Applied::Replayed
        );
        assert_eq!(
            s.apply_at(4, &del(Kind::Dashboard, "a")).unwrap(),
            Applied::Replayed
        );
        assert!(s.get(Kind::Dashboard, "a").unwrap().is_some());
        assert_eq!(s.applied_index().unwrap(), Some(5));
    }

    #[test]
    fn a_snapshot_rebuilds_an_identical_store() {
        let (a, _da) = store();
        a.apply_at(1, &put(Kind::Dashboard, "ops", dash())).unwrap();
        a.apply_at(
            2,
            &put(Kind::Theme, "night", json!({"base": "dark", "tokens": {}})),
        )
        .unwrap();
        a.apply_at(
            3,
            &put(Kind::SavedFilter, "degraded", json!({"state": "degraded"})),
        )
        .unwrap();
        let (b, _db) = store();
        b.apply_at(1, &put(Kind::Dashboard, "stale", dash()))
            .unwrap();
        b.replace(&a.snapshot().unwrap()).unwrap();
        assert_eq!(b.snapshot().unwrap(), a.snapshot().unwrap());
        assert!(b.get(Kind::Dashboard, "stale").unwrap().is_none());
        assert_eq!(b.applied_index().unwrap(), Some(3));
    }
}
