use std::io::Write;

#[derive(Debug)]
pub struct ToolCall {
    pub name: String,
    pub input: String,
}

pub const SYSTEM_PROMPT: &str = r#"You have access to tools that let you interact with the user's system. To use a tool, output an XML tag like this:

<tool name="shell">
command here
</tool>

Available tools:
- shell: Execute a shell command. The command runs via `sh -c`.
- read_file: Read the contents of a file. Pass the file path as the input.
- write_file: Write content to a file. First line is the file path, remaining lines are the content.
- list_directory: List files and directories at a path. Pass the directory path as input. Returns entries with [FILE] or [DIR] prefixes.
- search_files: Search file contents with a pattern. First line is the search pattern (grep basic regex), second line (optional) is the directory to search in (defaults to `.`). Returns matching lines with file paths and line numbers.
- web_search: Search the web for information. Pass a search query as input. Returns titles, URLs, and descriptions of matching results.
- edit_file: Apply a targeted find-and-replace edit to a file. Format:
  path/to/file
  <<<
  text to find (exact match)
  ===
  replacement text
  >>>
- glob: Find files matching a glob pattern. First line is the pattern (e.g. `**/*.rs`, `src/**/*.ts`). Second line (optional) is the base directory (defaults to `.`). Returns matching file paths, one per line.
- web_fetch: Fetch and read the content of a URL. Pass the URL as input. Returns the page text content with HTML tags stripped. Useful for reading pages found via web_search.
- geolocation: Get geolocation data for an IP address. Pass an IP address as input (or empty for your own IP). Returns city, country, timezone, coordinates, ISP info.

Rules:
- Use at most one tool call per response.
- When you have enough information to answer the user's question, respond normally without any tool tags.
- Show your reasoning before tool calls.
"#;

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
    match tool_call.name.as_str() {
        "shell" => {
            let output = tokio::process::Command::new("sh")
                .arg("-c")
                .arg(&tool_call.input)
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
                    // Truncate large output
                    if result.len() > 10_000 {
                        result.truncate(10_000);
                        result.push_str("\n... (truncated)");
                    }
                    result
                }
                Err(e) => format!("Error executing command: {e}"),
            }
        }
        "read_file" => {
            let path = tool_call.input.trim();
            match tokio::fs::read_to_string(path).await {
                Ok(mut contents) => {
                    if contents.is_empty() {
                        contents = "(empty file)".to_string();
                    }
                    if contents.len() > 10_000 {
                        contents.truncate(10_000);
                        contents.push_str("\n... (truncated)");
                    }
                    contents
                }
                Err(e) => format!("Error reading file: {e}"),
            }
        }
        "write_file" => {
            let input = tool_call.input.trim();
            match input.split_once('\n') {
                Some((path, content)) => {
                    let path = path.trim();
                    match tokio::fs::write(path, content).await {
                        Ok(()) => format!("Wrote {} bytes to {path}", content.len()),
                        Err(e) => format!("Error writing file: {e}"),
                    }
                }
                None => {
                    "Invalid input: expected first line as file path, remaining lines as content"
                        .to_string()
                }
            }
        }
        "list_directory" => {
            let path = tool_call.input.trim();
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
                        result.push_str(&format!("{prefix}  {name}\n"));
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
        "search_files" => {
            let input = tool_call.input.trim();
            let (pattern, dir) = match input.split_once('\n') {
                Some((p, d)) => (p.trim(), d.trim()),
                None => (input, "."),
            };
            let dir = if dir.is_empty() { "." } else { dir };
            let output = tokio::process::Command::new("grep")
                .args(["-rn", "--include=*", pattern, dir])
                .output()
                .await;
            match output {
                Ok(out) => {
                    let stdout = String::from_utf8_lossy(&out.stdout);
                    if stdout.is_empty() {
                        "No matches found.".to_string()
                    } else {
                        let mut result = stdout.to_string();
                        if result.len() > 10_000 {
                            result.truncate(10_000);
                            result.push_str("\n... (truncated)");
                        }
                        result
                    }
                }
                Err(e) => format!("Error running search: {e}"),
            }
        }
        "edit_file" => {
            let input = tool_call.input.trim();
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
                return "Invalid input: expected === separator between old and new text"
                    .to_string();
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
        "web_search" => {
            let api_key = match crate::config_get("FIRECRAWL_API_KEY") {
                Some(key) => key,
                None => {
                    return "Error: FIRECRAWL_API_KEY not set in ~/.aictl".to_string()
                }
            };
            let query = tool_call.input.trim();
            let client = reqwest::Client::new();
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
                                        let title =
                                            item["title"].as_str().unwrap_or("(no title)");
                                        let url = item["url"].as_str().unwrap_or("(no url)");
                                        let desc = item["description"]
                                            .as_str()
                                            .or_else(|| item["snippet"].as_str())
                                            .unwrap_or("(no description)");
                                        if i > 0 {
                                            output.push('\n');
                                        }
                                        output.push_str(&format!(
                                            "[{}] {}\nURL: {}\n{}\n",
                                            i + 1,
                                            title,
                                            url,
                                            desc
                                        ));
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
        "glob" => {
            let input = tool_call.input.trim();
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
                                result.push_str(&format!("(error: {e})"));
                            }
                        }
                        if result.len() > 10_000 {
                            result.truncate(10_000);
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
        "web_fetch" => {
            let url = tool_call.input.trim();
            let client = reqwest::Client::new();
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
                            if result.len() > 10_000 {
                                result.truncate(10_000);
                                result.push_str("\n... (truncated)");
                            }
                            result
                        }
                        Err(e) => format!("Error reading response body: {e}"),
                    }
                }
                Err(e) => format!("Error fetching URL: {e}"),
            }
        }
        "geolocation" => {
            let ip = tool_call.input.trim();
            let url = if ip.is_empty() {
                "http://ip-api.com/json/?fields=status,message,country,countryCode,region,regionName,city,zip,lat,lon,timezone,isp,org,as".to_string()
            } else {
                format!("http://ip-api.com/json/{ip}?fields=status,message,country,countryCode,region,regionName,city,zip,lat,lon,timezone,isp,org,as")
            };
            let client = reqwest::Client::new();
            match client.get(&url).send().await {
                Ok(resp) => match resp.json::<serde_json::Value>().await {
                    Ok(json) => {
                        if json["status"].as_str() == Some("fail") {
                            let msg = json["message"].as_str().unwrap_or("unknown error");
                            format!("Geolocation lookup failed: {msg}")
                        } else {
                            serde_json::to_string_pretty(&json)
                                .unwrap_or_else(|_| json.to_string())
                        }
                    }
                    Err(e) => format!("Error parsing geolocation response: {e}"),
                },
                Err(e) => format!("Error fetching geolocation data: {e}"),
            }
        }
        _ => format!("Unknown tool: {}", tool_call.name),
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
