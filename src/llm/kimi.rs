use serde::{Deserialize, Serialize};

use crate::llm::{TokenSink, TokenUsage};
use crate::{Message, Role};

#[derive(Serialize)]
struct KimiRequest {
    model: String,
    messages: Vec<KimiMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_options: Option<KimiStreamOptions>,
}

#[derive(Serialize)]
struct KimiStreamOptions {
    include_usage: bool,
}

#[derive(Serialize)]
struct KimiMessage {
    role: String,
    content: KimiContent,
}

#[derive(Serialize)]
#[serde(untagged)]
enum KimiContent {
    Text(String),
    Parts(Vec<KimiContentPart>),
}

#[derive(Serialize)]
#[serde(untagged)]
enum KimiContentPart {
    Text {
        #[serde(rename = "type")]
        part_type: String,
        text: String,
    },
    ImageUrl {
        #[serde(rename = "type")]
        part_type: String,
        image_url: KimiImageUrl,
    },
}

#[derive(Serialize)]
struct KimiImageUrl {
    url: String,
}

#[derive(Deserialize)]
struct KimiResponseMessage {
    content: String,
}

#[derive(Deserialize)]
struct KimiResponse {
    choices: Vec<KimiChoice>,
    usage: Option<KimiUsage>,
}

#[derive(Deserialize)]
struct KimiChoice {
    message: KimiResponseMessage,
}

#[derive(Deserialize)]
struct KimiUsage {
    prompt_tokens: u64,
    completion_tokens: u64,
    #[serde(default)]
    prompt_tokens_details: Option<KimiPromptTokensDetails>,
    #[serde(default)]
    cached_tokens: u64,
}

#[derive(Deserialize, Default)]
struct KimiPromptTokensDetails {
    #[serde(default)]
    cached_tokens: u64,
}

#[allow(clippy::too_many_lines)]
pub async fn call_kimi(
    api_key: &str,
    model: &str,
    messages: &[Message],
    on_token: Option<TokenSink>,
) -> Result<(String, TokenUsage), Box<dyn std::error::Error>> {
    let client = crate::config::http_client();

    let kimi_messages: Vec<KimiMessage> = messages
        .iter()
        .map(|m| {
            let role = match m.role {
                Role::System => "system".to_string(),
                Role::User => "user".to_string(),
                Role::Assistant => "assistant".to_string(),
            };
            let content = if m.images.is_empty() {
                KimiContent::Text(m.content.clone())
            } else {
                let mut parts = vec![KimiContentPart::Text {
                    part_type: "text".to_string(),
                    text: m.content.clone(),
                }];
                for img in &m.images {
                    parts.push(KimiContentPart::ImageUrl {
                        part_type: "image_url".to_string(),
                        image_url: KimiImageUrl {
                            url: format!("data:{};base64,{}", img.media_type, img.base64_data),
                        },
                    });
                }
                KimiContent::Parts(parts)
            };
            KimiMessage { role, content }
        })
        .collect();

    let stream = on_token.is_some();
    let body = KimiRequest {
        model: model.to_string(),
        messages: kimi_messages,
        stream: stream.then_some(true),
        stream_options: stream.then_some(KimiStreamOptions {
            include_usage: true,
        }),
    };

    let resp = client
        .post("https://api.moonshot.ai/v1/chat/completions")
        .header("Authorization", format!("Bearer {api_key}"))
        .json(&body)
        .send()
        .await?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("Kimi API error ({status}): {text}").into());
    }

    if let Some(sink) = on_token {
        let (content, usage) =
            crate::llm::stream::drive_openai_compatible_stream(resp, &sink, |v| {
                let u = v.get("usage")?;
                let prompt = u.get("prompt_tokens")?.as_u64()?;
                let completion = u
                    .get("completion_tokens")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0);
                let nested = u
                    .get("prompt_tokens_details")
                    .and_then(|d| d.get("cached_tokens"))
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0);
                let top = u
                    .get("cached_tokens")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0);
                let cached = nested.max(top);
                Some(TokenUsage {
                    input_tokens: prompt.saturating_sub(cached),
                    output_tokens: completion,
                    cache_read_input_tokens: cached,
                    ..TokenUsage::default()
                })
            })
            .await?;
        if content.is_empty() {
            return Err("No response from Kimi".into());
        }
        return Ok((content, usage));
    }

    let text = resp.text().await?;
    let parsed: KimiResponse = serde_json::from_str(&text)?;
    let content = parsed
        .choices
        .first()
        .map(|c| c.message.content.clone())
        .ok_or_else(|| -> Box<dyn std::error::Error> { "No response from Kimi".into() })?;
    let usage = parsed
        .usage
        .map(|u| {
            // Moonshot has reported cached_tokens both nested under
            // prompt_tokens_details (newer OpenAI-compat schema) and at the
            // top level of usage (older schema). Take whichever is populated.
            let nested = u
                .prompt_tokens_details
                .as_ref()
                .map_or(0, |d| d.cached_tokens);
            let cached = nested.max(u.cached_tokens);
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
