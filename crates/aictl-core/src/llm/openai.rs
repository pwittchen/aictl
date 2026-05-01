use serde::{Deserialize, Serialize};

use crate::error::AictlError;
use crate::llm::{TokenSink, TokenUsage};
use crate::{Message, Role};

// Visibility note: the request/response shapes are `pub(crate)` because
// `llm::server_proxy` reuses them when relaying chat completions to an
// `aictl-server` upstream. Both ends speak the OpenAI shape, and
// duplicating the structs would let them drift independently.

#[derive(Serialize)]
pub(crate) struct OpenAiRequest {
    pub model: String,
    pub messages: Vec<OpenAiMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_options: Option<StreamOptions>,
}

#[derive(Serialize)]
pub(crate) struct StreamOptions {
    pub include_usage: bool,
}

#[derive(Serialize)]
pub(crate) struct OpenAiMessage {
    pub role: String,
    pub content: OpenAiContent,
}

#[derive(Serialize)]
#[serde(untagged)]
pub(crate) enum OpenAiContent {
    Text(String),
    Parts(Vec<OpenAiContentPart>),
}

#[derive(Serialize)]
#[serde(untagged)]
pub(crate) enum OpenAiContentPart {
    Text {
        #[serde(rename = "type")]
        part_type: String,
        text: String,
    },
    ImageUrl {
        #[serde(rename = "type")]
        part_type: String,
        image_url: OpenAiImageUrl,
    },
}

#[derive(Serialize)]
pub(crate) struct OpenAiImageUrl {
    pub url: String,
}

#[derive(Deserialize)]
pub(crate) struct OpenAiResponseMessage {
    pub content: String,
}

#[derive(Deserialize)]
pub(crate) struct OpenAiResponse {
    pub choices: Vec<OpenAiChoice>,
    pub usage: Option<OpenAiUsage>,
}

#[derive(Deserialize)]
pub(crate) struct OpenAiChoice {
    pub message: OpenAiResponseMessage,
}

#[derive(Deserialize)]
pub(crate) struct OpenAiUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    #[serde(default)]
    pub prompt_tokens_details: Option<OpenAiPromptTokensDetails>,
}

#[derive(Deserialize, Default)]
pub(crate) struct OpenAiPromptTokensDetails {
    #[serde(default)]
    pub cached_tokens: u64,
}

pub(crate) fn build_messages(messages: &[Message]) -> Vec<OpenAiMessage> {
    messages
        .iter()
        .map(|m| {
            let role = match m.role {
                Role::System => "system".to_string(),
                Role::User => "user".to_string(),
                Role::Assistant => "assistant".to_string(),
            };
            let content = if m.images.is_empty() {
                OpenAiContent::Text(m.content.clone())
            } else {
                let mut parts = vec![OpenAiContentPart::Text {
                    part_type: "text".to_string(),
                    text: m.content.clone(),
                }];
                for img in &m.images {
                    parts.push(OpenAiContentPart::ImageUrl {
                        part_type: "image_url".to_string(),
                        image_url: OpenAiImageUrl {
                            url: format!("data:{};base64,{}", img.media_type, img.base64_data),
                        },
                    });
                }
                OpenAiContent::Parts(parts)
            };
            OpenAiMessage { role, content }
        })
        .collect()
}

/// Pull a `TokenUsage` out of any streamed event JSON that carries `OpenAI`'s
/// `usage` object. Returns `None` for events without it (most of them).
/// Shared with grok/mistral/zai which use the same shape, and with
/// `llm::server_proxy` (and a server-side roundtrip test) so the CLI's
/// proxy path bills cached input at the discounted rate.
pub fn parse_openai_usage(v: &serde_json::Value) -> Option<TokenUsage> {
    let u = v.get("usage")?;
    let prompt = u.get("prompt_tokens")?.as_u64()?;
    let completion = u
        .get("completion_tokens")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let cached = u
        .get("prompt_tokens_details")
        .and_then(|d| d.get("cached_tokens"))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    Some(TokenUsage {
        input_tokens: prompt.saturating_sub(cached),
        output_tokens: completion,
        cache_read_input_tokens: cached,
        ..TokenUsage::default()
    })
}

pub async fn call_openai(
    api_key: &str,
    model: &str,
    messages: &[Message],
    on_token: Option<TokenSink>,
) -> Result<(String, TokenUsage), AictlError> {
    let client = crate::config::http_client();
    let oai_messages = build_messages(messages);

    let stream = on_token.is_some();
    let body = OpenAiRequest {
        model: model.to_string(),
        messages: oai_messages,
        stream: stream.then_some(true),
        stream_options: stream.then_some(StreamOptions {
            include_usage: true,
        }),
    };

    let resp = client
        .post("https://api.openai.com/v1/chat/completions")
        .header("Authorization", format!("Bearer {api_key}"))
        .json(&body)
        .send()
        .await?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(AictlError::from_http("OpenAI", status, text));
    }

    if let Some(sink) = on_token {
        let (content, usage) =
            crate::llm::stream::drive_openai_compatible_stream(resp, &sink, parse_openai_usage)
                .await?;
        if content.is_empty() {
            return Err(AictlError::EmptyResponse { provider: "OpenAI" });
        }
        return Ok((content, usage));
    }

    let text = resp.text().await?;
    let parsed: OpenAiResponse = serde_json::from_str(&text)?;
    let content = parsed
        .choices
        .first()
        .map(|c| c.message.content.clone())
        .unwrap_or_default();
    if content.is_empty() {
        return Err(AictlError::EmptyResponse { provider: "OpenAI" });
    }
    let usage = parsed
        .usage
        .map(|u| {
            let cached = u.prompt_tokens_details.unwrap_or_default().cached_tokens;
            // prompt_tokens is inclusive of cached_tokens; subtract so fresh
            // input is billed at full price and cached at the discount rate.
            TokenUsage {
                input_tokens: u.prompt_tokens.saturating_sub(cached),
                output_tokens: u.completion_tokens,
                cache_read_input_tokens: cached,
                ..TokenUsage::default()
            }
        })
        .unwrap_or_default();
    Ok((content, usage))
}
