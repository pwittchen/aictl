//! `POST /v1/messages` — Anthropic Messages API gateway.
//!
//! Exists so clients that speak the native Anthropic shape (notably
//! Claude Code via `ANTHROPIC_BASE_URL`) can route through the server
//! without losing tool use, content blocks, prompt caching, or any
//! other Anthropic-specific feature.
//!
//! Phase 1 is a pure passthrough proxy: we authenticate the client
//! with the server's master key, run the prompt-injection guard and
//! redactor on user-text content, then forward the body to
//! `https://api.anthropic.com/v1/messages` with the operator's stored
//! key. The model **must** resolve to `Provider::Anthropic` — cross-
//! provider translation of the Anthropic shape (with its content
//! blocks, tool_use, and tool_result) is intentionally out of scope.
//!
//! Streaming responses are piped through as raw SSE bytes; no
//! reframing happens here. The CLI's `/v1/chat/completions` SSE shape
//! is a separate code path in `routes::gateway`.

use std::sync::Arc;
use std::time::Instant;

use axum::Json;
use axum::body::Body;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use futures::StreamExt;
use serde_json::Value;
use uuid::Uuid;

use aictl_core::audit;
use aictl_core::keys;
use aictl_core::run::Provider;
use aictl_core::security;
use aictl_core::security::redaction::{self, RedactionMode, RedactionPolicy, RedactionResult};
use aictl_core::tools::ToolCall;

use crate::error::ApiError;
use crate::openai::{key_name_for_provider, resolve_provider};
use crate::state::AppState;

const ANTHROPIC_MESSAGES_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION_DEFAULT: &str = "2023-06-01";

/// `POST /v1/messages` — passthrough proxy to api.anthropic.com.
#[allow(clippy::too_many_lines)]
pub async fn messages(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(mut body): Json<Value>,
) -> Result<Response, ApiError> {
    let permit = acquire_permit(&state)?;
    let request_id = short_id();

    let model = body
        .get("model")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::BadRequest {
            code: "body_malformed",
            message: "missing or non-string 'model' field".to_string(),
        })?
        .to_string();

    let provider = resolve_provider(&model).await?;
    if !matches!(provider, Provider::Anthropic) {
        return Err(ApiError::BadRequest {
            code: "model_not_found",
            message: format!(
                "model {model:?} does not resolve to an Anthropic provider; \
                 /v1/messages currently only proxies to Anthropic models"
            ),
        });
    }

    let api_key = resolve_api_key(&provider)?;
    apply_injection_guard(&body)?;
    redact_body(&mut body)?;

    let stream = body.get("stream").and_then(Value::as_bool).unwrap_or(false);

    let anthropic_version = headers
        .get("anthropic-version")
        .and_then(|v| v.to_str().ok())
        .unwrap_or(ANTHROPIC_VERSION_DEFAULT)
        .to_string();

    let beta = headers
        .get("anthropic-beta")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    let started = Instant::now();
    let client = aictl_core::config::http_client();
    let mut req_builder = client
        .post(ANTHROPIC_MESSAGES_URL)
        .header("x-api-key", &api_key)
        .header("anthropic-version", &anthropic_version)
        .header("content-type", "application/json");
    if let Some(b) = beta {
        req_builder = req_builder.header("anthropic-beta", b);
    }

    let upstream = req_builder.json(&body).send().await.map_err(|e| {
        tracing::warn!(event = "anthropic_dispatch_failed", request_id = %request_id, error = %e);
        ApiError::ServiceUnavailable {
            reason: "provider_unavailable",
        }
    })?;

    let status = upstream.status();
    audit_dispatch(&request_id, &body);

    if !status.is_success() {
        let bytes = upstream.bytes().await.unwrap_or_default();
        tracing::warn!(
            event = "anthropic_upstream_error",
            request_id = %request_id,
            status = status.as_u16(),
        );
        if status == axum::http::StatusCode::UNAUTHORIZED
            || status == axum::http::StatusCode::FORBIDDEN
        {
            drop(permit);
            return Err(ApiError::Forbidden {
                reason: "provider_auth_failed",
            });
        }
        let mut resp = Response::new(Body::from(bytes));
        *resp.status_mut() = axum::http::StatusCode::from_u16(status.as_u16()).unwrap_or(status);
        resp.headers_mut().insert(
            axum::http::header::CONTENT_TYPE,
            axum::http::HeaderValue::from_static("application/json"),
        );
        drop(permit);
        return Ok(resp);
    }

    if stream {
        let upstream_stream = upstream.bytes_stream();
        let request_id_for_log = request_id.clone();
        let model_for_log = model.clone();
        let proxied = async_stream::stream! {
            let mut s = upstream_stream;
            while let Some(chunk) = s.next().await {
                yield chunk.map_err(std::io::Error::other);
            }
            tracing::info!(
                event = "request_completed",
                request_id = %request_id_for_log,
                model = %model_for_log,
                elapsed_ms = started.elapsed().as_millis() as u64,
                streamed = true,
            );
            drop(permit);
        };
        let mut resp = Response::new(Body::from_stream(proxied));
        resp.headers_mut().insert(
            axum::http::header::CONTENT_TYPE,
            axum::http::HeaderValue::from_static("text/event-stream"),
        );
        resp.headers_mut().insert(
            axum::http::header::CACHE_CONTROL,
            axum::http::HeaderValue::from_static("no-cache"),
        );
        Ok(resp)
    } else {
        let bytes = upstream.bytes().await.map_err(|e| {
            tracing::warn!(event = "anthropic_read_failed", request_id = %request_id, error = %e);
            ApiError::ServiceUnavailable {
                reason: "provider_unavailable",
            }
        })?;
        tracing::info!(
            event = "request_completed",
            request_id = %request_id,
            model = %model,
            elapsed_ms = started.elapsed().as_millis() as u64,
            streamed = false,
        );
        drop(permit);
        let mut resp = Response::new(Body::from(bytes));
        resp.headers_mut().insert(
            axum::http::header::CONTENT_TYPE,
            axum::http::HeaderValue::from_static("application/json"),
        );
        Ok(resp)
    }
}

