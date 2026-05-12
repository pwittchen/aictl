//! OpenAI-compatible adapter — covers OpenAI, Grok, Mistral, DeepSeek,
//! Kimi, and Z.ai. All six speak the OpenAI `/v1/chat/completions`
//! shape with provider-specific base URLs and `Authorization: Bearer
//! <key>` auth, so the request- and response-translation logic is
//! shared.
//!
//! Per-provider differences resolved in [`endpoint_for`]:
//!
//! - DeepSeek's path is `/chat/completions` (no `/v1`).
//! - Z.ai sits under `/api/paas/v4/chat/completions`.
//! - Kimi's host can be flipped to `.cn` via `LLM_KIMI_BASE_URL`.
//!
//! The translator owns its own HTTP round-trip (not
//! `aictl_core::llm::call_*`, which speaks the engine's internal
//! text-in / text-out shape and uses XML tool calling). Native
//! `tools[]` and `tool_calls[]` flow through verbatim so Anthropic
//! `tool_use` / `tool_result` blocks round-trip cleanly.

use axum::response::Response;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use aictl_core::config::config_get;
use aictl_core::run::Provider;

use crate::error::ApiError;
use crate::messages::translator::ir::{
    AnthropicMessage, AnthropicRequest, AnthropicRole, AnthropicTool, AnthropicToolChoice,
    ContentBlock, ImageSource, SystemPrompt, ToolResultBlock, ToolResultContent,
};
use crate::messages::translator::{json_response, resolve_api_key, sse_response};

