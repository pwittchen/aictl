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
    // Security gate
    if let Err(reason) = crate::security::validate_tool(tool_call) {
        return format!("Security policy denied: {reason}");
    }

    let input = &tool_call.input;
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
        _ => format!("Unknown tool: {}", tool_call.name),
    };
    crate::security::sanitize_output(&result)
}

/// Truncate a result string to the output size limit.
fn truncate_output(s: &mut String) {
    if s.len() > MAX_TOOL_OUTPUT_LEN {
        s.truncate(MAX_TOOL_OUTPUT_LEN);
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
        assert_eq!(result, "hello world");
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
        assert_eq!(result, "(empty file)");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn exec_read_file_not_found() {
        let result = execute_tool(&ToolCall {
            name: "read_file".into(),
            input: "/tmp/aictl_nonexistent_file_xyz".into(),
        })
        .await;
        assert!(result.starts_with("Error reading file:"));
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
        assert!(result.starts_with("Wrote"));
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
        assert!(result.contains("Invalid input"));
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
        assert!(result.starts_with("Removed"));
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
        assert!(result.starts_with("Error removing file:"));
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
        assert!(result.starts_with("Created directory"));
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
        assert!(result.contains("[FILE]"));
        assert!(result.contains("[DIR]"));
        assert!(result.contains("a.txt"));
        assert!(result.contains("subdir"));
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
        assert_eq!(result, "(empty directory)");
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
        assert!(result.contains("replaced 1 occurrence"));
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
        assert!(result.contains("old text not found"));
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
        assert!(result.contains("found 2 times"));
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
        assert!(result.contains("a.rs"));
        assert!(!result.contains("b.txt"));
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
        assert_eq!(result, "No matches found.");
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
        assert!(result.contains("match.txt"));
        assert!(result.contains("needle in haystack"));
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
        assert_eq!(result, "No matches found.");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn exec_shell_stdout() {
        let result = execute_tool(&ToolCall {
            name: "exec_shell".into(),
            input: "echo hello".into(),
        })
        .await;
        assert_eq!(result.trim(), "hello");
    }

    #[tokio::test]
    async fn exec_shell_stderr() {
        let result = execute_tool(&ToolCall {
            name: "exec_shell".into(),
            input: "echo oops >&2".into(),
        })
        .await;
        assert!(result.contains("[stderr]"));
        assert!(result.contains("oops"));
    }

    #[tokio::test]
    async fn exec_shell_no_output() {
        let result = execute_tool(&ToolCall {
            name: "exec_shell".into(),
            input: "true".into(),
        })
        .await;
        assert_eq!(result, "(no output)");
    }

    #[tokio::test]
    async fn exec_fetch_datetime() {
        let result = execute_tool(&ToolCall {
            name: "fetch_datetime".into(),
            input: String::new(),
        })
        .await;
        assert!(!result.is_empty());
        assert!(result.starts_with("20"));
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

    #[tokio::test]
    async fn exec_unknown_tool() {
        let result = execute_tool(&ToolCall {
            name: "nonexistent".into(),
            input: String::new(),
        })
        .await;
        assert_eq!(result, "Unknown tool: nonexistent");
    }
}
