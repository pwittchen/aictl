use crate::ImageData;

use super::ToolOutput;

fn media_type_from_extension(path: &str) -> Option<&'static str> {
    let ext = path.rsplit('.').next()?.to_ascii_lowercase();
    match ext.as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "bmp" => Some("image/bmp"),
        "svg" => Some("image/svg+xml"),
        "ico" => Some("image/x-icon"),
        "tiff" | "tif" => Some("image/tiff"),
        _ => None,
    }
}

pub(super) async fn tool_read_image(input: &str) -> ToolOutput {
    use base64::Engine;

    let input = input.trim();
    if input.is_empty() {
        return ToolOutput::text("Error: no file path or URL provided".to_string());
    }

    let is_url = input.starts_with("http://") || input.starts_with("https://");

    let (bytes, media_type) = if is_url {
        let client = crate::config::http_client();
        let resp = match client.get(input).send().await {
            Ok(r) => r,
            Err(e) => return ToolOutput::text(format!("Error fetching image URL: {e}")),
        };
        if !resp.status().is_success() {
            return ToolOutput::text(format!("Error: HTTP status {}", resp.status()));
        }
        let content_type = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .and_then(|ct| ct.split(';').next())
            .map(|s| s.trim().to_string());
        let media = content_type
            .filter(|ct| ct.starts_with("image/"))
            .or_else(|| media_type_from_extension(input).map(String::from))
            .unwrap_or_else(|| "image/png".to_string());
        match resp.bytes().await {
            Ok(b) => (b.to_vec(), media),
            Err(e) => return ToolOutput::text(format!("Error reading image response: {e}")),
        }
    } else {
        let media = media_type_from_extension(input)
            .unwrap_or("image/png")
            .to_string();
        match tokio::fs::read(input).await {
            Ok(b) => (b, media),
            Err(e) => return ToolOutput::text(format!("Error reading image file: {e}")),
        }
    };

    let size_kb = bytes.len() / 1024;
    let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);

    let description = format!("[Image loaded: {media_type}, {size_kb}KB, from {input}]");

    ToolOutput {
        text: description,
        images: vec![ImageData {
            base64_data: encoded,
            media_type,
        }],
    }
}

/// Build a filename from the prompt slug and current timestamp.
fn image_filename(prompt: &str) -> String {
    let slug: String = prompt
        .split_whitespace()
        .take(5)
        .collect::<Vec<_>>()
        .join("_")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .take(40)
        .collect();
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    if slug.is_empty() {
        format!("generated_{ts}.png")
    } else {
        format!("{slug}_{ts}.png")
    }
}

/// Save decoded image bytes to disk and return a success message.
///
/// The destination is always `<working_dir>/<filename>` — never a
/// subdirectory and never the bare process cwd. `image_filename`
/// sanitizes the slug to `[A-Za-z0-9_-]`, so the joined path can't
/// climb out of `working_dir` via `..` or absolute components. The
/// returned message reports the filename only (no leading directory)
/// so frontends that parse "Image saved to <filename>" and join it
/// against their own workspace anchor still resolve correctly.
async fn save_image(bytes: &[u8], prompt: &str, provider: &str) -> String {
    let filename = image_filename(prompt);
    let working_dir = &crate::security::policy().paths.working_dir;
    if working_dir.as_os_str().is_empty() {
        return "Error saving image: no working directory configured (pick a workspace first)"
            .to_string();
    }
    let path = working_dir.join(&filename);
    if let Err(e) = tokio::fs::write(&path, bytes).await {
        return format!("Error saving image: {e}");
    }
    let size_kb = bytes.len() / 1024;
    format!("Image saved to {filename} ({size_kb}KB, generated via {provider})")
}

async fn generate_via_openai(api_key: &str, prompt: &str) -> Result<Vec<u8>, String> {
    use base64::Engine;

    let client = crate::config::http_client();
    let body = serde_json::json!({
        "model": "dall-e-3",
        "prompt": prompt,
        "n": 1,
        "size": "1024x1024",
        "response_format": "b64_json"
    });

    let resp = client
        .post("https://api.openai.com/v1/images/generations")
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Error calling DALL-E API: {e}"))?;

    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| format!("Error reading DALL-E response: {e}"))?;

    if !status.is_success() {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
            let msg = json["error"]["message"].as_str().unwrap_or("unknown error");
            return Err(format!("DALL-E API error ({status}): {msg}"));
        }
        return Err(format!("DALL-E API error ({status}): {text}"));
    }

    let json: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("Error parsing DALL-E response: {e}"))?;
    let b64 = json["data"][0]["b64_json"]
        .as_str()
        .ok_or("Error: no image data in DALL-E response")?;
    base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|e| format!("Error decoding image data: {e}"))
}