// --- Internals ---------------------------------------------------------------

fn acquire_permit(state: &Arc<AppState>) -> Result<tokio::sync::OwnedSemaphorePermit, ApiError> {
    state
        .semaphore
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError::ServiceUnavailable {
            reason: "concurrency_cap_reached",
        })
}

fn short_id() -> String {
    let id = Uuid::new_v4();
    id.simple().to_string()[..16].to_string()
}

fn resolve_api_key(provider: &Provider) -> Result<String, ApiError> {
    let Some(key_name) = key_name_for_provider(provider) else {
        return Ok(String::new());
    };
    keys::get_secret(key_name).ok_or(ApiError::ServiceUnavailable {
        reason: "provider_key_not_configured",
    })
}

fn apply_injection_guard(body: &Value) -> Result<(), ApiError> {
    let pol = security::policy();
    if !pol.enabled || !pol.injection_guard {
        return Ok(());
    }
    let Some(messages) = body.get("messages").and_then(Value::as_array) else {
        return Ok(());
    };
    for m in messages {
        if m.get("role").and_then(Value::as_str) != Some("user") {
            continue;
        }
        let text = collect_text(m.get("content"));
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

fn collect_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(parts)) => {
            let mut out = String::new();
            for part in parts {
                if part.get("type").and_then(Value::as_str) == Some("text")
                    && let Some(t) = part.get("text").and_then(Value::as_str)
                {
                    if !out.is_empty() {
                        out.push('\n');
                    }
                    out.push_str(t);
                }
            }
            out
        }
        _ => String::new(),
    }
}

