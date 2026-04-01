mod commands;
mod config;
mod llm;
mod llm_anthropic;
mod llm_openai;
mod tools;
mod ui;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use clap::{Parser, ValueEnum};

use config::{
    MAX_ITERATIONS, MAX_MESSAGES, SPINNER_PHRASES, SYSTEM_PROMPT, config_get, config_set,
    load_config,
};
use llm::TokenUsage;
use ui::{AgentUI, InteractiveUI, PlainUI};

/// Result of a single agent turn.
struct TurnResult {
    answer: String,
    usage: TokenUsage,
    llm_calls: u32,
    tool_calls: u32,
    elapsed: std::time::Duration,
    last_input_tokens: u64,
}

#[derive(Debug, Clone, ValueEnum)]
enum Provider {
    Openai,
    Anthropic,
}

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Fetch the version from the remote Cargo.toml on GitHub.
/// Returns `Some(version_string)` on success, `None` on failure.
pub(crate) async fn fetch_remote_version() -> Option<String> {
    let url = "https://raw.githubusercontent.com/pwittchen/aictl/refs/heads/master/Cargo.toml";
    let client = config::http_client();
    let body = client
        .get(url)
        .timeout(std::time::Duration::from_secs(3))
        .send()
        .await
        .ok()?
        .text()
        .await
        .ok()?;
    body.lines().find_map(|line| {
        let rest = line.strip_prefix("version")?;
        let (_, val) = rest.split_once('=')?;
        Some(val.trim().trim_matches('"').to_string())
    })
}

/// Format a version status string from a remote version check result.
pub(crate) fn version_info_string(remote: Option<&str>) -> String {
    match remote {
        Some(v) if v == VERSION => "(latest)".to_string(),
        Some(v) => format!("({v} available)"),
        None => String::new(),
    }
}

#[derive(Parser)]
#[command(name = "aictl", version = VERSION, disable_version_flag = true, about = "AI agent for the terminal", after_help = "Omit --message to start an interactive REPL with persistent conversation history.")]
#[allow(clippy::struct_excessive_bools)]
struct Cli {
    /// Print version information
    #[arg(short = 'V', long = "version")]
    version: bool,

    /// Update to the latest version
    #[arg(short = 'u', long = "update")]
    update: bool,

    /// LLM provider to use (default: `AICTL_PROVIDER` from ~/.aictl)
    #[arg(short, long)]
    provider: Option<Provider>,

    /// Model to use, e.g. gpt-4o, claude-sonnet-4-20250514 (default: `AICTL_MODEL` from ~/.aictl)
    #[arg(short = 'M', long)]
    model: Option<String>,

    /// Message to send to the LLM (omit for interactive mode)
    #[arg(short, long)]
    message: Option<String>,

    /// Run in autonomous mode (skip tool confirmation prompts)
    #[arg(short, long)]
    auto: bool,

    /// Suppress tool calls and reasoning, only print the final answer (requires --auto)
    #[arg(short, long, requires = "auto")]
    quiet: bool,
}

// --- Esc key interrupt support ---

/// Error type for user-initiated interruption via Esc key.
#[derive(Debug)]
pub(crate) struct Interrupted;

impl std::fmt::Display for Interrupted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "interrupted")
    }
}

impl std::error::Error for Interrupted {}