async fn generate_via_gemini(api_key: &str, prompt: &str) -> Result<Vec<u8>, String> {
    use base64::Engine;

    let client = crate::config::http_client();
    let model = "imagen-4.0-generate-001";
    let url = format!(
        "https://generativelanguage.googleapis.com/v1beta/models/{model}:predict?key={api_key}"
    );
    let body = serde_json::json!({
        "instances": [{"prompt": prompt}],
        "parameters": {
            "sampleCount": 1,
            "aspectRatio": "1:1"
        }
    });

    let resp = client
        .post(&url)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Error calling Imagen API: {e}"))?;

    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| format!("Error reading Imagen response: {e}"))?;

    if !status.is_success() {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
            let msg = json["error"]["message"].as_str().unwrap_or("unknown error");
            return Err(format!("Imagen API error ({status}): {msg}"));
        }
        return Err(format!("Imagen API error ({status}): {text}"));
    }

    let json: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("Error parsing Imagen response: {e}"))?;
    let b64 = json["predictions"][0]["bytesBase64Encoded"]
        .as_str()
        .ok_or("Error: no image data in Imagen response")?;
    base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|e| format!("Error decoding image data: {e}"))
}

async fn generate_via_grok(api_key: &str, prompt: &str) -> Result<Vec<u8>, String> {
    use base64::Engine;

    let client = crate::config::http_client();
    let body = serde_json::json!({
        "model": "grok-2-image",
        "prompt": prompt,
        "n": 1,
        "response_format": "b64_json"
    });

    let resp = client
        .post("https://api.x.ai/v1/images/generations")
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Error calling Grok image API: {e}"))?;

    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| format!("Error reading Grok image response: {e}"))?;

    if !status.is_success() {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
            let msg = json["error"]["message"].as_str().unwrap_or("unknown error");
            return Err(format!("Grok image API error ({status}): {msg}"));
        }
        return Err(format!("Grok image API error ({status}): {text}"));
    }

    let json: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("Error parsing Grok image response: {e}"))?;

    let b64_raw = json["data"][0]["b64_json"]
        .as_str()
        .ok_or("Error: no image data in Grok image response")?;
    // Grok may return a data URI prefix; strip it if present
    let b64 = b64_raw
        .strip_prefix("data:image/png;base64,")
        .or_else(|| b64_raw.strip_prefix("data:image/jpeg;base64,"))
        .unwrap_or(b64_raw);
    base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|e| format!("Error decoding image data: {e}"))
}

pub(super) async fn tool_generate_image(input: &str) -> String {
    let input = input.trim();
    if input.is_empty() {
        return "Error: no prompt provided for image generation".to_string();
    }

    // Try providers in order based on the active provider, then fall back
    // to whichever key is available: OpenAI → Gemini → Grok.
    let active = crate::config::config_get("AICTL_PROVIDER");
    let mut order: Vec<&str> = vec!["openai", "gemini", "grok"];

    // Move the active provider to the front so users get what they expect
    if let Some(ref p) = active
        && let Some(pos) = order.iter().position(|&x| x == p.as_str())
    {
        order.remove(pos);
        order.insert(0, p.as_str());
    }

    for provider in &order {
        match *provider {
            "openai" => {
                if let Some(key) = crate::keys::get_secret("LLM_OPENAI_API_KEY") {
                    return match generate_via_openai(&key, input).await {
                        Ok(bytes) => save_image(&bytes, input, "DALL-E").await,
                        Err(e) => e,
                    };
                }
            }
            "gemini" => {
                if let Some(key) = crate::keys::get_secret("LLM_GEMINI_API_KEY") {
                    return match generate_via_gemini(&key, input).await {
                        Ok(bytes) => save_image(&bytes, input, "Imagen").await,
                        Err(e) => e,
                    };
                }
            }
            "grok" => {
                if let Some(key) = crate::keys::get_secret("LLM_GROK_API_KEY") {
                    return match generate_via_grok(&key, input).await {
                        Ok(bytes) => save_image(&bytes, input, "Grok").await,
                        Err(e) => e,
                    };
                }
            }
            _ => {}
        }
    }

    "Error: no image generation API key available. Set LLM_OPENAI_API_KEY (DALL-E), LLM_GEMINI_API_KEY (Imagen), or LLM_GROK_API_KEY (Grok) in ~/.aictl/config or system keyring".to_string()
}
