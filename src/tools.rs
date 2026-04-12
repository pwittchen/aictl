use std::fmt::Write as _;
use std::io::Write;

use crate::ImageData;
use crate::config::MAX_TOOL_OUTPUT_LEN;

#[derive(Debug)]
pub struct ToolCall {
    pub name: String,
    pub input: String,
}

/// Result of executing a tool: text output plus optional image data.
pub struct ToolOutput {
    pub text: String,
    pub images: Vec<ImageData>,
}

impl ToolOutput {
    fn text(s: String) -> Self {
        Self {
            text: s,
            images: vec![],
        }
    }
}

pub const TOOL_COUNT: usize = 16;

pub fn parse_tool_call(response: &str) -> Option<ToolCall> {
    let start_prefix = "<tool name=\"";
    let start_idx = response.find(start_prefix)?;
    let after_prefix = start_idx + start_prefix.len();
    let name_end = response[after_prefix..].find('"')?;
    let name = response[after_prefix..after_prefix + name_end].to_string();
    let tag_close = response[after_prefix + name_end..].find('>')?;
    let content_start = after_prefix + name_end + tag_close + 1;
    let end_tag = "</tool>";
    let content_end = response[content_start..].find(end_tag)?;
    let input = response[content_start..content_start + content_end]
        .trim()
        .to_string();
    Some(ToolCall { name, input })
}

/// Returns `true` when the response clearly *attempted* a tool call but
/// [`parse_tool_call`] couldn't extract one — i.e. the `<tool>` XML is
/// malformed (missing close tag, wrong quote style, broken attribute, ...).
///
/// The agent loop uses this to ask the model to retry instead of surfacing
/// raw tool markup to the user as a "final answer".
pub fn looks_like_malformed_tool_call(response: &str) -> bool {
    if parse_tool_call(response).is_some() {
        return false;
    }
    // Strong signal: the exact prefix we parse is present but something
    // after it is broken (e.g. missing `"`, `>`, or `</tool>`).
    if response.contains("<tool name=") {
        return true;
    }
    // Also catch cases where both a tag-opener and a closer appear but the
    // name attribute uses the wrong quoting style or other variants.
    let has_open = response.contains("<tool>") || response.contains("<tool ");
    let has_close = response.contains("</tool>");
    has_open && has_close
}

/// Check whether tools are globally enabled via `AICTL_TOOLS_ENABLED` config.
/// Returns `true` when the key is absent or set to anything other than `false`/`0`.
pub fn tools_enabled() -> bool {
    crate::config::config_get("AICTL_TOOLS_ENABLED").is_none_or(|v| v != "false" && v != "0")
}

pub async fn execute_tool(tool_call: &ToolCall) -> ToolOutput {
    // Global tools switch
    if !tools_enabled() {
        return ToolOutput::text(
            "All tools are disabled (AICTL_TOOLS_ENABLED=false in config)".to_string(),
        );
    }

    // Security gate
    if let Err(reason) = crate::security::validate_tool(tool_call) {
        return ToolOutput::text(format!("Security policy denied: {reason}"));
    }

    let input = &tool_call.input;

    // read_image returns ToolOutput with image data
    if tool_call.name == "read_image" {
        let mut output = tool_read_image(input).await;
        output.text = crate::security::sanitize_output(&output.text);
        return output;
    }

    let result = match tool_call.name.as_str() {
        "exec_shell" => tool_exec_shell(input).await,
        "read_file" => tool_read_file(input).await,
        "write_file" => tool_write_file(input).await,
        "remove_file" => tool_remove_file(input).await,
        "create_directory" => tool_create_directory(input).await,
        "list_directory" => tool_list_directory(input).await,
        "search_files" => tool_search_files(input).await,
        "edit_file" => tool_edit_file(input).await,
        "search_web" => tool_search_web(input).await,
        "find_files" => tool_find_files(input),
        "fetch_url" => tool_fetch_url(input).await,
        "extract_website" => tool_extract_website(input).await,
        "fetch_datetime" => tool_fetch_datetime().await,
        "fetch_geolocation" => tool_fetch_geolocation(input).await,
        "generate_image" => tool_generate_image(input).await,
        _ => format!("Unknown tool: {}", tool_call.name),
    };
    ToolOutput::text(crate::security::sanitize_output(&result))
}

