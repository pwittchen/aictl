use serde::{Deserialize, Serialize};

use crate::config::MAX_RESPONSE_TOKENS;
use crate::llm::{TokenSink, TokenUsage};
use crate::{Message, Role};

// --- Request types ---

#[derive(Serialize)]
struct AnthropicRequest {
    model: String,
    max_tokens: u32,
    messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<Vec<ContentBlock>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
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

#[derive(Serialize, Clone)]
struct ContentBlock {
    #[serde(rename = "type")]
    block_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<ImageSource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_control: Option<CacheControl>,
}

#[derive(Serialize, Clone)]
struct ImageSource {
    #[serde(rename = "type")]
    source_type: String,
    media_type: String,
    data: String,
}

#[derive(Serialize, Clone)]
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
        text: Some(text),
        source: None,
        cache_control: Some(CacheControl {
            control_type: "ephemeral".to_string(),
        }),
    }])
}

#[allow(clippy::too_many_lines)]
pub async fn call_anthropic(
    api_key: &str,
    model: &str,
    messages: &[Message],
    on_token: Option<TokenSink>,
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
                let content = if m.images.is_empty() {
                    MessageContent::Text(m.content.clone())
                } else {
                    let mut blocks = Vec::new();
                    for img in &m.images {
                        blocks.push(ContentBlock {
                            block_type: "image".to_string(),
                            text: None,
                            source: Some(ImageSource {
                                source_type: "base64".to_string(),
                                media_type: img.media_type.clone(),
                                data: img.base64_data.clone(),
                            }),
                            cache_control: None,
                        });
                    }
                    blocks.push(ContentBlock {
                        block_type: "text".to_string(),
                        text: Some(m.content.clone()),
                        source: None,
                        cache_control: None,
                    });
                    MessageContent::Blocks(blocks)
                };
                api_messages.push(AnthropicMessage {
                    role: "user".to_string(),
                    content,
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

    // Cache breakpoints. Anthropic caches the longest matching prefix and
    // allows up to 4 `cache_control` markers total (we use 1 for the system
    // prompt, leaving 3 for messages). We place two:
    //
    //   1. A stable breakpoint on the first user message. Re-marking it on
    //      every call refreshes the 5-minute TTL, so the conversation's early
    //      prefix stays cached across long-running sessions.
    //   2. A rolling breakpoint on the second-to-last message. This captures
    //      the growing conversation prefix between consecutive iterations of
    //      the agent loop within a single turn.
    //
    // For very short conversations (len < 3) the rolling breakpoint already
    // lands on the first message, so we only set one.
    let mark_cached = |msg: &mut AnthropicMessage| {
        match &msg.content {
            MessageContent::Text(t) => {
                msg.content = cached_block(t.clone());
            }
            MessageContent::Blocks(_) => {
                // Add cache_control to the last block, preserving image blocks
                if let MessageContent::Blocks(ref mut blocks) = msg.content
                    && let Some(last) = blocks.last_mut()
                {
                    last.cache_control = Some(CacheControl {
                        control_type: "ephemeral".to_string(),
                    });
                }
            }
        }
    };

    if api_messages.len() >= 2 {
        let rolling_idx = api_messages.len() - 2;
        mark_cached(&mut api_messages[rolling_idx]);
        // Add the stable breakpoint only if it's a distinct earlier message;
        // otherwise the rolling marker already covers message 0.
        if rolling_idx > 0 {
            mark_cached(&mut api_messages[0]);
        }
    }

    // System prompt: always cached
    let system = system_text.map(|text| {
        vec![ContentBlock {
            block_type: "text".to_string(),
            text: Some(text),
            source: None,
            cache_control: Some(CacheControl {
                control_type: "ephemeral".to_string(),
            }),
        }]
    });

    let stream = on_token.is_some();
    let body = AnthropicRequest {
        model: model.to_string(),
        max_tokens: MAX_RESPONSE_TOKENS,
        messages: api_messages,
        system,
        stream: stream.then_some(true),
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
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("Anthropic API error ({status}): {text}").into());
    }

    if let Some(sink) = on_token {
        return drive_anthropic_stream(resp, &sink).await;
    }

    let text = resp.text().await?;
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

/// Consume Anthropic's typed SSE stream. Forwards each `content_block_delta`
/// text fragment to `on_token`, accumulates the full text, and assembles a
/// final [`TokenUsage`].
///
/// Anthropic emits four event kinds we care about:
///   - `message_start`: `usage.input_tokens` (fresh), `cache_creation_input_tokens`,
///     `cache_read_input_tokens`. `output_tokens` here is just a small initial
///     placeholder; the authoritative count arrives in `message_delta`.
///   - `content_block_delta`: `delta.text` is the incremental text fragment.
///   - `message_delta`: `usage.output_tokens` is **cumulative across the whole
///     response**, not incremental — REPLACE on every event, never sum.
///   - `message_stop`: terminator; we just stop reading.
async fn drive_anthropic_stream(
    response: reqwest::Response,
    on_token: &TokenSink,
) -> Result<(String, TokenUsage), Box<dyn std::error::Error>> {
    use futures_util::StreamExt;

    let mut bytes = response.bytes_stream();
    let mut sse = crate::llm::stream::SseLines::new();
    let mut full = String::new();
    let mut usage = TokenUsage::default();

    while let Some(chunk) = bytes.next().await {
        let chunk = chunk?;
        for line in sse.push(&chunk) {
            let Some(payload) = line.strip_prefix("data:") else {
                continue;
            };
            let payload = payload.trim();
            if payload.is_empty() {
                continue;
            }
            let Ok(v) = serde_json::from_str::<serde_json::Value>(payload) else {
                continue;
            };
            let Some(event_type) = v.get("type").and_then(|t| t.as_str()) else {
                continue;
            };
            match event_type {
                "message_start" => {
                    if let Some(u) = v.get("message").and_then(|m| m.get("usage")) {
                        if let Some(t) = u.get("input_tokens").and_then(serde_json::Value::as_u64) {
                            usage.input_tokens = t;
                        }
                        if let Some(t) = u
                            .get("cache_creation_input_tokens")
                            .and_then(serde_json::Value::as_u64)
                        {
                            usage.cache_creation_input_tokens = t;
                        }
                        if let Some(t) = u
                            .get("cache_read_input_tokens")
                            .and_then(serde_json::Value::as_u64)
                        {
                            usage.cache_read_input_tokens = t;
                        }
                    }
                }
                "content_block_delta" => {
                    if let Some(text) = v
                        .get("delta")
                        .and_then(|d| d.get("text"))
                        .and_then(|t| t.as_str())
                        && !text.is_empty()
                    {
                        full.push_str(text);
                        on_token(text);
                    }
                }
                "message_delta" => {
                    // output_tokens here is cumulative across the whole
                    // response — replace, do not add.
                    if let Some(t) = v
                        .get("usage")
                        .and_then(|u| u.get("output_tokens"))
                        .and_then(serde_json::Value::as_u64)
                    {
                        usage.output_tokens = t;
                    }
                }
                "error" => {
                    let msg = v
                        .get("error")
                        .and_then(|e| e.get("message"))
                        .and_then(|m| m.as_str())
                        .unwrap_or("unknown stream error");
                    return Err(format!("Anthropic stream error: {msg}").into());
                }
                _ => {}
            }
        }
    }

    if full.is_empty() {
        return Err("No response from Anthropic".into());
    }
    Ok((full, usage))
}
