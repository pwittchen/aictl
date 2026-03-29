mod llm;
mod tools;
mod ui;

use clap::{Parser, ValueEnum};

use llm::TokenUsage;
use ui::{AgentUI, InteractiveUI, PlainUI};

#[derive(Debug, Clone, ValueEnum)]
enum Provider {
    Openai,
    Anthropic,
}

#[derive(Parser)]
#[command(name = "aictl", about = "CLI tool for interacting with LLM APIs")]
struct Cli {
    /// LLM provider to use (default: AICTL_PROVIDER env var)
    #[arg(short, long)]
    provider: Option<Provider>,

    /// Model to use, e.g. gpt-4o, claude-sonnet-4-20250514 (default: AICTL_MODEL env var)
    #[arg(short, long)]
    model: Option<String>,

    /// Message to send to the LLM (omit for interactive mode)
    #[arg(short = 'M', long)]
    message: Option<String>,

    /// Run in autonomous mode (skip tool confirmation prompts)
    #[arg(long)]
    auto: bool,

    /// Suppress tool calls and reasoning, only print the final answer (requires --auto)
    #[arg(short, long, requires = "auto")]
    quiet: bool,
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

const MAX_ITERATIONS: usize = 20;

const SPINNER_PHRASES: &[&str] = &[
    "consulting the mass of wires...",
    "asking the silicon oracle...",
    "shaking the magic 8-ball...",
    "reticulating splines...",
    "bribing the electrons...",
    "poking the neural hamsters...",
    "unfolding the paper brain...",
    "warming up the thought lasers...",
    "juggling tensors...",
    "feeding the token monster...",
    "polishing the crystal CPU...",
    "summoning the context window...",
    "defrosting the weights...",
    "herding stochastic parrots...",
    "spinning up the vibe engine...",
    "negotiating with gradient descent...",
    "tuning the hallucination dial...",
    "charging the inference hamster wheel...",
    "compressing the universe into tokens...",
    "asking a very expensive rock to think...",
    "thinking...",
];

/// Run one turn of the agent loop: send user_message, handle tool calls,
/// return the final text answer.
async fn run_agent_turn(
    provider: &Provider,
    api_key: &str,
    model: &str,
    messages: &mut Vec<Message>,
    user_message: &str,
    auto: bool,
    ui: &dyn AgentUI,
) -> Result<(String, TokenUsage, u32, u32, std::time::Duration), Box<dyn std::error::Error>> {
    messages.push(Message {
        role: Role::User,
        content: user_message.to_string(),
    });

    let mut total_usage = TokenUsage::default();
    let mut llm_calls = 0u32;
    let mut tool_calls = 0u32;
    let turn_start = std::time::Instant::now();

    for _ in 0..MAX_ITERATIONS {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos() as usize;
        let phrase = SPINNER_PHRASES[nanos % SPINNER_PHRASES.len()];
        ui.start_spinner(phrase);

        let call_start = std::time::Instant::now();
        let result = match provider {
            Provider::Openai => llm::openai::call_openai(api_key, model, messages).await,
            Provider::Anthropic => llm::anthropic::call_anthropic(api_key, model, messages).await,
        };
        let call_elapsed = call_start.elapsed();

        ui.stop_spinner();

        let (response, usage) = result?;

        total_usage.input_tokens += usage.input_tokens;
        total_usage.output_tokens += usage.output_tokens;
        llm_calls += 1;

        messages.push(Message {
            role: Role::Assistant,
            content: response.clone(),
        });

        let tool_call = tools::parse_tool_call(&response);
        ui.show_token_usage(&usage, model, tool_call.is_none(), tool_calls, call_elapsed);

        let tool_call = match tool_call {
            Some(tc) => tc,
            None => {
                // No tool call — this is the final answer
                return Ok((response, total_usage, llm_calls, tool_calls, turn_start.elapsed()));
            }
        };

        // Print the LLM's reasoning (text before the tool tag)
        if let Some(idx) = response.find("<tool") {
            let reasoning = response[..idx].trim();
            if !reasoning.is_empty() {
                ui.show_reasoning(reasoning);
            }
        }

        let approved = if auto {
            ui.show_auto_tool(&tool_call);
            true
        } else {
            ui.confirm_tool(&tool_call)
        };

        if approved {
            tool_calls += 1;
            ui.start_spinner("running tool...");
            let result = tools::execute_tool(&tool_call).await;
            ui.stop_spinner();
            ui.show_tool_result(&result);
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

    Err(format!("Agent loop reached maximum iterations ({MAX_ITERATIONS}) after {:.1}s", turn_start.elapsed().as_secs_f64()).into())
}

/// Single-shot mode: run one message and print the result.
async fn run_agent_single(
    provider: &Provider,
    api_key: &str,
    model: &str,
    user_message: &str,
    auto: bool,
    quiet: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut messages = vec![Message {
        role: Role::System,
        content: tools::SYSTEM_PROMPT.to_string(),
    }];

    let ui = PlainUI { quiet };
    let (answer, usage, llm_calls, tool_calls, elapsed) = run_agent_turn(
        provider,
        api_key,
        model,
        &mut messages,
        user_message,
        auto,
        &ui,
    )
    .await?;
    ui.show_answer(&answer);
    ui.show_summary(&usage, model, llm_calls, tool_calls, elapsed);
    Ok(())
}

/// Interactive REPL mode: multi-turn conversation with persistent history.
async fn run_interactive(
    provider: &Provider,
    api_key: &str,
    model: &str,
    auto: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    use crossterm::style::{Attribute, Color, Stylize};
    use rustyline::error::ReadlineError;

    let ui = InteractiveUI::new();
    let provider_name = format!("{:?}", provider).to_lowercase();
    InteractiveUI::print_welcome(&provider_name, model);

    let mut messages = vec![Message {
        role: Role::System,
        content: tools::SYSTEM_PROMPT.to_string(),
    }];

    let mut rl = rustyline::DefaultEditor::new()?;

    // Load history
    let history_path = std::env::var("HOME")
        .map(|h| format!("{h}/.aictl_history"))
        .unwrap_or_default();
    if !history_path.is_empty() {
        let _ = rl.load_history(&history_path);
    }

    let prompt = format!("{} ", "❯".with(Color::Cyan).attribute(Attribute::Bold));

    loop {
        let line = rl.readline(&prompt);
        match line {
            Ok(input) => {
                let input = input.trim().to_string();
                if input.is_empty() {
                    continue;
                }
                if input == "exit" || input == "quit" {
                    break;
                }
                let _ = rl.add_history_entry(&input);

                match run_agent_turn(
                    provider,
                    api_key,
                    model,
                    &mut messages,
                    &input,
                    auto,
                    &ui,
                )
                .await
                {
                    Ok((answer, usage, llm_calls, tool_calls, elapsed)) => {
                        ui.show_answer(&answer);
                        ui.show_summary(&usage, model, llm_calls, tool_calls, elapsed);
                    }
                    Err(e) => ui.show_error(&format!("Error: {e}")),
                }
            }
            Err(ReadlineError::Interrupted) => {
                // Ctrl+C: cancel current line, continue
                continue;
            }
            Err(ReadlineError::Eof) => {
                // Ctrl+D: exit
                break;
            }
            Err(e) => {
                ui.show_error(&format!("Input error: {e}"));
                break;
            }
        }
    }

    // Save history
    if !history_path.is_empty() {
        let _ = rl.save_history(&history_path);
    }

    Ok(())
}

#[tokio::main]
async fn main() {
    load_env_file();

    let cli = Cli::parse();

    let provider = cli.provider.unwrap_or_else(|| {
        match std::env::var("AICTL_PROVIDER").as_deref() {
            Ok("openai") => Provider::Openai,
            Ok("anthropic") => Provider::Anthropic,
            Ok(other) => {
                eprintln!("Error: invalid AICTL_PROVIDER value '{other}' (expected 'openai' or 'anthropic')");
                std::process::exit(1);
            }
            Err(_) => {
                eprintln!("Error: no provider specified. Use --provider or set AICTL_PROVIDER in .env / environment");
                std::process::exit(1);
            }
        }
    });

    let model = cli.model.unwrap_or_else(|| {
        std::env::var("AICTL_MODEL").unwrap_or_else(|_| {
            eprintln!("Error: no model specified. Use --model or set AICTL_MODEL in .env / environment");
            std::process::exit(1);
        })
    });

    let env_var = match provider {
        Provider::Openai => "OPENAI_API_KEY",
        Provider::Anthropic => "ANTHROPIC_API_KEY",
    };

    let api_key = std::env::var(env_var).unwrap_or_else(|_| {
        eprintln!(
            "Error: API key not provided. Set {env_var} in .env or as an environment variable"
        );
        std::process::exit(1);
    });

    let result = match cli.message {
        Some(ref msg) => {
            run_agent_single(&provider, &api_key, &model, msg, cli.auto, cli.quiet).await
        }
        None => run_interactive(&provider, &api_key, &model, cli.auto).await,
    };

    if let Err(e) = result {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}