pub async fn dispatch(
    ir: &AnthropicRequest,
    provider: &Provider,
    request_id: &str,
    model: &str,
) -> Result<Response, ApiError> {
    let api_key = resolve_api_key(provider)?;
    let url = endpoint_for(provider);
    let req_body = build_openai_request(ir);

    let client = aictl_core::config::http_client();
    let req = client
        .post(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
        .json(&req_body);

    if ir.stream {
        let resp = req.send().await.map_err(|e| {
            tracing::warn!(
                event = "openai_family_dispatch_failed",
                request_id = %request_id,
                provider = ?provider,
                error = %e,
            );
            ApiError::ServiceUnavailable {
                reason: "provider_unavailable",
            }
        })?;
        if !resp.status().is_success() {
            return Err(map_upstream_error(resp.status()));
        }
        let bytes_stream = resp.bytes_stream();
        let body = super::stream::openai::translate(bytes_stream, request_id, model);
        Ok(sse_response(body))
    } else {
        let resp = req.send().await.map_err(|e| {
            tracing::warn!(
                event = "openai_family_dispatch_failed",
                request_id = %request_id,
                provider = ?provider,
                error = %e,
            );
            ApiError::ServiceUnavailable {
                reason: "provider_unavailable",
            }
        })?;
        let status = resp.status();
        let body_bytes = resp
            .bytes()
            .await
            .map_err(|_| ApiError::ServiceUnavailable {
                reason: "provider_unavailable",
            })?;
        if !status.is_success() {
            tracing::warn!(
                event = "openai_family_upstream_error",
                request_id = %request_id,
                status = status.as_u16(),
                body_preview = %String::from_utf8_lossy(&body_bytes[..body_bytes.len().min(200)]),
            );
            return Err(map_upstream_error(status));
        }
        let openai_resp: OpenAiChatResponse = serde_json::from_slice(&body_bytes).map_err(|e| {
            tracing::warn!(
                event = "openai_family_response_parse_failed",
                request_id = %request_id,
                error = %e,
            );
            ApiError::ServiceUnavailable {
                reason: "provider_unavailable",
            }
        })?;
        let anthropic_resp = openai_to_anthropic(&openai_resp, request_id, model);
        Ok(json_response(&anthropic_resp))
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

fn endpoint_for(provider: &Provider) -> String {
    let (env_key, default) = match provider {
        Provider::Openai => (
            "LLM_OPENAI_BASE_URL",
            "https://api.openai.com/v1/chat/completions",
        ),
        Provider::Grok => ("LLM_GROK_BASE_URL", "https://api.x.ai/v1/chat/completions"),
        Provider::Mistral => (
            "LLM_MISTRAL_BASE_URL",
            "https://api.mistral.ai/v1/chat/completions",
        ),
        Provider::Deepseek => (
            "LLM_DEEPSEEK_BASE_URL",
            "https://api.deepseek.com/chat/completions",
        ),
        Provider::Kimi => (
            "LLM_KIMI_BASE_URL",
            "https://api.moonshot.ai/v1/chat/completions",
        ),
        Provider::Zai => (
            "LLM_ZAI_BASE_URL",
            "https://api.z.ai/api/paas/v4/chat/completions",
        ),
        _ => return String::new(),
    };
    if let Some(raw) = config_get(env_key) {
        // Operator overrides — accept either a full URL with path or a
        // base; append the canonical path if it isn't already present.
        if raw.contains("/chat/completions") {
            return raw;
        }
        let trimmed = raw.trim_end_matches('/');
        let suffix = if matches!(provider, Provider::Deepseek) {
            "/chat/completions"
        } else if matches!(provider, Provider::Zai) {
            "/api/paas/v4/chat/completions"
        } else {
            "/v1/chat/completions"
        };
        return format!("{trimmed}{suffix}");
    }
    default.to_string()
}

// --- Request translation ----------------------------------------------------

pub(crate) fn build_openai_request(ir: &AnthropicRequest) -> Value {
    let mut obj = Map::new();
    obj.insert("model".to_string(), Value::String(ir.model.clone()));
    obj.insert("max_tokens".to_string(), Value::from(ir.max_tokens));

    if let Some(t) = ir.temperature {
        obj.insert("temperature".to_string(), Value::from(t));
    }
    if let Some(p) = ir.top_p {
        obj.insert("top_p".to_string(), Value::from(p));
    }
    if let Some(stop) = &ir.stop_sequences {
        obj.insert(
            "stop".to_string(),
            Value::Array(stop.iter().map(|s| Value::String(s.clone())).collect()),
        );
    }
    if let Some(meta) = &ir.metadata
        && let Some(uid) = &meta.user_id
    {
        obj.insert("user".to_string(), Value::String(uid.clone()));
    }
    if ir.stream {
        obj.insert("stream".to_string(), Value::Bool(true));
        obj.insert(
            "stream_options".to_string(),
            serde_json::json!({"include_usage": true}),
        );
    }

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

    if let Some(tools) = &ir.tools {
        let arr: Vec<Value> = tools.iter().map(translate_tool).collect();
        obj.insert("tools".to_string(), Value::Array(arr));
    }
    if let Some(choice) = &ir.tool_choice {
        obj.insert("tool_choice".to_string(), translate_tool_choice(choice));
    }

    Value::Object(obj)
}

fn translate_message(m: &AnthropicMessage) -> Vec<Value> {
    // Anthropic packs several tool_results into one user message; OpenAI
    // wants one `role:"tool"` message per result. We also need to split
    // `tool_use` blocks out of an assistant turn into `tool_calls[]`.
    let role = m.role.as_str();
    match m.role {
        AnthropicRole::User => translate_user_message(&m.content, role),
        AnthropicRole::Assistant => translate_assistant_message(&m.content, role),
    }
}

fn translate_user_message(blocks: &[ContentBlock], role: &str) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    let mut user_parts: Vec<Value> = Vec::new();

    for b in blocks {
        match b {
            ContentBlock::Text { text, .. } => {
                user_parts.push(serde_json::json!({"type":"text","text": text}));
            }
            ContentBlock::Image { source } => {
                let url = image_to_data_url(source);
                user_parts.push(serde_json::json!({
                    "type": "image_url",
                    "image_url": {"url": url},
                }));
            }
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                if !user_parts.is_empty() {
                    out.push(build_user_msg(role, std::mem::take(&mut user_parts)));
                }
                let content_str = tool_result_text(content, *is_error);
                out.push(serde_json::json!({
                    "role": "tool",
                    "tool_call_id": tool_use_id,
                    "content": content_str,
                }));
            }
            ContentBlock::ToolUse { .. } | ContentBlock::Document => {
                // User-side tool_use is not a real shape; document
                // blocks were rejected by the feature gate.
            }
        }
    }
    if !user_parts.is_empty() {
        out.push(build_user_msg(role, user_parts));
    }
    out
}

fn build_user_msg(role: &str, mut parts: Vec<Value>) -> Value {
    // OpenAI accepts a plain string content for simple text-only
    // messages and an array of parts for multimodal. Use the plain
    // string when every part is text — it's the more compatible shape
    // across older OpenAI-family providers.
    let all_text = parts
        .iter()
        .all(|p| p.get("type").and_then(Value::as_str) == Some("text"));
    if all_text {
        let joined = parts
            .iter_mut()
            .filter_map(|p| p.get("text").and_then(Value::as_str).map(str::to_string))
            .collect::<Vec<_>>()
            .join("\n");
        serde_json::json!({"role": role, "content": joined})
    } else {
        serde_json::json!({"role": role, "content": parts})
    }
}

fn translate_assistant_message(blocks: &[ContentBlock], role: &str) -> Vec<Value> {
    let mut text_parts: Vec<String> = Vec::new();
    let mut tool_calls: Vec<Value> = Vec::new();

    for b in blocks {
        match b {
            ContentBlock::Text { text, .. } => text_parts.push(text.clone()),
            ContentBlock::ToolUse { id, name, input } => {
                let args_str = serde_json::to_string(input).unwrap_or_else(|_| "{}".to_string());
                tool_calls.push(serde_json::json!({
                    "id": id,
                    "type": "function",
                    "function": {
                        "name": name,
                        "arguments": args_str,
                    },
                }));
            }
            ContentBlock::Image { .. }
            | ContentBlock::ToolResult { .. }
            | ContentBlock::Document => {
                // Not part of an assistant turn in the Anthropic shape.
            }
        }
    }

    if text_parts.is_empty() && tool_calls.is_empty() {
        return vec![];
    }

    let mut msg = Map::new();
    msg.insert("role".to_string(), Value::String(role.to_string()));
    let content_value = if text_parts.is_empty() {
        // OpenAI requires content to be present even when null.
        Value::Null
    } else {
        Value::String(text_parts.join("\n"))
    };
    msg.insert("content".to_string(), content_value);
    if !tool_calls.is_empty() {
        msg.insert("tool_calls".to_string(), Value::Array(tool_calls));
    }
    vec![Value::Object(msg)]
}

fn image_to_data_url(source: &ImageSource) -> String {
    match source {
        ImageSource::Base64 { media_type, data } => format!("data:{media_type};base64,{data}"),
        ImageSource::Url { url } => url.clone(),
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

fn translate_tool_choice(choice: &AnthropicToolChoice) -> Value {
    match choice {
        AnthropicToolChoice::Auto => Value::String("auto".into()),
        AnthropicToolChoice::Any => Value::String("required".into()),
        AnthropicToolChoice::None => Value::String("none".into()),
        AnthropicToolChoice::Tool { name } => serde_json::json!({
            "type": "function",
            "function": {"name": name},
        }),
    }
}

// --- OpenAI response shape --------------------------------------------------

#[derive(Debug, Deserialize)]
pub(crate) struct OpenAiChatResponse {
    pub id: String,
    pub choices: Vec<OpenAiChoice>,
    #[serde(default)]
    pub usage: Option<OpenAiUsage>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct OpenAiChoice {
    #[serde(default)]
    pub message: OpenAiMessage,
    #[serde(default)]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct OpenAiMessage {
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub tool_calls: Vec<OpenAiToolCall>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub(crate) struct OpenAiToolCall {
    pub id: String,
    #[serde(rename = "type", default = "default_function_type")]
    pub call_type: String,
    pub function: OpenAiToolCallFunction,
}

fn default_function_type() -> String {
    "function".to_string()
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub(crate) struct OpenAiToolCallFunction {
    pub name: String,
    #[serde(default)]
    pub arguments: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct OpenAiUsage {
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
    #[serde(default)]
    pub prompt_tokens_details: Option<OpenAiPromptDetails>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct OpenAiPromptDetails {
    #[serde(default)]
    pub cached_tokens: u64,
}

// --- Response translation ---------------------------------------------------

pub(crate) fn openai_to_anthropic(
    resp: &OpenAiChatResponse,
    request_id: &str,
    model: &str,
) -> Value {
    let mut content: Vec<Value> = Vec::new();
    let mut stop_reason = "end_turn";

    if let Some(choice) = resp.choices.first() {
        if let Some(text) = &choice.message.content
            && !text.is_empty()
        {
            content.push(serde_json::json!({"type":"text","text": text}));
        }
        for tc in &choice.message.tool_calls {
            let input: Value =
                serde_json::from_str(&tc.function.arguments).unwrap_or(Value::Object(Map::new()));
            content.push(serde_json::json!({
                "type": "tool_use",
                "id": tc.id,
                "name": tc.function.name,
                "input": input,
            }));
        }
        stop_reason = match choice.finish_reason.as_deref() {
            Some("length") => "max_tokens",
            Some("tool_calls" | "function_call") => "tool_use",
            Some("stop") | None => "end_turn",
            // content_filter / other → end_turn (best-effort).
            Some(_) => "end_turn",
        };
    }

    if content.is_empty() {
        // Anthropic always returns at least one content block.
        content.push(serde_json::json!({"type":"text","text":""}));
    }

    let (input_tokens, output_tokens, cache_read) = resp.usage.as_ref().map_or((0, 0, 0), |u| {
        let cached = u
            .prompt_tokens_details
            .as_ref()
            .map_or(0, |d| d.cached_tokens);
        let fresh = u.prompt_tokens.saturating_sub(cached);
        (fresh, u.completion_tokens, cached)
    });

    let id_prefix = if resp.id.starts_with("msg_") {
        resp.id.clone()
    } else {
        format!("msg_{request_id}")
    };

    let mut usage = Map::new();
    usage.insert("input_tokens".to_string(), Value::from(input_tokens));
    usage.insert("output_tokens".to_string(), Value::from(output_tokens));
    usage.insert("cache_creation_input_tokens".to_string(), Value::from(0));
    usage.insert(
        "cache_read_input_tokens".to_string(),
        Value::from(cache_read),
    );

    serde_json::json!({
        "id": id_prefix,
        "type": "message",
        "role": "assistant",
        "model": model,
        "content": content,
        "stop_reason": stop_reason,
        "stop_sequence": Value::Null,
        "usage": Value::Object(usage),
    })
}

// --- Helpers shared with the stream module ---------------------------------

pub(crate) fn stop_reason_from_openai(finish: Option<&str>) -> &'static str {
    match finish {
        Some("length") => "max_tokens",
        Some("tool_calls" | "function_call") => "tool_use",
        _ => "end_turn",
    }
}

// --- Compile-time sanity checks --------------------------------------------

#[allow(dead_code)]
const fn _ensure_provider_is_used(_p: &Provider) {}

// Helper for SystemPrompt::collect_text to be visible (already pub).
#[allow(dead_code)]
fn _sp_dummy(_s: &SystemPrompt) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::translator::ir::{self};
    use serde_json::json;

    fn ir_from(v: Value) -> AnthropicRequest {
        ir::parse(&v).unwrap()
    }

    #[test]
    fn request_carries_system_and_user() {
        let ir = ir_from(json!({
            "model": "gpt-4o-mini",
            "max_tokens": 100,
            "system": "be brief",
            "messages": [{"role": "user", "content": "hi"}],
        }));
        let r = build_openai_request(&ir);
        let msgs = r.get("messages").unwrap().as_array().unwrap();
        assert_eq!(msgs[0].get("role").unwrap(), "system");
        assert_eq!(msgs[0].get("content").unwrap(), "be brief");
        assert_eq!(msgs[1].get("role").unwrap(), "user");
        assert_eq!(msgs[1].get("content").unwrap(), "hi");
    }

    #[test]
    fn request_translates_tool_use_blocks() {
        let ir = ir_from(json!({
            "model": "gpt-4o-mini",
            "max_tokens": 100,
            "messages": [
                {"role": "user", "content": "what's 2+2?"},
                {"role": "assistant", "content": [
                    {"type": "tool_use", "id": "t1", "name": "calc", "input": {"x": 2, "y": 2}},
                ]},
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "t1", "content": "4"},
                ]},
            ],
        }));
        let r = build_openai_request(&ir);
        let msgs = r.get("messages").unwrap().as_array().unwrap();
        assert_eq!(msgs.len(), 3);
        // Assistant turn carries tool_calls[].
        let asst = &msgs[1];
        let tcs = asst.get("tool_calls").unwrap().as_array().unwrap();
        assert_eq!(tcs[0].get("id").unwrap(), "t1");
        assert_eq!(tcs[0].pointer("/function/name").unwrap(), "calc");
        // Tool result becomes role:"tool".
        assert_eq!(msgs[2].get("role").unwrap(), "tool");
        assert_eq!(msgs[2].get("tool_call_id").unwrap(), "t1");
        assert_eq!(msgs[2].get("content").unwrap(), "4");
    }

    #[test]
    fn request_translates_tools_and_tool_choice() {
        let ir = ir_from(json!({
            "model": "gpt-4o-mini",
            "max_tokens": 100,
            "messages": [{"role": "user", "content": "x"}],
            "tools": [{
                "name": "get_weather",
                "description": "current weather",
                "input_schema": {"type": "object", "properties": {"city": {"type": "string"}}},
            }],
            "tool_choice": "any",
        }));
        let r = build_openai_request(&ir);
        let tools = r.get("tools").unwrap().as_array().unwrap();
        assert_eq!(tools[0].get("type").unwrap(), "function");
        assert_eq!(tools[0].pointer("/function/name").unwrap(), "get_weather");
        assert_eq!(r.get("tool_choice").unwrap(), "required");
    }

    #[test]
    fn response_translates_text() {
        let resp = OpenAiChatResponse {
            id: "chatcmpl-abc".into(),
            choices: vec![OpenAiChoice {
                message: OpenAiMessage {
                    content: Some("hello".into()),
                    tool_calls: vec![],
                },
                finish_reason: Some("stop".into()),
            }],
            usage: Some(OpenAiUsage {
                prompt_tokens: 5,
                completion_tokens: 3,
                prompt_tokens_details: None,
            }),
        };
        let v = openai_to_anthropic(&resp, "req1", "gpt-4o-mini");
        assert_eq!(v.get("type").unwrap(), "message");
        let content = v.get("content").unwrap().as_array().unwrap();
        assert_eq!(content[0].get("text").unwrap(), "hello");
        assert_eq!(v.get("stop_reason").unwrap(), "end_turn");
        assert_eq!(v.pointer("/usage/input_tokens").unwrap(), 5);
        assert_eq!(v.pointer("/usage/output_tokens").unwrap(), 3);
    }

    #[test]
    fn response_translates_tool_calls() {
        let resp = OpenAiChatResponse {
            id: "chatcmpl-abc".into(),
            choices: vec![OpenAiChoice {
                message: OpenAiMessage {
                    content: None,
                    tool_calls: vec![OpenAiToolCall {
                        id: "t1".into(),
                        call_type: "function".into(),
                        function: OpenAiToolCallFunction {
                            name: "calc".into(),
                            arguments: r#"{"x":2}"#.into(),
                        },
                    }],
                },
                finish_reason: Some("tool_calls".into()),
            }],
            usage: None,
        };
        let v = openai_to_anthropic(&resp, "req1", "gpt-4o-mini");
        let content = v.get("content").unwrap().as_array().unwrap();
        // The Anthropic shape ensures at least one content block is
        // always present, so when OpenAI returns no text the
        // translator inserts an empty text block — the tool_use block
        // follows it.
        let tool_block = content
            .iter()
            .find(|b| b.get("type").and_then(Value::as_str) == Some("tool_use"))
            .unwrap();
        assert_eq!(tool_block.get("name").unwrap(), "calc");
        assert_eq!(tool_block.pointer("/input/x").unwrap(), 2);
        assert_eq!(v.get("stop_reason").unwrap(), "tool_use");
    }

    #[test]
    fn cached_tokens_split_out_of_input() {
        let resp = OpenAiChatResponse {
            id: "chatcmpl-x".into(),
            choices: vec![],
            usage: Some(OpenAiUsage {
                prompt_tokens: 100,
                completion_tokens: 10,
                prompt_tokens_details: Some(OpenAiPromptDetails { cached_tokens: 40 }),
            }),
        };
        let v = openai_to_anthropic(&resp, "req1", "gpt-4o-mini");
        assert_eq!(v.pointer("/usage/input_tokens").unwrap(), 60);
        assert_eq!(v.pointer("/usage/cache_read_input_tokens").unwrap(), 40);
    }
}