/// Truncate a result string to the output size limit.
/// Walks back to the nearest UTF-8 char boundary so multi-byte characters
/// landing on the cut don't trigger a panic in `String::truncate`.
fn truncate_output(s: &mut String) {
    if s.len() > MAX_TOOL_OUTPUT_LEN {
        let mut idx = MAX_TOOL_OUTPUT_LEN;
        while idx > 0 && !s.is_char_boundary(idx) {
            idx -= 1;
        }
        s.truncate(idx);
        s.push_str("\n... (truncated)");
    }
}

async fn tool_exec_shell(input: &str) -> String {
    let mut cmd = tokio::process::Command::new("sh");
    cmd.arg("-c").arg(input);

    // Environment scrubbing
    cmd.env_clear();
    for (key, value) in crate::security::scrubbed_env() {
        cmd.env(key, value);
    }

    let future = cmd.output();
    let output = if let Some(timeout) = crate::security::shell_timeout() {
        match tokio::time::timeout(timeout, future).await {
            Ok(result) => result,
            Err(_) => return format!("Error: command timed out after {}s", timeout.as_secs()),
        }
    } else {
        future.await
    };

    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            let mut result = String::new();
            if !stdout.is_empty() {
                result.push_str(&stdout);
            }
            if !stderr.is_empty() {
                if !result.is_empty() {
                    result.push('\n');
                }
                result.push_str("[stderr]\n");
                result.push_str(&stderr);
            }
            if result.is_empty() {
                result.push_str("(no output)");
            }
            truncate_output(&mut result);
            result
        }
        Err(e) => format!("Error executing command: {e}"),
    }
}

async fn tool_read_file(input: &str) -> String {
    let path = input.trim();
    match tokio::fs::read_to_string(path).await {
        Ok(mut contents) => {
            if contents.is_empty() {
                contents = "(empty file)".to_string();
            }
            truncate_output(&mut contents);
            contents
        }
        Err(e) => format!("Error reading file: {e}"),
    }
}

async fn tool_write_file(input: &str) -> String {
    let input = input.trim();
    match input.split_once('\n') {
        Some((path, content)) => {
            let path = path.trim();
            match tokio::fs::write(path, content).await {
                Ok(()) => format!("Wrote {} bytes to {path}", content.len()),
                Err(e) => format!("Error writing file: {e}"),
            }
        }
        None => "Invalid input: expected first line as file path, remaining lines as content"
            .to_string(),
    }
}

async fn tool_remove_file(input: &str) -> String {
    let path = input.trim();
    match tokio::fs::remove_file(path).await {
        Ok(()) => format!("Removed {path}"),
        Err(e) => format!("Error removing file: {e}"),
    }
}

async fn tool_create_directory(input: &str) -> String {
    let path = input.trim();
    match tokio::fs::create_dir_all(path).await {
        Ok(()) => format!("Created directory {path}"),
        Err(e) => format!("Error creating directory: {e}"),
    }
}

async fn tool_list_directory(input: &str) -> String {
    let path = input.trim();
    let path = if path.is_empty() { "." } else { path };
    match tokio::fs::read_dir(path).await {
        Ok(mut entries) => {
            let mut result = String::new();
            while let Ok(Some(entry)) = entries.next_entry().await {
                let name = entry.file_name().to_string_lossy().to_string();
                let prefix = match entry.file_type().await {
                    Ok(ft) if ft.is_dir() => "[DIR]",
                    Ok(ft) if ft.is_symlink() => "[LINK]",
                    _ => "[FILE]",
                };
                let _ = writeln!(result, "{prefix}  {name}");
            }
            if result.is_empty() {
                "(empty directory)".to_string()
            } else {
                result
            }
        }
        Err(e) => format!("Error listing directory: {e}"),
    }
}

async fn tool_search_files(input: &str) -> String {
    let input = input.trim();
    let (pattern, dir) = match input.split_once('\n') {
        Some((p, d)) => (p.trim(), d.trim()),
        None => (input, "."),
    };
    let dir = if dir.is_empty() { "." } else { dir };
    let pattern = pattern.to_string();
    let dir = dir.to_string();
    tokio::task::spawn_blocking(move || search_files_blocking(&pattern, &dir))
        .await
        .unwrap_or_else(|e| format!("Error running search: {e}"))
}

