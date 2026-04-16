use serde::{Deserialize, Serialize};

use crate::llm::TokenUsage;
use crate::{Message, Role};

#[derive(Serialize)]
struct ZaiRequest {
    model: String,
    messages: Vec<ZaiMessage>,
}

#[derive(Serialize)]
struct ZaiMessage {
    role: String,
    content: ZaiContent,
}

#[derive(Serialize)]
#[serde(untagged)]
enum ZaiContent {
    Text(String),
    Parts(Vec<ZaiContentPart>),
}

#[derive(Serialize)]
#[serde(untagged)]
enum ZaiContentPart {
    Text {
        #[serde(rename = "type")]
        part_type: String,
        text: String,
    },
    ImageUrl {
        #[serde(rename = "type")]
        part_type: String,
        image_url: ZaiImageUrl,
    },
}

#[derive(Serialize)]
struct ZaiImageUrl {
    url: String,
}

#[derive(Deserialize)]
struct ZaiResponseMessage {
    content: String,
}

#[derive(Deserialize)]
struct ZaiResponse {
    choices: Vec<ZaiChoice>,
    usage: Option<ZaiUsage>,
}

#[derive(Deserialize)]
struct ZaiChoice {
    message: ZaiResponseMessage,
}

#[derive(Deserialize)]
struct ZaiUsage {
    prompt_tokens: u64,
    completion_tokens: u64,
}

pub async fn call_zai(
    api_key: &str,
    model: &str,
    messages: &[Message],
) -> Result<(String, TokenUsage), Box<dyn std::error::Error>> {
    let client = crate::config::http_client();

    let zai_messages: Vec<ZaiMessage> = messages
        .iter()
        .map(|m| {
            let role = match m.role {
                Role::System => "system".to_string(),
                Role::User => "user".to_string(),
                Role::Assistant => "assistant".to_string(),
            };
            let content = if m.images.is_empty() {
                ZaiContent::Text(m.content.clone())
            } else {
                let mut parts = vec![ZaiContentPart::Text {
                    part_type: "text".to_string(),
                    text: m.content.clone(),
                }];
                for img in &m.images {
                    parts.push(ZaiContentPart::ImageUrl {
                        part_type: "image_url".to_string(),
                        image_url: ZaiImageUrl {
                            url: format!("data:{};base64,{}", img.media_type, img.base64_data),
                        },
                    });
                }
                ZaiContent::Parts(parts)
            };
            ZaiMessage { role, content }
        })
        .collect();

    let body = ZaiRequest {
        model: model.to_string(),
        messages: zai_messages,
    };

    let resp = client
        .post("https://api.z.ai/api/paas/v4/chat/completions")
        .header("Authorization", format!("Bearer {api_key}"))
        .json(&body)
        .send()
        .await?;

    let status = resp.status();
    let text = resp.text().await?;

    if !status.is_success() {
        return Err(format!("Z.ai API error ({status}): {text}").into());
    }

    let parsed: ZaiResponse = serde_json::from_str(&text)?;
    let content = parsed
        .choices
        .first()
        .map(|c| c.message.content.clone())
        .ok_or_else(|| -> Box<dyn std::error::Error> { "No response from Z.ai".into() })?;
    let usage = parsed
        .usage
        .map(|u| TokenUsage {
            input_tokens: u.prompt_tokens,
            output_tokens: u.completion_tokens,
            ..TokenUsage::default()
        })
        .unwrap_or_default();
    Ok((content, usage))
}
