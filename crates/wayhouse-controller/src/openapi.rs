//! The OpenAPI description of the controller's HTTP API.
//!
//! Each module that serves routes describes them with `#[utoipa::path]` on its handlers and a
//! small `…Doc` type that lists them; [`openapi`] merges those into one document.
//! `docs/reference/openapi/controller.json` is the checked-in rendering the docs site is built
//! from, and the test `the_checked_in_spec_is_current` fails when it is stale.

use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityRequirement, SecurityScheme};
use utoipa::{Modify, OpenApi};

/// The JSON error shape every route of this API uses.
#[derive(serde::Serialize, utoipa::ToSchema)]
#[allow(dead_code)] // documentation type: the handlers build `{"error": ...}` inline
pub(crate) struct ErrorBody {
    /// What went wrong, in one sentence.
    #[schema(example = "no revision 99")]
    pub error: String,
}

/// The head of the document: title, description and the bearer scheme.
#[derive(OpenApi)]
#[openapi(
    info(
        title = "wayhouse controller API",
        description = "The HTTP API of `wayhouse-controller` (default `:9901`): config revisions, enrollment, \
        plugins, secrets, certificates, feature overrides, the HA cluster and the console's records.",
        license(name = "MIT OR Apache-2.0"),
    ),
    components(schemas(ErrorBody)),
    modifiers(&BearerAuth),
)]
struct ControllerApi;

/// Adds the bearer scheme.
struct BearerAuth;

impl Modify for BearerAuth {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "bearer",
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .description(Some(
                        "`Authorization: Bearer <token>`. The admin token (`--auth-token`) works on every route; \
                         the proxy, agent, UI and origin tokens work on the routes each operation lists. \
                         With no `--auth-token` the API is open.",
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

/// Records on each operation which scoped tokens may call it (`x-wayhouse-tokens`), from the same
/// tables the server enforces.
fn note_scoped_tokens(openapi: &mut utoipa::openapi::OpenApi) {
    use utoipa::openapi::path::{Operation, PathItem};
    type Pick = fn(&mut PathItem) -> Option<&mut Operation>;
    let methods: [(axum::http::Method, Pick); 5] = [
        (axum::http::Method::GET, |i| i.get.as_mut()),
        (axum::http::Method::POST, |i| i.post.as_mut()),
        (axum::http::Method::PUT, |i| i.put.as_mut()),
        (axum::http::Method::PATCH, |i| i.patch.as_mut()),
        (axum::http::Method::DELETE, |i| i.delete.as_mut()),
    ];
    for (path, item) in &mut openapi.paths.paths {
        for (method, pick) in &methods {
            let Some(op) = pick(item) else { continue };
            let tokens = crate::scoped_auth::accepted_by(method, path);
            op.extensions = Some(
                utoipa::openapi::extensions::Extensions::builder()
                    .add("x-wayhouse-tokens", serde_json::json!(tokens))
                    .build(),
            );
        }
    }
}

/// The merged document.
pub fn openapi() -> utoipa::openapi::OpenApi {
    let mut doc = ControllerApi::openapi();
    doc.merge(crate::serve::ServiceDoc::openapi());
    doc.merge(crate::api::ConfigDoc::openapi());
    doc.merge(crate::enroll::EnrollDoc::openapi());
    doc.merge(crate::ha::members::MembersDoc::openapi());
    doc.merge(crate::adopt::AdoptDoc::openapi());
    doc.merge(crate::addresses::api::AddressesDoc::openapi());
    doc.merge(crate::intent::api::IntentApiDoc::openapi());
    doc.merge(crate::registry::RegistriesDoc::openapi());
    doc.merge(crate::origin::PskDoc::openapi());
    doc.merge(crate::plugins::api::PluginsDoc::openapi());
    doc.merge(crate::plugins::routes::PluginRoutesDoc::openapi());
    doc.merge(crate::plugins::hooks::HooksDoc::openapi());
    doc.merge(crate::plugins::declared::PlanDoc::openapi());
    doc.merge(crate::ui_data::api::UiDataDoc::openapi());
    doc.merge(crate::acme::api::AcmeDoc::openapi());
    doc.merge(crate::acme::bundles::BundlesDoc::openapi());
    doc.merge(crate::acme::challenge::ChallengeDoc::openapi());
    note_scoped_tokens(&mut doc);
    doc
}

/// The document as JSON.
pub fn openapi_json() -> serde_json::Value {
    serde_json::to_value(openapi()).expect("the spec serializes")
}

#[cfg(test)]
mod tests {
    /// `docs/reference/openapi/controller.json` is the rendering the docs site is built from.
    /// Regenerate it with `WAYHOUSE_UPDATE_OPENAPI=1 cargo test -p wayhouse-controller openapi`.
    #[test]
    fn the_checked_in_spec_is_current() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/reference/openapi/controller.json");
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
            "{} is out of date; run `WAYHOUSE_UPDATE_OPENAPI=1 cargo test -p wayhouse-controller openapi` and commit it",
            path.display()
        );
    }
}