/// Walk the Anthropic-shaped body and run the redactor on every text
/// surface a client could leak secrets through: `system` (string or
/// content-block array) and each entry in `messages[*].content` (same
/// shape). Tool inputs and tool results are deliberately left alone —
/// the Anthropic schema lets them carry arbitrary JSON whose semantic
/// meaning is opaque to a regex pass, and rewriting them risks
/// corrupting valid tool dialogs.
fn redact_body(body: &mut Value) -> Result<(), ApiError> {
    let pol = redaction::policy();
    if matches!(pol.mode, RedactionMode::Off) {
        return Ok(());
    }

    if let Some(sys) = body.get_mut("system") {
        redact_text_surface(sys, &pol)?;
    }
    if let Some(messages) = body.get_mut("messages").and_then(Value::as_array_mut) {
        for m in messages.iter_mut() {
            if let Some(content) = m.get_mut("content") {
                redact_text_surface(content, &pol)?;
            }
        }
    }
    Ok(())
}

fn redact_text_surface(v: &mut Value, pol: &RedactionPolicy) -> Result<(), ApiError> {
    match v {
        Value::String(s) => redact_string(s, pol),
        Value::Array(parts) => {
            for part in parts.iter_mut() {
                if part.get("type").and_then(Value::as_str) != Some("text") {
                    continue;
                }
                if let Some(Value::String(t)) = part.get_mut("text") {
                    redact_string(t, pol)?;
                }
            }
            Ok(())
        }
        _ => Ok(()),
    }
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

fn audit_dispatch(request_id: &str, body: &Value) {
    let preview = body
        .get("messages")
        .and_then(Value::as_array)
        .map(|msgs| {
            msgs.iter()
                .filter(|m| m.get("role").and_then(Value::as_str) == Some("user"))
                .map(|m| collect_text(m.get("content")))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    let tool = ToolCall {
        name: "gateway:anthropic".to_string(),
        input: preview,
    };
    audit::log_tool(&tool, audit::Outcome::Executed { result: request_id });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn collect_text_handles_string_and_array() {
        assert_eq!(collect_text(Some(&json!("hello"))), "hello");
        let arr = json!([
            {"type": "text", "text": "part one"},
            {"type": "image", "source": {}},
            {"type": "text", "text": "part two"},
        ]);
        assert_eq!(collect_text(Some(&arr)), "part one\npart two");
        assert_eq!(collect_text(None), "");
    }

    fn redact_mode_policy() -> RedactionPolicy {
        RedactionPolicy {
            mode: RedactionMode::Redact,
            skip_local: true,
            enabled_detectors: vec![],
            extra_patterns: vec![],
            allowlist: vec![],
            ner_requested: false,
        }
    }

    #[test]
    fn redact_text_surface_rewrites_strings_with_redact_mode() {
        let pol = redact_mode_policy();
        let mut v = json!("contact alice@example.com please");
        redact_text_surface(&mut v, &pol).unwrap();
        let s = v.as_str().unwrap();
        assert!(s.contains("[REDACTED:"), "expected redaction marker: {s}");
    }

    #[test]
    fn redact_text_surface_skips_non_text_blocks() {
        let pol = redact_mode_policy();
        let mut v = json!([
            {"type": "text", "text": "email me at alice@example.com"},
            {"type": "tool_use", "id": "t1", "name": "calc", "input": {"x": 1}},
        ]);
        redact_text_surface(&mut v, &pol).unwrap();
        let arr = v.as_array().unwrap();
        let first_text = arr[0].get("text").unwrap().as_str().unwrap();
        assert!(first_text.contains("[REDACTED:"));
        // Tool block left untouched.
        assert_eq!(arr[1].get("name").unwrap().as_str().unwrap(), "calc");
    }

    #[test]
    fn audit_preview_includes_user_text_only() {
        let body = json!({
            "messages": [
                {"role": "user", "content": "u1"},
                {"role": "assistant", "content": "a1"},
                {"role": "user", "content": [
                    {"type": "text", "text": "u2-part-a"},
                    {"type": "text", "text": "u2-part-b"},
                ]},
            ]
        });
        let preview = body
            .get("messages")
            .and_then(Value::as_array)
            .map(|msgs| {
                msgs.iter()
                    .filter(|m| m.get("role").and_then(Value::as_str) == Some("user"))
                    .map(|m| collect_text(m.get("content")))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        assert_eq!(preview, "u1\nu2-part-a\nu2-part-b");
    }
}
