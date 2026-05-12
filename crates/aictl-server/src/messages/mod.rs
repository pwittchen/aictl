//! `/v1/messages` — Anthropic Messages API gateway.
//!
//! Two modes:
//!
//! - **Passthrough** (default): when the resolved model is Anthropic,
//!   the request body is forwarded verbatim to `api.anthropic.com`.
//!   Tool use, content blocks, prompt caching, `anthropic-beta`
//!   features, and the native SSE event sequence all survive
//!   byte-for-byte. Code lives in [`passthrough`].
//!
//! - **Cross-provider translator** (planned, off by default): when the
//!   resolved model is non-Anthropic *and*
//!   `AICTL_SERVER_MESSAGES_CROSS_PROVIDER=true`, the request is parsed
//!   into an Anthropic IR, translated into each provider's native shape,
//!   dispatched directly to that provider, and the response is
//!   translated back into the Anthropic shape. Streaming bridges the
//!   provider's event sequence to the Anthropic SSE event sequence.
//!   Code lives in [`translator`].
//!
//! When the cross-provider mode is off (the default) and the model is
//! non-Anthropic, the handler returns `400 model_not_found` to preserve
//! today's behavior for operators who have not opted in.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use serde_json::Value;

use aictl_core::config::config_get;
use aictl_core::run::Provider;

use crate::error::ApiError;
use crate::openai::resolve_provider;
use crate::state::AppState;

pub mod passthrough;
pub mod translator;

/// Master switch for the cross-provider translator. Default `false` so
/// existing operators see no behavior change until they opt in.
#[must_use]
pub fn cross_provider_enabled() -> bool {
    matches!(
        config_get("AICTL_SERVER_MESSAGES_CROSS_PROVIDER").as_deref(),
        Some("true" | "1")
    )
}

/// Allow-list of providers reachable through the cross-provider mode.
/// `*` (the default) means "any non-Anthropic provider".
#[must_use]
pub fn allowed_providers() -> Option<Vec<String>> {
    let raw = config_get("AICTL_SERVER_MESSAGES_TRANSLATE_PROVIDERS")?;
    if raw.trim() == "*" || raw.trim().is_empty() {
        return None;
    }
    Some(
        raw.split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_ascii_lowercase)
            .collect(),
    )
}

/// `POST /v1/messages` — top-level dispatcher.
pub async fn messages(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Response, ApiError> {
    let model = body
        .get("model")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::BadRequest {
            code: "body_malformed",
            message: "missing or non-string 'model' field".to_string(),
        })?
        .to_string();

    let provider = resolve_provider(&model).await?;

    if matches!(provider, Provider::Anthropic) {
        return passthrough::forward(state, headers, body, model).await;
    }

    if !cross_provider_enabled() {
        return Err(ApiError::BadRequest {
            code: "model_not_found",
            message: format!(
                "model {model:?} does not resolve to an Anthropic provider; \
                 set AICTL_SERVER_MESSAGES_CROSS_PROVIDER=true to enable the \
                 cross-provider translator"
            ),
        });
    }

    if let Some(allow) = allowed_providers() {
        let tag = provider_tag(&provider);
        if !allow.iter().any(|t| t == tag) {
            return Err(ApiError::BadRequest {
                code: "provider_not_allowed",
                message: format!(
                    "provider {tag:?} is not in \
                     AICTL_SERVER_MESSAGES_TRANSLATE_PROVIDERS"
                ),
            });
        }
    }

    translator::translate_and_dispatch(state, body, model, provider).await
}

pub(crate) fn provider_tag(p: &Provider) -> &'static str {
    match p {
        Provider::Openai => "openai",
        Provider::Anthropic => "anthropic",
        Provider::Gemini => "gemini",
        Provider::Grok => "grok",
        Provider::Mistral => "mistral",
        Provider::Deepseek => "deepseek",
        Provider::Kimi => "kimi",
        Provider::Zai => "zai",
        Provider::Ollama => "ollama",
        Provider::Gguf => "gguf",
        Provider::Mlx => "mlx",
        Provider::Mock => "mock",
        Provider::AictlServer => "aictl-server",
    }
}
