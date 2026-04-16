use serde::{Deserialize, Serialize};

use crate::llm::TokenUsage;
use crate::{Message, Role};

#[derive(Serialize)]
struct DeepSeekRequest {
    model: String,
    messages: Vec<DeepSeekMessage>,
}

#[derive(Serialize)]
struct DeepSeekMessage {
    role: String,
    content: DeepSeekContent,
}

#[derive(Serialize)]
#[serde(untagged)]
enum DeepSeekContent {
    Text(String),
    Parts(Vec<DeepSeekContentPart>),
}

#[derive(Serialize)]
#[serde(untagged)]
enum DeepSeekContentPart {
    Text {
        #[serde(rename = "type")]
        part_type: String,
        text: String,
    },
    ImageUrl {
        #[serde(rename = "type")]
        part_type: String,
        image_url: DeepSeekImageUrl,
    },
}

#[derive(Serialize)]
struct DeepSeekImageUrl {
    url: String,
}

#[derive(Deserialize)]
struct DeepSeekResponseMessage {
    content: String,
}

#[derive(Deserialize)]
struct DeepSeekResponse {
    choices: Vec<DeepSeekChoice>,
    usage: Option<DeepSeekUsage>,
}

#[derive(Deserialize)]
struct DeepSeekChoice {
    message: DeepSeekResponseMessage,
}

#[derive(Deserialize)]
#[allow(clippy::struct_field_names)]
struct DeepSeekUsage {
    prompt_tokens: u64,
    completion_tokens: u64,
    // DeepSeek splits prompt tokens into cache-hit / cache-miss counts.
    // prompt_tokens = hit + miss.
    #[serde(default)]
    prompt_cache_hit_tokens: u64,
}

pub async fn call_deepseek(
    api_key: &str,
    model: &str,
    messages: &[Message],
) -> Result<(String, TokenUsage), Box<dyn std::error::Error>> {
    let client = crate::config::http_client();

    let deepseek_messages: Vec<DeepSeekMessage> = messages
        .iter()
        .map(|m| {
            let role = match m.role {
                Role::System => "system".to_string(),
                Role::User => "user".to_string(),
                Role::Assistant => "assistant".to_string(),
            };
            let content = if m.images.is_empty() {
                DeepSeekContent::Text(m.content.clone())
            } else {
                let mut parts = vec![DeepSeekContentPart::Text {
                    part_type: "text".to_string(),
                    text: m.content.clone(),
                }];
                for img in &m.images {
                    parts.push(DeepSeekContentPart::ImageUrl {
                        part_type: "image_url".to_string(),
                        image_url: DeepSeekImageUrl {
                            url: format!("data:{};base64,{}", img.media_type, img.base64_data),
                        },
                    });
                }
                DeepSeekContent::Parts(parts)
            };
            DeepSeekMessage { role, content }
        })
        .collect();

    let body = DeepSeekRequest {
        model: model.to_string(),
        messages: deepseek_messages,
    };

    let resp = client
        .post("https://api.deepseek.com/chat/completions")
        .header("Authorization", format!("Bearer {api_key}"))
        .json(&body)
        .send()
        .await?;

    let status = resp.status();
    let text = resp.text().await?;

    if !status.is_success() {
        return Err(format!("DeepSeek API error ({status}): {text}").into());
    }

    let parsed: DeepSeekResponse = serde_json::from_str(&text)?;
    let content = parsed
        .choices
        .first()
        .map(|c| c.message.content.clone())
        .ok_or_else(|| -> Box<dyn std::error::Error> { "No response from DeepSeek".into() })?;
    let usage = parsed
        .usage
        .map(|u| TokenUsage {
            input_tokens: u.prompt_tokens.saturating_sub(u.prompt_cache_hit_tokens),
            output_tokens: u.completion_tokens,
            cache_read_input_tokens: u.prompt_cache_hit_tokens,
            ..TokenUsage::default()
        })
        .unwrap_or_default();
    Ok((content, usage))
}
