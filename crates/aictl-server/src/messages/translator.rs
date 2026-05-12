//! Cross-provider translator entry point.
//!
//! When the cross-provider mode is enabled, the dispatcher in
//! [`crate::messages`] hands the parsed body off to
//! [`translate_and_dispatch`]. From there:
//!
//! 1. Parse the Anthropic shape into [`ir::AnthropicRequest`].
//! 2. Run the prompt-injection guard on user-role text and the
//!    redactor on every text surface.
//! 3. Apply the feature gate (`strip` / `warn` / `reject`) to drop or
//!    reject Anthropic-only features that the target provider can't
//!    honor (e.g. `cache_control`, `thinking`).
//! 4. Fan out by provider family — OpenAI-compatible (OpenAI, Grok,
//!    Mistral, DeepSeek, Kimi, Z.ai) all share one adapter; Gemini
//!    and Ollama each get a dedicated adapter.
//! 5. Translate the response back into the Anthropic shape. Streaming
//!    bridges provider-native chunks to the Anthropic SSE event
//!    sequence.

use std::sync::Arc;
use std::time::Instant;

use axum::body::Body;
use axum::response::Response;
use serde_json::Value;
use uuid::Uuid;

use aictl_core::audit;
use aictl_core::keys;
use aictl_core::run::Provider;
use aictl_core::security;
use aictl_core::security::redaction::{self, RedactionMode, RedactionPolicy, RedactionResult};
use aictl_core::tools::ToolCall;

use crate::error::ApiError;
use crate::messages::provider_tag;
use crate::openai::key_name_for_provider;
use crate::state::AppState;

pub mod feature_gate;
pub mod gemini;
pub mod ir;
pub mod ollama;
pub mod openai_family;
pub mod stream;

/// Entry point for the cross-provider translator. Returns a fully
/// formed HTTP response (JSON or SSE) the dispatcher hands back to
/// axum.
pub async fn translate_and_dispatch(
    state: Arc<AppState>,
    body: Value,
    model: String,
    provider: Provider,
) -> Result<Response, ApiError> {
    let permit =
        state
            .semaphore
            .clone()
            .try_acquire_owned()
            .map_err(|_| ApiError::ServiceUnavailable {
                reason: "concurrency_cap_reached",
            })?;

    let request_id = short_id();
    let started = Instant::now();

    let mut ir = ir::parse(&body)?;
    apply_injection_guard(&ir)?;
    redact_ir(&mut ir)?;
    let dropped = feature_gate::apply(&mut ir, &provider)?;

    if matches!(provider, Provider::Gguf | Provider::Mlx) {
        return Err(ApiError::BadRequest {
            code: "model_unsupported_for_cross_provider",
            message: "in-process backends (GGUF, MLX) do not expose native tool calling; \
                      use Ollama with a tool-capable model for local + tools"
                .to_string(),
        });
    }

    let stream = ir.stream;
    audit_dispatch(&request_id, &ir, &provider, &dropped);

    let response = match provider {
        Provider::Openai
        | Provider::Grok
        | Provider::Mistral
        | Provider::Deepseek
        | Provider::Kimi
        | Provider::Zai => openai_family::dispatch(&ir, &provider, &request_id, &model).await,
        Provider::Gemini => gemini::dispatch(&ir, &request_id, &model).await,
        Provider::Ollama => ollama::dispatch(&ir, &request_id, &model).await,
        Provider::Anthropic => unreachable!("anthropic handled by passthrough"),
        Provider::Gguf | Provider::Mlx => unreachable!("rejected above"),
        Provider::Mock | Provider::AictlServer => {
            return Err(ApiError::BadRequest {
                code: "provider_unsupported",
                message: format!("provider {provider:?} is not reachable via /v1/messages"),
            });
        }
    }?;

    tracing::info!(
        event = "request_completed",
        request_id = %request_id,
        model = %model,
        provider = provider_tag(&provider),
        elapsed_ms = started.elapsed().as_millis() as u64,
        streamed = stream,
        cross_provider = true,
        dropped_features = ?dropped,
    );

    drop(permit);
    Ok(response)
}

