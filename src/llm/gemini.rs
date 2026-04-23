use serde::{Deserialize, Serialize};

use crate::error::AictlError;
use crate::llm::{TokenSink, TokenUsage};
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
    on_token: Option<TokenSink>,
) -> Result<(String, TokenUsage), AictlError> {
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

    let url = if on_token.is_some() {
        format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{model}:streamGenerateContent?alt=sse&key={api_key}"
        )
    } else {
        format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent?key={api_key}"
        )
    };

    let resp = client
        .post(&url)
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return surface_gemini_error(status, &text, model);
    }

    if let Some(sink) = on_token {
        return drive_gemini_stream(resp, &sink).await;
    }

    let text = resp.text().await?;
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
        .ok_or(AictlError::EmptyResponse { provider: "Gemini" })?;
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

/// Format a Gemini API error response. Tries to pull `error.message` and the
/// quota-violation model out of the JSON body; falls back to raw text.
fn surface_gemini_error(
    status: reqwest::StatusCode,
    text: &str,
    model: &str,
) -> Result<(String, TokenUsage), AictlError> {
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(text) {
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
        return Err(AictlError::from_http(
            "Gemini",
            status,
            format!("{msg} [model: {model_name}]"),
        ));
    }
    Err(AictlError::from_http("Gemini", status, text.to_string()))
}

/// Consume Gemini's `streamGenerateContent?alt=sse` stream. Each event carries
/// a partial `candidates[0].content.parts[].text` plus a cumulative
/// `usage_metadata` block — last one wins.
async fn drive_gemini_stream(
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
            if let Some(parts) = v["candidates"][0]["content"]["parts"].as_array() {
                for p in parts {
                    if let Some(t) = p.get("text").and_then(|t| t.as_str())
                        && !t.is_empty()
                    {
                        full.push_str(t);
                        on_token(t);
                    }
                }
            }
            if let Some(u) = v.get("usageMetadata") {
                let prompt = u
                    .get("promptTokenCount")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0);
                let cached = u
                    .get("cachedContentTokenCount")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0);
                let candidates = u
                    .get("candidatesTokenCount")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0);
                // Cumulative — replace, do not sum.
                usage = TokenUsage {
                    input_tokens: prompt.saturating_sub(cached),
                    output_tokens: candidates,
                    cache_read_input_tokens: cached,
                    ..TokenUsage::default()
                };
            }
        }
    }

    if full.is_empty() {
        return Err(AictlError::EmptyResponse { provider: "Gemini" });
    }
    Ok((full, usage))
}
