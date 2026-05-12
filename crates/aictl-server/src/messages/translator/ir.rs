//! Anthropic Messages API intermediate representation.
//!
//! Serde structs that mirror the request shape clients send to
//! `POST /v1/messages`. The dispatcher parses the raw JSON into
//! [`AnthropicRequest`] once; every adapter (OpenAI, Gemini, Ollama)
//! consumes the same IR. Keeping the IR strongly typed (rather than
//! threading a raw `serde_json::Value` through every translator) makes
//! the feature-gate and translation logic readable and lets the
//! compiler catch shape mistakes.
//!
//! See [`messages-cross-provider plan`] for the full field-by-field
//! translation matrix.
//!
//! [`messages-cross-provider plan`]: ../../../../../../.claude/plans/messages-cross-provider.md

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::ApiError;

#[derive(Debug, Clone)]
#[allow(clippy::struct_excessive_bools)]
pub struct AnthropicRequest {
    pub model: String,
    pub messages: Vec<AnthropicMessage>,
    pub system: Option<SystemPrompt>,
    pub tools: Option<Vec<AnthropicTool>>,
    pub tool_choice: Option<AnthropicToolChoice>,
    pub max_tokens: u32,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub top_k: Option<u32>,
    pub stop_sequences: Option<Vec<String>>,
    pub stream: bool,
    pub metadata: Option<AnthropicMetadata>,
    /// `true` if any block carried a `cache_control` marker (so the
    /// feature gate can flag it). We intentionally do not retain the
    /// markers themselves — they're stripped during parsing.
    pub cache_control_seen: bool,
    /// `true` if the request set `thinking` (Anthropic extended
    /// thinking). Stripped on translation; surfaced to feature_gate.
    pub thinking_seen: bool,
    /// `true` if any content block was a `document` (PDF). Rejected
    /// outright by the feature gate on every non-Anthropic provider.
    pub document_block_seen: bool,
}

#[derive(Debug, Clone)]
pub enum SystemPrompt {
    Text(String),
    Blocks(Vec<TextBlock>),
}

impl SystemPrompt {
    /// Mutable iterator over every text fragment the system prompt
    /// holds. Used by the redactor to rewrite in-place.
    pub fn text_mut(&mut self) -> Vec<&mut String> {
        match self {
            Self::Text(s) => vec![s],
            Self::Blocks(blocks) => blocks.iter_mut().map(|b| &mut b.text).collect(),
        }
    }

