use serde::{Deserialize, Serialize};

use crate::{Message, Role};

#[derive(Serialize)]
struct OpenAiRequest {
    model: String,
    messages: Vec<OpenAiMessage>,
}

#[derive(Serialize, Deserialize)]
struct OpenAiMessage {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct OpenAiResponse {
    choices: Vec<OpenAiChoice>,
}

#[derive(Deserialize)]
struct OpenAiChoice {
    message: OpenAiMessage,
}

pub async fn call_openai(
    api_key: &str,
    model: &str,
    messages: &[Message],
) -> Result<String, Box<dyn std::error::Error>> {
    let client = reqwest::Client::new();

    let oai_messages: Vec<OpenAiMessage> = messages
        .iter()
        .map(|m| OpenAiMessage {
            role: match m.role {
                Role::System => "system".to_string(),
                Role::User => "user".to_string(),
                Role::Assistant => "assistant".to_string(),
            },
            content: m.content.clone(),
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
    parsed
        .choices
        .first()
        .map(|c| c.message.content.clone())
        .ok_or_else(|| "No response from OpenAI".into())
}
