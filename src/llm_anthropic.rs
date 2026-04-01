use serde::{Deserialize, Serialize};

use crate::config::MAX_RESPONSE_TOKENS;
use crate::llm::TokenUsage;
use crate::{Message, Role};

#[derive(Serialize)]
struct AnthropicRequest {
    model: String,
    max_tokens: u32,
    messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
}

#[derive(Serialize)]
struct AnthropicMessage {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct AnthropicResponse {
    content: Vec<AnthropicContent>,
    usage: Option<AnthropicUsage>,
}

#[derive(Deserialize)]
struct AnthropicContent {
    text: String,
}

#[derive(Deserialize)]
struct AnthropicUsage {
    input_tokens: u64,
    output_tokens: u64,
}

pub async fn call_anthropic(
    api_key: &str,
    model: &str,
    messages: &[Message],
) -> Result<(String, TokenUsage), Box<dyn std::error::Error>> {
    let client = crate::config::http_client();

    let mut system_text: Option<String> = None;
    let mut api_messages: Vec<AnthropicMessage> = Vec::new();

    for m in messages {
        match m.role {
            Role::System => {
                system_text = Some(m.content.clone());
            }
            Role::User => {
                api_messages.push(AnthropicMessage {
                    role: "user".to_string(),
                    content: m.content.clone(),
                });
            }
            Role::Assistant => {
                api_messages.push(AnthropicMessage {
                    role: "assistant".to_string(),
                    content: m.content.clone(),
                });
            }
        }
    }

    let body = AnthropicRequest {
        model: model.to_string(),
        max_tokens: MAX_RESPONSE_TOKENS,
        messages: api_messages,
        system: system_text,
    };

    let resp = client
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await?;

    let status = resp.status();
    let text = resp.text().await?;

    if !status.is_success() {
        return Err(format!("Anthropic API error ({status}): {text}").into());
    }

    let parsed: AnthropicResponse = serde_json::from_str(&text)?;
    let content = parsed
        .content
        .first()
        .map(|c| c.text.clone())
        .ok_or_else(|| -> Box<dyn std::error::Error> { "No response from Anthropic".into() })?;
    let usage = parsed
        .usage
        .map(|u| TokenUsage {
            input_tokens: u.input_tokens,
            output_tokens: u.output_tokens,
        })
        .unwrap_or_default();
    Ok((content, usage))
}
