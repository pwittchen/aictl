//! OpenAI-compatible request/response translation.
//!
//! The gateway accepts OpenAI's `/v1/chat/completions` request schema,
//! translates `messages[]` into the engine's provider-agnostic
//! `Vec<aictl_core::Message>`, dispatches to the appropriate
//! `aictl_core::llm::call_<provider>` function, and wraps the result
//! back into OpenAI's response shape so SDKs that already speak
//! OpenAI keep working.
//!
//! Tool-call passthrough is **not** implemented in this phase — any
//! request with a non-empty `tools` array is rejected with 400
//! `tools_unsupported_for_provider` per plan §7. Adding it later means
//! propagating the JSON straight through to providers that support it
//! (Anthropic, OpenAI, Gemini) and translating the response.

use serde::{Deserialize, Serialize};

use aictl_core::llm::{MODELS, TokenUsage};
use aictl_core::message::{Message, Role};
use aictl_core::run::Provider;

use crate::error::ApiError;

// --- Request shape -----------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(default)]
    pub stream: Option<bool>,
    #[serde(default)]
    pub tools: Option<serde_json::Value>,
    #[serde(default)]
    pub functions: Option<serde_json::Value>,
    // Optional fields are accepted but currently ignored — captured here
    // so serde doesn't reject them as unknown.
    #[serde(default)]
    pub temperature: Option<f64>,
    #[serde(default)]
    pub top_p: Option<f64>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub tool_choice: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    #[serde(default)]
    pub content: ChatContent,
}

#[derive(Debug, Deserialize, Default)]
#[serde(untagged)]
pub enum ChatContent {
    #[default]
    Empty,
    Text(String),
    Parts(Vec<ChatContentPart>),
}

impl ChatContent {
    fn into_text(self) -> String {
        match self {
            Self::Empty => String::new(),
            Self::Text(s) => s,
            Self::Parts(parts) => parts
                .into_iter()
                .filter_map(|p| p.text)
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }
}

/// One element of OpenAI's `content` array. We only forward the text
/// component to upstream providers in this phase — image and audio
/// parts are dropped (image passthrough is a future enhancement).
#[derive(Debug, Deserialize)]
pub struct ChatContentPart {
    #[serde(rename = "type", default)]
    pub part_type: String,
    #[serde(default)]
    pub text: Option<String>,
}

// --- Legacy completions request ---------------------------------------------

#[derive(Debug, Deserialize)]
pub struct CompletionRequest {
    pub model: String,
    pub prompt: PromptField,
    #[serde(default)]
    pub stream: Option<bool>,
    #[serde(default)]
    pub temperature: Option<f64>,
    #[serde(default)]
    pub top_p: Option<f64>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum PromptField {
    Text(String),
    List(Vec<String>),
}

impl PromptField {
    pub fn into_text(self) -> String {
        match self {
            Self::Text(s) => s,
            Self::List(parts) => parts.join("\n"),
        }
    }
}

// --- Response shape ---------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct ChatCompletionResponse {
    pub id: String,
    pub object: &'static str,
    pub created: u64,
    pub model: String,
    pub choices: Vec<ChatCompletionChoice>,
    pub usage: ChatCompletionUsage,
}

#[derive(Debug, Serialize)]
pub struct ChatCompletionChoice {
    pub index: u32,
    pub message: ChatCompletionResponseMessage,
    pub finish_reason: &'static str,
}

#[derive(Debug, Serialize)]
pub struct ChatCompletionResponseMessage {
    pub role: &'static str,
    pub content: String,
}

#[derive(Debug, Serialize)]
pub struct ChatCompletionUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
}

#[derive(Debug, Serialize)]
pub struct CompletionResponse {
    pub id: String,
    pub object: &'static str,
    pub created: u64,
    pub model: String,
    pub choices: Vec<CompletionChoice>,
    pub usage: ChatCompletionUsage,
}

#[derive(Debug, Serialize)]
pub struct CompletionChoice {
    pub index: u32,
    pub text: String,
    pub finish_reason: &'static str,
}

// --- Streaming response shape -----------------------------------------------

#[derive(Debug, Serialize)]
pub struct ChatCompletionChunk {
    pub id: String,
    pub object: &'static str,
    pub created: u64,
    pub model: String,
    pub choices: Vec<ChatCompletionChunkChoice>,
}