    pub fn collect_text(&self) -> String {
        match self {
            Self::Text(s) => s.clone(),
            Self::Blocks(blocks) => blocks
                .iter()
                .map(|b| b.text.as_str())
                .collect::<Vec<_>>()
                .join("\n\n"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct TextBlock {
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnthropicRole {
    User,
    Assistant,
}

impl AnthropicRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
        }
    }
}

#[derive(Debug, Clone)]
pub struct AnthropicMessage {
    pub role: AnthropicRole,
    pub content: Vec<ContentBlock>,
}

impl AnthropicMessage {
    pub fn collect_text(&self) -> String {
        self.content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn text_surfaces_mut(&mut self) -> Vec<&mut String> {
        self.content
            .iter_mut()
            .filter_map(|b| match b {
                ContentBlock::Text { text, .. } => Some(text),
                _ => None,
            })
            .collect()
    }
}

#[derive(Debug, Clone)]
pub enum ContentBlock {
    Text {
        text: String,
        /// Cache markers are stripped at parse-time on the cross-
        /// provider path; the flag on the parent request is the
        /// only thing surviving so the feature gate can warn once.
        had_cache_control: bool,
    },
    Image {
        source: ImageSource,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        /// Either a plain string or an array of text / image blocks.
        content: ToolResultContent,
        is_error: bool,
    },
    /// PDF document block — feature_gate rejects on every
    /// non-Anthropic provider.
    Document,
}

#[derive(Debug, Clone)]
pub enum ImageSource {
    Base64 { media_type: String, data: String },
    Url { url: String },
}

#[derive(Debug, Clone)]
pub enum ToolResultContent {
    Text(String),
    Blocks(Vec<ToolResultBlock>),
}

#[derive(Debug, Clone)]
pub enum ToolResultBlock {
    Text(String),
    Image(ImageSource),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicTool {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub input_schema: Value,
}

#[derive(Debug, Clone)]
pub enum AnthropicToolChoice {
    Auto,
    Any,
    Tool { name: String },
    None,
}

#[derive(Debug, Clone, Default)]
pub struct AnthropicMetadata {
    pub user_id: Option<String>,
}

// --- Parsing -----------------------------------------------------------------

pub fn parse(body: &Value) -> Result<AnthropicRequest, ApiError> {
    let obj = body.as_object().ok_or_else(|| ApiError::BadRequest {
        code: "body_malformed",
        message: "request body must be a JSON object".to_string(),
    })?;

    let model = obj
        .get("model")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::BadRequest {
            code: "body_malformed",
            message: "missing or non-string 'model' field".to_string(),
        })?
        .to_string();

    let max_tokens = obj
        .get("max_tokens")
        .and_then(Value::as_u64)
        .ok_or_else(|| ApiError::BadRequest {
            code: "body_malformed",
            message: "missing or non-integer 'max_tokens' field".to_string(),
        })?;
    let max_tokens = u32::try_from(max_tokens).unwrap_or(u32::MAX);

    let mut req = AnthropicRequest {
        model,
        messages: Vec::new(),
        system: None,
        tools: None,
        tool_choice: None,
        max_tokens,
        temperature: obj
            .get("temperature")
            .and_then(Value::as_f64)
            .map(|v| v as f32),
        top_p: obj.get("top_p").and_then(Value::as_f64).map(|v| v as f32),
        top_k: obj
            .get("top_k")
            .and_then(Value::as_u64)
            .map(|v| u32::try_from(v).unwrap_or(u32::MAX)),
        stop_sequences: obj
            .get("stop_sequences")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            }),
        stream: obj.get("stream").and_then(Value::as_bool).unwrap_or(false),
        metadata: parse_metadata(obj.get("metadata")),
        cache_control_seen: false,
        thinking_seen: obj.get("thinking").is_some(),
        document_block_seen: false,
    };

    if let Some(sys) = obj.get("system") {
        req.system = Some(parse_system(sys, &mut req)?);
    }

    let messages = obj
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| ApiError::BadRequest {
            code: "body_malformed",
            message: "missing or non-array 'messages' field".to_string(),
        })?;
    for m in messages {
        req.messages.push(parse_message(
            m,
            &mut req.cache_control_seen,
            &mut req.document_block_seen,
        )?);
    }
    if req.messages.is_empty() {
        return Err(ApiError::BadRequest {
            code: "body_malformed",
            message: "'messages' must contain at least one entry".to_string(),
        });
    }

    if let Some(tools) = obj.get("tools").and_then(Value::as_array) {
        let mut out = Vec::with_capacity(tools.len());
        for t in tools {
            out.push(parse_tool(t)?);
        }
        if !out.is_empty() {
            req.tools = Some(out);
        }
    }
    if let Some(tc) = obj.get("tool_choice") {
        req.tool_choice = parse_tool_choice(tc)?;
    }

    Ok(req)
}

fn parse_system(v: &Value, req: &mut AnthropicRequest) -> Result<SystemPrompt, ApiError> {
    match v {
        Value::String(s) => Ok(SystemPrompt::Text(s.clone())),
        Value::Array(parts) => {
            let mut out = Vec::new();
            for part in parts {
                if part.get("type").and_then(Value::as_str) != Some("text") {
                    continue;
                }
                if part.get("cache_control").is_some() {
                    req.cache_control_seen = true;
                }
                if let Some(t) = part.get("text").and_then(Value::as_str) {
                    out.push(TextBlock {
                        text: t.to_string(),
                    });
                }
            }
            Ok(SystemPrompt::Blocks(out))
        }
        _ => Err(ApiError::BadRequest {
            code: "body_malformed",
            message: "'system' must be a string or an array of text blocks".to_string(),
        }),
    }
}