fn search_files_blocking(pattern: &str, dir: &str) -> String {
    let glob_pattern = format!("{dir}/**/*");
    let entries = match glob::glob(&glob_pattern) {
        Ok(paths) => paths,
        Err(e) => return format!("Error: invalid path pattern: {e}"),
    };
    let mut result = String::new();
    for entry in entries {
        let Ok(path) = entry else { continue };
        if !path.is_file() {
            continue;
        }
        let Ok(contents) = std::fs::read_to_string(&path) else {
            continue; // skip binary / unreadable files
        };
        let path_str = path.to_string_lossy();
        for (i, line) in contents.lines().enumerate() {
            if line.contains(pattern) {
                if !result.is_empty() {
                    result.push('\n');
                }
                let _ = write!(result, "{path_str}:{}:{line}", i + 1);
                if result.len() > MAX_TOOL_OUTPUT_LEN {
                    result.truncate(MAX_TOOL_OUTPUT_LEN);
                    result.push_str("\n... (truncated)");
                    return result;
                }
            }
        }
    }
    if result.is_empty() {
        "No matches found.".to_string()
    } else {
        result
    }
}

async fn tool_edit_file(input: &str) -> String {
    let input = input.trim();
    // Parse: path\n<<<\nold\n===\nnew\n>>>
    let Some((path, rest)) = input.split_once('\n') else {
        return "Invalid input: expected file path on first line".to_string();
    };
    let path = path.trim();
    let rest = rest.trim();
    let Some(rest) = rest.strip_prefix("<<<") else {
        return "Invalid input: expected <<< delimiter after file path".to_string();
    };
    let Some((old_new, _)) = rest.split_once(">>>") else {
        return "Invalid input: expected >>> closing delimiter".to_string();
    };
    let Some((old_text, new_text)) = old_new.split_once("===") else {
        return "Invalid input: expected === separator between old and new text".to_string();
    };
    let old_text = old_text.strip_prefix('\n').unwrap_or(old_text);
    let old_text = old_text.strip_suffix('\n').unwrap_or(old_text);
    let new_text = new_text.strip_prefix('\n').unwrap_or(new_text);
    let new_text = new_text.strip_suffix('\n').unwrap_or(new_text);

    let contents = match tokio::fs::read_to_string(path).await {
        Ok(c) => c,
        Err(e) => return format!("Error reading file: {e}"),
    };
    let count = contents.matches(old_text).count();
    if count == 0 {
        return "Error: old text not found in file".to_string();
    }
    if count > 1 {
        return format!(
            "Error: old text found {count} times in file — provide more context to match uniquely"
        );
    }
    let updated = contents.replacen(old_text, new_text, 1);
    match tokio::fs::write(path, &updated).await {
        Ok(()) => format!("Edited {path} (replaced 1 occurrence)"),
        Err(e) => format!("Error writing file: {e}"),
    }
}

async fn tool_search_web(input: &str) -> String {
    let Some(api_key) = crate::keys::get_secret("FIRECRAWL_API_KEY") else {
        return "Error: FIRECRAWL_API_KEY not set in ~/.aictl/config or system keyring".to_string();
    };
    let query = input.trim();
    let client = crate::config::http_client();
    let body = serde_json::json!({
        "query": query,
        "limit": 5
    });
    match client
        .post("https://api.firecrawl.dev/v2/search")
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
    {
        Ok(resp) => {
            if !resp.status().is_success() {
                return format!("Error: Firecrawl API returned status {}", resp.status());
            }
            match resp.json::<serde_json::Value>().await {
                Ok(json) => {
                    let results = json["data"]
                        .as_array()
                        .or_else(|| json["data"]["web"].as_array());
                    match results {
                        Some(items) if !items.is_empty() => {
                            let mut output = String::new();
                            for (i, item) in items.iter().enumerate() {
                                let title = item["title"].as_str().unwrap_or("(no title)");
                                let url = item["url"].as_str().unwrap_or("(no url)");
                                let desc = item["description"]
                                    .as_str()
                                    .or_else(|| item["snippet"].as_str())
                                    .unwrap_or("(no description)");
                                if i > 0 {
                                    output.push('\n');
                                }
                                let _ = write!(
                                    output,
                                    "[{}] {}\nURL: {}\n{}\n",
                                    i + 1,
                                    title,
                                    url,
                                    desc
                                );
                            }
                            output
                        }
                        _ => "No results found.".to_string(),
                    }
                }
                Err(e) => format!("Error parsing Firecrawl response: {e}"),
            }
        }
        Err(e) => format!("Error calling Firecrawl API: {e}"),
    }
}