#[derive(Debug, Serialize)]
pub struct ChatCompletionChunkChoice {
    pub index: u32,
    pub delta: ChatCompletionChunkDelta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<&'static str>,
}

#[derive(Debug, Serialize, Default)]
pub struct ChatCompletionChunkDelta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

// --- Translation -------------------------------------------------------------

/// Convert OpenAI-shaped chat messages into the engine's
/// provider-agnostic [`Message`] vector.
pub fn to_internal(messages: Vec<ChatMessage>) -> Result<Vec<Message>, ApiError> {
    if messages.is_empty() {
        return Err(ApiError::BadRequest {
            code: "body_malformed",
            message: "messages must not be empty".to_string(),
        });
    }
    let mut out = Vec::with_capacity(messages.len());
    for m in messages {
        let role = match m.role.as_str() {
            "system" | "developer" => Role::System,
            "user" => Role::User,
            "assistant" => Role::Assistant,
            "tool" | "function" => {
                return Err(ApiError::BadRequest {
                    code: "tools_unsupported_for_provider",
                    message: format!(
                        "messages with role={} are not supported in this phase",
                        m.role
                    ),
                });
            }
            other => {
                return Err(ApiError::BadRequest {
                    code: "body_malformed",
                    message: format!("unknown message role: {other}"),
                });
            }
        };
        out.push(Message {
            role,
            content: m.content.into_text(),
            images: vec![],
        });
    }
    Ok(out)
}

/// Resolve the provider for a given model name. Cloud providers come
/// from the static `MODELS` catalogue; local providers (Ollama, GGUF,
/// MLX) are detected by listing locally available models at request
/// time. Returns 400 `model_not_found` when no provider claims the id.
pub async fn resolve_provider(model: &str) -> Result<Provider, ApiError> {
    if let Some(tag) = aictl_core::llm::provider_for_model(model) {
        return Ok(provider_from_tag(tag));
    }
    // Local providers — scan the live catalogues. These are best-effort:
    // when none can serve the model, we surface 400.
    let ollama = aictl_core::llm::ollama::list_models().await;
    if ollama.iter().any(|m| m == model) {
        return Ok(Provider::Ollama);
    }
    if aictl_core::llm::gguf::is_available()
        && aictl_core::llm::gguf::list_models()
            .iter()
            .any(|m| m == model)
    {
        return Ok(Provider::Gguf);
    }
    if aictl_core::llm::mlx::is_available()
        && aictl_core::llm::mlx::list_models()
            .iter()
            .any(|m| m == model)
    {
        return Ok(Provider::Mlx);
    }
    Err(ApiError::BadRequest {
        code: "model_not_found",
        message: format!("no provider knows how to serve model {model:?}"),
    })
}

fn provider_from_tag(tag: &str) -> Provider {
    match tag {
        "openai" => Provider::Openai,
        "anthropic" => Provider::Anthropic,
        "gemini" => Provider::Gemini,
        "grok" => Provider::Grok,
        "mistral" => Provider::Mistral,
        "deepseek" => Provider::Deepseek,
        "kimi" => Provider::Kimi,
        "zai" => Provider::Zai,
        _ => Provider::Openai, // defensive — MODELS only declares the tags above
    }
}

/// Resolve the env-style API-key name for a remote provider. Returns
/// `None` for local providers (Ollama/GGUF/MLX).
#[must_use]
pub fn key_name_for_provider(provider: &Provider) -> Option<&'static str> {
    match provider {
        Provider::Openai => Some("LLM_OPENAI_API_KEY"),
        Provider::Anthropic => Some("LLM_ANTHROPIC_API_KEY"),
        Provider::Gemini => Some("LLM_GEMINI_API_KEY"),
        Provider::Grok => Some("LLM_GROK_API_KEY"),
        Provider::Mistral => Some("LLM_MISTRAL_API_KEY"),
        Provider::Deepseek => Some("LLM_DEEPSEEK_API_KEY"),
        Provider::Kimi => Some("LLM_KIMI_API_KEY"),
        Provider::Zai => Some("LLM_ZAI_API_KEY"),
        Provider::Ollama | Provider::Gguf | Provider::Mlx | Provider::Mock => None,
    }
}

