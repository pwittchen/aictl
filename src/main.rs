mod agents;
mod commands;
mod config;
mod keys;
mod llm;
mod security;
mod session;
mod stats;
mod tools;
mod ui;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use clap::{Parser, ValueEnum};

use commands::MemoryMode;
use config::{
    MAX_MESSAGES, SHORT_TERM_MEMORY_WINDOW, SPINNER_PHRASES, SYSTEM_PROMPT, auto_compact_threshold,
    config_get, config_set, load_config, load_prompt_file, max_iterations,
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
    Gemini,
    Grok,
    Mistral,
    Deepseek,
    Kimi,
    Zai,
    Ollama,
    Gguf,
    Mlx,
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
#[command(name = "aictl", version = VERSION, disable_version_flag = true, about = "AI agent in your terminal", after_help = "Omit --message to start an interactive REPL with persistent conversation history.")]
#[allow(clippy::struct_excessive_bools)]
struct Cli {
    /// Print version information
    #[arg(short = 'v', long = "version")]
    version: bool,

    /// Update to the latest version
    #[arg(long = "update")]
    update: bool,

    /// Remove the aictl binary from `~/.cargo/bin/` and `~/.local/bin/` (and
    /// `$AICTL_INSTALL_DIR` if set) and exit. Leaves `~/.aictl/` untouched.
    #[arg(long = "uninstall")]
    uninstall: bool,

    /// LLM provider to use (default: `AICTL_PROVIDER` from ~/.aictl/config)
    #[arg(long)]
    provider: Option<Provider>,

    /// Model to use, e.g. gpt-4o, claude-sonnet-4-20250514 (default: `AICTL_MODEL` from ~/.aictl/config)
    #[arg(long)]
    model: Option<String>,

    /// Message to send to the LLM (omit for interactive mode)
    #[arg(long)]
    message: Option<String>,

    /// Run in autonomous mode (skip tool confirmation prompts)
    #[arg(long)]
    auto: bool,

    /// Suppress tool calls and reasoning, only print the final answer (requires --auto)
    #[arg(long, requires = "auto")]
    quiet: bool,

    /// Disable security restrictions (use with caution)
    #[arg(long)]
    unrestricted: bool,

    /// Load a saved session by uuid or name (interactive mode only)
    #[arg(long = "session")]
    session: Option<String>,

    /// List all saved sessions and exit
    #[arg(long = "list-sessions")]
    list_sessions: bool,

    /// Clear all saved sessions and exit
    #[arg(long = "clear-sessions")]
    clear_sessions: bool,

    /// Start in incognito mode: interactive REPL without saving sessions
    #[arg(long)]
    incognito: bool,

    /// Load a saved agent by name
    #[arg(long = "agent")]
    agent: Option<String>,

    /// List all saved agents and exit
    #[arg(long = "list-agents")]
    list_agents: bool,

    /// Interactive configuration wizard for provider, model, and API keys
    #[arg(long = "config")]
    config: bool,

    /// Migrate API keys from ~/.aictl/config into the system keyring and exit
    #[arg(long = "lock-keys")]
    lock_keys: bool,

    /// Migrate API keys from the system keyring back into ~/.aictl/config and exit
    #[arg(long = "unlock-keys")]
    unlock_keys: bool,

    /// Remove API keys from both ~/.aictl/config and the system keyring and exit
    #[arg(long = "clear-keys")]
    clear_keys: bool,

    /// [experimental] Download a native local GGUF model (spec: hf:owner/repo/file.gguf,
    /// owner/repo:file.gguf, or an https:// URL). Saved under ~/.aictl/models/gguf/.
    #[arg(long = "pull-gguf-model", value_name = "SPEC")]
    pull_gguf_model: Option<String>,

    /// [experimental] List all downloaded native local GGUF models and exit.
    #[arg(long = "list-gguf-models")]
    list_gguf_models: bool,

    /// [experimental] Remove a downloaded native local GGUF model by name and exit.
    #[arg(long = "remove-gguf-model", value_name = "NAME")]
    remove_gguf_model: Option<String>,

    /// [experimental] Remove every downloaded native local GGUF model and exit.
    #[arg(long = "clear-gguf-models")]
    clear_gguf_models: bool,

    /// [experimental] Download a native MLX model from Hugging Face (spec:
    /// mlx:owner/repo or owner/repo). Saved under ~/.aictl/models/mlx/.
    #[arg(long = "pull-mlx-model", value_name = "SPEC")]
    pull_mlx_model: Option<String>,

    /// [experimental] List all downloaded MLX models and exit.
    #[arg(long = "list-mlx-models")]
    list_mlx_models: bool,

    /// [experimental] Remove a downloaded MLX model by name and exit.
    #[arg(long = "remove-mlx-model", value_name = "NAME")]
    remove_mlx_model: Option<String>,

    /// [experimental] Remove every downloaded MLX model and exit.
    #[arg(long = "clear-mlx-models")]
    clear_mlx_models: bool,
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
pub struct ImageData {
    pub base64_data: String,
    pub media_type: String,
}

#[derive(Debug, Clone)]
pub struct Message {
    pub role: Role,
    pub content: String,
    pub images: Vec<ImageData>,
}

/// Build the full system prompt, appending the project prompt file and loaded agent if present.
fn build_system_prompt() -> String {
    let mut prompt = SYSTEM_PROMPT.to_string();
    if let Some((name, content)) = load_prompt_file() {
        prompt.push_str("\n\n# Project prompt file (");
        prompt.push_str(&name);
        prompt.push_str(")\n\n");
        prompt.push_str(&content);
    }
    if let Some((name, agent_prompt)) = agents::loaded_agent() {
        prompt.push_str("\n\n# Agent: ");
        prompt.push_str(&name);
        prompt.push_str("\n\n");
        prompt.push_str(&agent_prompt);
    }
    prompt
}

// --- Agent loop ---

enum ToolAction {
    Executed,
    Denied,
}

/// Handle a single tool call: display reasoning, get approval, execute, push result.
async fn handle_tool_call(
    tool_call: &tools::ToolCall,
    response: &str,
    auto: &mut bool,
    ui: &dyn AgentUI,
    messages: &mut Vec<Message>,
) -> Result<ToolAction, Interrupted> {
    // Print the LLM's reasoning (text before the tool tag)
    if let Some(idx) = response.find("<tool") {
        let reasoning = response[..idx].trim();
        if !reasoning.is_empty() {
            ui.show_reasoning(reasoning);
        }
    }

    let approval = if *auto {
        ui.show_auto_tool(tool_call);
        ui::ToolApproval::Allow
    } else {
        ui.confirm_tool(tool_call)
    };

    if approval == ui::ToolApproval::AutoAccept {
        *auto = true;
    }

    if approval == ui::ToolApproval::Allow || approval == ui::ToolApproval::AutoAccept {
        ui.start_spinner("running tool...");
        let output = with_esc_cancel(tools::execute_tool(tool_call)).await?;
        ui.stop_spinner();
        ui.show_tool_result(&output.text);
        messages.push(Message {
            role: Role::User,
            content: format!("<tool_result>\n{}\n</tool_result>", output.text),
            images: output.images,
        });
        Ok(ToolAction::Executed)
    } else {
        messages.push(Message {
            role: Role::User,
            content: "Tool call denied by user. Try a different approach or answer without tools."
                .to_string(),
            images: vec![],
        });
        Ok(ToolAction::Denied)
    }
}

/// Build a windowed view of messages for short-term memory mode.
/// Keeps the system prompt (first message) and the most recent `window` messages.
fn windowed_messages(messages: &[Message], window: usize) -> Vec<Message> {
    if messages.len() <= 1 + window {
        return messages.to_vec();
    }
    let mut result = vec![messages[0].clone()];
    result.extend_from_slice(&messages[messages.len() - window..]);
    result
}

/// Run one turn of the agent loop: send `user_message`, handle tool calls,
/// return the final text answer.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn run_agent_turn(
    provider: &Provider,
    api_key: &str,
    model: &str,
    messages: &mut Vec<Message>,
    user_message: &str,
    auto: &mut bool,
    ui: &dyn AgentUI,
    memory: MemoryMode,
) -> Result<TurnResult, Box<dyn std::error::Error>> {
    if security::policy().enabled
        && security::policy().injection_guard
        && let Err(reason) = security::detect_prompt_injection(user_message)
    {
        return Err(format!("blocked: possible prompt injection ({reason})").into());
    }

    messages.push(Message {
        role: Role::User,
        content: user_message.to_string(),
        images: vec![],
    });

    let mut total_usage = TokenUsage::default();
    let mut tool_calls = 0u32;
    let turn_start = std::time::Instant::now();
    #[allow(unused_assignments)]
    let mut last_input_tokens = 0u64;

    let max_iter = max_iterations();
    for llm_calls in 1..=max_iter {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos() as usize;
        let phrase = SPINNER_PHRASES[nanos % SPINNER_PHRASES.len()];
        ui.start_spinner(phrase);

        // In LongTerm mode we pass the history directly as a slice, avoiding a
        // full clone of every message on every loop iteration. ShortTerm mode
        // still materializes a windowed Vec, but `short_term_buf` owns it only
        // when that branch runs.
        let short_term_buf;
        let llm_messages: &[Message] = match memory {
            MemoryMode::LongTerm => messages.as_slice(),
            MemoryMode::ShortTerm => {
                short_term_buf = windowed_messages(messages, SHORT_TERM_MEMORY_WINDOW);
                &short_term_buf
            }
        };

        let call_start = std::time::Instant::now();
        let llm_timeout = config::llm_timeout();
        let result = match provider {
            Provider::Openai => {
                with_esc_cancel(tokio::time::timeout(
                    llm_timeout,
                    llm::openai::call_openai(api_key, model, llm_messages),
                ))
                .await
            }
            Provider::Anthropic => {
                with_esc_cancel(tokio::time::timeout(
                    llm_timeout,
                    llm::anthropic::call_anthropic(api_key, model, llm_messages),
                ))
                .await
            }
            Provider::Gemini => {
                with_esc_cancel(tokio::time::timeout(
                    llm_timeout,
                    llm::gemini::call_gemini(api_key, model, llm_messages),
                ))
                .await
            }
            Provider::Grok => {
                with_esc_cancel(tokio::time::timeout(
                    llm_timeout,
                    llm::grok::call_grok(api_key, model, llm_messages),
                ))
                .await
            }
            Provider::Mistral => {
                with_esc_cancel(tokio::time::timeout(
                    llm_timeout,
                    llm::mistral::call_mistral(api_key, model, llm_messages),
                ))
                .await
            }
            Provider::Deepseek => {
                with_esc_cancel(tokio::time::timeout(
                    llm_timeout,
                    llm::deepseek::call_deepseek(api_key, model, llm_messages),
                ))
                .await
            }
            Provider::Kimi => {
                with_esc_cancel(tokio::time::timeout(
                    llm_timeout,
                    llm::kimi::call_kimi(api_key, model, llm_messages),
                ))
                .await
            }
            Provider::Zai => {
                with_esc_cancel(tokio::time::timeout(
                    llm_timeout,
                    llm::zai::call_zai(api_key, model, llm_messages),
                ))
                .await
            }
            Provider::Ollama => {
                with_esc_cancel(tokio::time::timeout(
                    llm_timeout,
                    llm::ollama::call_ollama(model, llm_messages),
                ))
                .await
            }
            Provider::Gguf => {
                with_esc_cancel(tokio::time::timeout(
                    llm_timeout,
                    llm::gguf::call_gguf(model, llm_messages),
                ))
                .await
            }
            Provider::Mlx => {
                with_esc_cancel(tokio::time::timeout(
                    llm_timeout,
                    llm::mlx::call_mlx(model, llm_messages),
                ))
                .await
            }
        };
        let call_elapsed = call_start.elapsed();

        ui.stop_spinner();

        let result = result.map_err(|e| -> Box<dyn std::error::Error> { Box::new(e) })?;
        let result = result.map_err(|_| -> Box<dyn std::error::Error> {
            format!(
                "LLM call exceeded the {}s timeout. Increase AICTL_LLM_TIMEOUT in ~/.aictl/config (seconds, 0 disables) if this is expected on your hardware.",
                llm_timeout.as_secs()
            )
            .into()
        })?;
        let (response, usage) = result?;

        total_usage.input_tokens += usage.input_tokens;
        total_usage.output_tokens += usage.output_tokens;
        total_usage.cache_creation_input_tokens += usage.cache_creation_input_tokens;
        total_usage.cache_read_input_tokens += usage.cache_read_input_tokens;
        last_input_tokens = usage.input_tokens;

        let token_pct = llm::pct(last_input_tokens, llm::context_limit(model));
        let message_pct = llm::pct_usize(messages.len(), MAX_MESSAGES);
        let context_pct = token_pct.max(message_pct);

        messages.push(Message {
            role: Role::Assistant,
            content: response.clone(),
            images: vec![],
        });

        let tool_call = tools::parse_tool_call(&response);
        let malformed_tool_call =
            tool_call.is_none() && tools::looks_like_malformed_tool_call(&response);
        ui.show_token_usage(
            &usage,
            model,
            tool_call.is_none() && !malformed_tool_call,
            tool_calls,
            call_elapsed,
            context_pct,
            &memory.to_string(),
        );

        if malformed_tool_call {
            // The model tried to emit a tool call but produced invalid XML.
            // Ask it to retry instead of surfacing raw markup as a final answer.
            ui.show_reasoning(
                "(detected a malformed <tool> tag — asking the model to retry with valid syntax)",
            );
            messages.push(Message {
                role: Role::User,
                content: "Your previous response contained a `<tool>` tag that could not be parsed. Retry using exactly this syntax: `<tool name=\"<tool_name>\">input</tool>`. If you did not intend to call a tool, reply with your final answer without any `<tool>` tags.".to_string(),
                images: vec![],
            });
            continue;
        }

        let Some(tool_call) = tool_call else {
            // No tool call — this is the final answer
            return Ok(TurnResult {
                answer: response,
                usage: total_usage,
                #[allow(clippy::cast_possible_truncation)] // max_iter is small (default 20)
                llm_calls: llm_calls as u32,
                tool_calls,
                elapsed: turn_start.elapsed(),
                last_input_tokens,
            });
        };

        // Abort the turn if the model is trying to repeat a tool call it
        // has already made this session. `tools::execute_tool` would reject
        // the duplicate anyway, but continuing the loop just gives the
        // model another chance to emit the same call.
        if tools::is_duplicate_call(&tool_call) {
            if let Some(idx) = response.find("<tool") {
                let reasoning = response[..idx].trim();
                if !reasoning.is_empty() {
                    ui.show_reasoning(reasoning);
                }
            }
            return Err(format!(
                "Agent stopped: model tried to call `{}` again with the same input — it is looping. Try a stronger model or rephrase the request.",
                tool_call.name
            )
            .into());
        }

        match handle_tool_call(&tool_call, &response, auto, ui, messages).await {
            Ok(ToolAction::Executed) => {
                tool_calls += 1;
            }
            Ok(ToolAction::Denied) => {}
            Err(e) => return Err(Box::new(e)),
        }
    }

    Err(format!(
        "Agent loop reached maximum iterations ({max_iter}) after {:.1}s",
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
        content: build_system_prompt(),
        images: vec![],
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
        MemoryMode::LongTerm,
    )
    .await?;
    stats::record(model, turn.llm_calls, turn.tool_calls, &turn.usage);
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

enum ReplAction {
    Continue,
    Break,
    RunAgentTurn,
    /// Run an agent turn with this message instead of the typed input
    /// (used by `/retry` to re-submit the previous user prompt).
    RunAgentTurnWith(String),
}

/// Handle a single REPL input line: dispatch slash commands, auto-compact, etc.
#[allow(
    clippy::too_many_arguments,
    clippy::fn_params_excessive_bools,
    clippy::too_many_lines
)]
async fn handle_repl_input(
    input: &str,
    last_answer: &mut String,
    ui: &InteractiveUI,
    rl: &mut rustyline::Editor<SlashCommandHelper, rustyline::history::DefaultHistory>,
    messages: &mut Vec<Message>,
    last_input_tokens: &mut u64,
    provider: &mut Provider,
    api_key: &mut String,
    model: &mut String,
    auto: &mut bool,
    memory: &mut MemoryMode,
    version_info: &str,
) -> ReplAction {
    use crossterm::style::{Color, Stylize};

    if input.is_empty() {
        return ReplAction::Continue;
    }
    if input == "exit" || input == "quit" {
        return ReplAction::Break;
    }

    match commands::handle(input, last_answer, &|msg| ui.show_error(msg)) {
        commands::CommandResult::Exit => return ReplAction::Break,
        commands::CommandResult::Clear => {
            let _ = rl.add_history_entry(input);
            messages.truncate(1);
            tools::clear_call_history();
            last_answer.clear();
            *last_input_tokens = 0;
            println!();
            println!("  {} context cleared", "✓".with(Color::Green));
            println!();
            return ReplAction::Continue;
        }
        commands::CommandResult::Compact => {
            let _ = rl.add_history_entry(input);
            commands::compact(
                provider,
                api_key,
                model,
                messages,
                ui,
                &memory.to_string(),
                false,
            )
            .await;
            *last_input_tokens = 0;
            session::save_current(messages);
            return ReplAction::Continue;
        }
        commands::CommandResult::Session => {
            let _ = rl.add_history_entry(input);
            if session::is_incognito() {
                println!();
                println!(
                    "  {} incognito mode: session functionality is disabled",
                    "⚠".with(Color::Yellow)
                );
                println!();
            } else {
                if commands::run_session_menu(messages, &|msg| ui.show_error(msg)) {
                    *last_input_tokens = 0;
                }
                session::save_current(messages);
            }
            return ReplAction::Continue;
        }
        commands::CommandResult::Agent => {
            let _ = rl.add_history_entry(input);
            commands::run_agent_menu(provider, api_key, model, messages, ui, &|msg| {
                ui.show_error(msg);
            })
            .await;
            return ReplAction::Continue;
        }
        commands::CommandResult::Gguf => {
            let _ = rl.add_history_entry(input);
            commands::run_gguf_menu(&|msg| ui.show_error(msg)).await;
            return ReplAction::Continue;
        }
        commands::CommandResult::Mlx => {
            let _ = rl.add_history_entry(input);
            commands::run_mlx_menu(&|msg| ui.show_error(msg)).await;
            return ReplAction::Continue;
        }
        commands::CommandResult::Context => {
            let _ = rl.add_history_entry(input);
            commands::print_context(model, messages.len(), *last_input_tokens, MAX_MESSAGES);
            return ReplAction::Continue;
        }
        commands::CommandResult::History(args) => {
            let _ = rl.add_history_entry(input);
            commands::print_history(messages, &args);
            return ReplAction::Continue;
        }
        commands::CommandResult::Info => {
            let _ = rl.add_history_entry(input);
            let pname = format!("{provider:?}").to_lowercase();
            let ollama_models = llm::ollama::list_models().await;
            commands::print_info(&pname, model, *auto, *memory, version_info, &ollama_models);
            return ReplAction::Continue;
        }
        commands::CommandResult::Security => {
            let _ = rl.add_history_entry(input);
            commands::print_security();
            return ReplAction::Continue;
        }
        commands::CommandResult::Continue => {
            let _ = rl.add_history_entry(input);
            return ReplAction::Continue;
        }
        commands::CommandResult::Keys => {
            let _ = rl.add_history_entry(input);
            commands::run_keys_menu(&|msg| ui.show_error(msg));
            return ReplAction::Continue;
        }
        commands::CommandResult::Stats => {
            let _ = rl.add_history_entry(input);
            commands::run_stats_menu(&|msg| ui.show_error(msg));
            return ReplAction::Continue;
        }
        commands::CommandResult::Retry => {
            let _ = rl.add_history_entry(input);
            let Some(prompt) = commands::retry_last_exchange(messages) else {
                ui.show_error("nothing to retry");
                return ReplAction::Continue;
            };
            tools::clear_call_history();
            last_answer.clear();
            *last_input_tokens = 0;
            let preview: String = prompt.chars().take(80).collect();
            let ellipsis = if prompt.chars().count() > 80 {
                "…"
            } else {
                ""
            };
            println!();
            println!(
                "  {} retry — resending: {}{}",
                "↩".with(Color::Yellow),
                preview.replace('\n', " ").with(Color::DarkGrey),
                ellipsis.with(Color::DarkGrey),
            );
            println!();
            session::save_current(messages);
            return ReplAction::RunAgentTurnWith(prompt);
        }
        commands::CommandResult::Config => {
            let _ = rl.add_history_entry(input);
            commands::run_config_wizard(true);
            // Re-read provider/model/api_key from config so the change takes
            // effect mid-session. The wizard may have been cancelled, in which
            // case the config values are unchanged and these reads are no-ops.
            if let Some(new_prov) = config_get("AICTL_PROVIDER") {
                let resolved = match new_prov.as_str() {
                    "openai" => Some(Provider::Openai),
                    "anthropic" => Some(Provider::Anthropic),
                    "gemini" => Some(Provider::Gemini),
                    "grok" => Some(Provider::Grok),
                    "mistral" => Some(Provider::Mistral),
                    "deepseek" => Some(Provider::Deepseek),
                    "kimi" => Some(Provider::Kimi),
                    "zai" => Some(Provider::Zai),
                    "ollama" => Some(Provider::Ollama),
                    "gguf" => Some(Provider::Gguf),
                    "mlx" => Some(Provider::Mlx),
                    _ => None,
                };
                if let Some(p) = resolved {
                    *provider = p;
                }
            }
            if let Some(new_model) = config_get("AICTL_MODEL") {
                *model = new_model;
            }
            if matches!(provider, Provider::Ollama | Provider::Gguf | Provider::Mlx) {
                *api_key = String::new();
            } else {
                let key_name = match provider {
                    Provider::Openai => "LLM_OPENAI_API_KEY",
                    Provider::Anthropic => "LLM_ANTHROPIC_API_KEY",
                    Provider::Gemini => "LLM_GEMINI_API_KEY",
                    Provider::Grok => "LLM_GROK_API_KEY",
                    Provider::Mistral => "LLM_MISTRAL_API_KEY",
                    Provider::Deepseek => "LLM_DEEPSEEK_API_KEY",
                    Provider::Kimi => "LLM_KIMI_API_KEY",
                    Provider::Zai => "LLM_ZAI_API_KEY",
                    Provider::Ollama | Provider::Gguf | Provider::Mlx => unreachable!(),
                };
                if let Some(k) = keys::get_secret(key_name) {
                    *api_key = k;
                } else {
                    ui.show_error(&format!(
                        "API key for {key_name} is not set — current session may fail until you run /config or /keys"
                    ));
                }
            }
            return ReplAction::Continue;
        }
        commands::CommandResult::Update => {
            let _ = rl.add_history_entry(input);
            if commands::run_update(&|msg| ui.show_error(msg)).await {
                return ReplAction::Break;
            }
            return ReplAction::Continue;
        }
        commands::CommandResult::Uninstall => {
            let _ = rl.add_history_entry(input);
            if commands::run_uninstall_repl(&|msg| ui.show_error(msg)) {
                return ReplAction::Break;
            }
            return ReplAction::Continue;
        }
        commands::CommandResult::Version => {
            let _ = rl.add_history_entry(input);
            commands::run_version(&|msg| ui.show_error(msg)).await;
            return ReplAction::Continue;
        }
        commands::CommandResult::Model => {
            let _ = rl.add_history_entry(input);
            let ollama_models = llm::ollama::list_models().await;
            let local_models = llm::gguf::list_models();
            let mlx_models = llm::mlx::list_models();
            if let Some((new_provider, new_model, api_key_name)) =
                commands::select_model(model, &ollama_models, &local_models, &mlx_models)
            {
                if matches!(
                    new_provider,
                    Provider::Ollama | Provider::Gguf | Provider::Mlx
                ) {
                    let pname = match new_provider {
                        Provider::Ollama => "ollama",
                        Provider::Gguf => "gguf",
                        Provider::Mlx => "mlx",
                        _ => unreachable!(),
                    };
                    config_set("AICTL_PROVIDER", pname);
                    config_set("AICTL_MODEL", &new_model);
                    *provider = new_provider;
                    *model = new_model;
                    *api_key = String::new();
                } else {
                    let Some(new_api_key) = keys::get_secret(&api_key_name) else {
                        ui.show_error(&format!(
                            "API key not found. Set {api_key_name} in ~/.aictl/config or run /keys to migrate from another provider"
                        ));
                        return ReplAction::Continue;
                    };
                    config_set(
                        "AICTL_PROVIDER",
                        &format!("{new_provider:?}").to_lowercase(),
                    );
                    config_set("AICTL_MODEL", &new_model);
                    *provider = new_provider;
                    *model = new_model;
                    *api_key = new_api_key;
                }
                let pname = format!("{provider:?}").to_lowercase();
                println!();
                println!("  {} switched to {pname}/{model}", "✓".with(Color::Green));
                println!();
            }
            return ReplAction::Continue;
        }
        commands::CommandResult::Behavior => {
            let _ = rl.add_history_entry(input);
            if let Some(new_auto) = commands::select_behavior(*auto) {
                *auto = new_auto;
                let behavior = if *auto { "auto" } else { "human-in-the-loop" };
                println!();
                println!(
                    "  {} switched to {behavior} behavior",
                    "✓".with(Color::Green)
                );
                println!();
            }
            return ReplAction::Continue;
        }
        commands::CommandResult::Memory => {
            let _ = rl.add_history_entry(input);
            if let Some(new_memory) = commands::select_memory(*memory) {
                *memory = new_memory;
                config_set("AICTL_MEMORY", &format!("{new_memory}"));
                println!();
                println!(
                    "  {} switched to {new_memory} memory",
                    "✓".with(Color::Green)
                );
                println!();
            }
            return ReplAction::Continue;
        }
        commands::CommandResult::NotACommand => {}
    }

    let _ = rl.add_history_entry(input);

    // Auto-compact if context is >= configured threshold (default 80%)
    let token_pct = llm::pct(*last_input_tokens, llm::context_limit(model));
    let message_pct = llm::pct_usize(messages.len(), MAX_MESSAGES);
    let context_pct = token_pct.max(message_pct);
    if context_pct >= auto_compact_threshold() {
        println!();
        println!(
            "  {} context at {context_pct}%, auto-compacting...",
            "⚠".with(Color::Yellow)
        );
        commands::compact(
            provider,
            api_key,
            model,
            messages,
            ui,
            &memory.to_string(),
            true,
        )
        .await;
        *last_input_tokens = 0;
        session::save_current(messages);
    }

    ReplAction::RunAgentTurn
}

/// Run an agent turn and display the result, updating REPL state.
#[allow(clippy::too_many_arguments)]
async fn run_and_display_turn(
    provider: &Provider,
    api_key: &str,
    model: &str,
    messages: &mut Vec<Message>,
    input: &str,
    auto: &mut bool,
    ui: &InteractiveUI,
    last_answer: &mut String,
    last_input_tokens: &mut u64,
    memory: MemoryMode,
) {
    use crossterm::style::{Color, Stylize};

    let msg_len_before = messages.len();
    match run_agent_turn(provider, api_key, model, messages, input, auto, ui, memory).await {
        Ok(turn) => {
            stats::record(model, turn.llm_calls, turn.tool_calls, &turn.usage);
            ui.show_answer(&turn.answer);
            *last_answer = turn.answer;
            *last_input_tokens = turn.last_input_tokens;
            if turn.llm_calls > 1 {
                let tp = llm::pct(turn.last_input_tokens, llm::context_limit(model));
                let mp = llm::pct_usize(messages.len(), MAX_MESSAGES);
                let cp = tp.max(mp);
                ui.show_summary(
                    &turn.usage,
                    model,
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

/// Interactive REPL mode: multi-turn conversation with persistent history.
#[allow(clippy::too_many_lines)]
async fn run_interactive(
    mut provider: Provider,
    mut api_key: String,
    mut model: String,
    auto: bool,
    session_key: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    use crossterm::style::{Attribute, Color, Stylize};
    use rustyline::error::ReadlineError;

    // Kick off the remote version check before anything else so it runs on
    // another worker while the rest of startup (config, session init, file I/O)
    // proceeds. At banner time we only consume the result if it's already
    // ready — we never block the banner on the network call. This replaces a
    // previous pattern that stalled the REPL for up to 3s on every launch.
    let version_fetch = tokio::spawn(fetch_remote_version());

    let mut auto = auto;
    let mut memory = match config_get("AICTL_MEMORY").as_deref() {
        Some("short-term") => MemoryMode::ShortTerm,
        _ => MemoryMode::LongTerm,
    };
    let ui = InteractiveUI::new();

    let mut messages = vec![Message {
        role: Role::System,
        content: build_system_prompt(),
        images: vec![],
    }];

    // Initialize session: load if requested, otherwise create a new one.
    // Skipped entirely in incognito mode.
    let mut loaded_ok = false;
    if session::is_incognito() {
        if session_key.is_some() {
            ui.show_error("--session is ignored in incognito mode");
        }
    } else if let Some(key) = session_key.as_ref() {
        if let Some(id) = session::resolve(key) {
            match session::load_messages(&id) {
                Ok(loaded) => {
                    let name = session::name_for(&id);
                    messages = loaded;
                    let label = name
                        .as_deref()
                        .map_or_else(|| id.clone(), |n| format!("{id} ({n})"));
                    session::set_current(id, name);
                    println!("  {} loaded session: {label}", "✓".with(Color::Green));
                    loaded_ok = true;
                }
                Err(e) => {
                    ui.show_error(&format!("Failed to load session '{key}': {e}"));
                }
            }
        } else {
            ui.show_error(&format!(
                "Session '{key}' not found. Starting a new session."
            ));
        }
    }
    if !loaded_ok && !session::is_incognito() {
        let id = session::generate_uuid();
        session::set_current(id, None);
    }
    if !session::is_incognito() {
        stats::record_session();
    }
    session::save_current(&messages);

    // Only display the remote-version notice if the background fetch has
    // already completed. Otherwise drop the handle and show an empty string
    // so the banner prints immediately.
    let version_info = if version_fetch.is_finished() {
        match version_fetch.await {
            Ok(remote) => version_info_string(remote.as_deref()),
            Err(_) => String::new(),
        }
    } else {
        String::new()
    };

    InteractiveUI::print_welcome(
        &format!("{provider:?}").to_lowercase(),
        &model,
        memory,
        &version_info,
    );

    let mut rl = rustyline::Editor::new()?;
    rl.set_helper(Some(SlashCommandHelper));

    // Load history
    let history_path = std::env::var("HOME")
        .map(|h| format!("{h}/.aictl/history"))
        .unwrap_or_default();
    if !history_path.is_empty() {
        let _ = rl.load_history(&history_path);
    }

    let mut last_answer = String::new();
    let mut last_input_tokens: u64 = 0;

    loop {
        let unrestricted = !crate::security::policy().enabled;
        let agent_prefix = agents::loaded_agent_name()
            .map(|name| format!("{} ", format!("[{name}]").with(Color::Magenta)));
        let ap = agent_prefix.as_deref().unwrap_or("");
        let prompt = match (auto, unrestricted) {
            (true, true) => format!(
                "{ap}{} {} {} ",
                "[auto]".with(Color::Yellow),
                "[unrestricted]".with(Color::Red),
                "❯".with(Color::Cyan).attribute(Attribute::Bold),
            ),
            (true, false) => format!(
                "{ap}{} {} ",
                "[auto]".with(Color::Yellow),
                "❯".with(Color::Cyan).attribute(Attribute::Bold),
            ),
            (false, true) => format!(
                "{ap}{} {} ",
                "[unrestricted]".with(Color::Red),
                "❯".with(Color::Cyan).attribute(Attribute::Bold),
            ),
            (false, false) => {
                format!("{ap}{} ", "❯".with(Color::Cyan).attribute(Attribute::Bold))
            }
        };
        let line = rl.readline(&prompt);
        match line {
            Ok(input) => {
                let input = input.trim().to_string();

                let retry_input = match handle_repl_input(
                    &input,
                    &mut last_answer,
                    &ui,
                    &mut rl,
                    &mut messages,
                    &mut last_input_tokens,
                    &mut provider,
                    &mut api_key,
                    &mut model,
                    &mut auto,
                    &mut memory,
                    &version_info,
                )
                .await
                {
                    ReplAction::Continue => continue,
                    ReplAction::Break => break,
                    ReplAction::RunAgentTurn => None,
                    ReplAction::RunAgentTurnWith(s) => Some(s),
                };

                let turn_input = retry_input.as_deref().unwrap_or(input.as_str());
                run_and_display_turn(
                    &provider,
                    &api_key,
                    &model,
                    &mut messages,
                    turn_input,
                    &mut auto,
                    &ui,
                    &mut last_answer,
                    &mut last_input_tokens,
                    memory,
                )
                .await;
                session::save_current(&messages);
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

    // Final save and exit notification.
    session::save_current(&messages);
    if let Some((id, name)) = session::current_info() {
        let label = name
            .as_deref()
            .map_or_else(|| id.clone(), |n| format!("{id} ({n})"));
        let resume_arg = name.as_deref().unwrap_or(&id);
        println!();
        println!("  {} session saved: {label}", "✓".with(Color::Green));
        println!(
            "  {} resume with: {} {}",
            "→".with(Color::Cyan),
            "aictl --session".with(Color::Cyan),
            resume_arg.with(Color::Cyan)
        );
        println!();
    }

    Ok(())
}

#[tokio::main]
#[allow(clippy::too_many_lines)]
async fn main() {
    load_config();

    let cli = Cli::parse();

    security::init(cli.unrestricted);
    if cli.unrestricted {
        eprintln!("Warning: security restrictions disabled (--unrestricted)");
    }

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

    if cli.uninstall {
        commands::run_uninstall_cli();
        return;
    }

    if cli.list_sessions {
        commands::print_sessions_cli();
        return;
    }

    if cli.clear_sessions {
        session::clear_all();
        println!("All saved sessions cleared.");
        return;
    }

    if cli.list_agents {
        commands::print_agents_cli();
        return;
    }

    if cli.config {
        commands::run_config_wizard(false);
        return;
    }

    if cli.lock_keys {
        commands::run_lock_keys(&|msg| eprintln!("Error: {msg}"));
        return;
    }

    if cli.unlock_keys {
        commands::run_unlock_keys(&|msg| eprintln!("Error: {msg}"));
        return;
    }

    if cli.clear_keys {
        commands::run_clear_keys_unconfirmed();
        return;
    }

    if let Some(spec) = cli.pull_gguf_model.as_deref() {
        match llm::gguf::download_model(spec, None).await {
            Ok(name) => println!("downloaded GGUF model: {name}"),
            Err(e) => {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    if cli.list_gguf_models {
        let models = llm::gguf::list_models();
        if models.is_empty() {
            println!(
                "No GGUF models downloaded. Use `aictl --pull-gguf-model <spec>` to fetch one."
            );
        } else {
            for m in models {
                println!("{m}");
            }
        }
        return;
    }

    if let Some(name) = cli.remove_gguf_model.as_deref() {
        match llm::gguf::remove_model(name) {
            Ok(()) => println!("removed GGUF model: {name}"),
            Err(e) => {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    if cli.clear_gguf_models {
        match llm::gguf::clear_models() {
            Ok(n) => println!("removed {n} GGUF model(s)"),
            Err(e) => {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    if let Some(spec) = cli.pull_mlx_model.as_deref() {
        match llm::mlx::download_model(spec, None).await {
            Ok(name) => println!("downloaded MLX model: {name}"),
            Err(e) => {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    if cli.list_mlx_models {
        let models = llm::mlx::list_models();
        if models.is_empty() {
            println!("No MLX models downloaded. Use `aictl --pull-mlx-model <spec>` to fetch one.");
        } else {
            for m in models {
                println!("{m}");
            }
        }
        return;
    }

    if let Some(name) = cli.remove_mlx_model.as_deref() {
        match llm::mlx::remove_model(name) {
            Ok(()) => println!("removed MLX model: {name}"),
            Err(e) => {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    if cli.clear_mlx_models {
        match llm::mlx::clear_models() {
            Ok(n) => println!("removed {n} MLX model(s)"),
            Err(e) => {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    let provider = cli.provider.unwrap_or_else(|| {
        match config_get("AICTL_PROVIDER").as_deref() {
            Some("openai") => Provider::Openai,
            Some("anthropic") => Provider::Anthropic,
            Some("gemini") => Provider::Gemini,
            Some("grok") => Provider::Grok,
            Some("mistral") => Provider::Mistral,
            Some("deepseek") => Provider::Deepseek,
            Some("kimi") => Provider::Kimi,
            Some("zai") => Provider::Zai,
            Some("ollama") => Provider::Ollama,
            Some("gguf") => Provider::Gguf,
            Some("mlx") => Provider::Mlx,
            Some(other) => {
                eprintln!("Error: invalid AICTL_PROVIDER value '{other}' (expected 'openai', 'anthropic', 'gemini', 'grok', 'mistral', 'deepseek', 'kimi', 'zai', 'ollama', 'gguf', or 'mlx')");
                std::process::exit(1);
            }
            None => {
                eprintln!("Error: no provider specified. Use --provider, set AICTL_PROVIDER in ~/.aictl/config, or run aictl --config");
                std::process::exit(1);
            }
        }
    });

    let model = cli.model.unwrap_or_else(|| {
        config_get("AICTL_MODEL").unwrap_or_else(|| {
            eprintln!("Error: no model specified. Use --model, set AICTL_MODEL in ~/.aictl/config, or run aictl --config");
            std::process::exit(1);
        })
    });

    let api_key = if matches!(provider, Provider::Ollama | Provider::Gguf | Provider::Mlx) {
        String::new()
    } else {
        let key_name = match provider {
            Provider::Openai => "LLM_OPENAI_API_KEY",
            Provider::Anthropic => "LLM_ANTHROPIC_API_KEY",
            Provider::Gemini => "LLM_GEMINI_API_KEY",
            Provider::Grok => "LLM_GROK_API_KEY",
            Provider::Mistral => "LLM_MISTRAL_API_KEY",
            Provider::Deepseek => "LLM_DEEPSEEK_API_KEY",
            Provider::Kimi => "LLM_KIMI_API_KEY",
            Provider::Zai => "LLM_ZAI_API_KEY",
            Provider::Ollama | Provider::Gguf | Provider::Mlx => unreachable!(),
        };
        keys::get_secret(key_name).unwrap_or_else(|| {
            eprintln!("Error: API key not provided. Set {key_name} in ~/.aictl/config (or use /lock-keys to store it in the system keyring), or run aictl --config");
            std::process::exit(1);
        })
    };

    let incognito = cli.incognito
        || match config_get("AICTL_INCOGNITO").as_deref() {
            Some("true") => true,
            Some("false") | None => false,
            Some(other) => {
                eprintln!(
                    "Error: invalid AICTL_INCOGNITO value '{other}' (expected 'true' or 'false')"
                );
                std::process::exit(1);
            }
        };
    session::set_incognito(incognito);

    if let Some(ref name) = cli.agent {
        if let Ok(prompt) = agents::read_agent(name) {
            agents::load_agent(name, &prompt);
        } else {
            eprintln!("Error: agent '{name}' not found");
            std::process::exit(1);
        }
    }

    let result = match cli.message {
        Some(ref msg) => {
            run_agent_single(&provider, &api_key, &model, msg, cli.auto, cli.quiet).await
        }
        None => run_interactive(provider, api_key, model, cli.auto, cli.session.clone()).await,
    };

    if let Err(e) = result {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}