/// Wrap a future so that pressing Esc cancels it.
///
/// Enables crossterm raw mode for the duration of the call and spawns
/// a blocking listener that polls for Esc key events. Returns
/// `Ok(value)` on normal completion or `Err(Interrupted)` if the user
/// pressed Esc.
pub(crate) async fn with_esc_cancel<F: std::future::Future>(
    future: F,
) -> Result<F::Output, Interrupted> {
    use crossterm::event::{self, Event, KeyCode, KeyEventKind};
    use tokio::sync::oneshot;

    let (tx, rx) = oneshot::channel::<()>();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_clone = stop.clone();

    let listener = tokio::task::spawn_blocking(move || {
        let _ = crossterm::terminal::enable_raw_mode();
        let mut tx = Some(tx);
        loop {
            if stop_clone.load(Ordering::Relaxed) {
                break;
            }
            if event::poll(std::time::Duration::from_millis(50)).unwrap_or(false)
                && let Ok(Event::Key(key)) = event::read()
                && key.code == KeyCode::Esc
                && key.kind == KeyEventKind::Press
            {
                if let Some(tx) = tx.take() {
                    let _ = tx.send(());
                }
                break;
            }
        }
        let _ = crossterm::terminal::disable_raw_mode();
    });

    let result = tokio::select! {
        value = future => Ok(value),
        _ = rx => Err(Interrupted),
    };

    stop.store(true, Ordering::Relaxed);
    let _ = listener.await;

    result
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

/// Run one turn of the agent loop: send `user_message`, handle tool calls,
/// return the final text answer.
#[allow(clippy::too_many_lines)]
async fn run_agent_turn(
    provider: &Provider,
    api_key: &str,
    model: &str,
    messages: &mut Vec<Message>,
    user_message: &str,
    auto: &mut bool,
    ui: &dyn AgentUI,
) -> Result<TurnResult, Box<dyn std::error::Error>> {
    messages.push(Message {
        role: Role::User,
        content: user_message.to_string(),
    });

    let mut total_usage = TokenUsage::default();
    let mut tool_calls = 0u32;
    let turn_start = std::time::Instant::now();
    #[allow(unused_assignments)]
    let mut last_input_tokens = 0u64;

    for llm_calls in 1..=MAX_ITERATIONS {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos() as usize;
        let phrase = SPINNER_PHRASES[nanos % SPINNER_PHRASES.len()];
        ui.start_spinner(phrase);

        let call_start = std::time::Instant::now();
        let result = match provider {
            Provider::Openai => {
                with_esc_cancel(llm_openai::call_openai(api_key, model, messages)).await
            }
            Provider::Anthropic => {
                with_esc_cancel(llm_anthropic::call_anthropic(api_key, model, messages)).await
            }
        };
        let call_elapsed = call_start.elapsed();

        ui.stop_spinner();

        let result = result.map_err(|e| -> Box<dyn std::error::Error> { Box::new(e) })?;
        let (response, usage) = result?;

        total_usage.input_tokens += usage.input_tokens;
        total_usage.output_tokens += usage.output_tokens;
        last_input_tokens = usage.input_tokens;

        let token_pct = llm::pct(last_input_tokens, llm::context_limit(model));
        let message_pct = llm::pct_usize(messages.len(), MAX_MESSAGES);
        let context_pct = token_pct.max(message_pct);

        messages.push(Message {
            role: Role::Assistant,
            content: response.clone(),
        });

        let tool_call = tools::parse_tool_call(&response);
        ui.show_token_usage(
            &usage,
            model,
            tool_call.is_none(),
            tool_calls,
            call_elapsed,
            context_pct,
        );

        let Some(tool_call) = tool_call else {
            // No tool call — this is the final answer
            return Ok(TurnResult {
                answer: response,
                usage: total_usage,
                #[allow(clippy::cast_possible_truncation)] // MAX_ITERATIONS is 20
                llm_calls: llm_calls as u32,
                tool_calls,
                elapsed: turn_start.elapsed(),
                last_input_tokens,
            });
        };

        // Print the LLM's reasoning (text before the tool tag)
        if let Some(idx) = response.find("<tool") {
            let reasoning = response[..idx].trim();
            if !reasoning.is_empty() {
                ui.show_reasoning(reasoning);
            }
        }

        let approval = if *auto {
            ui.show_auto_tool(&tool_call);
            ui::ToolApproval::Allow
        } else {
            ui.confirm_tool(&tool_call)
        };

        if approval == ui::ToolApproval::AutoAccept {
            *auto = true;
        }

        if approval == ui::ToolApproval::Allow || approval == ui::ToolApproval::AutoAccept {
            tool_calls += 1;
            ui.start_spinner("running tool...");
            let result = with_esc_cancel(tools::execute_tool(&tool_call)).await;
            ui.stop_spinner();
            let result = result.map_err(|e| -> Box<dyn std::error::Error> { Box::new(e) })?;
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

    Err(format!(
        "Agent loop reached maximum iterations ({MAX_ITERATIONS}) after {:.1}s",
        turn_start.elapsed().as_secs_f64()
    )
    .into())
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
        content: SYSTEM_PROMPT.to_string(),
    }];

    let mut auto = auto;
    let ui = PlainUI { quiet };
    let turn = run_agent_turn(
        provider,
        api_key,
        model,
        &mut messages,
        user_message,
        &mut auto,
        &ui,
    )
    .await?;
    ui.show_answer(&turn.answer);
    if turn.llm_calls > 1 {
        ui.show_summary(
            &turn.usage,
            model,
            turn.llm_calls,
            turn.tool_calls,
            turn.elapsed,
            0,
        );
    }
    Ok(())
}

// --- Slash command tab completion ---

struct SlashCommandHelper;

impl rustyline::completion::Completer for SlashCommandHelper {
    type Candidate = rustyline::completion::Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &rustyline::Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Self::Candidate>)> {
        if let Some(prefix) = line[..pos].strip_prefix('/') {
            let matches: Vec<_> = commands::COMMANDS
                .iter()
                .filter(|cmd| cmd.starts_with(prefix))
                .map(|cmd| rustyline::completion::Pair {
                    display: format!("/{cmd}"),
                    replacement: format!("/{cmd}"),
                })
                .collect();
            Ok((0, matches))
        } else {
            Ok((0, vec![]))
        }
    }
}

