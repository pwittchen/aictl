use serde::{Deserialize, Serialize};

use crate::error::AictlError;
use crate::llm::{TokenSink, TokenUsage};
use crate::{Message, Role};

#[derive(Serialize)]
struct ZaiRequest {
    model: String,
    messages: Vec<ZaiMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_options: Option<ZaiStreamOptions>,
}

#[derive(Serialize)]
struct ZaiStreamOptions {
    include_usage: bool,
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
    on_token: Option<TokenSink>,
) -> Result<(String, TokenUsage), AictlError> {
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

    let stream = on_token.is_some();
    let body = ZaiRequest {
        model: model.to_string(),
        messages: zai_messages,
        stream: stream.then_some(true),
        stream_options: stream.then_some(ZaiStreamOptions {
            include_usage: true,
        }),
    };

    let resp = client
        .post("https://api.z.ai/api/paas/v4/chat/completions")
        .header("Authorization", format!("Bearer {api_key}"))
        .json(&body)
        .send()
        .await?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(AictlError::from_http("Z.ai", status, text));
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
            return Err(AictlError::EmptyResponse { provider: "Z.ai" });
        }
        return Ok((content, usage));
    }

    let text = resp.text().await?;
    let parsed: ZaiResponse = serde_json::from_str(&text)?;
    let content = parsed
        .choices
        .first()
        .map(|c| c.message.content.clone())
        .ok_or(AictlError::EmptyResponse { provider: "Z.ai" })?;
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
