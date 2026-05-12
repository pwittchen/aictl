//! Ollama adapter. Speaks the `/api/chat` shape which sits between
//! OpenAI and Anthropic — flat `messages[]` array with separate
//! `tools[]` and per-message `tool_calls[]` / `images[]` fields. Auth
//! is none; the operator typically runs Ollama on localhost.
//!
//! Tool calling is only available on models that declare the `tools`
//! capability (Qwen 2.5, Llama 3.1+, Mistral Nemo, …). The adapter
//! does not probe before dispatch — if the model can't honor the
//! tools, Ollama returns an error or ignores them, and the
//! corresponding error surfaces to the client. Documented in the
//! cross-provider trade-off table.
//!
//! Response shape:
//! - Non-streaming: single JSON `{message: {role, content, tool_calls?}, prompt_eval_count, eval_count, done_reason}`.
//! - Streaming: NDJSON (one JSON object per line) with the same fields
//!   per chunk; `done: true` marks the final chunk.

use axum::response::Response;
use serde::Deserialize;
use serde_json::{Map, Value};

use aictl_core::config::config_get;

use crate::error::ApiError;
use crate::messages::translator::ir::{
    AnthropicMessage, AnthropicRequest, AnthropicRole, AnthropicTool, ContentBlock, ImageSource,
    ToolResultBlock, ToolResultContent,
};
use crate::messages::translator::{json_response, sse_response};

const DEFAULT_BASE: &str = "http://localhost:11434";

pub async fn dispatch(
    ir: &AnthropicRequest,
    request_id: &str,
    model: &str,
) -> Result<Response, ApiError> {
    let base = config_get("LLM_OLLAMA_HOST").unwrap_or_else(|| DEFAULT_BASE.to_string());
    let base = base.trim_end_matches('/');
    let url = format!("{base}/api/chat");
    let body = build_ollama_request(ir);

    let client = aictl_core::config::http_client();
    let req = client
        .post(&url)
        .header("Content-Type", "application/json")
        .json(&body);

    if ir.stream {
        let resp = req.send().await.map_err(|e| {
            tracing::warn!(event = "ollama_dispatch_failed", request_id = %request_id, error = %e);
            ApiError::ServiceUnavailable {
                reason: "provider_unavailable",
            }
        })?;
        if !resp.status().is_success() {
            return Err(map_upstream_error(resp.status()));
        }
        let body = super::stream::ollama::translate(resp.bytes_stream(), request_id, model);
        Ok(sse_response(body))
    } else {
        let resp = req.send().await.map_err(|e| {
            tracing::warn!(event = "ollama_dispatch_failed", request_id = %request_id, error = %e);
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
                event = "ollama_upstream_error",
                request_id = %request_id,
                status = status.as_u16(),
                body_preview = %String::from_utf8_lossy(&bytes[..bytes.len().min(200)]),
            );
            return Err(map_upstream_error(status));
        }
        // Non-streaming Ollama still returns NDJSON in some versions
        // — concatenate any newline-separated bodies and take the last
        // complete object as the canonical result.
        let parsed = parse_final(&bytes)?;
        Ok(json_response(&ollama_to_anthropic(
            &parsed, request_id, model,
        )))
    }
}

fn map_upstream_error(status: axum::http::StatusCode) -> ApiError {
    if status == axum::http::StatusCode::NOT_FOUND {
        ApiError::BadRequest {
            code: "model_not_found",
            message: "ollama does not have this model loaded".to_string(),
        }
    } else if status == axum::http::StatusCode::TOO_MANY_REQUESTS {
        ApiError::TooManyRequests
    } else {
        ApiError::ServiceUnavailable {
            reason: "provider_unavailable",
        }
    }
}

fn parse_final(bytes: &[u8]) -> Result<OllamaResponse, ApiError> {
    // Try parsing as a single object first; fall back to NDJSON.
    if let Ok(single) = serde_json::from_slice::<OllamaResponse>(bytes) {
        return Ok(single);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| ApiError::ServiceUnavailable {
        reason: "provider_unavailable",
    })?;
    let mut last: Option<OllamaResponse> = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(obj) = serde_json::from_str::<OllamaResponse>(line) {
            last = Some(obj);
        }
    }
    last.ok_or(ApiError::ServiceUnavailable {
        reason: "provider_unavailable",
    })
}

// --- Request translation ----------------------------------------------------