fn tool_find_files(input: &str) -> String {
    let input = input.trim();
    let (pattern, base_dir) = match input.split_once('\n') {
        Some((p, d)) => (p.trim(), d.trim()),
        None => (input, "."),
    };
    let base_dir = if base_dir.is_empty() { "." } else { base_dir };
    let full_pattern = if std::path::Path::new(pattern).is_absolute() {
        pattern.to_string()
    } else {
        format!("{base_dir}/{pattern}")
    };
    match glob::glob(&full_pattern) {
        Ok(paths) => {
            let mut result = String::new();
            for entry in paths {
                match entry {
                    Ok(path) => {
                        if !result.is_empty() {
                            result.push('\n');
                        }
                        result.push_str(&path.to_string_lossy());
                    }
                    Err(e) => {
                        if !result.is_empty() {
                            result.push('\n');
                        }
                        let _ = write!(result, "(error: {e})");
                    }
                }
                if result.len() > MAX_TOOL_OUTPUT_LEN {
                    result.truncate(MAX_TOOL_OUTPUT_LEN);
                    result.push_str("\n... (truncated)");
                    break;
                }
            }
            if result.is_empty() {
                "No matches found.".to_string()
            } else {
                result
            }
        }
        Err(e) => format!("Error parsing glob pattern: {e}"),
    }
}

async fn tool_fetch_url(input: &str) -> String {
    let url = input.trim();
    let client = crate::config::http_client();
    match client.get(url).send().await {
        Ok(resp) => {
            if !resp.status().is_success() {
                return format!("Error: HTTP status {}", resp.status());
            }
            match resp.text().await {
                Ok(body) => {
                    // Strip HTML tags
                    let mut result = String::with_capacity(body.len());
                    let mut in_tag = false;
                    for ch in body.chars() {
                        if ch == '<' {
                            in_tag = true;
                        } else if ch == '>' {
                            in_tag = false;
                        } else if !in_tag {
                            result.push(ch);
                        }
                    }
                    // Collapse whitespace runs
                    let mut collapsed = String::with_capacity(result.len());
                    let mut prev_ws = false;
                    for ch in result.chars() {
                        if ch.is_whitespace() {
                            if !prev_ws {
                                collapsed.push(if ch == '\n' { '\n' } else { ' ' });
                            }
                            prev_ws = true;
                        } else {
                            collapsed.push(ch);
                            prev_ws = false;
                        }
                    }
                    let mut result = collapsed.trim().to_string();
                    if result.is_empty() {
                        result = "(empty page)".to_string();
                    }
                    truncate_output(&mut result);
                    result
                }
                Err(e) => format!("Error reading response body: {e}"),
            }
        }
        Err(e) => format!("Error fetching URL: {e}"),
    }
}

async fn tool_extract_website(input: &str) -> String {
    let url = input.trim();
    let client = crate::config::http_client();
    match client.get(url).send().await {
        Ok(resp) => {
            if !resp.status().is_success() {
                return format!("Error: HTTP status {}", resp.status());
            }
            match resp.text().await {
                Ok(body) => {
                    let document = scraper::Html::parse_document(&body);
                    let noise_selectors = [
                        "script", "style", "nav", "header", "footer", "noscript", "svg", "form",
                        "iframe",
                    ];
                    let mut remove_ids = std::collections::HashSet::new();
                    for sel_str in &noise_selectors {
                        if let Ok(sel) = scraper::Selector::parse(sel_str) {
                            for el in document.select(&sel) {
                                remove_ids.insert(el.id());
                            }
                        }
                    }
                    let mut text = String::new();
                    for node_ref in document.tree.root().descendants() {
                        if let scraper::node::Node::Text(t) = node_ref.value() {
                            let skip = node_ref.ancestors().any(|a| remove_ids.contains(&a.id()));
                            if !skip {
                                text.push_str(&t.text);
                            }
                        }
                    }
                    // Collapse whitespace
                    let mut result = String::with_capacity(text.len());
                    let mut prev_ws = false;
                    for ch in text.chars() {
                        if ch.is_whitespace() {
                            if !prev_ws {
                                result.push(if ch == '\n' { '\n' } else { ' ' });
                            }
                            prev_ws = true;
                        } else {
                            result.push(ch);
                            prev_ws = false;
                        }
                    }
                    let mut result = result.trim().to_string();
                    if result.is_empty() {
                        result = "(no content extracted)".to_string();
                    }
                    truncate_output(&mut result);
                    result
                }
                Err(e) => format!("Error reading response body: {e}"),
            }
        }
        Err(e) => format!("Error fetching URL: {e}"),
    }
}

