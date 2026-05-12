//! Gemini adapter. Translates the Anthropic IR into Google's
//! `generateContent` / `streamGenerateContent` shape and back. Auth is
//! via the `key=` query parameter (the dominant Gemini pattern), not an
//! `Authorization` header.
//!
//! Native shape (request):
//! ```text
//! POST /v1beta/models/{model}:generateContent?key=...
//! {
//!   "systemInstruction": {"parts": [{"text": "..."}]},
//!   "contents": [
//!     {"role": "user", "parts": [{"text": "..."}, {"inlineData": {...}}]},
//!     {"role": "model", "parts": [{"functionCall": {...}}]},
//!     {"role": "user", "parts": [{"functionResponse": {...}}]}
//!   ],
//!   "tools": [{"functionDeclarations": [...]}],
//!   "toolConfig": {"functionCallingConfig": {"mode": "AUTO"}},
//!   "generationConfig": {"temperature": 0.7, "maxOutputTokens": 1024}
//! }
//! ```

use axum::response::Response;
use serde::Deserialize;
use serde_json::{Map, Value};

use aictl_core::config::config_get;

use crate::error::ApiError;
use crate::messages::translator::ir::{
    AnthropicMessage, AnthropicRequest, AnthropicRole, AnthropicTool, AnthropicToolChoice,
    ContentBlock, ImageSource, ToolResultBlock, ToolResultContent,
};
use crate::messages::translator::{json_response, sse_response};

const DEFAULT_BASE: &str = "https://generativelanguage.googleapis.com/v1beta";

pub async fn dispatch(
    ir: &AnthropicRequest,
    request_id: &str,
    model: &str,
) -> Result<Response, ApiError> {
    let api_key = super::resolve_api_key(&aictl_core::run::Provider::Gemini)?;
    let base = config_get("LLM_GEMINI_BASE_URL").unwrap_or_else(|| DEFAULT_BASE.to_string());
    let base = base.trim_end_matches('/');
    let method = if ir.stream {
        "streamGenerateContent"
    } else {
        "generateContent"
    };
    let url = format!("{base}/models/{model}:{method}?key={api_key}");
    let body = build_gemini_request(ir);

    let client = aictl_core::config::http_client();
    let req = client
        .post(&url)
        .header("Content-Type", "application/json")
        .json(&body);

    if ir.stream {
        // Gemini's streamGenerateContent supports SSE when the
        // `alt=sse` query parameter is supplied; otherwise it returns
        // an array of partial JSON objects. We request SSE for a
        // tighter bridge to Anthropic's event sequence.
        let url_sse = format!("{url}&alt=sse");
        let resp = client
            .post(&url_sse)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                tracing::warn!(event = "gemini_dispatch_failed", request_id = %request_id, error = %e);
                ApiError::ServiceUnavailable {
                    reason: "provider_unavailable",
                }
            })?;
        if !resp.status().is_success() {
            return Err(map_upstream_error(resp.status()));
        }
        let body = super::stream::gemini::translate(resp.bytes_stream(), request_id, model);
        Ok(sse_response(body))
    } else {
        let resp = req.send().await.map_err(|e| {
            tracing::warn!(event = "gemini_dispatch_failed", request_id = %request_id, error = %e);
            ApiError::ServiceUnavailable {
                reason: "provider_unavailable",
            }
        })?;
        let status = resp.status();
        let bytes = resp
            .bytes()
            .await
            .map_err(|_| ApiError::ServiceUnavailable {
                reason: "provider_unavailable",
            })?;
        if !status.is_success() {
            tracing::warn!(
                event = "gemini_upstream_error",
                request_id = %request_id,
                status = status.as_u16(),
                body_preview = %String::from_utf8_lossy(&bytes[..bytes.len().min(200)]),
            );
            return Err(map_upstream_error(status));
        }
        let parsed: GeminiResponse =
            serde_json::from_slice(&bytes).map_err(|_| ApiError::ServiceUnavailable {
                reason: "provider_unavailable",
            })?;
        Ok(json_response(&gemini_to_anthropic(
            &parsed, request_id, model,
        )))
    }
}

fn map_upstream_error(status: axum::http::StatusCode) -> ApiError {
    if status == axum::http::StatusCode::UNAUTHORIZED || status == axum::http::StatusCode::FORBIDDEN
    {
        ApiError::Forbidden {
            reason: "provider_auth_failed",
        }
    } else if status == axum::http::StatusCode::TOO_MANY_REQUESTS {
        ApiError::TooManyRequests
    } else {
        ApiError::ServiceUnavailable {
            reason: "provider_unavailable",
        }
    }
}

// --- Request translation ----------------------------------------------------

