use serde::{Deserialize, Serialize};

use crate::error::AictlError;
use crate::llm::{TokenSink, TokenUsage};
use crate::{Message, Role};

#[derive(Serialize)]
struct GrokRequest {
    model: String,
    messages: Vec<GrokMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_options: Option<GrokStreamOptions>,
}

#[derive(Serialize)]
struct GrokStreamOptions {
    include_usage: bool,
}

#[derive(Serialize)]
struct GrokMessage {
    role: String,
    content: GrokContent,
}

#[derive(Serialize)]
#[serde(untagged)]
enum GrokContent {
    Text(String),
    Parts(Vec<GrokContentPart>),
}

#[derive(Serialize)]
#[serde(untagged)]
enum GrokContentPart {
    Text {
        #[serde(rename = "type")]
        part_type: String,
        text: String,
    },
    ImageUrl {
        #[serde(rename = "type")]
        part_type: String,
        image_url: GrokImageUrl,
    },
}

#[derive(Serialize)]
struct GrokImageUrl {
    url: String,
}

#[derive(Deserialize)]
struct GrokResponseMessage {
    content: String,
}

#[derive(Deserialize)]
struct GrokResponse {
    choices: Vec<GrokChoice>,
    usage: Option<GrokUsage>,
}

#[derive(Deserialize)]
struct GrokChoice {
    message: GrokResponseMessage,
}

#[derive(Deserialize)]
struct GrokUsage {
    prompt_tokens: u64,
    completion_tokens: u64,
    #[serde(default)]
    prompt_tokens_details: Option<GrokPromptTokensDetails>,
}

#[derive(Deserialize, Default)]
struct GrokPromptTokensDetails {
    #[serde(default)]
    cached_tokens: u64,
}

pub async fn call_grok(
    api_key: &str,
    model: &str,
    messages: &[Message],
    on_token: Option<TokenSink>,
) -> Result<(String, TokenUsage), AictlError> {
    let client = crate::config::http_client();

    let grok_messages: Vec<GrokMessage> = messages
        .iter()
        .map(|m| {
            let role = match m.role {
                Role::System => "system".to_string(),
                Role::User => "user".to_string(),
                Role::Assistant => "assistant".to_string(),
            };
            let content = if m.images.is_empty() {
                GrokContent::Text(m.content.clone())
            } else {
                let mut parts = vec![GrokContentPart::Text {
                    part_type: "text".to_string(),
                    text: m.content.clone(),
                }];
                for img in &m.images {
                    parts.push(GrokContentPart::ImageUrl {
                        part_type: "image_url".to_string(),
                        image_url: GrokImageUrl {
                            url: format!("data:{};base64,{}", img.media_type, img.base64_data),
                        },
                    });
                }
                GrokContent::Parts(parts)
            };
            GrokMessage { role, content }
        })
        .collect();

    let stream = on_token.is_some();
    let body = GrokRequest {
        model: model.to_string(),
        messages: grok_messages,
        stream: stream.then_some(true),
        stream_options: stream.then_some(GrokStreamOptions {
            include_usage: true,
        }),
    };

    let resp = client
        .post("https://api.x.ai/v1/chat/completions")
        .header("Authorization", format!("Bearer {api_key}"))
        .json(&body)
        .send()
        .await?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(AictlError::from_http("Grok", status, text));
    }

    if let Some(sink) = on_token {
        let (content, usage) = crate::llm::stream::drive_openai_compatible_stream(
            resp,
            &sink,
            crate::llm::openai::parse_openai_usage,
        )
        .await?;
        if content.is_empty() {
            return Err(AictlError::EmptyResponse { provider: "Grok" });
        }
        return Ok((content, usage));
    }

    let text = resp.text().await?;
    let parsed: GrokResponse = serde_json::from_str(&text)?;
    let content = parsed
        .choices
        .first()
        .map(|c| c.message.content.clone())
        .ok_or(AictlError::EmptyResponse { provider: "Grok" })?;
    let usage = parsed
        .usage
        .map(|u| {
            let cached = u.prompt_tokens_details.unwrap_or_default().cached_tokens;
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
