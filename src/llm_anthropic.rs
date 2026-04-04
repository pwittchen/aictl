use serde::{Deserialize, Serialize};

use crate::config::MAX_RESPONSE_TOKENS;
use crate::llm::TokenUsage;
use crate::{Message, Role};

// --- Request types ---

#[derive(Serialize)]
struct AnthropicRequest {
    model: String,
    max_tokens: u32,
    messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<Vec<ContentBlock>>,
}

#[derive(Serialize)]
struct AnthropicMessage {
    role: String,
    content: MessageContent,
}

#[derive(Serialize)]
#[serde(untagged)]
enum MessageContent {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

#[derive(Serialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    block_type: String,
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_control: Option<CacheControl>,
}

#[derive(Serialize)]
struct CacheControl {
    #[serde(rename = "type")]
    control_type: String,
}

// --- Response types ---

#[derive(Deserialize)]
struct AnthropicResponse {
    content: Vec<AnthropicResponseContent>,
    usage: Option<AnthropicUsage>,
}

#[derive(Deserialize)]
struct AnthropicResponseContent {
    text: String,
}

#[derive(Deserialize)]
#[allow(clippy::struct_field_names)]
struct AnthropicUsage {
    input_tokens: u64,
    output_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
}

// --- Helpers ---

fn cached_block(text: String) -> MessageContent {
    MessageContent::Blocks(vec![ContentBlock {
        block_type: "text".to_string(),
        text,
        cache_control: Some(CacheControl {
            control_type: "ephemeral".to_string(),
        }),
    }])
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
                    content: MessageContent::Text(m.content.clone()),
                });
            }
            Role::Assistant => {
                api_messages.push(AnthropicMessage {
                    role: "assistant".to_string(),
                    content: MessageContent::Text(m.content.clone()),
                });
            }
        }
    }

    // Place a cache breakpoint on the second-to-last message so the
    // conversation prefix is cached between consecutive calls within a turn.
    if api_messages.len() >= 2 {
        let idx = api_messages.len() - 2;
        let text = match &api_messages[idx].content {
            MessageContent::Text(t) => t.clone(),
            MessageContent::Blocks(blocks) => {
                blocks.first().map_or(String::new(), |b| b.text.clone())
            }
        };
        api_messages[idx].content = cached_block(text);
    }

    // System prompt: always cached
    let system = system_text.map(|text| {
        vec![ContentBlock {
            block_type: "text".to_string(),
            text,
            cache_control: Some(CacheControl {
                control_type: "ephemeral".to_string(),
            }),
        }]
    });

    let body = AnthropicRequest {
        model: model.to_string(),
        max_tokens: MAX_RESPONSE_TOKENS,
        messages: api_messages,
        system,
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
            cache_creation_input_tokens: u.cache_creation_input_tokens,
            cache_read_input_tokens: u.cache_read_input_tokens,
        })
        .unwrap_or_default();
    Ok((content, usage))
}
