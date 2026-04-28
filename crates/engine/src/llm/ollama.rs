use serde::{Deserialize, Serialize};

use crate::error::AictlError;
use crate::llm::{TokenSink, TokenUsage};
use crate::{Message, Role};

const DEFAULT_BASE_URL: &str = "http://localhost:11434";

/// Return the Ollama base URL from config or the default.
fn base_url() -> String {
    crate::config::config_get("LLM_OLLAMA_HOST").unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
}

#[derive(Serialize)]
struct OllamaRequest {
    model: String,
    messages: Vec<OllamaMessage>,
    stream: bool,
}

#[derive(Serialize)]
struct OllamaMessage {
    role: String,
    content: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    images: Vec<String>,
}

#[derive(Deserialize)]
struct OllamaResponseMessage {
    content: String,
}

#[derive(Deserialize)]
struct OllamaResponse {
    message: Option<OllamaResponseMessage>,
    prompt_eval_count: Option<u64>,
    eval_count: Option<u64>,
}

#[derive(Deserialize)]
struct OllamaTagsResponse {
    models: Option<Vec<OllamaModelEntry>>,
}

#[derive(Deserialize)]
struct OllamaModelEntry {
    name: String,
}

/// Fetch the list of locally available Ollama model names.
/// Returns an empty vec if Ollama is not running or errors.
pub async fn list_models() -> Vec<String> {
    let url = format!("{}/api/tags", base_url());
    let client = crate::config::http_client();
    let resp = client
        .get(&url)
        .timeout(std::time::Duration::from_secs(2))
        .send()
        .await;

    let Ok(resp) = resp else {
        return Vec::new();
    };

    let Ok(text) = resp.text().await else {
        return Vec::new();
    };

    let Ok(parsed) = serde_json::from_str::<OllamaTagsResponse>(&text) else {
        return Vec::new();
    };

    parsed
        .models
        .unwrap_or_default()
        .into_iter()
        .map(|m| m.name)
        .collect()
}

pub async fn call_ollama(
    model: &str,
    messages: &[Message],
    on_token: Option<TokenSink>,
) -> Result<(String, TokenUsage), AictlError> {
    let client = crate::config::http_client();
    let url = format!("{}/api/chat", base_url());

    let ollama_messages: Vec<OllamaMessage> = messages
        .iter()
        .map(|m| OllamaMessage {
            role: match m.role {
                Role::System => "system".to_string(),
                Role::User => "user".to_string(),
                Role::Assistant => "assistant".to_string(),
            },
            content: m.content.clone(),
            images: m.images.iter().map(|img| img.base64_data.clone()).collect(),
        })
        .collect();

    let stream = on_token.is_some();
    let body = OllamaRequest {
        model: model.to_string(),
        messages: ollama_messages,
        stream,
    };

    let resp = client.post(&url).json(&body).send().await?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(AictlError::from_http("Ollama", status, text));
    }

    if let Some(sink) = on_token {
        return drive_ollama_stream(resp, &sink).await;
    }

    let text = resp.text().await?;
    let parsed: OllamaResponse = serde_json::from_str(&text)?;
    let content = parsed
        .message
        .map(|m| m.content)
        .ok_or(AictlError::EmptyResponse { provider: "Ollama" })?;
    let usage = TokenUsage {
        input_tokens: parsed.prompt_eval_count.unwrap_or(0),
        output_tokens: parsed.eval_count.unwrap_or(0),
        ..TokenUsage::default()
    };
    Ok((content, usage))
}

/// Consume Ollama's JSONL streaming response. Each line is a complete
/// `OllamaResponse`-shaped JSON object. The final line carries `done: true`
/// and the cumulative `prompt_eval_count` / `eval_count`.
async fn drive_ollama_stream(
    response: reqwest::Response,
    on_token: &TokenSink,
) -> Result<(String, TokenUsage), AictlError> {
    use futures_util::StreamExt;

    let mut bytes = response.bytes_stream();
    let mut sse = crate::llm::stream::SseLines::new();
    let mut full = String::new();
    let mut usage = TokenUsage::default();

    while let Some(chunk) = bytes.next().await {
        let chunk = chunk?;
        for line in sse.push(&chunk) {
            if line.is_empty() {
                continue;
            }
            let Ok(parsed) = serde_json::from_str::<OllamaResponse>(&line) else {
                continue;
            };
            if let Some(msg) = parsed.message
                && !msg.content.is_empty()
            {
                full.push_str(&msg.content);
                on_token(&msg.content);
            }
            if parsed.prompt_eval_count.is_some() || parsed.eval_count.is_some() {
                usage = TokenUsage {
                    input_tokens: parsed.prompt_eval_count.unwrap_or(usage.input_tokens),
                    output_tokens: parsed.eval_count.unwrap_or(usage.output_tokens),
                    ..TokenUsage::default()
                };
            }
        }
    }
    // Drain any unterminated trailing line.
    if let Some(line) = sse.take_remaining()
        && let Ok(parsed) = serde_json::from_str::<OllamaResponse>(&line)
    {
        if let Some(msg) = parsed.message
            && !msg.content.is_empty()
        {
            full.push_str(&msg.content);
            on_token(&msg.content);
        }
        if parsed.prompt_eval_count.is_some() || parsed.eval_count.is_some() {
            usage = TokenUsage {
                input_tokens: parsed.prompt_eval_count.unwrap_or(usage.input_tokens),
                output_tokens: parsed.eval_count.unwrap_or(usage.output_tokens),
                ..TokenUsage::default()
            };
        }
    }

    if full.is_empty() {
        return Err(AictlError::EmptyResponse { provider: "Ollama" });
    }
    Ok((full, usage))
}
