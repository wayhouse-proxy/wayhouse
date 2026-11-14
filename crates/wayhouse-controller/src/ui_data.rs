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
    /// An operator's tags for one node, named by its instance id.
    NodeTags,
    /// A cluster secret (K1): ciphertext only, named by the secret. The controller seals the
    /// value before it proposes the entry, so no log entry or snapshot holds plaintext.
    ClusterSecret,
    /// A one-time enrollment code, named by its public id; holds only a hash of the secret.
    Enrollment,
    /// The token one enrolled origin presents, named by the SHA-256 of the token (hex) so a
    /// request finds its record in one read; holds only the origin's name.
    OriginToken,
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
    /// Re-encrypts one cluster secret under a newer key: applied only if the secret still holds
    /// the ciphertext whose nonce is `from_nonce`, so a `PUT` that landed meanwhile wins.
    RewrapSecret {
        name: String,
        from_nonce: String,
        value: Value,
    },
    /// Spends one use of an enrollment code. Atomic in the state machine, so a single-use code
    /// redeems exactly once however many nodes race. `now` is the proposer's clock: every
    /// replica must reach the same answer.
    Redeem {
        id: String,
        hash: String,
        now: u64,
        /// A controller replica's join proof, hashed (see `enroll`): recorded on the code in
        /// the same atomic step that spends it. Absent for every other role.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        join_hash: Option<String>,
        /// Set only when the code is a pool code (a controller code with several uses): the
        /// replica this use is for. Applying it allocates the node id and writes the child.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pool: Option<PoolChild>,
        /// Set for a prefix code: the name this redeem claims. Refused (`NameTaken`) in the same
        /// atomic step when the name already holds a token or a tunnel key, so the check cannot
        /// race another redeem or a lagging follower.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
}

/// One replica enrolled from a pool code, as the proposer asks for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PoolChild {
    /// The child record's id: the front of the replica's join proof.
    pub id: String,
    /// The pod's name; one controller record per name, ever (until revoked).
    pub name: String,
    /// Hash of the join proof, like `join_hash` on a single-use code.
    pub join_hash: String,
    /// One above the highest Raft member the proposer knows; the store adds its own highest.
    pub min_node_id: u64,
}

/// Most uses a pool code can have (one per replica).
pub const POOL_MAX_USES: u64 = 16;

