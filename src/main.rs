mod llm;
mod tools;

use clap::{Parser, ValueEnum};

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

fn load_env_file() {
    let contents = match std::fs::read_to_string(".env") {
        Ok(c) => c,
        Err(_) => return,
    };

    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let line = line.strip_prefix("export ").unwrap_or(line);

        let Some((key, value)) = line.split_once('=') else {
            continue;
        };

        let key = key.trim();
        let mut value = value.trim();

        if (value.starts_with('"') && value.ends_with('"'))
            || (value.starts_with('\'') && value.ends_with('\''))
        {
            value = &value[1..value.len() - 1];
        }

        // SAFETY: called early in main() before any other threads are spawned.
        unsafe { std::env::set_var(key, value) };
    }
}

// --- Provider-agnostic types ---

#[derive(Debug, Clone)]
pub enum Role {
    System,
    User,
    Assistant,
}

#[derive(Debug, Clone)]
pub struct Message {
    pub role: Role,
    pub content: String,
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
            content: tools::SYSTEM_PROMPT.to_string(),
        },
        Message {
            role: Role::User,
            content: user_message.to_string(),
        },
    ];

    const MAX_ITERATIONS: usize = 20;

    for _ in 0..MAX_ITERATIONS {
        let response = match provider {
            Provider::Openai => llm::openai::call_openai(api_key, model, &messages).await?,
            Provider::Anthropic => {
                llm::anthropic::call_anthropic(api_key, model, &messages).await?
            }
        };

        messages.push(Message {
            role: Role::Assistant,
            content: response.clone(),
        });

        let tool_call = match tools::parse_tool_call(&response) {
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
            tools::confirm_tool_call(&tool_call)
        };

        if approved {
            let result = tools::execute_tool(&tool_call).await;
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
    load_env_file();

    let cli = Cli::parse();

    let env_var = match cli.provider {
        Provider::Openai => "OPENAI_API_KEY",
        Provider::Anthropic => "ANTHROPIC_API_KEY",
    };

    let api_key = std::env::var(env_var).unwrap_or_else(|_| {
        eprintln!(
            "Error: API key not provided. Set {env_var} in .env or as an environment variable"
        );
        std::process::exit(1);
    });

    if let Err(e) = run_agent(&cli.provider, &api_key, &cli.model, &cli.message, cli.auto).await {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}
