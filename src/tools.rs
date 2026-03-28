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