pub(crate) fn build_ollama_request(ir: &AnthropicRequest) -> Value {
    let mut obj = Map::new();
    obj.insert("model".to_string(), Value::String(ir.model.clone()));
    obj.insert("stream".to_string(), Value::Bool(ir.stream));

    let mut messages: Vec<Value> = Vec::new();
    if let Some(sys) = &ir.system {
        messages.push(serde_json::json!({
            "role": "system",
            "content": sys.collect_text(),
        }));
    }
    for m in &ir.messages {
        for v in translate_message(m) {
            messages.push(v);
        }
    }
    obj.insert("messages".to_string(), Value::Array(messages));

    let mut options = Map::new();
    options.insert("num_predict".to_string(), Value::from(ir.max_tokens));
    if let Some(t) = ir.temperature {
        options.insert("temperature".to_string(), Value::from(t));
    }
    if let Some(p) = ir.top_p {
        options.insert("top_p".to_string(), Value::from(p));
    }
    if let Some(k) = ir.top_k {
        options.insert("top_k".to_string(), Value::from(k));
    }
    if let Some(stop) = &ir.stop_sequences {
        options.insert(
            "stop".to_string(),
            Value::Array(stop.iter().map(|s| Value::String(s.clone())).collect()),
        );
    }
    obj.insert("options".to_string(), Value::Object(options));

    if let Some(tools) = &ir.tools {
        let arr: Vec<Value> = tools.iter().map(translate_tool).collect();
        obj.insert("tools".to_string(), Value::Array(arr));
    }

    Value::Object(obj)
}

fn translate_message(m: &AnthropicMessage) -> Vec<Value> {
    let role = match m.role {
        AnthropicRole::User => "user",
        AnthropicRole::Assistant => "assistant",
    };

    let mut text_parts: Vec<String> = Vec::new();
    let mut images: Vec<String> = Vec::new();
    let mut tool_calls: Vec<Value> = Vec::new();
    let mut tool_results: Vec<Value> = Vec::new();

    for b in &m.content {
        match b {
            ContentBlock::Text { text, .. } => text_parts.push(text.clone()),
            ContentBlock::Image { source } => {
                if let Some(data) = image_to_b64(source) {
                    images.push(data);
                }
            }
            ContentBlock::ToolUse { id, name, input } => {
                tool_calls.push(serde_json::json!({
                    "id": id,
                    "function": {
                        "name": name,
                        "arguments": input,
                    },
                }));
            }
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                let body = tool_result_text(content, *is_error);
                tool_results.push(serde_json::json!({
                    "role": "tool",
                    "content": body,
                    "tool_call_id": tool_use_id,
                }));
            }
            ContentBlock::Document => {}
        }
    }

    let mut out = Vec::new();
    if !text_parts.is_empty() || !images.is_empty() || !tool_calls.is_empty() {
        let mut msg = Map::new();
        msg.insert("role".to_string(), Value::String(role.to_string()));
        msg.insert("content".to_string(), Value::String(text_parts.join("\n")));
        if !images.is_empty() {
            msg.insert(
                "images".to_string(),
                Value::Array(images.into_iter().map(Value::String).collect()),
            );
        }
        if !tool_calls.is_empty() {
            msg.insert("tool_calls".to_string(), Value::Array(tool_calls));
        }
        out.push(Value::Object(msg));
    }
    out.extend(tool_results);
    out
}

fn image_to_b64(source: &ImageSource) -> Option<String> {
    match source {
        ImageSource::Base64 { data, .. } => Some(data.clone()),
        ImageSource::Url { .. } => None, // feature_gate rejects upstream
    }
}

