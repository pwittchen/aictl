use serde::{Deserialize, Serialize};

use crate::llm::TokenUsage;
use crate::{Message, Role};

#[derive(Serialize)]
struct KimiRequest {
    model: String,
    messages: Vec<KimiMessage>,
}

#[derive(Serialize, Deserialize)]
struct KimiMessage {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct KimiResponse {
    choices: Vec<KimiChoice>,
    usage: Option<KimiUsage>,
}

#[derive(Deserialize)]
struct KimiChoice {
    message: KimiMessage,
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

pub async fn call_kimi(
    api_key: &str,
    model: &str,
    messages: &[Message],
) -> Result<(String, TokenUsage), Box<dyn std::error::Error>> {
    let client = crate::config::http_client();

    let kimi_messages: Vec<KimiMessage> = messages
        .iter()
        .map(|m| KimiMessage {
            role: match m.role {
                Role::System => "system".to_string(),
                Role::User => "user".to_string(),
                Role::Assistant => "assistant".to_string(),
            },
            content: m.content.clone(),
        })
        .collect();

    let body = KimiRequest {
        model: model.to_string(),
        messages: kimi_messages,
    };

    let resp = client
        .post("https://api.moonshot.ai/v1/chat/completions")
        .header("Authorization", format!("Bearer {api_key}"))
        .json(&body)
        .send()
        .await?;

    let status = resp.status();
    let text = resp.text().await?;

    if !status.is_success() {
        return Err(format!("Kimi API error ({status}): {text}").into());
    }

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