/// How long after its redeem a controller replica may still ask to be added to the Raft group.
pub const JOIN_WINDOW_SECS: u64 = 3600;

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
    /// A rewrap lost a race: the secret was replaced since it was read.
    Stale,
    /// A redeem that matched no live code. Unknown id, wrong secret, expired and spent all look
    /// the same, so the answer reveals nothing about which codes exist.
    Refused,
    /// A controller code asked for a node id another code already holds.
    Taken,
    /// A prefix-code redeem named an origin that already holds a token or a tunnel key.
    NameTaken,
    /// A pool-code redeem spent a use and made a child record holding this node id.
    Allocated(u64),
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
    /// Every kind, for the HA snapshot: a kind missing here is lost to a replica that catches up
    /// from a snapshot (the test below fails to compile when a variant is added).
    pub const ALL: [Kind; 10] = [
        Kind::Dashboard,
        Kind::Theme,
        Kind::SavedFilter,
        Kind::FormLayout,
        Kind::Defaults,
        Kind::FeatureOverride,
        Kind::NodeTags,
        Kind::ClusterSecret,
        Kind::Enrollment,
        Kind::OriginToken,
    ];

    fn tag(self) -> &'static str {
        match self {
            Kind::Dashboard => "dashboards",
            Kind::Theme => "themes",
            Kind::SavedFilter => "filters",
            Kind::FormLayout => "layout",
            Kind::Defaults => "defaults",
            Kind::FeatureOverride => "feature_overrides",
            Kind::NodeTags => "node_tags",
            Kind::ClusterSecret => "cluster_secrets",
            Kind::Enrollment => "enrollments",
            Kind::OriginToken => "origin_tokens",
        }
    }

    fn max_bytes(self) -> usize {
        match self {
            Kind::Dashboard | Kind::FormLayout => 64 << 10,
            Kind::Theme => 8 << 10,
            Kind::NodeTags => 4 << 10,
            Kind::SavedFilter => 4 << 10,
            Kind::Defaults | Kind::FeatureOverride => 1 << 10,
            Kind::ClusterSecret => 8 << 10,
            Kind::Enrollment | Kind::OriginToken => 1 << 10,
        }
    }

    /// Most named records of this kind; `None` for the single records.
    pub(crate) fn max_records(self) -> Option<usize> {
        match self {
            Kind::Dashboard => Some(100),
            Kind::Theme => Some(50),
            Kind::SavedFilter => Some(200),
            Kind::FeatureOverride => Some(64),
            // Sized for a fleet of about 500 nodes with room to spare.
            Kind::NodeTags => Some(1000),
            // Room for a fleet of origins: each holds one tunnel key (two during a rotation).
            Kind::ClusterSecret => Some(2048),
            Kind::Enrollment => Some(200),
            Kind::OriginToken => Some(2048),
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
    if kind == Kind::ClusterSecret && !wayhouse_secrets::valid_cluster_name(name) {
        return Err(
            "a secret name is 1 to 63 of a-z, 0-9 and '-', not starting or ending with '-'".into(),
        );
    }
    if kind == Kind::OriginToken && !is_token_hash(name) {
        return Err("an origin token is named by 64 lowercase hex digits".into());
    }
    if kind == Kind::FeatureOverride
        && !name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
    {
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

/// The name of an [`Kind::OriginToken`] record: SHA-256 of the token, lowercase hex.
pub fn is_token_hash(name: &str) -> bool {
    name.len() == 64 && name.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
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
            if obj.get("version").is_some_and(|v| !v.is_u64()) {
                return Err("version must be a whole number".into());
            }
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
            only_keys(
                obj,
                "",
                &["value", "reason", "actor", "set_at", "expires_at"],
            )?;
            if !obj.get("value").is_some_and(Value::is_boolean) {
                return Err("value must be true or false".into());
            }
            string_max(obj.get("reason").unwrap_or(&Value::Null), "reason", 500)?;
            if let Some(a) = obj.get("actor") {
                string_max(a, "actor", 128)?;
            }
            let set_at = obj
                .get("set_at")
                .and_then(Value::as_u64)
                .ok_or("set_at must be a unix time")?;
            if let Some(e) = obj.get("expires_at") {
                if !e.as_u64().is_some_and(|e| e > set_at) {
                    return Err("expires_at must be a unix time after set_at".into());
                }
            }
            Ok(())
        }
        Kind::ClusterSecret => {
            only_keys(
                obj,
                "",
                &["key_id", "nonce", "ciphertext", "set_at", "actor"],
            )?;
            for k in ["key_id", "nonce", "ciphertext"] {
                string_max(obj.get(k).unwrap_or(&Value::Null), k, 6 << 10)?;
            }
            if !obj.get("set_at").is_some_and(Value::is_u64) {
                return Err("set_at must be a unix time".into());
            }
            if let Some(a) = obj.get("actor") {
                string_max(a, "actor", 128)?;
            }
            Ok(())
        }
        Kind::Enrollment => {
            only_keys(
                obj,
                "",
                &[
                    "role",
                    "origin",
                    "hash",
                    "created_at",
                    "expires_at",
                    "uses",
                    "uses_left",
                    "created_by",
                    "parent_url",
                    "name_prefix",
                    "node_id",
                    "join_hash",
                    "join_until",
                    "joined",
                    "redeemed_at",
                ],
            )?;
            let role = obj.get("role").and_then(Value::as_str);
            if !matches!(
                role,
                Some("agent" | "proxy" | "aggregator" | "ui" | "controller")
            ) {
                return Err("role must be agent, proxy, aggregator, ui or controller".into());
            }
            let is_controller = role == Some("controller");
            let uses = obj.get("uses").and_then(Value::as_u64);
            // A controller code with several uses is a pool: each redeem gets its own node id, so
            // the code holds none, and each pod names itself inside the prefix.
            let is_pool = is_controller && uses.is_some_and(|u| u > 1);
            if (is_controller && !is_pool) != obj.contains_key("node_id") {
                return Err("node_id is set on a single-use controller code and only there".into());
            }
            if is_pool != obj.contains_key("name_prefix") && is_controller {
                return Err(
                    "a pool controller code needs name_prefix, a single-use one has none".into(),
                );
            }
            if !is_controller
                && ["join_hash", "join_until", "joined"]
                    .iter()
                    .any(|k| obj.contains_key(*k))
            {
                return Err("join fields belong to a controller code".into());
            }
            if let Some(id) = obj.get("node_id") {
                if !id.as_u64().is_some_and(|i| (1..=1_000_000).contains(&i)) {
                    return Err("node_id must be 1 to 1000000".into());
                }
            }
            if let Some(h) = obj.get("join_hash") {
                let h = string_max(h, "join_hash", 64)?;
                if h.len() != 64 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return Err("join_hash must be 64 hex digits".into());
                }
            }
            if obj.get("join_until").is_some_and(|u| !u.is_u64()) {
                return Err("join_until must be a unix time".into());
            }
            if obj.get("joined").is_some_and(|j| !j.is_boolean()) {
                return Err("joined must be true or false".into());
            }
            if let Some(u) = obj.get("parent_url") {
                string_max(u, "parent_url", 512)?;
            }
            if let Some(p) = obj.get("name_prefix") {
                let p = string_max(p, "name_prefix", 64)?;
                crate::registry::validate_name(p)?;
                if !matches!(role, Some("agent" | "proxy")) && !is_pool {
                    return Err("name_prefix is for an agent, proxy or pool controller code".into());
                }
            }
            let origin = string_max(obj.get("origin").unwrap_or(&Value::Null), "origin", 64)?;
            crate::registry::validate_name(origin)?;
            let hash = string_max(obj.get("hash").unwrap_or(&Value::Null), "hash", 64)?;
            if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err("hash must be 64 hex digits".into());
            }
            for k in ["created_at", "expires_at"] {
                if !obj.get(k).is_some_and(Value::is_u64) {
                    return Err(format!("{k} must be a unix time"));
                }
            }
            let max_uses = if is_controller { POOL_MAX_USES } else { 1000 };
            if !uses.is_some_and(|u| (1..=max_uses).contains(&u)) {
                return Err(format!("uses must be 1 to {max_uses}"));
            }
            if !obj
                .get("uses_left")
                .and_then(Value::as_u64)
                .is_some_and(|l| Some(l) <= uses)
            {
                return Err("uses_left must be a whole number up to uses".into());
            }
            if let Some(a) = obj.get("created_by") {
                string_max(a, "created_by", 128)?;
            }
            Ok(())
        }
        Kind::OriginToken => {
            only_keys(obj, "", &["origin", "created_at"])?;
            let origin = string_max(obj.get("origin").unwrap_or(&Value::Null), "origin", 64)?;
            crate::registry::validate_name(origin)?;
            if !obj.get("created_at").is_some_and(Value::is_u64) {
                return Err("created_at must be a unix time".into());
            }
            Ok(())
        }
        Kind::NodeTags => {
            only_keys(obj, "", &["name", "tags"])?;
            let tags: std::collections::BTreeMap<String, String> =
                object(obj.get("tags").unwrap_or(&Value::Null), "tags")?
                    .iter()
                    .map(|(k, v)| match v.as_str() {
                        Some(v) => Ok((k.clone(), v.to_string())),
                        None => Err(format!("tags.{k} must be a string")),
                    })
                    .collect::<Result<_, _>>()?;
            if tags.is_empty() {
                return Err("tags is empty: DELETE the record to clear a node's tags".into());
            }
            wayhouse_http::labels::check_labels(None, &tags)
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
    /// Not a write: the node id a pool redeem handed out, which becomes the answer.
    Allocated(u64),
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

    /// How many records of `kind` there are, without parsing them.
    fn count(&self, kind: Kind) -> usize {
        self.records.scan_prefix(key(kind, "")).count()
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
                // A node id belongs to one controller code: checked here, in the atomic step, so
                // two admins minting at once on different replicas cannot both get it.
                if *kind == Kind::Enrollment && value["role"] == "controller" {
                    for (other, rec) in self.list(Kind::Enrollment)? {
                        if other != *name
                            && rec["role"] == "controller"
                            && rec["node_id"] == value["node_id"]
                        {
                            return Ok(Err(Applied::Taken));
                        }
                    }
                }
                if let Some(max) = kind.max_records() {
                    if self.records.get(&k)?.is_none() && self.count(*kind) >= max {
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
            UiOp::RewrapSecret {
                name,
                from_nonce,
                value,
            } => {
                if let Err(why) = validate(Kind::ClusterSecret, name, value) {
                    return Ok(Err(Applied::Invalid(why)));
                }
                let k = key(Kind::ClusterSecret, name);
                let Some(current) = self.records.get(&k)? else {
                    return Ok(Err(Applied::NotFound));
                };
                let current: Value = serde_json::from_slice(&current)?;
                if current["nonce"].as_str() != Some(from_nonce) {
                    return Ok(Err(Applied::Stale));
                }
                Ok(Ok(vec![Write::Insert(k, serde_json::to_vec(value)?)]))
            }
            UiOp::Redeem {
                id,
                hash,
                now,
                join_hash,
                pool,
                name,
            } => {
                let Some(raw) = self.records.get(key(Kind::Enrollment, id))? else {
                    return Ok(Err(Applied::Refused));
                };
                let mut rec: Value = serde_json::from_slice(&raw)?;
                let live = rec["expires_at"].as_u64().is_some_and(|e| *now < e)
                    && rec["uses_left"].as_u64().is_some_and(|l| l > 0)
                    && rec["hash"]
                        .as_str()
                        .is_some_and(|h| wayhouse_http::token_eq(hash, h));
                // A pool code (a controller code with several uses) is redeemed with a child and
                // nothing else does; the two must agree or the redeem is refused.
                let is_pool = rec["role"] == "controller" && rec["uses"].as_u64() > Some(1);
                if !live || is_pool != pool.is_some() {
                    return Ok(Err(Applied::Refused));
                }
                if let Some(name) = name {
                    let token = self
                        .list(Kind::OriginToken)?
                        .iter()
                        .any(|(_, v)| v["origin"] == name.as_str());
                    let key = self
                        .get(Kind::ClusterSecret, &wayhouse_enroll::psk_secret_name(name))?
                        .is_some();
                    if token || key {
                        return Ok(Err(Applied::NameTaken));
                    }
                }
                rec["uses_left"] = (rec["uses_left"].as_u64().unwrap_or(1) - 1).into();
                // When the code was last spent: revoking an old code must not cut off a host
                // that has since enrolled again with a newer one.
                rec["redeemed_at"] = (*now).into();
                let mut writes = vec![];
                let mut allocated = None;
                if let Some(child) = pool {
                    let child_key = key(Kind::Enrollment, &child.id);
                    let mut next = child.min_node_id.max(1);
                    for (other, r) in self.list(Kind::Enrollment)? {
                        if other == child.id {
                            return Ok(Err(Applied::Refused));
                        }
                        if r["role"] == "controller"
                            && r["uses"] == 1
                            && r["origin"] == child.name.as_str()
                        {
                            return Ok(Err(Applied::Refused));
                        }
                        next = next.max(r["node_id"].as_u64().map_or(0, |n| n + 1));
                    }
                    let count = self.count(Kind::Enrollment);
                    if let Some(max) = Kind::Enrollment.max_records().filter(|m| count + 1 > *m) {
                        return Ok(Err(Applied::Full(max)));
                    }
                    let child_rec = serde_json::json!({
                        "role": "controller", "origin": child.name, "hash": child.join_hash,
                        "created_at": now, "expires_at": rec["expires_at"], "uses": 1,
                        "uses_left": 0, "node_id": next, "join_hash": child.join_hash,
                        "join_until": *now + JOIN_WINDOW_SECS,
                    });
                    if validate(Kind::Enrollment, &child.id, &child_rec).is_err() {
                        return Ok(Err(Applied::Refused));
                    }
                    writes.push(Write::Insert(child_key, serde_json::to_vec(&child_rec)?));
                    allocated = Some(next);
                } else if let (Some(jh), "controller") =
                    (join_hash, rec["role"].as_str().unwrap_or(""))
                {
                    rec["join_hash"] = jh.as_str().into();
                    rec["join_until"] = (*now + JOIN_WINDOW_SECS).into();
                }
                writes.push(Write::Insert(
                    key(Kind::Enrollment, id),
                    serde_json::to_vec(&rec)?,
                ));
                if let Some(n) = allocated {
                    writes.push(Write::Allocated(n));
                }
                Ok(Ok(writes))
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
                            let mut answer = Applied::Done;
                            for w in writes {
                                match w {
                                    Write::Insert(k, v) => {
                                        records.insert(k.as_slice(), v.as_slice())?;
                                    }
                                    Write::Remove(k) => {
                                        records.remove(k.as_slice())?;
                                    }
                                    Write::Allocated(n) => answer = Applied::Allocated(*n),
                                }
                            }
                            Ok(answer)
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
        for kind in Kind::ALL {
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
    /// A freshly sealed record: every call draws a new random nonce, like a real `PUT`.
    fn secret() -> Value {
        let ring = wayhouse_secrets::Keyring::parse("AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=")
            .unwrap();
        let sealed = ring
            .seal_cluster("gossip-psk", b"correct-horse-battery")
            .unwrap();
        json!({"key_id": sealed.key_id, "nonce": sealed.nonce, "ciphertext": sealed.ciphertext, "set_at": 5, "actor": "ops"})
    }

    fn nonce(record: &Value) -> String {
        record["nonce"].as_str().unwrap().to_owned()
    }

    fn rewrap(from: &Value, value: Value) -> UiOp {
        UiOp::RewrapSecret {
            name: "gossip-psk".into(),
            from_nonce: nonce(from),
            value,
        }
    }

    #[test]
    fn a_rewrap_replaces_the_secret_only_if_it_was_not_replaced_since() {
        let (s, _d) = store();
        let name = "gossip-psk";
        let (read, replaced, rewrapped) = (secret(), secret(), secret());
        assert_eq!(
            s.apply_local(&rewrap(&read, rewrapped.clone())).unwrap(),
            Applied::NotFound
        );
        s.apply_local(&put(Kind::ClusterSecret, name, read.clone()))
            .unwrap();
        // An operator's PUT landed after the rewrap read the old ciphertext.
        s.apply_local(&put(Kind::ClusterSecret, name, replaced.clone()))
            .unwrap();
        assert_eq!(
            s.apply_local(&rewrap(&read, rewrapped.clone())).unwrap(),
            Applied::Stale
        );
        assert_eq!(
            s.get(Kind::ClusterSecret, name).unwrap(),
            Some(replaced.clone())
        );
        assert_eq!(
            s.apply_local(&rewrap(&replaced, rewrapped.clone()))
                .unwrap(),
            Applied::Done
        );
        assert_eq!(s.get(Kind::ClusterSecret, name).unwrap(), Some(rewrapped));
    }

    #[test]
    fn the_snapshot_covers_every_kind() {
        // One record per kind, built by an exhaustive match that does not use `Kind::ALL`:
        // a new kind will not compile here, and the snapshot must then hold it.
        let literal = [
            Kind::Dashboard,
            Kind::Theme,
            Kind::SavedFilter,
            Kind::FormLayout,
            Kind::Defaults,
            Kind::FeatureOverride,
            Kind::NodeTags,
            Kind::ClusterSecret,
            Kind::Enrollment,
            Kind::OriginToken,
        ];
        let (store, _d) = store();
        for kind in literal {
            let value = match kind {
                Kind::Dashboard
                | Kind::Theme
                | Kind::SavedFilter
                | Kind::FormLayout
                | Kind::Defaults
                | Kind::FeatureOverride
                | Kind::NodeTags
                | Kind::ClusterSecret
                | Kind::Enrollment
                | Kind::OriginToken => json!({}),
            };
            // Validation may refuse a bare `{}`; write the record directly.
            store
                .records
                .insert(key(kind, "x"), serde_json::to_vec(&value).unwrap())
                .unwrap();
        }
        let snap = store.snapshot().unwrap();
        for kind in literal {
            assert!(
                snap.records.iter().any(|(k, _, _)| *k == kind),
                "the snapshot misses {}",
                kind.tag()
            );
        }
        let tags: std::collections::BTreeSet<_> = Kind::ALL.iter().map(|k| k.tag()).collect();
        assert_eq!(tags.len(), Kind::ALL.len(), "a kind is listed twice");
    }

    #[test]
    fn the_rolling_upgrade_gate_refuses_a_member_without_this_levels_kinds() {
        use crate::ha::routes::{Whoami, UI_SUPPORT};
        let who = |ui_support| Whoami {
            node_id: 2,
            log_empty: false,
            pre_ha: crate::ha::import::PreHaSummary::default(),
            plugin_support: 0,
            secret_key_ids: Vec::new(),
            acme_support: 0,
            confirm_support: 0,
            ui_support,
        };
        assert!(check_support_answers(&[(2, Ok(who(UI_SUPPORT)))]).is_ok());
        // Level 5 predates enrollment codes and cannot decode the redeem entry.
        assert!(check_support_answers(&[(2, Ok(who(5)))]).is_err());
        assert!(
            check_support_answers(&[(2, Ok(who(3)))]).is_err(),
            "level 3 cannot decode cluster secrets"
        );
    }

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

    fn code_record(hash: &str, uses: u64, expires_at: u64) -> Value {
        json!({"role": "agent", "origin": "home", "hash": hash, "created_at": 100,
               "expires_at": expires_at, "uses": uses, "uses_left": uses, "created_by": "ops"})
    }

    fn redeem(id: &str, hash: &str, now: u64) -> UiOp {
        UiOp::Redeem {
            id: id.into(),
            hash: hash.into(),
            now,
            join_hash: None,
            pool: None,
            name: None,
        }
    }

    #[test]
    fn a_redeem_for_a_name_that_holds_credentials_is_refused_and_spends_nothing() {
        let (store, _d) = store();
        let rec = json!({"role": "agent", "origin": "pods", "hash": H1, "created_at": 100,
            "expires_at": 1000, "uses": 3, "uses_left": 3, "name_prefix": "k8s-"});
        store
            .apply_local(&UiOp::Put {
                kind: Kind::Enrollment,
                name: "abcd1234".into(),
                value: rec,
            })
            .unwrap();
        store
            .apply_local(&UiOp::Put {
                kind: Kind::OriginToken,
                name: "a".repeat(64),
                value: json!({"origin": "k8s-a", "created_at": 1}),
            })
            .unwrap();
        let mut op = redeem("abcd1234", H1, 200);
        let UiOp::Redeem { name, .. } = &mut op else {
            unreachable!()
        };
        *name = Some("k8s-a".into());
        assert_eq!(store.apply_local(&op).unwrap(), Applied::NameTaken);
        let rec = store.get(Kind::Enrollment, "abcd1234").unwrap().unwrap();
        assert_eq!(rec["uses_left"], 3, "no use spent");
        let UiOp::Redeem { name, .. } = &mut op else {
            unreachable!()
        };
        *name = Some("k8s-b".into());
        assert_eq!(store.apply_local(&op).unwrap(), Applied::Done);
    }

    fn pool_record(uses: u64) -> Value {
        json!({"role": "controller", "origin": "ctl-pool", "hash": H1, "created_at": 100,
               "expires_at": 1000, "uses": uses, "uses_left": uses, "name_prefix": "ctl-"})
    }

    fn pool_redeem(name: &str, min_node_id: u64) -> UiOp {
        let child = format!("{:08x}", name.bytes().map(u32::from).sum::<u32>());
        UiOp::Redeem {
            id: "abcd1234".into(),
            hash: H1.into(),
            now: 200,
            join_hash: None,
            name: None,
            pool: Some(PoolChild {
                id: child,
                name: name.into(),
                join_hash: H2.into(),
                min_node_id,
            }),
        }
    }

    fn pool_store(uses: u64) -> (UiStore, tempfile::TempDir) {
        let (s, d) = store();
        s.apply_local(&put(Kind::Enrollment, "abcd1234", pool_record(uses)))
            .unwrap();
        (s, d)
    }

    #[test]
    fn a_pool_code_hands_each_pod_its_own_node_id_above_the_members() {
        let (s, _d) = pool_store(3);
        assert_eq!(
            s.apply_local(&pool_redeem("ctl-0", 4)).unwrap(),
            Applied::Allocated(4)
        );
        assert_eq!(
            s.apply_local(&pool_redeem("ctl-1", 4)).unwrap(),
            Applied::Allocated(5)
        );
        assert_eq!(
            s.apply_local(&pool_redeem("ctl-2", 2)).unwrap(),
            Applied::Allocated(6)
        );
        // The pool is spent after its uses.
        assert_eq!(
            s.apply_local(&pool_redeem("ctl-3", 2)).unwrap(),
            Applied::Refused
        );
        let parent = s.get(Kind::Enrollment, "abcd1234").unwrap().unwrap();
        assert_eq!(parent["uses_left"], 0);
        assert!(parent.get("node_id").is_none());
    }

    #[test]
    fn a_pool_redeem_writes_a_spent_child_that_a_join_can_use() {
        let (s, _d) = pool_store(2);
        let op = pool_redeem("ctl-0", 3);
        let UiOp::Redeem { pool: Some(c), .. } = &op else {
            unreachable!()
        };
        let child_id = c.id.clone();
        s.apply_local(&op).unwrap();
        let child = s.get(Kind::Enrollment, &child_id).unwrap().unwrap();
        assert_eq!(child["role"], "controller");
        assert_eq!(
            (child["origin"].as_str(), child["node_id"].as_u64()),
            (Some("ctl-0"), Some(3))
        );
        assert_eq!(
            (child["uses"].as_u64(), child["uses_left"].as_u64()),
            (Some(1), Some(0))
        );
        assert_eq!(child["join_hash"], H2);
        assert!(child["join_until"].as_u64().is_some_and(|u| u > 200));
        assert!(validate(Kind::Enrollment, &child_id, &child).is_ok());
    }

    #[test]
    fn a_name_that_already_has_a_replica_is_refused_even_with_uses_left() {
        let (s, _d) = pool_store(3);
        assert_eq!(
            s.apply_local(&pool_redeem("ctl-0", 2)).unwrap(),
            Applied::Allocated(2)
        );
        assert_eq!(
            s.apply_local(&pool_redeem("ctl-0", 2)).unwrap(),
            Applied::Refused
        );
        assert_eq!(
            s.get(Kind::Enrollment, "abcd1234").unwrap().unwrap()["uses_left"],
            2
        );
    }

    #[test]
    fn revoking_a_replica_record_frees_its_name_and_never_its_node_id() {
        let (s, _d) = pool_store(3);
        s.apply_local(&pool_redeem("ctl-0", 2)).unwrap();
        let UiOp::Redeem { pool: Some(c), .. } = pool_redeem("ctl-0", 2) else {
            unreachable!()
        };
        s.apply_local(&UiOp::Delete {
            kind: Kind::Enrollment,
            name: c.id,
        })
        .unwrap();
        // The deleted record held id 2; the next replica of the same name gets a higher one only
        // when the proposer's membership view says so, so the proposer must pass it (members).
        assert_eq!(
            s.apply_local(&pool_redeem("ctl-0", 3)).unwrap(),
            Applied::Allocated(3)
        );
    }

    #[test]
    fn a_pool_code_refuses_a_redeem_without_a_child_and_a_single_use_code_refuses_one_with() {
        let (s, _d) = pool_store(2);
        assert_eq!(
            s.apply_local(&redeem("abcd1234", H1, 200)).unwrap(),
            Applied::Refused
        );
        let (s, _d) = store();
        let mut single = pool_record(1);
        single["node_id"] = 2.into();
        single.as_object_mut().unwrap().remove("name_prefix");
        s.apply_local(&put(Kind::Enrollment, "abcd1234", single))
            .unwrap();
        assert_eq!(
            s.apply_local(&pool_redeem("ctl-0", 2)).unwrap(),
            Applied::Refused
        );
    }

    #[test]
    fn a_pool_redeem_whose_child_id_is_taken_or_whose_store_is_full_changes_nothing() {
        let (s, _d) = pool_store(3);
        let mut clash = pool_redeem("ctl-0", 2);
        let UiOp::Redeem { pool: Some(c), .. } = &mut clash else {
            unreachable!()
        };
        c.id = "abcd1234".into(); // the pool's own id
        assert_eq!(s.apply_local(&clash).unwrap(), Applied::Refused);
        assert_eq!(
            s.get(Kind::Enrollment, "abcd1234").unwrap().unwrap()["uses_left"],
            3
        );
        // Fill the collection to its cap: the next child does not fit and no use is spent.
        let cap = Kind::Enrollment.max_records().unwrap();
        for i in 0..cap - 1 {
            s.apply_local(&put(
                Kind::Enrollment,
                &format!("{i:08x}"),
                code_record(H2, 1, 1000),
            ))
            .unwrap();
        }
        assert_eq!(
            s.apply_local(&pool_redeem("ctl-0", 2)).unwrap(),
            Applied::Full(cap)
        );
        assert_eq!(
            s.get(Kind::Enrollment, "abcd1234").unwrap().unwrap()["uses_left"],
            3
        );
    }

    #[test]
    fn a_pool_code_has_no_node_id_a_prefix_and_at_most_sixteen_uses() {
        assert!(validate(Kind::Enrollment, "abcd1234", &pool_record(3)).is_ok());
        assert!(validate(Kind::Enrollment, "abcd1234", &pool_record(17)).is_err());
        let mut with_id = pool_record(3);
        with_id["node_id"] = 2.into();
        assert!(validate(Kind::Enrollment, "abcd1234", &with_id).is_err());
        let mut no_prefix = pool_record(3);
        no_prefix.as_object_mut().unwrap().remove("name_prefix");
        assert!(validate(Kind::Enrollment, "abcd1234", &no_prefix).is_err());
        let mut single = pool_record(1);
        assert!(
            validate(Kind::Enrollment, "abcd1234", &single).is_err(),
            "single use needs an id, no prefix"
        );
        single.as_object_mut().unwrap().remove("name_prefix");
        single["node_id"] = 2.into();
        assert!(validate(Kind::Enrollment, "abcd1234", &single).is_ok());
    }

    const H1: &str = "1111111111111111111111111111111111111111111111111111111111111111";
    const H2: &str = "2222222222222222222222222222222222222222222222222222222222222222";

    #[test]
    fn a_code_redeems_once_and_every_later_try_is_refused() {
        let (s, _d) = store();
        s.apply_local(&put(Kind::Enrollment, "abcd1234", code_record(H1, 1, 1000)))
            .unwrap();
        assert_eq!(
            s.apply_local(&redeem("abcd1234", H1, 200)).unwrap(),
            Applied::Done
        );
        assert_eq!(
            s.apply_local(&redeem("abcd1234", H1, 201)).unwrap(),
            Applied::Refused
        );
        assert_eq!(
            s.get(Kind::Enrollment, "abcd1234").unwrap().unwrap()["uses_left"],
            0
        );
    }

    #[test]
    fn a_wrong_secret_an_unknown_id_and_an_expired_code_are_all_refused_alike() {
        let (s, _d) = store();
        s.apply_local(&put(Kind::Enrollment, "abcd1234", code_record(H1, 2, 1000)))
            .unwrap();
        for op in [
            redeem("abcd1234", H2, 200),
            redeem("ffffffff", H1, 200),
            redeem("abcd1234", H1, 1000),
        ] {
            assert_eq!(s.apply_local(&op).unwrap(), Applied::Refused, "{op:?}");
        }
        // The failed tries burnt nothing.
        assert_eq!(
            s.get(Kind::Enrollment, "abcd1234").unwrap().unwrap()["uses_left"],
            2
        );
    }

    #[test]
    fn a_multi_use_code_serves_exactly_its_uses() {
        let (s, _d) = store();
        s.apply_local(&put(Kind::Enrollment, "abcd1234", code_record(H1, 3, 1000)))
            .unwrap();
        let done = (0..5)
            .filter(|i| s.apply_local(&redeem("abcd1234", H1, 200 + i)).unwrap() == Applied::Done)
            .count();
        assert_eq!(done, 3);
    }

    #[test]
    fn concurrent_redeems_of_a_single_use_code_give_exactly_one_success() {
        let (s, _d) = store();
        s.apply_local(&put(Kind::Enrollment, "abcd1234", code_record(H1, 1, 1000)))
            .unwrap();
        let wins: usize = std::thread::scope(|sc| {
            let hs: Vec<_> = (0..16)
                .map(|_| {
                    sc.spawn(|| {
                        s.apply_local(&redeem("abcd1234", H1, 200)).unwrap() == Applied::Done
                    })
                })
                .collect();
            hs.into_iter().map(|h| usize::from(h.join().unwrap())).sum()
        });
        assert_eq!(wins, 1);
    }

    #[test]
    fn a_replicated_redeem_replays_without_a_second_use() {
        let (s, _d) = store();
        s.apply_at(
            1,
            &put(Kind::Enrollment, "abcd1234", code_record(H1, 2, 1000)),
        )
        .unwrap();
        assert_eq!(
            s.apply_at(2, &redeem("abcd1234", H1, 200)).unwrap(),
            Applied::Done
        );
        assert_eq!(
            s.apply_at(2, &redeem("abcd1234", H1, 200)).unwrap(),
            Applied::Replayed
        );
        assert_eq!(
            s.get(Kind::Enrollment, "abcd1234").unwrap().unwrap()["uses_left"],
            1
        );
    }

    #[test]
    fn an_enrollment_record_has_a_fixed_shape_and_never_holds_a_plaintext_code() {
        let mut bad = code_record(H1, 1, 1000);
        bad["code"] = "abcd1234.secret".into();
        assert!(validate(Kind::Enrollment, "abcd1234", &bad).is_err());
        assert!(validate(Kind::Enrollment, "abcd1234", &code_record("short", 1, 1000)).is_err());
        assert!(validate(Kind::Enrollment, "abcd1234", &code_record(H1, 0, 1000)).is_err());
        assert!(validate(Kind::Enrollment, "abcd1234", &code_record(H1, 1, 1000)).is_ok());
    }

    #[test]
    fn a_feature_override_needs_a_reason_and_a_sane_expiry() {
        let (s, _d) = store();
        let ok = json!({"value": false, "reason": "noisy", "actor": "ops", "set_at": 100});
        assert_eq!(
            s.apply_local(&put(Kind::FeatureOverride, "gossip_verdicts", ok.clone()))
                .unwrap(),
            Applied::Done
        );
        assert_eq!(
            s.get(Kind::FeatureOverride, "gossip_verdicts").unwrap(),
            Some(ok)
        );
        for (bad, needle) in [
            (json!({"value": "no", "reason": "r", "set_at": 1}), "value"),
            (json!({"value": false, "reason": "", "set_at": 1}), "reason"),
            (json!({"value": false, "reason": "r"}), "set_at"),
            (
                json!({"value": false, "reason": "r", "set_at": 5, "expires_at": 5}),
                "expires_at",
            ),
            (
                json!({"value": false, "reason": "r", "set_at": 1, "x": 1}),
                "known key",
            ),
        ] {
            let why = invalid(
                s.apply_local(&put(Kind::FeatureOverride, "gossip_verdicts", bad))
                    .unwrap(),
            );
            assert!(why.contains(needle), "{why}");
        }
        let ok = json!({"value": false, "reason": "r", "set_at": 1});
        let why = invalid(
            s.apply_local(&put(Kind::FeatureOverride, "a/b", ok))
                .unwrap(),
        );
        assert!(why.contains("name"), "{why}");
    }

    #[test]
    fn operator_tags_follow_the_node_label_rules_and_the_record_cap() {
        let (s, _d) = store();
        let ok = json!({"tags": {"env": "prod", "rack": "r12"}});
        assert_eq!(
            s.apply_local(&put(Kind::NodeTags, "fra-1", ok.clone()))
                .unwrap(),
            Applied::Done
        );
        assert_eq!(s.get(Kind::NodeTags, "fra-1").unwrap(), Some(ok));
        let many: serde_json::Map<String, Value> =
            (0..17).map(|i| (format!("k{i}"), json!("v"))).collect();
        for (bad, needle) in [
            (json!({"tags": {"Env": "prod"}}), "tag key"),
            (json!({"tags": {"env": "pr od"}}), "value of tag"),
            (json!({"tags": many}), "more than 16"),
            (json!({"tags": ["env"]}), "tags must be an object"),
            (json!({"tags": {"env": 1}}), "string"),
            (json!({"tags": {}, "region": "eu"}), "known key"),
            (json!({"tags": {}}), "DELETE"),
        ] {
            let why = invalid(s.apply_local(&put(Kind::NodeTags, "fra-1", bad)).unwrap());
            assert!(why.contains(needle), "{needle}: {why}");
        }
        let one = json!({"tags": {"a": "b"}});
        for i in 1..1000 {
            s.apply_local(&put(Kind::NodeTags, &format!("n{i}"), one.clone()))
                .unwrap();
        }
        // "fra-1" plus 999 others fill the collection; a new node is refused, an edit is not.
        assert_eq!(
            s.apply_local(&put(Kind::NodeTags, "one-more", one.clone()))
                .unwrap(),
            Applied::Full(1000)
        );
        assert_eq!(
            s.apply_local(&put(Kind::NodeTags, "fra-1", one)).unwrap(),
            Applied::Done
        );
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
    fn a_dashboard_version_must_be_a_whole_number() {
        let (s, _d) = store();
        let mut d = dash();
        d["version"] = json!(3);
        assert_eq!(
            s.apply_local(&put(Kind::Dashboard, "v", d.clone()))
                .unwrap(),
            Applied::Done
        );
        for bad in [json!("3"), json!(-1), json!(1.5), json!(null)] {
            d["version"] = bad;
            let got = s
                .apply_local(&put(Kind::Dashboard, "v", d.clone()))
                .unwrap();
            assert!(invalid(got).contains("version"), "{d}");
        }
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
