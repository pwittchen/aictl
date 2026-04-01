use std::fmt::Write as _;
use std::io::Write;

use crate::config::MAX_TOOL_OUTPUT_LEN;

#[derive(Debug)]
pub struct ToolCall {
    pub name: String,
    pub input: String,
}

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

pub async fn execute_tool(tool_call: &ToolCall) -> String {
    let input = &tool_call.input;
    match tool_call.name.as_str() {
        "exec_shell" => tool_exec_shell(input).await,
        "read_file" => tool_read_file(input).await,
        "write_file" => tool_write_file(input).await,
        "list_directory" => tool_list_directory(input).await,
        "search_files" => tool_search_files(input).await,
        "edit_file" => tool_edit_file(input).await,
        "search_web" => tool_search_web(input).await,
        "find_files" => tool_find_files(input),
        "fetch_url" => tool_fetch_url(input).await,
        "extract_website" => tool_extract_website(input).await,
        "fetch_datetime" => tool_fetch_datetime().await,
        "fetch_geolocation" => tool_fetch_geolocation(input).await,
        _ => format!("Unknown tool: {}", tool_call.name),
    }
}

/// Truncate a result string to the output size limit.
fn truncate_output(s: &mut String) {
    if s.len() > MAX_TOOL_OUTPUT_LEN {
        s.truncate(MAX_TOOL_OUTPUT_LEN);
        s.push_str("\n... (truncated)");
    }
}

async fn tool_exec_shell(input: &str) -> String {
    let output = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(input)
        .output()
        .await;
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
    let Some(api_key) = crate::config::config_get("FIRECRAWL_API_KEY") else {
        return "Error: FIRECRAWL_API_KEY not set in ~/.aictl".to_string();
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
}