fn parse_metadata(v: Option<&Value>) -> Option<AnthropicMetadata> {
    let obj = v?.as_object()?;
    Some(AnthropicMetadata {
        user_id: obj
            .get("user_id")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

fn parse_message(
    v: &Value,
    cache_control_seen: &mut bool,
    document_block_seen: &mut bool,
) -> Result<AnthropicMessage, ApiError> {
    let obj = v.as_object().ok_or_else(|| ApiError::BadRequest {
        code: "body_malformed",
        message: "each message must be an object".to_string(),
    })?;
    let role = match obj.get("role").and_then(Value::as_str) {
        Some("user") => AnthropicRole::User,
        Some("assistant") => AnthropicRole::Assistant,
        Some(other) => {
            return Err(ApiError::BadRequest {
                code: "body_malformed",
                message: format!("unknown message role: {other}"),
            });
        }
        None => {
            return Err(ApiError::BadRequest {
                code: "body_malformed",
                message: "message missing 'role'".to_string(),
            });
        }
    };
    let content = match obj.get("content") {
        Some(Value::String(s)) => vec![ContentBlock::Text {
            text: s.clone(),
            had_cache_control: false,
        }],
        Some(Value::Array(parts)) => {
            let mut out = Vec::with_capacity(parts.len());
            for part in parts {
                out.push(parse_content_block(
                    part,
                    cache_control_seen,
                    document_block_seen,
                )?);
            }
            out
        }
        Some(_) => {
            return Err(ApiError::BadRequest {
                code: "body_malformed",
                message: "'content' must be a string or array".to_string(),
            });
        }
        None => Vec::new(),
    };
    Ok(AnthropicMessage { role, content })
}

fn parse_content_block(
    v: &Value,
    cache_control_seen: &mut bool,
    document_block_seen: &mut bool,
) -> Result<ContentBlock, ApiError> {
    let obj = v.as_object().ok_or_else(|| ApiError::BadRequest {
        code: "body_malformed",
        message: "each content block must be an object".to_string(),
    })?;
    let had_cache_control = obj.get("cache_control").is_some();
    if had_cache_control {
        *cache_control_seen = true;
    }
    let ty = obj.get("type").and_then(Value::as_str).unwrap_or("");
    match ty {
        "text" => {
            let text = obj
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            Ok(ContentBlock::Text {
                text,
                had_cache_control,
            })
        }
        "image" => {
            let source = obj.get("source").ok_or_else(|| ApiError::BadRequest {
                code: "body_malformed",
                message: "image block missing 'source'".to_string(),
            })?;
            Ok(ContentBlock::Image {
                source: parse_image_source(source)?,
            })
        }
        "tool_use" => {
            let id = obj
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let name = obj
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let input = obj
                .get("input")
                .cloned()
                .unwrap_or_else(|| Value::Object(serde_json::Map::default()));
            Ok(ContentBlock::ToolUse { id, name, input })
        }
        "tool_result" => {
            let tool_use_id = obj
                .get("tool_use_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let is_error = obj
                .get("is_error")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let content = match obj.get("content") {
                Some(Value::String(s)) => ToolResultContent::Text(s.clone()),
                Some(Value::Array(parts)) => {
                    let mut out = Vec::with_capacity(parts.len());
                    for part in parts {
                        let pty = part.get("type").and_then(Value::as_str).unwrap_or("");
                        match pty {
                            "text" => {
                                let t = part
                                    .get("text")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_string();
                                out.push(ToolResultBlock::Text(t));
                            }
                            "image" => {
                                if let Some(src) = part.get("source") {
                                    out.push(ToolResultBlock::Image(parse_image_source(src)?));
                                }
                            }
                            _ => {}
                        }
                    }
                    ToolResultContent::Blocks(out)
                }
                _ => ToolResultContent::Text(String::new()),
            };
            Ok(ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            })
        }
        "document" => {
            *document_block_seen = true;
            Ok(ContentBlock::Document)
        }
        other => Err(ApiError::BadRequest {
            code: "body_malformed",
            message: format!("unsupported content block type: {other}"),
        }),
    }
}

fn parse_image_source(v: &Value) -> Result<ImageSource, ApiError> {
    let obj = v.as_object().ok_or_else(|| ApiError::BadRequest {
        code: "body_malformed",
        message: "image 'source' must be an object".to_string(),
    })?;
    match obj.get("type").and_then(Value::as_str) {
        Some("base64") => {
            let media_type = obj
                .get("media_type")
                .and_then(Value::as_str)
                .unwrap_or("image/png")
                .to_string();
            let data = obj
                .get("data")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            Ok(ImageSource::Base64 { media_type, data })
        }
        Some("url") => {
            let url = obj
                .get("url")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            Ok(ImageSource::Url { url })
        }
        Some(other) => Err(ApiError::BadRequest {
            code: "body_malformed",
            message: format!("unsupported image source type: {other}"),
        }),
        None => Err(ApiError::BadRequest {
            code: "body_malformed",
            message: "image source missing 'type'".to_string(),
        }),
    }
}

fn parse_tool(v: &Value) -> Result<AnthropicTool, ApiError> {
    let obj = v.as_object().ok_or_else(|| ApiError::BadRequest {
        code: "body_malformed",
        message: "tool must be an object".to_string(),
    })?;
    let name = obj
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::BadRequest {
            code: "body_malformed",
            message: "tool missing 'name'".to_string(),
        })?
        .to_string();
    let description = obj
        .get("description")
        .and_then(Value::as_str)
        .map(str::to_string);
    let input_schema = obj
        .get("input_schema")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({"type":"object"}));
    Ok(AnthropicTool {
        name,
        description,
        input_schema,
    })
}

fn parse_tool_choice(v: &Value) -> Result<Option<AnthropicToolChoice>, ApiError> {
    match v {
        Value::String(s) => match s.as_str() {
            "auto" => Ok(Some(AnthropicToolChoice::Auto)),
            "any" => Ok(Some(AnthropicToolChoice::Any)),
            "none" => Ok(Some(AnthropicToolChoice::None)),
            other => Err(ApiError::BadRequest {
                code: "body_malformed",
                message: format!("unknown tool_choice value: {other}"),
            }),
        },
        Value::Object(obj) => match obj.get("type").and_then(Value::as_str) {
            Some("auto") => Ok(Some(AnthropicToolChoice::Auto)),
            Some("any") => Ok(Some(AnthropicToolChoice::Any)),
            Some("none") => Ok(Some(AnthropicToolChoice::None)),
            Some("tool") => {
                let name = obj
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| ApiError::BadRequest {
                        code: "body_malformed",
                        message: "tool_choice type=tool missing 'name'".to_string(),
                    })?
                    .to_string();
                Ok(Some(AnthropicToolChoice::Tool { name }))
            }
            _ => Ok(None),
        },
        Value::Null => Ok(None),
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_minimal_request() {
        let v = json!({
            "model": "gpt-4o-mini",
            "max_tokens": 100,
            "messages": [{"role": "user", "content": "hi"}],
        });
        let req = parse(&v).unwrap();
        assert_eq!(req.model, "gpt-4o-mini");
        assert_eq!(req.max_tokens, 100);
        assert_eq!(req.messages.len(), 1);
        assert!(matches!(req.messages[0].role, AnthropicRole::User));
        assert!(!req.stream);
    }

    #[test]
    fn parses_content_blocks() {
        let v = json!({
            "model": "gpt-4o-mini",
            "max_tokens": 100,
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "text", "text": "Look at this:"},
                    {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "aGVsbG8="}},
                ],
            }],
        });
        let req = parse(&v).unwrap();
        assert_eq!(req.messages[0].content.len(), 2);
        assert!(matches!(
            req.messages[0].content[1],
            ContentBlock::Image { .. }
        ));
    }

    #[test]
    fn parses_tool_use_and_result() {
        let v = json!({
            "model": "gpt-4o-mini",
            "max_tokens": 100,
            "messages": [
                {"role": "assistant", "content": [
                    {"type": "tool_use", "id": "t1", "name": "calc", "input": {"x": 1}},
                ]},
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "t1", "content": "42"},
                ]},
            ],
        });
        let req = parse(&v).unwrap();
        assert!(matches!(
            req.messages[0].content[0],
            ContentBlock::ToolUse { .. }
        ));
        assert!(matches!(
            req.messages[1].content[0],
            ContentBlock::ToolResult { .. }
        ));
    }

    #[test]
    fn detects_cache_control_marker() {
        let v = json!({
            "model": "gpt-4o-mini",
            "max_tokens": 100,
            "system": [
                {"type": "text", "text": "be helpful", "cache_control": {"type": "ephemeral"}},
            ],
            "messages": [{"role": "user", "content": "hi"}],
        });
        let req = parse(&v).unwrap();
        assert!(req.cache_control_seen);
    }

    #[test]
    fn detects_thinking() {
        let v = json!({
            "model": "gpt-4o-mini",
            "max_tokens": 100,
            "thinking": {"type": "enabled", "budget_tokens": 5000},
            "messages": [{"role": "user", "content": "hi"}],
        });
        let req = parse(&v).unwrap();
        assert!(req.thinking_seen);
    }

    #[test]
    fn rejects_empty_messages() {
        let v = json!({"model": "gpt-4o-mini", "max_tokens": 100, "messages": []});
        let err = parse(&v).unwrap_err();
        assert!(matches!(
            err,
            ApiError::BadRequest {
                code: "body_malformed",
                ..
            }
        ));
    }

    #[test]
    fn parses_tool_choice_variants() {
        let cases = [
            (json!("auto"), AnthropicToolChoice::Auto),
            (json!("any"), AnthropicToolChoice::Any),
            (
                json!({"type": "tool", "name": "foo"}),
                AnthropicToolChoice::Tool { name: "foo".into() },
            ),
        ];
        for (v, _) in cases {
            let wrapped = json!({
                "model": "gpt-4o-mini",
                "max_tokens": 100,
                "messages": [{"role": "user", "content": "hi"}],
                "tool_choice": v,
            });
            let req = parse(&wrapped).unwrap();
            assert!(req.tool_choice.is_some());
        }
    }
}
