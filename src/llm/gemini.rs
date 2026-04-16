use serde::{Deserialize, Serialize};

use crate::llm::TokenUsage;
use crate::{Message, Role};

// --- Request types ---

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiRequest {
    contents: Vec<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system_instruction: Option<GeminiContent>,
}

#[derive(Serialize, Deserialize)]
struct GeminiContent {
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<String>,
    parts: Vec<GeminiPart>,
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum GeminiPart {
    Text { text: String },
    InlineData { inline_data: GeminiInlineData },
}

#[derive(Serialize, Deserialize)]
struct GeminiInlineData {
    mime_type: String,
    data: String,
}

// --- Response types ---

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiResponse {
    candidates: Option<Vec<GeminiCandidate>>,
    usage_metadata: Option<GeminiUsageMetadata>,
}

#[derive(Deserialize)]
struct GeminiCandidate {
    content: GeminiContent,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(clippy::struct_field_names)]
struct GeminiUsageMetadata {
    #[serde(default)]
    prompt_token_count: u64,
    #[serde(default)]
    candidates_token_count: u64,
    // Implicit and explicit caching populate this field on a cache hit.
    // prompt_token_count includes these tokens.
    #[serde(default)]
    cached_content_token_count: u64,
}

pub async fn call_gemini(
    api_key: &str,
    model: &str,
    messages: &[Message],
) -> Result<(String, TokenUsage), Box<dyn std::error::Error>> {
    let client = crate::config::http_client();

    let mut system_text: Option<String> = None;
    let mut contents: Vec<GeminiContent> = Vec::new();

    for m in messages {
        match m.role {
            Role::System => {
                system_text = Some(m.content.clone());
            }
            Role::User => {
                let mut parts = vec![GeminiPart::Text {
                    text: m.content.clone(),
                }];
                for img in &m.images {
                    parts.push(GeminiPart::InlineData {
                        inline_data: GeminiInlineData {
                            mime_type: img.media_type.clone(),
                            data: img.base64_data.clone(),
                        },
                    });
                }
                contents.push(GeminiContent {
                    role: Some("user".to_string()),
                    parts,
                });
            }
            Role::Assistant => {
                contents.push(GeminiContent {
                    role: Some("model".to_string()),
                    parts: vec![GeminiPart::Text {
                        text: m.content.clone(),
                    }],
                });
            }
        }
    }

    let system_instruction = system_text.map(|text| GeminiContent {
        role: None,
        parts: vec![GeminiPart::Text { text }],
    });

    let body = GeminiRequest {
        contents,
        system_instruction,
    };

    let url = format!(
        "https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent?key={api_key}"
    );

    let resp = client
        .post(&url)
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await?;

    let status = resp.status();
    let text = resp.text().await?;

    if !status.is_success() {
        // Try to extract a concise error message from the JSON response
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
            let msg = json["error"]["message"]
                .as_str()
                .and_then(|m| m.lines().next())
                .unwrap_or("unknown error");
            let model_name = json["error"]["details"]
                .as_array()
                .and_then(|details| {
                    details.iter().find_map(|d| {
                        d["violations"].as_array()?.first()?["quotaDimensions"]["model"].as_str()
                    })
                })
                .unwrap_or(model);
            return Err(format!("Gemini API error ({status}): {msg} [model: {model_name}]").into());
        }
        return Err(format!("Gemini API error ({status}): {text}").into());
    }

    let parsed: GeminiResponse = serde_json::from_str(&text)?;
    let content = parsed
        .candidates
        .as_ref()
        .and_then(|c| c.first())
        .and_then(|c| c.content.parts.first())
        .and_then(|p| match p {
            GeminiPart::Text { text } => Some(text.clone()),
            GeminiPart::InlineData { .. } => None,
        })
        .ok_or_else(|| -> Box<dyn std::error::Error> { "No response from Gemini".into() })?;
    let usage = parsed
        .usage_metadata
        .map(|u| TokenUsage {
            input_tokens: u
                .prompt_token_count
                .saturating_sub(u.cached_content_token_count),
            output_tokens: u.candidates_token_count,
            cache_read_input_tokens: u.cached_content_token_count,
            ..TokenUsage::default()
        })
        .unwrap_or_default();
    Ok((content, usage))
}