async fn tool_fetch_datetime() -> String {
    match tokio::process::Command::new("date")
        .arg("+%Y-%m-%d %H:%M:%S %Z (%A)")
        .output()
        .await
    {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if stdout.is_empty() {
                "(could not determine date/time)".to_string()
            } else {
                stdout
            }
        }
        Err(e) => format!("Error fetching date/time: {e}"),
    }
}

async fn tool_fetch_geolocation(input: &str) -> String {
    let ip = input.trim();
    let url = if ip.is_empty() {
        "http://ip-api.com/json/?fields=status,message,country,countryCode,region,regionName,city,zip,lat,lon,timezone,isp,org,as".to_string()
    } else {
        format!(
            "http://ip-api.com/json/{ip}?fields=status,message,country,countryCode,region,regionName,city,zip,lat,lon,timezone,isp,org,as"
        )
    };
    let client = crate::config::http_client();
    match client.get(&url).send().await {
        Ok(resp) => match resp.json::<serde_json::Value>().await {
            Ok(json) => {
                if json["status"].as_str() == Some("fail") {
                    let msg = json["message"].as_str().unwrap_or("unknown error");
                    format!("Geolocation lookup failed: {msg}")
                } else {
                    serde_json::to_string_pretty(&json).unwrap_or_else(|_| json.to_string())
                }
            }
            Err(e) => format!("Error parsing geolocation response: {e}"),
        },
        Err(e) => format!("Error fetching geolocation data: {e}"),
    }
}

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

