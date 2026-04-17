use serde::{Deserialize, Serialize};

use crate::llm::{TokenSink, TokenUsage};
use crate::{Message, Role};

#[derive(Serialize)]
struct MistralRequest {
    model: String,
    messages: Vec<MistralMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
}

#[derive(Serialize)]
struct MistralMessage {
    role: String,
    content: MistralContent,
}

#[derive(Serialize)]
#[serde(untagged)]
enum MistralContent {
    Text(String),
    Parts(Vec<MistralContentPart>),
}

#[derive(Serialize)]
#[serde(untagged)]
enum MistralContentPart {
    Text {
        #[serde(rename = "type")]
        part_type: String,
        text: String,
    },
    ImageUrl {
        #[serde(rename = "type")]
        part_type: String,
        image_url: MistralImageUrl,
    },
}

#[derive(Serialize)]
struct MistralImageUrl {
    url: String,
}

#[derive(Deserialize)]
struct MistralResponseMessage {
    content: String,
}

#[derive(Deserialize)]
struct MistralResponse {
    choices: Vec<MistralChoice>,
    usage: Option<MistralUsage>,
}

#[derive(Deserialize)]
struct MistralChoice {
    message: MistralResponseMessage,
}

#[derive(Deserialize)]
struct MistralUsage {
    prompt_tokens: u64,
    completion_tokens: u64,
}

pub async fn call_mistral(
    api_key: &str,
    model: &str,
    messages: &[Message],
    on_token: Option<TokenSink>,
) -> Result<(String, TokenUsage), Box<dyn std::error::Error>> {
    let client = crate::config::http_client();

    let mistral_messages: Vec<MistralMessage> = messages
        .iter()
        .map(|m| {
            let role = match m.role {
                Role::System => "system".to_string(),
                Role::User => "user".to_string(),
                Role::Assistant => "assistant".to_string(),
            };
            let content = if m.images.is_empty() {
                MistralContent::Text(m.content.clone())
            } else {
                let mut parts = vec![MistralContentPart::Text {
                    part_type: "text".to_string(),
                    text: m.content.clone(),
                }];
                for img in &m.images {
                    parts.push(MistralContentPart::ImageUrl {
                        part_type: "image_url".to_string(),
                        image_url: MistralImageUrl {
                            url: format!("data:{};base64,{}", img.media_type, img.base64_data),
                        },
                    });
                }
                MistralContent::Parts(parts)
            };
            MistralMessage { role, content }
        })
        .collect();

    let stream = on_token.is_some();
    let body = MistralRequest {
        model: model.to_string(),
        messages: mistral_messages,
        stream: stream.then_some(true),
    };

    let resp = client
        .post("https://api.mistral.ai/v1/chat/completions")
        .header("Authorization", format!("Bearer {api_key}"))
        .json(&body)
        .send()
        .await?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("Mistral API error ({status}): {text}").into());
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
                Some(TokenUsage {
                    input_tokens: prompt,
                    output_tokens: completion,
                    ..TokenUsage::default()
                })
            })
            .await?;
        if content.is_empty() {
            return Err("No response from Mistral".into());
        }
        return Ok((content, usage));
    }

    let text = resp.text().await?;
    let parsed: MistralResponse = serde_json::from_str(&text)?;
    let content = parsed
        .choices
        .first()
        .map(|c| c.message.content.clone())
        .ok_or_else(|| -> Box<dyn std::error::Error> { "No response from Mistral".into() })?;
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
