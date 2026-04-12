use serde::{Deserialize, Serialize};

use crate::llm::TokenUsage;
use crate::{Message, Role};

#[derive(Serialize)]
struct OpenAiRequest {
    model: String,
    messages: Vec<OpenAiMessage>,
}

#[derive(Serialize)]
struct OpenAiMessage {
    role: String,
    content: OpenAiContent,
}

#[derive(Serialize)]
#[serde(untagged)]
enum OpenAiContent {
    Text(String),
    Parts(Vec<OpenAiContentPart>),
}

#[derive(Serialize)]
#[serde(untagged)]
enum OpenAiContentPart {
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
struct OpenAiImageUrl {
    url: String,
}

#[derive(Deserialize)]
struct OpenAiResponseMessage {
    content: String,
}

#[derive(Deserialize)]
struct OpenAiResponse {
    choices: Vec<OpenAiChoice>,
    usage: Option<OpenAiUsage>,
}

#[derive(Deserialize)]
struct OpenAiChoice {
    message: OpenAiResponseMessage,
}

#[derive(Deserialize)]
struct OpenAiUsage {
    prompt_tokens: u64,
    completion_tokens: u64,
    #[serde(default)]
    prompt_tokens_details: Option<OpenAiPromptTokensDetails>,
}

#[derive(Deserialize, Default)]
struct OpenAiPromptTokensDetails {
    #[serde(default)]
    cached_tokens: u64,
}

pub async fn call_openai(
    api_key: &str,
    model: &str,
    messages: &[Message],
) -> Result<(String, TokenUsage), Box<dyn std::error::Error>> {
    let client = crate::config::http_client();

    let oai_messages: Vec<OpenAiMessage> = messages
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
        .collect();

    let body = OpenAiRequest {
        model: model.to_string(),
        messages: oai_messages,
    };

    let resp = client
        .post("https://api.openai.com/v1/chat/completions")
        .header("Authorization", format!("Bearer {api_key}"))
        .json(&body)
        .send()
        .await?;

    let status = resp.status();
    let text = resp.text().await?;

    if !status.is_success() {
        return Err(format!("OpenAI API error ({status}): {text}").into());
    }

    let parsed: OpenAiResponse = serde_json::from_str(&text)?;
    let content = parsed
        .choices
        .first()
        .map(|c| c.message.content.clone())
        .unwrap_or_default();
    if content.is_empty() {
        return Err("No response from OpenAI".into());
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