impl rustyline::hint::Hinter for SlashCommandHelper {
    type Hint = String;

    fn hint(&self, line: &str, pos: usize, _ctx: &rustyline::Context<'_>) -> Option<String> {
        if pos != line.len() {
            return None;
        }
        let prefix = line.strip_prefix('/')?;
        if prefix.is_empty() {
            return None;
        }
        commands::COMMANDS
            .iter()
            .find(|cmd| cmd.starts_with(prefix) && **cmd != prefix)
            .map(|cmd| cmd[prefix.len()..].to_string())
    }
}
impl rustyline::highlight::Highlighter for SlashCommandHelper {
    fn highlight_hint<'h>(&self, hint: &'h str) -> std::borrow::Cow<'h, str> {
        std::borrow::Cow::Owned(format!("\x1b[90m{hint}\x1b[0m"))
    }
}
impl rustyline::validate::Validator for SlashCommandHelper {}
impl rustyline::Helper for SlashCommandHelper {}

/// Interactive REPL mode: multi-turn conversation with persistent history.
#[allow(clippy::too_many_lines)]
async fn run_interactive(
    mut provider: Provider,
    mut api_key: String,
    mut model: String,
    auto: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    use crossterm::style::{Attribute, Color, Stylize};
    use rustyline::error::ReadlineError;

    let mut auto = auto;
    let ui = InteractiveUI::new();
    let version_info = version_info_string(fetch_remote_version().await.as_deref());
    InteractiveUI::print_welcome(
        &format!("{provider:?}").to_lowercase(),
        &model,
        &version_info,
    );

    let mut messages = vec![Message {
        role: Role::System,
        content: SYSTEM_PROMPT.to_string(),
    }];

    let mut rl = rustyline::Editor::new()?;
    rl.set_helper(Some(SlashCommandHelper));

    // Load history
    let history_path = std::env::var("HOME")
        .map(|h| format!("{h}/.aictl_history"))
        .unwrap_or_default();
    if !history_path.is_empty() {
        let _ = rl.load_history(&history_path);
    }

    let mut last_answer = String::new();
    let mut last_input_tokens: u64 = 0;

    loop {
        let prompt = if auto {
            format!(
                "{} {} ",
                "[auto]".with(Color::Yellow),
                "❯".with(Color::Cyan).attribute(Attribute::Bold),
            )
        } else {
            format!("{} ", "❯".with(Color::Cyan).attribute(Attribute::Bold))
        };
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

                // Slash commands
                match commands::handle(&input, &last_answer, &|msg| ui.show_error(msg)) {
                    commands::CommandResult::Exit => break,
                    commands::CommandResult::Clear => {
                        let _ = rl.add_history_entry(&input);
                        messages.truncate(1); // keep only system prompt
                        last_answer.clear();
                        last_input_tokens = 0;
                        println!();
                        println!("  {} context cleared", "✓".with(Color::Green));
                        println!();
                        continue;
                    }
                    commands::CommandResult::Compact => {
                        let _ = rl.add_history_entry(&input);
                        commands::compact(&provider, &api_key, &model, &mut messages, &ui).await;
                        last_input_tokens = 0;
                        continue;
                    }
                    commands::CommandResult::Context => {
                        let _ = rl.add_history_entry(&input);
                        commands::print_context(
                            &model,
                            messages.len(),
                            last_input_tokens,
                            MAX_MESSAGES,
                        );
                        continue;
                    }
                    commands::CommandResult::Info => {
                        let _ = rl.add_history_entry(&input);
                        let pname = format!("{provider:?}").to_lowercase();
                        commands::print_info(&pname, &model, auto, &version_info);
                        continue;
                    }
                    commands::CommandResult::Update => {
                        let _ = rl.add_history_entry(&input);
                        if commands::run_update(&|msg| ui.show_error(msg)).await {
                            break;
                        }
                        continue;
                    }
                    commands::CommandResult::Model => {
                        let _ = rl.add_history_entry(&input);
                        if let Some((new_provider, new_model, api_key_name)) =
                            commands::select_model(&model)
                        {
                            let Some(new_api_key) = config_get(&api_key_name) else {
                                ui.show_error(&format!(
                                    "API key not found. Set {api_key_name} in ~/.aictl"
                                ));
                                continue;
                            };
                            config_set(
                                "AICTL_PROVIDER",
                                &format!("{new_provider:?}").to_lowercase(),
                            );
                            config_set("AICTL_MODEL", &new_model);
                            provider = new_provider;
                            model = new_model;
                            api_key = new_api_key;
                            let pname = format!("{provider:?}").to_lowercase();
                            println!();
                            println!("  {} switched to {pname}/{model}", "✓".with(Color::Green));
                            println!();
                        }
                        continue;
                    }
                    commands::CommandResult::Mode => {
                        let _ = rl.add_history_entry(&input);
                        if let Some(new_auto) = commands::select_mode(auto) {
                            auto = new_auto;
                            let mode_name = if auto { "auto" } else { "human-in-the-loop" };
                            println!();
                            println!("  {} switched to {mode_name} mode", "✓".with(Color::Green));
                            println!();
                        }
                        continue;
                    }
                    commands::CommandResult::Continue => {
                        let _ = rl.add_history_entry(&input);
                        continue;
                    }
                    commands::CommandResult::NotACommand => {}
                }

                let _ = rl.add_history_entry(&input);

                // Auto-compact if context is >= 80%
                let token_pct = llm::pct(last_input_tokens, llm::context_limit(&model));
                let message_pct = llm::pct_usize(messages.len(), MAX_MESSAGES);
                let context_pct = token_pct.max(message_pct);
                if context_pct >= 80 {
                    println!();
                    println!(
                        "  {} context at {context_pct}%, auto-compacting...",
                        "⚠".with(Color::Yellow)
                    );
                    commands::compact(&provider, &api_key, &model, &mut messages, &ui).await;
                    last_input_tokens = 0;
                }

                let msg_len_before = messages.len();
                match run_agent_turn(
                    &provider,
                    &api_key,
                    &model,
                    &mut messages,
                    &input,
                    &mut auto,
                    &ui,
                )
                .await
                {
                    Ok(turn) => {
                        ui.show_answer(&turn.answer);
                        last_answer = turn.answer;
                        last_input_tokens = turn.last_input_tokens;
                        if turn.llm_calls > 1 {
                            let tp = llm::pct(turn.last_input_tokens, llm::context_limit(&model));
                            let mp = llm::pct_usize(messages.len(), MAX_MESSAGES);
                            let cp = tp.max(mp);
                            ui.show_summary(
                                &turn.usage,
                                &model,
                                turn.llm_calls,
                                turn.tool_calls,
                                turn.elapsed,
                                cp,
                            );
                        }
                    }
                    Err(e) => {
                        if e.downcast_ref::<Interrupted>().is_some() {
                            messages.truncate(msg_len_before);
                            println!("\n  {} interrupted\n", "✗".with(Color::Yellow));
                        } else {
                            ui.show_error(&format!("Error: {e}"));
                        }
                    }
                }
            }
            Err(ReadlineError::Interrupted) => {
                // Ctrl+C: cancel current line
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
    load_config();

    let cli = Cli::parse();

    if cli.version {
        let version_info = version_info_string(fetch_remote_version().await.as_deref());
        if version_info.is_empty() {
            println!("aictl {VERSION}");
        } else {
            println!("aictl {VERSION} {version_info}");
        }
        return;
    }

    if cli.update {
        commands::run_update_cli().await;
        return;
    }

    let provider = cli.provider.unwrap_or_else(|| {
        match config_get("AICTL_PROVIDER").as_deref() {
            Some("openai") => Provider::Openai,
            Some("anthropic") => Provider::Anthropic,
            Some(other) => {
                eprintln!("Error: invalid AICTL_PROVIDER value '{other}' (expected 'openai' or 'anthropic')");
                std::process::exit(1);
            }
            None => {
                eprintln!("Error: no provider specified. Use --provider or set AICTL_PROVIDER in ~/.aictl");
                std::process::exit(1);
            }
        }
    });

    let model = cli.model.unwrap_or_else(|| {
        config_get("AICTL_MODEL").unwrap_or_else(|| {
            eprintln!("Error: no model specified. Use --model or set AICTL_MODEL in ~/.aictl");
            std::process::exit(1);
        })
    });

    let key_name = match provider {
        Provider::Openai => "OPENAI_API_KEY",
        Provider::Anthropic => "ANTHROPIC_API_KEY",
    };

    let api_key = config_get(key_name).unwrap_or_else(|| {
        eprintln!("Error: API key not provided. Set {key_name} in ~/.aictl");
        std::process::exit(1);
    });

    let result = match cli.message {
        Some(ref msg) => {
            run_agent_single(&provider, &api_key, &model, msg, cli.auto, cli.quiet).await
        }
        None => run_interactive(provider, api_key, model, cli.auto).await,
    };

    if let Err(e) = result {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}