pub(crate) fn build_gemini_request(ir: &AnthropicRequest) -> Value {
    let mut obj = Map::new();

    if let Some(sys) = &ir.system {
        obj.insert(
            "systemInstruction".to_string(),
            serde_json::json!({"parts": [{"text": sys.collect_text()}]}),
        );
    }

    let contents: Vec<Value> = ir.messages.iter().map(translate_message).collect();
    obj.insert("contents".to_string(), Value::Array(contents));

    let mut generation = Map::new();
    generation.insert("maxOutputTokens".to_string(), Value::from(ir.max_tokens));
    if let Some(t) = ir.temperature {
        generation.insert("temperature".to_string(), Value::from(t));
    }
    if let Some(p) = ir.top_p {
        generation.insert("topP".to_string(), Value::from(p));
    }
    if let Some(k) = ir.top_k {
        generation.insert("topK".to_string(), Value::from(k));
    }
    if let Some(stop) = &ir.stop_sequences {
        generation.insert(
            "stopSequences".to_string(),
            Value::Array(stop.iter().map(|s| Value::String(s.clone())).collect()),
        );
    }
    obj.insert("generationConfig".to_string(), Value::Object(generation));

    if let Some(tools) = &ir.tools {
        let decls: Vec<Value> = tools.iter().map(translate_tool_declaration).collect();
        obj.insert(
            "tools".to_string(),
            serde_json::json!([{"functionDeclarations": decls}]),
        );
        if let Some(choice) = &ir.tool_choice {
            obj.insert("toolConfig".to_string(), translate_tool_choice(choice));
        }
    }

    Value::Object(obj)
}

fn translate_message(m: &AnthropicMessage) -> Value {
    let role = match m.role {
        AnthropicRole::User => "user",
        AnthropicRole::Assistant => "model",
    };
    let mut parts: Vec<Value> = Vec::new();
    for b in &m.content {
        match b {
            ContentBlock::Text { text, .. } => {
                if !text.is_empty() {
                    parts.push(serde_json::json!({"text": text}));
                }
            }
            ContentBlock::Image { source } => {
                if let Some(p) = image_to_inline(source) {
                    parts.push(p);
                }
            }
            ContentBlock::ToolUse { name, input, .. } => {
                parts.push(serde_json::json!({
                    "functionCall": {
                        "name": name,
                        "args": input,
                    },
                }));
            }
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                // Gemini's `functionResponse.name` field names the
                // function, not the call ID. Anthropic's tool_use_id is
                // an opaque correlation id — we fold it into a wrapper
                // object so downstream Anthropic clients keying off the
                // id continue to work after translation.
                let body = tool_result_value(content, *is_error);
                parts.push(serde_json::json!({
                    "functionResponse": {
                        "name": tool_use_id,
                        "response": {"content": body},
                    },
                }));
            }
            ContentBlock::Document => {}
        }
    }
    serde_json::json!({"role": role, "parts": parts})
}

fn image_to_inline(source: &ImageSource) -> Option<Value> {
    match source {
        ImageSource::Base64 { media_type, data } => Some(serde_json::json!({
            "inlineData": {"mimeType": media_type, "data": data},
        })),
        ImageSource::Url { .. } => None, // feature_gate rejects this upstream
    }
}