fn short_id() -> String {
    let id = Uuid::new_v4();
    id.simple().to_string()[..16].to_string()
}

/// Resolve the operator-side API key for a target provider. The
/// translator dispatches directly to provider HTTPS endpoints (not
/// through `aictl_core::llm::call_*`, which speaks the engine's
/// internal text-in / text-out abstraction), so the key fetch is the
/// same lookup the engine uses.
pub(crate) fn resolve_api_key(provider: &Provider) -> Result<String, ApiError> {
    let Some(key_name) = key_name_for_provider(provider) else {
        return Ok(String::new());
    };
    keys::get_secret(key_name).ok_or(ApiError::ServiceUnavailable {
        reason: "provider_key_not_configured",
    })
}

fn apply_injection_guard(ir: &ir::AnthropicRequest) -> Result<(), ApiError> {
    let pol = security::policy();
    if !pol.enabled || !pol.injection_guard {
        return Ok(());
    }
    for m in &ir.messages {
        if !matches!(m.role, ir::AnthropicRole::User) {
            continue;
        }
        let text = m.collect_text();
        if text.is_empty() {
            continue;
        }
        if let Err(reason) = security::detect_prompt_injection(&text) {
            return Err(ApiError::BadRequest {
                code: "prompt_injection",
                message: reason,
            });
        }
    }
    Ok(())
}

fn redact_ir(ir: &mut ir::AnthropicRequest) -> Result<(), ApiError> {
    let pol = redaction::policy();
    if matches!(pol.mode, RedactionMode::Off) {
        return Ok(());
    }
    if let Some(sys) = ir.system.as_mut() {
        for s in sys.text_mut() {
            redact_string(s, &pol)?;
        }
    }
    for m in &mut ir.messages {
        for s in m.text_surfaces_mut() {
            redact_string(s, &pol)?;
        }
    }
    Ok(())
}

fn redact_string(s: &mut String, pol: &RedactionPolicy) -> Result<(), ApiError> {
    match redaction::redact(s, pol) {
        RedactionResult::Clean => Ok(()),
        RedactionResult::Redacted { text, .. } => {
            *s = text;
            Ok(())
        }
        RedactionResult::Blocked { .. } => Err(ApiError::BadRequest {
            code: "redaction_blocked",
            message: "sensitive data detected in outbound message".to_string(),
        }),
    }
}

fn audit_dispatch(
    request_id: &str,
    ir: &ir::AnthropicRequest,
    provider: &Provider,
    dropped: &[&'static str],
) {
    let preview = ir
        .messages
        .iter()
        .filter(|m| matches!(m.role, ir::AnthropicRole::User))
        .map(ir::AnthropicMessage::collect_text)
        .collect::<Vec<_>>()
        .join("\n");
    let tool = ToolCall {
        name: format!("gateway:messages:{}", provider_tag(provider)),
        input: preview,
    };
    audit::log_tool(&tool, audit::Outcome::Executed { result: request_id });
    if !dropped.is_empty() {
        let preview = dropped.join(",");
        let evt = ToolCall {
            name: format!("feature_dropped:{}", provider_tag(provider)),
            input: preview,
        };
        audit::log_tool(&evt, audit::Outcome::Executed { result: request_id });
    }
}

/// Standardized helper for emitting JSON responses with the Anthropic
/// content type.
pub(crate) fn json_response(body: &Value) -> Response {
    let bytes = serde_json::to_vec(body).unwrap_or_else(|_| b"{}".to_vec());
    let mut resp = Response::new(Body::from(bytes));
    resp.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("application/json"),
    );
    resp
}

/// Standardized helper for emitting an Anthropic SSE response body.
pub(crate) fn sse_response(body: Body) -> Response {
    let mut resp = Response::new(body);
    resp.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("text/event-stream"),
    );
    resp.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-cache"),
    );
    resp
}
