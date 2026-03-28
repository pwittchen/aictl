use clap::{Parser, ValueEnum};
use serde::{Deserialize, Serialize};
use std::io::Write;

#[derive(Debug, Clone, ValueEnum)]
enum Provider {
    Openai,
    Anthropic,
}

#[derive(Parser)]
#[command(name = "aictl", about = "CLI tool for interacting with LLM APIs")]
struct Cli {
    /// LLM provider to use
    #[arg(short, long)]
    provider: Provider,

    /// API key for the provider (falls back to OPENAI_API_KEY or ANTHROPIC_API_KEY)
    #[arg(short = 'k', long)]
    api_key: Option<String>,

    /// Model to use (e.g. gpt-4o, claude-sonnet-4-20250514)
    #[arg(short, long)]
    model: String,

    /// Message to send to the LLM
    #[arg(short = 'M', long)]
    message: String,

    /// Run in autonomous mode (skip tool confirmation prompts)
    #[arg(long)]
    auto: bool,
}

// --- Provider-agnostic types ---

#[derive(Debug, Clone)]
enum Role {
    System,
    User,
    Assistant,
}

#[derive(Debug, Clone)]
struct Message {
    role: Role,
    content: String,
}

#[derive(Debug)]
struct ToolCall {
    name: String,
    input: String,
}

const SYSTEM_PROMPT: &str = r#"You have access to tools that let you interact with the user's system. To use a tool, output an XML tag like this:

<tool name="shell">
command here
</tool>

Available tools:
- shell: Execute a shell command. The command runs via `sh -c`.

Rules:
- Use at most one tool call per response.
- When you have enough information to answer the user's question, respond normally without any tool tags.
- Show your reasoning before tool calls.
"#;

// --- OpenAI types ---

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

// --- Anthropic types ---

#[derive(Serialize)]
struct AnthropicRequest {
    model: String,
    max_tokens: u32,
    messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
}

#[derive(Serialize)]
struct AnthropicMessage {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct AnthropicResponse {
    content: Vec<AnthropicContent>,
}

#[derive(Deserialize)]
struct AnthropicContent {
    text: String,
}

// --- Tool functions ---

fn parse_tool_call(response: &str) -> Option<ToolCall> {
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

async fn execute_tool(tool_call: &ToolCall) -> String {
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
        _ => format!("Unknown tool: {}", tool_call.name),
    }
}

fn confirm_tool_call(tool_call: &ToolCall) -> bool {
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

// --- Provider call functions ---

async fn call_openai(
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

async fn call_anthropic(
    api_key: &str,
    model: &str,
    messages: &[Message],
) -> Result<String, Box<dyn std::error::Error>> {
    let client = reqwest::Client::new();

    let mut system_text: Option<String> = None;
    let mut api_messages: Vec<AnthropicMessage> = Vec::new();

    for m in messages {
        match m.role {
            Role::System => {
                system_text = Some(m.content.clone());
            }
            Role::User => {
                api_messages.push(AnthropicMessage {
                    role: "user".to_string(),
                    content: m.content.clone(),
                });
            }
            Role::Assistant => {
                api_messages.push(AnthropicMessage {
                    role: "assistant".to_string(),
                    content: m.content.clone(),
                });
            }
        }
    }

    let body = AnthropicRequest {
        model: model.to_string(),
        max_tokens: 4096,
        messages: api_messages,
        system: system_text,
    };

    let resp = client
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await?;

    let status = resp.status();
    let text = resp.text().await?;

    if !status.is_success() {
        return Err(format!("Anthropic API error ({status}): {text}").into());
    }

    let parsed: AnthropicResponse = serde_json::from_str(&text)?;
    parsed
        .content
        .first()
        .map(|c| c.text.clone())
        .ok_or_else(|| "No response from Anthropic".into())
}

// --- Agent loop ---

async fn run_agent(
    provider: &Provider,
    api_key: &str,
    model: &str,
    user_message: &str,
    auto: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut messages = vec![
        Message {
            role: Role::System,
            content: SYSTEM_PROMPT.to_string(),
        },
        Message {
            role: Role::User,
            content: user_message.to_string(),
        },
    ];

    const MAX_ITERATIONS: usize = 20;

    for _ in 0..MAX_ITERATIONS {
        let response = match provider {
            Provider::Openai => call_openai(api_key, model, &messages).await?,
            Provider::Anthropic => call_anthropic(api_key, model, &messages).await?,
        };

        messages.push(Message {
            role: Role::Assistant,
            content: response.clone(),
        });

        let tool_call = match parse_tool_call(&response) {
            Some(tc) => tc,
            None => {
                // No tool call — this is the final answer
                println!("{response}");
                return Ok(());
            }
        };

        // Print the LLM's reasoning (text before the tool tag) to stderr
        if let Some(idx) = response.find("<tool") {
            let reasoning = response[..idx].trim();
            if !reasoning.is_empty() {
                eprintln!("{reasoning}");
            }
        }

        let approved = if auto {
            eprintln!("[auto] Running: {}", tool_call.input);
            true
        } else {
            confirm_tool_call(&tool_call)
        };

        if approved {
            let result = execute_tool(&tool_call).await;
            eprintln!("{result}");
            messages.push(Message {
                role: Role::User,
                content: format!("<tool_result>\n{result}\n</tool_result>"),
            });
        } else {
            messages.push(Message {
                role: Role::User,
                content:
                    "Tool call denied by user. Try a different approach or answer without tools."
                        .to_string(),
            });
        }
    }

    eprintln!("Agent loop reached maximum iterations ({MAX_ITERATIONS})");
    Ok(())
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let env_var = match cli.provider {
        Provider::Openai => "OPENAI_API_KEY",
        Provider::Anthropic => "ANTHROPIC_API_KEY",
    };

    let api_key = cli.api_key.unwrap_or_else(|| {
        std::env::var(env_var).unwrap_or_else(|_| {
            eprintln!("Error: API key not provided. Pass --api-key or set {env_var}");
            std::process::exit(1);
        })
    });

    if let Err(e) = run_agent(&cli.provider, &api_key, &cli.model, &cli.message, cli.auto).await {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}
