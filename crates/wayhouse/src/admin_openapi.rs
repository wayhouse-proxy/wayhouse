//! Schema types for the admin API's OpenAPI description (`admin::AdminApi`).
//!
//! Request bodies are the handlers' own types. The two JSON responses whose Rust types live in
//! other crates (`GET /features`, `GET /web`) are described here by mirror types: they exist only
//! to be documented, and the tests at the bottom serialize the real values and compare the keys,
//! so a field added to one side without the other fails the build.

use serde::Serialize;
use utoipa::ToSchema;

/// One feature flag, as `GET /features` lists it.
#[derive(Serialize, ToSchema)]
#[allow(dead_code)] // documentation type, never constructed
pub struct FeatureRow {
    /// The flag's name, for example `admin_write.backends`.
    #[schema(example = "admin_write.backends")]
    pub name: String,
    /// The value in force now.
    pub value: bool,
    /// Where `value` comes from: the built-in `default`, the `features:` section of the config
    /// (`config`), a controller `override`, or the `build` (the binary was built without the code
    /// the flag switches).
    #[schema(example = "default")]
    pub source: FeatureSource,
    /// The built-in value.
    pub default: bool,
    /// Which program reads the flag.
    #[schema(example = "proxy")]
    pub scope: FeatureScope,
    /// When a change takes effect: `live` or only at the next `startup`.
    #[schema(example = "live")]
    pub apply: FeatureApply,
    /// Which way a controller override may flip the flag. `off_only` flags open new network
    /// exposure, so a stolen token cannot switch them on.
    #[serde(rename = "override")]
    #[schema(rename = "override", example = "both")]
    pub override_mode: FeatureOverride,
    /// Turning the flag off removes a protection (rate limits, ACLs), so an override of it needs an expiry.
    pub protection: bool,
    /// Whether the runtime consults the flag yet. An unwired flag is accepted and reported but changes nothing.
    pub wired: bool,
    /// Whether this build contains the code the flag switches.
    pub available: bool,
    /// The config `schema_version` that introduced the flag.
    pub since: u32,
    /// What turning the flag off costs, in one sentence.
    pub risk: String,
    /// What happens to connections that are already open.
    pub impact: String,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
pub enum FeatureSource {
    Default,
    Config,
    Override,
    Build,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
pub enum FeatureScope {
    Proxy,
    Controller,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
pub enum FeatureApply {
    Live,
    Startup,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
pub enum FeatureOverride {
    Both,
    OffOnly,
}

/// The envelope `GET /features` answers.
#[derive(Serialize, ToSchema)]
#[allow(dead_code)]
pub struct FeaturesReply {
    pub flags: Vec<FeatureRow>,
}

/// The resolved `web:` section, as `wayhouse-web-sync` reads it.
#[derive(Serialize, ToSchema)]
#[allow(dead_code)]
pub struct WebReply {
    /// The pool the entry node's :80/:443 listeners send to (Caddy's).
    #[schema(example = "web")]
    pub pool: String,
    /// Where Caddy listens.
    pub listen: WebListen,
    /// CIDRs Caddy trusts PROXY headers from, besides loopback.
    #[schema(example = json!(["10.1.0.2/32"]))]
    pub trusted_proxy: Vec<String>,
    /// Loopback address of the generated `via: wayhouse` listeners.
    #[schema(example = "127.0.0.1")]
    pub internal_bind: String,
    pub sites: Vec<WebSite>,
}

#[derive(Serialize, ToSchema)]
#[allow(dead_code)]
pub struct WebListen {
    #[schema(example = "127.0.0.1:8080")]
    pub http: String,
    #[schema(example = "127.0.0.1:8443")]
    pub https: String,
}

#[derive(Serialize, ToSchema)]
#[allow(dead_code)]
pub struct WebSite {
    #[schema(example = json!(["panel.example.com"]))]
    pub hosts: Vec<String>,
    /// Each upstream is `{"addr": "ip:port"}`, or `{"pool": name, "via": "direct"|"wayhouse"}`
    /// (`via: wayhouse` also carries `local`, the generated loopback listener).
    #[schema(example = json!([{"addr": "10.0.0.9:8080"}]))]
    pub upstreams: Vec<serde_json::Value>,
    /// The certificate and key files of the site, when it has them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tls: Option<WebSiteTls>,
    /// `"verify"`, `"skip"` or `{"ca_file": path}`; absent means plain HTTP to the upstream.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(example = "skip")]
    pub upstream_tls: Option<serde_json::Value>,
    /// Path for the active health check, when set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub health_path: Option<String>,
}

#[derive(Serialize, ToSchema)]
#[allow(dead_code)]
pub struct WebSiteTls {
    pub cert: String,
    pub key: String,
}

/// What `POST /config/validate` answers on success.
#[derive(Serialize, ToSchema)]
#[allow(dead_code)]
pub struct ValidateOk {
    /// Always `true`: a failure is a `422` with an `ErrorBody` instead.
    pub ok: bool,
    /// The `schema_version` of the document that was checked.
    #[schema(example = 4)]
    pub schema_version: u32,
}

/// The JSON error shape the config routes use.
#[derive(Serialize, ToSchema)]
#[allow(dead_code)]
pub struct ErrorBody {
    /// What went wrong, in one sentence.
    #[schema(example = "the config file changed since you read it")]
    pub error: String,
    /// A stable code for the cases a client branches on; today only `stale`.
    #[schema(example = "stale")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

/// What `PUT /config/file` answers once the file is written.
#[derive(Serialize, ToSchema)]
#[allow(dead_code)]
pub struct ConfigSaved {
    /// Always `true` on a `200`.
    pub ok: bool,
    /// The file was written. It is the file that changed, whether or not the reload took it (`applied`).
    pub saved: bool,
    /// Whether the reload took the new file within the wait (5 s).
    pub applied: bool,
    /// The ETag of the file as saved; send it as `If-Match` on the next edit.
    #[schema(example = "\"fdee278e74f64af6b5db0b9baf6dd2bd865cded805a982bc8af39270b16d4166\"")]
    pub etag: String,
    /// The revision counter of the file as saved; the `base` of the next edit.
    #[schema(example = 279199307691254_u64)]
    pub revision: u64,
    /// Present when `applied` is false: what that does and does not mean.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    fn keys(v: &serde_json::Value) -> BTreeSet<String> {
        v.as_object().unwrap().keys().cloned().collect()
    }

    /// The mirror of `FeatureView` lists exactly the keys the real rows serve.
    #[test]
    fn the_feature_row_mirror_matches_the_real_row() {
        let rows = crate::features::effective(&std::collections::BTreeMap::new());
        let real = serde_json::to_value(&rows[0]).unwrap();
        let doc = crate::admin::openapi_json();
        let props = &doc["components"]["schemas"]["FeatureRow"]["properties"];
        assert_eq!(
            keys(props),
            keys(&real),
            "FeatureRow drifted from FeatureView"
        );
    }
}