fn tool_result_value(content: &ToolResultContent, is_error: bool) -> Value {
    let text = match content {
        ToolResultContent::Text(s) => s.clone(),
        ToolResultContent::Blocks(blocks) => blocks
            .iter()
            .filter_map(|b| match b {
                ToolResultBlock::Text(t) => Some(t.as_str()),
                ToolResultBlock::Image(_) => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
    };
    if is_error {
        serde_json::json!({"error": text})
    } else {
        serde_json::json!({"result": text})
    }
}

fn translate_tool_declaration(t: &AnthropicTool) -> Value {
    let mut obj = Map::new();
    obj.insert("name".to_string(), Value::String(t.name.clone()));
    if let Some(d) = &t.description {
        obj.insert("description".to_string(), Value::String(d.clone()));
    }
    obj.insert("parameters".to_string(), t.input_schema.clone());
    Value::Object(obj)
}

fn translate_tool_choice(choice: &AnthropicToolChoice) -> Value {
    match choice {
        AnthropicToolChoice::Auto => serde_json::json!({
            "functionCallingConfig": {"mode": "AUTO"},
        }),
        AnthropicToolChoice::Any => serde_json::json!({
            "functionCallingConfig": {"mode": "ANY"},
        }),
        AnthropicToolChoice::None => serde_json::json!({
            "functionCallingConfig": {"mode": "NONE"},
        }),
        AnthropicToolChoice::Tool { name } => serde_json::json!({
            "functionCallingConfig": {"mode": "ANY", "allowedFunctionNames": [name]},
        }),
    }
}

// --- Response shape ---------------------------------------------------------

#[derive(Debug, Deserialize)]
pub(crate) struct GeminiResponse {
    #[serde(default)]
    pub candidates: Vec<GeminiCandidate>,
    #[serde(default)]
    #[serde(rename = "usageMetadata")]
    pub usage_metadata: Option<GeminiUsage>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct GeminiCandidate {
    #[serde(default)]
    pub content: GeminiContent,
    #[serde(default)]
    #[serde(rename = "finishReason")]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct GeminiContent {
    #[serde(default)]
    pub parts: Vec<Value>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct GeminiUsage {
    #[serde(default)]
    #[serde(rename = "promptTokenCount")]
    pub prompt_token_count: u64,
    #[serde(default)]
    #[serde(rename = "candidatesTokenCount")]
    pub candidates_token_count: u64,
    #[serde(default)]
    #[serde(rename = "cachedContentTokenCount")]
    pub cached_content_token_count: u64,
}

pub(crate) fn gemini_to_anthropic(resp: &GeminiResponse, request_id: &str, model: &str) -> Value {
    let mut content: Vec<Value> = Vec::new();
    let mut stop_reason = "end_turn";

    if let Some(cand) = resp.candidates.first() {
        for part in &cand.content.parts {
            if let Some(text) = part.get("text").and_then(Value::as_str)
                && !text.is_empty()
            {
                content.push(serde_json::json!({"type":"text","text": text}));
            }
            if let Some(call) = part.get("functionCall") {
                let name = call.get("name").and_then(Value::as_str).unwrap_or_default();
                let input = call
                    .get("args")
                    .cloned()
                    .unwrap_or(Value::Object(Map::new()));
                content.push(serde_json::json!({
                    "type": "tool_use",
                    "id": format!("call_{request_id}_{}", content.len()),
                    "name": name,
                    "input": input,
                }));
                stop_reason = "tool_use";
            }
        }
        stop_reason = match cand.finish_reason.as_deref() {
            Some("MAX_TOKENS") => "max_tokens",
            Some("STOP") | None => {
                if stop_reason == "tool_use" {
                    "tool_use"
                } else {
                    "end_turn"
                }
            }
            _ => stop_reason,
        };
    }

    if content.is_empty() {
        content.push(serde_json::json!({"type":"text","text":""}));
    }

    let (input_tokens, output_tokens, cache_read) =
        resp.usage_metadata.as_ref().map_or((0, 0, 0), |u| {
            let fresh = u
                .prompt_token_count
                .saturating_sub(u.cached_content_token_count);
            (
                fresh,
                u.candidates_token_count,
                u.cached_content_token_count,
            )
        });

    let mut usage = Map::new();
    usage.insert("input_tokens".to_string(), Value::from(input_tokens));
    usage.insert("output_tokens".to_string(), Value::from(output_tokens));
    usage.insert("cache_creation_input_tokens".to_string(), Value::from(0));
    usage.insert(
        "cache_read_input_tokens".to_string(),
        Value::from(cache_read),
    );

    serde_json::json!({
        "id": format!("msg_{request_id}"),
        "type": "message",
        "role": "assistant",
        "model": model,
        "content": content,
        "stop_reason": stop_reason,
        "stop_sequence": Value::Null,
        "usage": Value::Object(usage),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::translator::ir;
    use serde_json::json;

    fn ir_from(v: Value) -> AnthropicRequest {
        ir::parse(&v).unwrap()
    }

    #[test]
    fn request_carries_system_instruction() {
        let ir = ir_from(json!({
            "model": "gemini-2.0-flash",
            "max_tokens": 100,
            "system": "be brief",
            "messages": [{"role": "user", "content": "hi"}],
        }));
        let r = build_gemini_request(&ir);
        assert_eq!(
            r.pointer("/systemInstruction/parts/0/text").unwrap(),
            "be brief"
        );
        let contents = r.get("contents").unwrap().as_array().unwrap();
        assert_eq!(contents[0].get("role").unwrap(), "user");
        assert_eq!(contents[0].pointer("/parts/0/text").unwrap(), "hi");
    }

    #[test]
    fn request_translates_assistant_role_to_model() {
        let ir = ir_from(json!({
            "model": "gemini-2.0-flash",
            "max_tokens": 100,
            "messages": [
                {"role": "user", "content": "hi"},
                {"role": "assistant", "content": "hello"},
            ],
        }));
        let r = build_gemini_request(&ir);
        let contents = r.get("contents").unwrap().as_array().unwrap();
        assert_eq!(contents[1].get("role").unwrap(), "model");
    }

    #[test]
    fn response_translates_function_call() {
        let resp: GeminiResponse = serde_json::from_value(json!({
            "candidates": [{
                "content": {
                    "parts": [
                        {"functionCall": {"name": "calc", "args": {"x": 1}}},
                    ],
                },
                "finishReason": "STOP",
            }],
            "usageMetadata": {"promptTokenCount": 10, "candidatesTokenCount": 5},
        }))
        .unwrap();
        let v = gemini_to_anthropic(&resp, "req1", "gemini-2.0-flash");
        let content = v.get("content").unwrap().as_array().unwrap();
        let tool = content
            .iter()
            .find(|b| b.get("type").and_then(Value::as_str) == Some("tool_use"))
            .unwrap();
        assert_eq!(tool.get("name").unwrap(), "calc");
        assert_eq!(v.get("stop_reason").unwrap(), "tool_use");
    }
}