async fn tool_read_image(input: &str) -> ToolOutput {
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
async fn save_image(bytes: &[u8], prompt: &str, provider: &str) -> String {
    let filename = image_filename(prompt);
    if let Err(e) = tokio::fs::write(&filename, bytes).await {
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

async fn tool_generate_image(input: &str) -> String {
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

pub fn confirm_tool_call(tool_call: &ToolCall) -> bool {
    eprint!(
        "Tool call [{}]: {}\nAllow? [y/N] ",
        tool_call.name, tool_call.input
    );
    std::io::stderr().flush().ok();
    let mut input = String::new();
    if std::io::stdin().read_line(&mut input).is_err() {
        return false;
    }
    matches!(input.trim(), "y" | "Y" | "yes" | "Yes")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_simple() {
        let resp = r#"<tool name="read_file">src/main.rs</tool>"#;
        let tc = parse_tool_call(resp).unwrap();
        assert_eq!(tc.name, "read_file");
        assert_eq!(tc.input, "src/main.rs");
    }

    #[test]
    fn parse_valid_multiline_input() {
        let resp = "<tool name=\"write_file\">\npath/to/file\nline one\nline two\n</tool>";
        let tc = parse_tool_call(resp).unwrap();
        assert_eq!(tc.name, "write_file");
        assert_eq!(tc.input, "path/to/file\nline one\nline two");
    }

    #[test]
    fn parse_extra_text_around_tags() {
        let resp = "Let me read that file for you.\n<tool name=\"read_file\">foo.txt</tool>\nDone.";
        let tc = parse_tool_call(resp).unwrap();
        assert_eq!(tc.name, "read_file");
        assert_eq!(tc.input, "foo.txt");
    }

    #[test]
    fn parse_missing_closing_tag() {
        let resp = r#"<tool name="exec_shell">ls -la"#;
        assert!(parse_tool_call(resp).is_none());
    }

    #[test]
    fn parse_missing_opening_tag() {
        let resp = "some text</tool>";
        assert!(parse_tool_call(resp).is_none());
    }

    #[test]
    fn parse_empty_input_between_tags() {
        let resp = r#"<tool name="fetch_datetime"></tool>"#;
        let tc = parse_tool_call(resp).unwrap();
        assert_eq!(tc.name, "fetch_datetime");
        assert_eq!(tc.input, "");
    }

    #[test]
    fn parse_tool_name_with_underscore() {
        let resp = r#"<tool name="search_files">pattern</tool>"#;
        let tc = parse_tool_call(resp).unwrap();
        assert_eq!(tc.name, "search_files");
    }

    #[test]
    fn parse_no_tool_call_plain_text() {
        let resp = "Here is the answer to your question.";
        assert!(parse_tool_call(resp).is_none());
    }

    #[test]
    fn parse_incomplete_opening_tag() {
        let resp = r#"<tool name="exec_shell"#;
        assert!(parse_tool_call(resp).is_none());
    }

    // --- Malformed tool call detection ---

    #[test]
    fn malformed_detects_missing_closing_tag() {
        // LLM wrote a tool call but forgot `</tool>` — regression test for bug
        // where this was surfaced to the user as a raw-XML "final answer".
        let resp = r#"I'll read that file.
<tool name="read_file">src/main.rs"#;
        assert!(parse_tool_call(resp).is_none());
        assert!(looks_like_malformed_tool_call(resp));
    }

    #[test]
    fn malformed_detects_unterminated_name_attribute() {
        let resp = r#"<tool name="exec_shell>ls -la</tool>"#;
        assert!(parse_tool_call(resp).is_none());
        assert!(looks_like_malformed_tool_call(resp));
    }

    #[test]
    fn malformed_detects_single_quoted_name() {
        // Wrong quote style — parser expects double quotes.
        let resp = "<tool name='read_file'>foo.txt</tool>";
        assert!(parse_tool_call(resp).is_none());
        assert!(looks_like_malformed_tool_call(resp));
    }

    #[test]
    fn malformed_detects_truncated_opening_tag() {
        let resp = r#"<tool name="exec_shell"#;
        assert!(parse_tool_call(resp).is_none());
        assert!(looks_like_malformed_tool_call(resp));
    }

    #[test]
    fn malformed_detects_bare_tool_tags_without_name_attr() {
        let resp = "<tool>read_file src/main.rs</tool>";
        assert!(parse_tool_call(resp).is_none());
        assert!(looks_like_malformed_tool_call(resp));
    }

    #[test]
    fn malformed_rejects_valid_tool_call() {
        let resp = r#"<tool name="read_file">src/main.rs</tool>"#;
        assert!(parse_tool_call(resp).is_some());
        assert!(!looks_like_malformed_tool_call(resp));
    }

    #[test]
    fn malformed_rejects_plain_text_answer() {
        let resp = "Here is the answer to your question. It is 42.";
        assert!(!looks_like_malformed_tool_call(resp));
    }

    #[test]
    fn malformed_rejects_answer_mentioning_tool_word() {
        // The word "toolchain" must not trip the heuristic.
        let resp = "You can install it via the standard Rust toolchain.";
        assert!(!looks_like_malformed_tool_call(resp));
    }

    #[test]
    fn malformed_rejects_answer_with_only_closing_tag_mention() {
        // A final answer that happens to mention the closing tag textually
        // (no `<tool` open anywhere) must not trigger retry.
        let resp = "The closing XML marker is </tool> — that's how it ends.";
        assert!(!looks_like_malformed_tool_call(resp));
    }

    #[test]
    fn malformed_rejects_valid_call_with_leading_text() {
        let resp = "Sure, let me check.\n<tool name=\"read_file\">a.txt</tool>\nDone.";
        assert!(parse_tool_call(resp).is_some());
        assert!(!looks_like_malformed_tool_call(resp));
    }

    // --- Tool execution tests ---

    fn tmp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("aictl_test_{name}_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    #[tokio::test]
    async fn exec_read_file() {
        let dir = tmp_dir("read");
        let path = dir.join("hello.txt");
        std::fs::write(&path, "hello world").unwrap();
        let result = execute_tool(&ToolCall {
            name: "read_file".into(),
            input: path.to_string_lossy().into(),
        })
        .await;
        assert_eq!(result.text, "hello world");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn exec_read_file_empty() {
        let dir = tmp_dir("read_empty");
        let path = dir.join("empty.txt");
        std::fs::write(&path, "").unwrap();
        let result = execute_tool(&ToolCall {
            name: "read_file".into(),
            input: path.to_string_lossy().into(),
        })
        .await;
        assert_eq!(result.text, "(empty file)");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn exec_read_file_not_found() {
        let result = execute_tool(&ToolCall {
            name: "read_file".into(),
            input: "/tmp/aictl_nonexistent_file_xyz".into(),
        })
        .await;
        assert!(result.text.starts_with("Error reading file:"));
    }

    #[tokio::test]
    async fn exec_write_file() {
        let dir = tmp_dir("write");
        let path = dir.join("out.txt");
        let input = format!("{}\nfile content here", path.display());
        let result = execute_tool(&ToolCall {
            name: "write_file".into(),
            input,
        })
        .await;
        assert!(result.text.starts_with("Wrote"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "file content here");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn exec_write_file_no_newline() {
        let result = execute_tool(&ToolCall {
            name: "write_file".into(),
            input: "single_line_no_newline".into(),
        })
        .await;
        assert!(result.text.contains("Invalid input"));
    }

    #[tokio::test]
    async fn exec_remove_file() {
        let dir = tmp_dir("remove");
        let path = dir.join("deleteme.txt");
        std::fs::write(&path, "gone soon").unwrap();
        assert!(path.exists());
        let result = execute_tool(&ToolCall {
            name: "remove_file".into(),
            input: path.to_string_lossy().into(),
        })
        .await;
        assert!(result.text.starts_with("Removed"));
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn exec_remove_file_not_found() {
        let result = execute_tool(&ToolCall {
            name: "remove_file".into(),
            input: "/tmp/aictl_nonexistent_file_xyz".into(),
        })
        .await;
        assert!(result.text.starts_with("Error removing file:"));
    }

    #[tokio::test]
    async fn exec_create_directory() {
        let dir = tmp_dir("create_dir");
        let new_dir = dir.join("a/b/c");
        assert!(!new_dir.exists());
        let result = execute_tool(&ToolCall {
            name: "create_directory".into(),
            input: new_dir.to_string_lossy().into(),
        })
        .await;
        assert!(result.text.starts_with("Created directory"));
        assert!(new_dir.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn exec_list_directory() {
        let dir = tmp_dir("listdir");
        std::fs::write(dir.join("a.txt"), "").unwrap();
        std::fs::create_dir_all(dir.join("subdir")).unwrap();
        let result = execute_tool(&ToolCall {
            name: "list_directory".into(),
            input: dir.to_string_lossy().into(),
        })
        .await;
        assert!(result.text.contains("[FILE]"));
        assert!(result.text.contains("[DIR]"));
        assert!(result.text.contains("a.txt"));
        assert!(result.text.contains("subdir"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn exec_list_directory_empty() {
        let dir = tmp_dir("listdir_empty");
        let result = execute_tool(&ToolCall {
            name: "list_directory".into(),
            input: dir.to_string_lossy().into(),
        })
        .await;
        assert_eq!(result.text, "(empty directory)");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn exec_edit_file_success() {
        let dir = tmp_dir("edit_ok");
        let path = dir.join("file.txt");
        std::fs::write(&path, "hello world").unwrap();
        let input = format!("{}\n<<<\nhello\n===\ngoodbye\n>>>", path.display());
        let result = execute_tool(&ToolCall {
            name: "edit_file".into(),
            input,
        })
        .await;
        assert!(result.text.contains("replaced 1 occurrence"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "goodbye world");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn exec_edit_file_not_found() {
        let dir = tmp_dir("edit_nf");
        let path = dir.join("file.txt");
        std::fs::write(&path, "hello world").unwrap();
        let input = format!(
            "{}\n<<<\nno such text\n===\nreplacement\n>>>",
            path.display()
        );
        let result = execute_tool(&ToolCall {
            name: "edit_file".into(),
            input,
        })
        .await;
        assert!(result.text.contains("old text not found"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn exec_edit_file_multiple() {
        let dir = tmp_dir("edit_multi");
        let path = dir.join("file.txt");
        std::fs::write(&path, "aaa bbb aaa").unwrap();
        let input = format!("{}\n<<<\naaa\n===\nccc\n>>>", path.display());
        let result = execute_tool(&ToolCall {
            name: "edit_file".into(),
            input,
        })
        .await;
        assert!(result.text.contains("found 2 times"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn exec_find_files() {
        let dir = tmp_dir("find");
        std::fs::write(dir.join("a.rs"), "").unwrap();
        std::fs::write(dir.join("b.txt"), "").unwrap();
        let input = format!("*.rs\n{}", dir.display());
        let result = execute_tool(&ToolCall {
            name: "find_files".into(),
            input,
        })
        .await;
        assert!(result.text.contains("a.rs"));
        assert!(!result.text.contains("b.txt"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn exec_find_files_no_matches() {
        let dir = tmp_dir("find_none");
        let input = format!("*.xyz\n{}", dir.display());
        let result = execute_tool(&ToolCall {
            name: "find_files".into(),
            input,
        })
        .await;
        assert_eq!(result.text, "No matches found.");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn exec_search_files() {
        let dir = tmp_dir("search");
        std::fs::write(dir.join("match.txt"), "needle in haystack").unwrap();
        std::fs::write(dir.join("other.txt"), "nothing here").unwrap();
        let input = format!("needle\n{}", dir.display());
        let result = execute_tool(&ToolCall {
            name: "search_files".into(),
            input,
        })
        .await;
        assert!(result.text.contains("match.txt"));
        assert!(result.text.contains("needle in haystack"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn exec_search_files_no_matches() {
        let dir = tmp_dir("search_none");
        std::fs::write(dir.join("file.txt"), "hello").unwrap();
        let input = format!("zzzzz\n{}", dir.display());
        let result = execute_tool(&ToolCall {
            name: "search_files".into(),
            input,
        })
        .await;
        assert_eq!(result.text, "No matches found.");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn exec_shell_stdout() {
        let result = execute_tool(&ToolCall {
            name: "exec_shell".into(),
            input: "echo hello".into(),
        })
        .await;
        assert_eq!(result.text.trim(), "hello");
    }

    #[tokio::test]
    async fn exec_shell_stderr() {
        let result = execute_tool(&ToolCall {
            name: "exec_shell".into(),
            input: "echo oops >&2".into(),
        })
        .await;
        assert!(result.text.contains("[stderr]"));
        assert!(result.text.contains("oops"));
    }

    #[tokio::test]
    async fn exec_shell_no_output() {
        let result = execute_tool(&ToolCall {
            name: "exec_shell".into(),
            input: "true".into(),
        })
        .await;
        assert_eq!(result.text, "(no output)");
    }

    #[tokio::test]
    async fn exec_fetch_datetime() {
        let result = execute_tool(&ToolCall {
            name: "fetch_datetime".into(),
            input: String::new(),
        })
        .await;
        assert!(!result.text.is_empty());
        assert!(result.text.starts_with("20"));
    }

    #[test]
    fn truncate_output_short() {
        let mut s = "short".to_string();
        truncate_output(&mut s);
        assert_eq!(s, "short");
    }

    #[test]
    fn truncate_output_over_limit() {
        let mut s = "x".repeat(MAX_TOOL_OUTPUT_LEN + 100);
        truncate_output(&mut s);
        assert!(s.ends_with("\n... (truncated)"));
        assert!(s.len() <= MAX_TOOL_OUTPUT_LEN + 20);
    }

    #[test]
    fn truncate_output_multibyte_on_boundary() {
        // Build a string where a multi-byte UTF-8 char straddles MAX_TOOL_OUTPUT_LEN.
        // 'é' is 2 bytes in UTF-8. Padding to MAX_TOOL_OUTPUT_LEN - 1 bytes of ASCII
        // then appending 'é' puts the char's first byte at MAX-1 and second byte at MAX,
        // so a naive truncate(MAX) would split it and panic.
        let mut s = "a".repeat(MAX_TOOL_OUTPUT_LEN - 1);
        s.push('é');
        s.push_str(&"b".repeat(100));
        truncate_output(&mut s);
        assert!(s.ends_with("\n... (truncated)"));
        // Result must still be valid UTF-8 (implicit: String guarantees this only if
        // truncate didn't panic, which is what we're verifying).
        assert!(s.is_char_boundary(s.len()));
    }

    #[tokio::test]
    async fn exec_unknown_tool() {
        let result = execute_tool(&ToolCall {
            name: "nonexistent".into(),
            input: String::new(),
        })
        .await;
        assert_eq!(result.text, "Unknown tool: nonexistent");
    }

    #[tokio::test]
    async fn exec_read_image_file() {
        let dir = tmp_dir("read_img");
        let path = dir.join("test.png");
        // Write a minimal valid PNG (1x1 pixel, white)
        let png_bytes: &[u8] = &[
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, // PNG signature
            0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, // IHDR chunk
            0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90,
            0x77, 0x53, 0xDE,
        ];
        std::fs::write(&path, png_bytes).unwrap();
        let result = execute_tool(&ToolCall {
            name: "read_image".into(),
            input: path.to_string_lossy().into(),
        })
        .await;
        assert!(result.text.contains("Image loaded"));
        assert!(result.text.contains("image/png"));
        assert_eq!(result.images.len(), 1);
        assert_eq!(result.images[0].media_type, "image/png");
        assert!(!result.images[0].base64_data.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn exec_read_image_not_found() {
        let result = execute_tool(&ToolCall {
            name: "read_image".into(),
            input: "/tmp/aictl_nonexistent_image.png".into(),
        })
        .await;
        assert!(result.text.starts_with("Error reading image file:"));
        assert!(result.images.is_empty());
    }

    #[tokio::test]
    async fn exec_read_image_empty_input() {
        let result = execute_tool(&ToolCall {
            name: "read_image".into(),
            input: String::new(),
        })
        .await;
        assert!(result.text.contains("no file path or URL"));
        assert!(result.images.is_empty());
    }
}