/// Reject requests that include OpenAI tool-calling fields, which we
/// don't translate in Phase 1. Plan §7 — surface a stable 400 code.
pub fn reject_tool_request(req: &ChatCompletionRequest) -> Result<(), ApiError> {
    if req.functions.is_some() {
        return Err(ApiError::BadRequest {
            code: "tools_unsupported_for_provider",
            message:
                "the legacy 'functions' field is not supported; use 'tools' instead, or omit it"
                    .to_string(),
        });
    }
    if let Some(tools) = &req.tools
        && tools.is_array()
        && !tools.as_array().is_some_and(Vec::is_empty)
    {
        return Err(ApiError::BadRequest {
            code: "tools_unsupported_for_provider",
            message: "tool-calling passthrough is not supported in this phase".to_string(),
        });
    }
    if let Some(choice) = &req.tool_choice
        && !choice.is_null()
        && !matches!(choice.as_str(), Some("none"))
    {
        return Err(ApiError::BadRequest {
            code: "tools_unsupported_for_provider",
            message: "tool_choice is not supported in this phase".to_string(),
        });
    }
    Ok(())
}

// --- Wrapping responses -----------------------------------------------------

#[must_use]
pub fn wrap_chat_response(
    request_id: &str,
    model: &str,
    content: String,
    usage: &TokenUsage,
) -> ChatCompletionResponse {
    let prompt_tokens =
        usage.input_tokens + usage.cache_creation_input_tokens + usage.cache_read_input_tokens;
    ChatCompletionResponse {
        id: format!("chatcmpl-{request_id}"),
        object: "chat.completion",
        created: now_secs(),
        model: model.to_string(),
        choices: vec![ChatCompletionChoice {
            index: 0,
            message: ChatCompletionResponseMessage {
                role: "assistant",
                content,
            },
            finish_reason: "stop",
        }],
        usage: ChatCompletionUsage {
            prompt_tokens,
            completion_tokens: usage.output_tokens,
            total_tokens: prompt_tokens + usage.output_tokens,
        },
    }
}

#[must_use]
pub fn wrap_completion_response(
    request_id: &str,
    model: &str,
    text: String,
    usage: &TokenUsage,
) -> CompletionResponse {
    let prompt_tokens =
        usage.input_tokens + usage.cache_creation_input_tokens + usage.cache_read_input_tokens;
    CompletionResponse {
        id: format!("cmpl-{request_id}"),
        object: "text_completion",
        created: now_secs(),
        model: model.to_string(),
        choices: vec![CompletionChoice {
            index: 0,
            text,
            finish_reason: "stop",
        }],
        usage: ChatCompletionUsage {
            prompt_tokens,
            completion_tokens: usage.output_tokens,
            total_tokens: prompt_tokens + usage.output_tokens,
        },
    }
}

#[must_use]
pub fn chunk(
    request_id: &str,
    model: &str,
    delta: Option<String>,
    role: bool,
) -> ChatCompletionChunk {
    ChatCompletionChunk {
        id: format!("chatcmpl-{request_id}"),
        object: "chat.completion.chunk",
        created: now_secs(),
        model: model.to_string(),
        choices: vec![ChatCompletionChunkChoice {
            index: 0,
            delta: ChatCompletionChunkDelta {
                role: if role { Some("assistant") } else { None },
                content: delta,
            },
            finish_reason: None,
        }],
    }
}