fn tool_result_text(content: &ToolResultContent, is_error: bool) -> String {
    let prefix = if is_error { "[error] " } else { "" };
    let body = match content {
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
    format!("{prefix}{body}")
}

fn translate_tool(t: &AnthropicTool) -> Value {
    let mut function = Map::new();
    function.insert("name".to_string(), Value::String(t.name.clone()));
    if let Some(d) = &t.description {
        function.insert("description".to_string(), Value::String(d.clone()));
    }
    function.insert("parameters".to_string(), t.input_schema.clone());
    serde_json::json!({"type": "function", "function": Value::Object(function)})
}

// --- Response shape ---------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub(crate) struct OllamaResponse {
    #[serde(default)]
    pub message: Option<OllamaResponseMessage>,
    #[serde(default)]
    pub done_reason: Option<String>,
    #[serde(default)]
    pub prompt_eval_count: u64,
    #[serde(default)]
    pub eval_count: u64,
    #[serde(default)]
    pub done: bool,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct OllamaResponseMessage {
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub tool_calls: Vec<OllamaToolCall>,
}

#[derive(Debug, Deserialize, Clone)]
pub(crate) struct OllamaToolCall {
    #[serde(default)]
    pub id: Option<String>,
    pub function: OllamaToolFunction,
}

#[derive(Debug, Deserialize, Clone)]
pub(crate) struct OllamaToolFunction {
    pub name: String,
    /// Ollama sends arguments as a JSON object (not a string the way
    /// OpenAI does). Keep it as a raw Value.
    #[serde(default)]
    pub arguments: Value,
}

pub(crate) fn ollama_to_anthropic(resp: &OllamaResponse, request_id: &str, model: &str) -> Value {
    let mut content: Vec<Value> = Vec::new();
    let mut stop_reason = "end_turn";

    if let Some(m) = &resp.message {
        if !m.content.is_empty() {
            content.push(serde_json::json!({"type":"text","text": m.content}));
        }
        for (i, tc) in m.tool_calls.iter().enumerate() {
            let id = tc
                .id
                .clone()
                .unwrap_or_else(|| format!("call_{request_id}_{i}"));
            content.push(serde_json::json!({
                "type": "tool_use",
                "id": id,
                "name": tc.function.name,
                "input": tc.function.arguments,
            }));
            stop_reason = "tool_use";
        }
    }
    stop_reason = match resp.done_reason.as_deref() {
        Some("length") => "max_tokens",
        _ => stop_reason,
    };

    if content.is_empty() {
        content.push(serde_json::json!({"type":"text","text":""}));
    }

    let mut usage = Map::new();
    usage.insert(
        "input_tokens".to_string(),
        Value::from(resp.prompt_eval_count),
    );
    usage.insert("output_tokens".to_string(), Value::from(resp.eval_count));
    usage.insert("cache_creation_input_tokens".to_string(), Value::from(0));
    usage.insert("cache_read_input_tokens".to_string(), Value::from(0));

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
    fn request_carries_system_and_user() {
        let ir = ir_from(json!({
            "model": "qwen2.5-coder",
            "max_tokens": 100,
            "system": "be brief",
            "messages": [{"role": "user", "content": "hi"}],
        }));
        let r = build_ollama_request(&ir);
        let msgs = r.get("messages").unwrap().as_array().unwrap();
        assert_eq!(msgs[0].get("role").unwrap(), "system");
        assert_eq!(msgs[1].get("role").unwrap(), "user");
        assert_eq!(r.get("model").unwrap(), "qwen2.5-coder");
        assert_eq!(r.pointer("/options/num_predict").unwrap(), 100);
    }

    #[test]
    fn request_passes_images_through() {
        let ir = ir_from(json!({
            "model": "llava",
            "max_tokens": 100,
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "text", "text": "what is this?"},
                    {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "abc123"}},
                ],
            }],
        }));
        let r = build_ollama_request(&ir);
        let msg = &r.get("messages").unwrap().as_array().unwrap()[0];
        assert_eq!(msg.pointer("/images/0").unwrap(), "abc123");
    }

    #[test]
    fn response_translates_tool_calls() {
        let resp = OllamaResponse {
            message: Some(OllamaResponseMessage {
                content: String::new(),
                tool_calls: vec![OllamaToolCall {
                    id: Some("t1".into()),
                    function: OllamaToolFunction {
                        name: "calc".into(),
                        arguments: json!({"x": 2}),
                    },
                }],
            }),
            done_reason: None,
            prompt_eval_count: 5,
            eval_count: 3,
            done: true,
        };
        let v = ollama_to_anthropic(&resp, "req1", "qwen2.5-coder");
        let content = v.get("content").unwrap().as_array().unwrap();
        let tool = content
            .iter()
            .find(|b| b.get("type").and_then(Value::as_str) == Some("tool_use"))
            .unwrap();
        assert_eq!(tool.get("name").unwrap(), "calc");
        assert_eq!(tool.pointer("/input/x").unwrap(), 2);
        assert_eq!(v.get("stop_reason").unwrap(), "tool_use");
    }
}
