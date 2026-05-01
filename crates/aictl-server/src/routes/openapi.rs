//! `GET /openapi.json` — auth-free OpenAPI 3.1 discovery document.
//!
//! The spec is shipped as a static JSON file alongside the source
//! (`openapi.json`) and embedded into the binary via `include_str!`.
//! `${VERSION}` placeholders are filled in once with `aictl_core::VERSION`
//! and the result is cached for the life of the process.

use std::sync::OnceLock;

use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};

const TEMPLATE: &str = include_str!("../openapi.json");

fn rendered() -> &'static str {
    static CACHE: OnceLock<String> = OnceLock::new();
    CACHE.get_or_init(|| TEMPLATE.replace("${VERSION}", aictl_core::VERSION))
}

pub async fn spec() -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        rendered(),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The embedded spec must always be valid JSON — otherwise SDK
    /// generators that fetch it will choke. Catch typos at test time
    /// rather than after deploy.
    #[test]
    fn embedded_spec_is_valid_json() {
        let v: serde_json::Value =
            serde_json::from_str(rendered()).expect("openapi.json must be valid JSON");
        assert_eq!(v["openapi"], "3.1.0");
        assert_eq!(v["info"]["version"], aictl_core::VERSION);
        assert!(v["paths"]["/v1/chat/completions"]["post"].is_object());
        assert!(v["paths"]["/openapi.json"]["get"].is_object());
    }

    /// The version placeholder must be substituted, not left as a literal
    /// `${VERSION}` token in the served document.
    #[test]
    fn version_placeholder_is_substituted() {
        assert!(!rendered().contains("${VERSION}"));
    }
}
