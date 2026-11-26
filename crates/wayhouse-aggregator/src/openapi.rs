//! The OpenAPI description of the aggregator's HTTP API.
//!
//! Each module that serves routes describes them with `#[utoipa::path]` on its handlers and a
//! small `…Doc` type that lists them; [`openapi`] merges those into one document.
//! `docs/reference/openapi/aggregator.json` is the checked-in rendering the docs site is built
//! from, and the test `the_checked_in_spec_is_current` fails when it is stale.

use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityRequirement, SecurityScheme};
use utoipa::{Modify, OpenApi};

/// The JSON error shape the aggregator's routes use.
#[derive(serde::Serialize, utoipa::ToSchema)]
#[allow(dead_code)] // documentation type: the handlers build `{"error": ...}` inline
pub(crate) struct ErrorBody {
    /// What went wrong, in one sentence.
    #[schema(example = "range must be 5m, 15m or 1h")]
    pub error: String,
}

/// The head of the document: title, description and the bearer schemes.
#[derive(OpenApi)]
#[openapi(
    info(
        title = "wayhouse aggregator API",
        description = "The HTTP API of `wayhouse-aggregator` (default `:9902`): the push endpoint the proxies report to, \
        the fleet read views, and the verbs that fan out to every instance's own admin API.",
        license(name = "MIT OR Apache-2.0"),
    ),
    components(schemas(ErrorBody)),
    modifiers(&Schemes),
)]
struct AggregatorApi;

/// Adds the bearer schemes.
struct Schemes;

impl Modify for Schemes {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "bearer",
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .description(Some(
                        "`Authorization: Bearer <token>` with the token of `--auth-token`. With no `--auth-token` the \
                         API is open, which is only accepted on a loopback listener. A missing or wrong token is \
                         answered `401`.",
                    ))
                    .build(),
            ),
        );
        components.add_security_scheme(
            "ingest",
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .description(Some(
                        "`Authorization: Bearer <token>` with the token of `--ingest-token`. It is a different secret \
                         from `--auth-token`, so the proxies that push telemetry cannot read or drive the fleet.",
                    ))
                    .build(),
            ),
        );
        openapi.security = Some(vec![SecurityRequirement::new(
            "bearer",
            Vec::<String>::new(),
        )]);
    }
}

/// The whole document.
pub fn openapi() -> utoipa::openapi::OpenApi {
    let mut doc = AggregatorApi::openapi();
    doc.merge(crate::api::ApiDoc::openapi());
    doc.merge(crate::fanout::FanoutDoc::openapi());
    doc.merge(crate::serve::ServiceDoc::openapi());
    doc
}

/// The document as JSON.
pub fn openapi_json() -> serde_json::Value {
    serde_json::to_value(openapi()).expect("the spec serializes")
}

#[cfg(test)]
mod tests {
    /// `docs/reference/openapi/aggregator.json` is the rendering the docs site is built from.
    /// Regenerate it with `WAYHOUSE_UPDATE_OPENAPI=1 cargo test -p wayhouse-aggregator openapi`.
    #[test]
    fn the_checked_in_spec_is_current() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/reference/openapi/aggregator.json");
        let mut want = serde_json::to_string_pretty(&super::openapi_json()).unwrap();
        want.push('\n');
        if std::env::var_os("WAYHOUSE_UPDATE_OPENAPI").is_some() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &want).unwrap();
            return;
        }
        let have = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(
            have == want,
            "{} is out of date; run `WAYHOUSE_UPDATE_OPENAPI=1 cargo test -p wayhouse-aggregator openapi` and commit it",
            path.display()
        );
    }
}