#[must_use]
pub fn final_chunk(request_id: &str, model: &str) -> ChatCompletionChunk {
    ChatCompletionChunk {
        id: format!("chatcmpl-{request_id}"),
        object: "chat.completion.chunk",
        created: now_secs(),
        model: model.to_string(),
        choices: vec![ChatCompletionChunkChoice {
            index: 0,
            delta: ChatCompletionChunkDelta::default(),
            finish_reason: Some("stop"),
        }],
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// --- Models listing ---------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct ModelsList {
    pub object: &'static str,
    pub data: Vec<ModelEntry>,
}

#[derive(Debug, Serialize)]
pub struct ModelEntry {
    pub id: String,
    pub object: &'static str,
    pub owned_by: String,
    pub context_window: u64,
    pub available: bool,
}

pub async fn list_models() -> ModelsList {
    let mut data = Vec::new();
    for (provider, model, key_name) in MODELS {
        let key_present = aictl_core::keys::get_secret(key_name).is_some();
        data.push(ModelEntry {
            id: (*model).to_string(),
            object: "model",
            owned_by: pretty_provider(provider).to_string(),
            context_window: aictl_core::llm::context_limit(model),
            available: key_present,
        });
    }
    for m in aictl_core::llm::ollama::list_models().await {
        data.push(ModelEntry {
            context_window: aictl_core::llm::context_limit(&m),
            id: m,
            object: "model",
            owned_by: "Ollama".to_string(),
            available: true,
        });
    }
    if aictl_core::llm::gguf::is_available() {
        for m in aictl_core::llm::gguf::list_models() {
            data.push(ModelEntry {
                context_window: aictl_core::llm::context_limit(&m),
                id: m,
                object: "model",
                owned_by: "GGUF".to_string(),
                available: true,
            });
        }
    }
    if aictl_core::llm::mlx::is_available() {
        for m in aictl_core::llm::mlx::list_models() {
            data.push(ModelEntry {
                context_window: aictl_core::llm::context_limit(&m),
                id: m,
                object: "model",
                owned_by: "MLX".to_string(),
                available: true,
            });
        }
    }
    ModelsList {
        object: "list",
        data,
    }
}

fn pretty_provider(tag: &str) -> &'static str {
    match tag {
        "openai" => "OpenAI",
        "anthropic" => "Anthropic",
        "gemini" => "Google",
        "grok" => "xAI",
        "mistral" => "Mistral",
        "deepseek" => "DeepSeek",
        "kimi" => "Moonshot",
        "zai" => "Z.ai",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_internal_rejects_empty() {
        let err = to_internal(vec![]).unwrap_err();
        assert!(matches!(
            err,
            ApiError::BadRequest {
                code: "body_malformed",
                ..
            }
        ));
    }

    #[test]
    fn to_internal_translates_roles() {
        let msgs = vec![
            ChatMessage {
                role: "system".to_string(),
                content: ChatContent::Text("sys".to_string()),
            },
            ChatMessage {
                role: "user".to_string(),
                content: ChatContent::Text("hi".to_string()),
            },
            ChatMessage {
                role: "assistant".to_string(),
                content: ChatContent::Text("hello".to_string()),
            },
        ];
        let out = to_internal(msgs).unwrap();
        assert_eq!(out.len(), 3);
        assert!(matches!(out[0].role, Role::System));
        assert!(matches!(out[1].role, Role::User));
        assert!(matches!(out[2].role, Role::Assistant));
    }

    #[test]
    fn to_internal_rejects_tool_role() {
        let msgs = vec![ChatMessage {
            role: "tool".to_string(),
            content: ChatContent::Text("x".to_string()),
        }];
        let err = to_internal(msgs).unwrap_err();
        assert!(matches!(
            err,
            ApiError::BadRequest {
                code: "tools_unsupported_for_provider",
                ..
            }
        ));
    }

    #[test]
    fn key_name_for_remote_providers() {
        assert_eq!(
            key_name_for_provider(&Provider::Openai),
            Some("LLM_OPENAI_API_KEY")
        );
        assert_eq!(
            key_name_for_provider(&Provider::Anthropic),
            Some("LLM_ANTHROPIC_API_KEY")
        );
        assert_eq!(key_name_for_provider(&Provider::Ollama), None);
        assert_eq!(key_name_for_provider(&Provider::Gguf), None);
    }

    #[test]
    fn reject_tools_payload() {
        let req = ChatCompletionRequest {
            model: "gpt-4o".to_string(),
            messages: vec![],
            stream: None,
            tools: Some(serde_json::json!([{"type": "function"}])),
            functions: None,
            temperature: None,
            top_p: None,
            max_tokens: None,
            tool_choice: None,
        };
        let err = reject_tool_request(&req).unwrap_err();
        assert!(matches!(
            err,
            ApiError::BadRequest {
                code: "tools_unsupported_for_provider",
                ..
            }
        ));
    }

    #[test]
    fn reject_legacy_functions() {
        let req = ChatCompletionRequest {
            model: "gpt-4o".to_string(),
            messages: vec![],
            stream: None,
            tools: None,
            functions: Some(serde_json::json!([{"name": "x"}])),
            temperature: None,
            top_p: None,
            max_tokens: None,
            tool_choice: None,
        };
        let err = reject_tool_request(&req).unwrap_err();
        assert!(matches!(
            err,
            ApiError::BadRequest {
                code: "tools_unsupported_for_provider",
                ..
            }
        ));
    }
}
