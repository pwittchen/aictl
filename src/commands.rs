use std::io::Write;
use std::sync::atomic::{AtomicU32, Ordering};

use crossterm::style::{Color, Stylize};

static MANUAL_COMPACTIONS: AtomicU32 = AtomicU32::new(0);
static AUTO_COMPACTIONS: AtomicU32 = AtomicU32::new(0);

pub fn compaction_counts() -> (u32, u32) {
    (
        MANUAL_COMPACTIONS.load(Ordering::Relaxed),
        AUTO_COMPACTIONS.load(Ordering::Relaxed),
    )
}

use crate::llm;
use crate::llm::MODELS;
use crate::ui::AgentUI;
use crate::{Message, Provider, Role};

/// Thinking mode: controls conversation history optimization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkingMode {
    /// All messages, no optimization.
    Smart,
    /// Sliding window with most recent messages and optional compaction.
    Fast,
}

impl std::fmt::Display for ThinkingMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Smart => write!(f, "smart"),
            Self::Fast => write!(f, "fast"),
        }
    }
}

/// All slash command names (without `/`), sorted alphabetically.
/// Used by the REPL tab completer.
pub const COMMANDS: &[&str] = &[
    "behavior", "clear", "compact", "context", "copy", "exit", "help", "info", "issues", "model",
    "security", "session", "thinking", "tools", "update",
];

/// Result of handling a slash command.
pub enum CommandResult {
    /// Exit the REPL.
    Exit,
    /// Clear conversation context and continue.
    Clear,
    /// Compact conversation context via LLM summarization.
    Compact,
    /// Show context usage info.
    Context,
    /// Show setup info (provider, model, version, etc.).
    Info,
    /// Show security policy status.
    Security,
    /// Switch model interactively.
    Model,
    /// Switch auto/human-in-the-loop behavior.
    Behavior,
    /// Switch thinking mode (smart/fast).
    Thinking,
    /// Update to the latest version.
    Update,
    /// Open the session management menu.
    Session,
    /// Fetch and display known issues.
    Issues,
    /// Command handled, continue the loop.
    Continue,
    /// Not a slash command, proceed normally.
    NotACommand,
}

/// Handle slash command input. Returns how the REPL should proceed.
pub fn handle(input: &str, last_answer: &str, show_error: &dyn Fn(&str)) -> CommandResult {
    let Some(cmd) = input.strip_prefix('/') else {
        return CommandResult::NotACommand;
    };

    match cmd {
        "exit" => CommandResult::Exit,
        "clear" => CommandResult::Clear,
        "compact" => CommandResult::Compact,
        "context" => CommandResult::Context,
        "info" => CommandResult::Info,
        "security" => {
            print_security();
            CommandResult::Security
        }
        "model" => CommandResult::Model,
        "behavior" => CommandResult::Behavior,
        "thinking" => CommandResult::Thinking,
        "update" => CommandResult::Update,
        "session" => CommandResult::Session,
        "issues" => CommandResult::Issues,
        "copy" => {
            copy_to_clipboard(last_answer, show_error);
            CommandResult::Continue
        }
        "help" => {
            print_help();
            CommandResult::Continue
        }
        "tools" => {
            print_tools();
            CommandResult::Continue
        }
        _ => {
            show_error("Unknown command. Type /help for available commands.");
            CommandResult::Continue
        }
    }
}

fn copy_to_clipboard(text: &str, show_error: &dyn Fn(&str)) {
    use std::io::Write;
    use std::process::{Command, Stdio};

    if text.is_empty() {
        show_error("Nothing to copy yet.");
        return;
    }

    match Command::new("pbcopy").stdin(Stdio::piped()).spawn() {
        Ok(mut child) => {
            if let Some(ref mut stdin) = child.stdin {
                let _ = stdin.write_all(text.as_bytes());
            }
            match child.wait() {
                Ok(_) => {
                    println!();
                    println!("  {} copied to clipboard", "✓".with(Color::Green));
                    println!();
                }
                Err(e) => show_error(&format!("Clipboard error: {e}")),
            }
        }
        Err(e) => show_error(&format!("Failed to run pbcopy: {e}")),
    }
}

#[allow(clippy::too_many_lines)]
pub async fn compact(
    provider: &Provider,
    api_key: &str,
    model: &str,
    messages: &mut Vec<Message>,
    ui: &dyn AgentUI,
    thinking: &str,
    is_auto: bool,
) {
    if messages.len() <= 1 {
        ui.show_error("Nothing to compact.");
        return;
    }

    ui.start_spinner("compacting context...");

    let mut summary_msgs = messages.clone();
    summary_msgs.push(Message {
        role: Role::User,
        content: "Summarize our conversation so far in a compact form. \
            Include all key facts, decisions, code changes, file paths, \
            and open tasks so we can continue without losing context. \
            Be concise but thorough."
            .to_string(),
    });

    let result = match provider {
        Provider::Openai => {
            crate::with_esc_cancel(crate::llm_openai::call_openai(
                api_key,
                model,
                &summary_msgs,
            ))
            .await
        }
        Provider::Anthropic => {
            crate::with_esc_cancel(crate::llm_anthropic::call_anthropic(
                api_key,
                model,
                &summary_msgs,
            ))
            .await
        }
        Provider::Gemini => {
            crate::with_esc_cancel(crate::llm_gemini::call_gemini(
                api_key,
                model,
                &summary_msgs,
            ))
            .await
        }
        Provider::Grok => {
            crate::with_esc_cancel(crate::llm_grok::call_grok(api_key, model, &summary_msgs)).await
        }
        Provider::Mistral => {
            crate::with_esc_cancel(crate::llm_mistral::call_mistral(
                api_key,
                model,
                &summary_msgs,
            ))
            .await
        }
        Provider::Deepseek => {
            crate::with_esc_cancel(crate::llm_deepseek::call_deepseek(
                api_key,
                model,
                &summary_msgs,
            ))
            .await
        }
        Provider::Zai => {
            crate::with_esc_cancel(crate::llm_zai::call_zai(api_key, model, &summary_msgs)).await
        }
        Provider::Ollama => {
            crate::with_esc_cancel(crate::llm_ollama::call_ollama(model, &summary_msgs)).await
        }
    };

    ui.stop_spinner();

    let result = match result {
        Ok(inner) => inner,
        Err(_interrupted) => {
            println!("\n  {} interrupted\n", "✗".with(Color::Yellow));
            return;
        }
    };

    match result {
        Ok((summary, usage)) => {
            let system = messages[0].clone();
            messages.clear();
            messages.push(system);
            messages.push(Message {
                role: Role::User,
                content: format!("Here is a summary of our conversation so far:\n\n{summary}"),
            });
            messages.push(Message {
                role: Role::Assistant,
                content: "Understood. I have the context from our previous \
                    conversation. How can I help you next?"
                    .to_string(),
            });
            println!();
            ui.show_token_usage(
                &usage,
                model,
                false,
                0,
                std::time::Duration::ZERO,
                0,
                thinking,
            );
            if is_auto {
                AUTO_COMPACTIONS.fetch_add(1, Ordering::Relaxed);
            } else {
                MANUAL_COMPACTIONS.fetch_add(1, Ordering::Relaxed);
            }
            println!("  {} context compacted", "✓".with(Color::Green));
            println!();
        }
        Err(e) => ui.show_error(&format!("Compact failed: {e}")),
    }
}

pub fn print_context(
    model: &str,
    messages_len: usize,
    last_input_tokens: u64,
    max_messages: usize,
) {
    let limit = llm::context_limit(model);
    let token_pct = llm::pct(last_input_tokens, limit);
    let message_pct = llm::pct_usize(messages_len, max_messages);
    let context_pct = token_pct.max(message_pct).min(100);

    let bar_width = 30;
    let filled = (context_pct as usize * bar_width / 100).min(bar_width);
    let empty = bar_width - filled;
    let bar_color = if context_pct >= 80 {
        Color::Red
    } else if context_pct >= 50 {
        Color::Yellow
    } else {
        Color::Green
    };

    println!();
    println!(
        "  {} {}{} {context_pct}%",
        format!("{:<12}", "context:").with(Color::Cyan),
        "█".repeat(filled).with(bar_color),
        "░".repeat(empty).with(Color::DarkGrey),
    );
    println!(
        "  {} {last_input_tokens} / {limit}",
        format!("{:<12}", "tokens:").with(Color::DarkGrey),
    );
    println!(
        "  {} {messages_len} / {max_messages}",
        format!("{:<12}", "messages:").with(Color::DarkGrey),
    );
    let (manual, auto) = compaction_counts();
    println!(
        "  {} manual: {manual}, auto: {auto}",
        format!("{:<12}", "compactions:").with(Color::DarkGrey),
    );
    println!();
}

fn print_help() {
    let entries: &[(&str, &str)] = &[
        ("/clear", "clear conversation context"),
        ("/compact", "compact context into a summary"),
        ("/context", "show context usage"),
        ("/copy", "copy last response to clipboard"),
        ("/help", "show this help message"),
        ("/info", "show setup info"),
        ("/issues", "show known issues"),
        ("/behavior", "switch auto/human-in-the-loop behavior"),
        ("/model", "switch model and provider"),
        ("/security", "show security policy"),
        ("/session", "manage sessions"),
        ("/thinking", "switch thinking mode (smart/fast)"),
        ("/tools", "show available tools"),
        ("/update", "update to the latest version"),
        ("/exit", "exit the REPL"),
    ];
    let max_len = entries.iter().map(|(c, _)| c.len()).max().unwrap_or(0);
    println!();
    for (cmd, desc) in entries {
        let pad = max_len - cmd.len() + 2;
        println!("  {}{:pad$}{desc}", cmd.with(Color::Cyan), "");
    }
    println!();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noop_error(_msg: &str) {}

    #[test]
    fn cmd_exit() {
        assert!(matches!(
            handle("/exit", "", &noop_error),
            CommandResult::Exit
        ));
    }

    #[test]
    fn cmd_clear() {
        assert!(matches!(
            handle("/clear", "", &noop_error),
            CommandResult::Clear
        ));
    }

    #[test]
    fn cmd_compact() {
        assert!(matches!(
            handle("/compact", "", &noop_error),
            CommandResult::Compact
        ));
    }

    #[test]
    fn cmd_context() {
        assert!(matches!(
            handle("/context", "", &noop_error),
            CommandResult::Context
        ));
    }

    #[test]
    fn cmd_info() {
        assert!(matches!(
            handle("/info", "", &noop_error),
            CommandResult::Info
        ));
    }

    #[test]
    fn cmd_issues() {
        assert!(matches!(
            handle("/issues", "", &noop_error),
            CommandResult::Issues
        ));
    }

    #[test]
    fn cmd_model() {
        assert!(matches!(
            handle("/model", "", &noop_error),
            CommandResult::Model
        ));
    }

    #[test]
    fn cmd_behavior() {
        assert!(matches!(
            handle("/behavior", "", &noop_error),
            CommandResult::Behavior
        ));
    }

    #[test]
    fn cmd_thinking() {
        assert!(matches!(
            handle("/thinking", "", &noop_error),
            CommandResult::Thinking
        ));
    }

    #[test]
    fn cmd_unknown() {
        assert!(matches!(
            handle("/foo", "", &noop_error),
            CommandResult::Continue
        ));
    }

    #[test]
    fn cmd_not_a_command() {
        assert!(matches!(
            handle("hello", "", &noop_error),
            CommandResult::NotACommand
        ));
    }

    #[test]
    fn cmd_help_returns_continue() {
        assert!(matches!(
            handle("/help", "", &noop_error),
            CommandResult::Continue
        ));
    }

    #[test]
    fn cmd_tools_returns_continue() {
        assert!(matches!(
            handle("/tools", "", &noop_error),
            CommandResult::Continue
        ));
    }

    #[test]
    fn commands_list_matches_handler() {
        for cmd in COMMANDS {
            let input = format!("/{cmd}");
            assert!(
                !matches!(handle(&input, "", &noop_error), CommandResult::NotACommand),
                "/{cmd} should be recognized as a command"
            );
        }
    }
}

fn print_security() {
    let summary = crate::security::policy_summary();
    let max_key = summary.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
    println!();
    for (key, value) in &summary {
        let pad = max_key - key.len() + 2;
        println!("  {}:{:pad$}{}", key.as_str().with(Color::Cyan), "", value);
    }
    println!();
}

fn print_tools() {
    let tools: &[(&str, &str)] = &[
        ("exec_shell", "execute a shell command via sh -c"),
        ("read_file", "read the contents of a file"),
        ("write_file", "write content to a file"),
        ("remove_file", "remove (delete) a file"),
        ("edit_file", "edit a file with find-and-replace"),
        (
            "create_directory",
            "create a directory and any missing parents",
        ),
        ("list_directory", "list files and directories at a path"),
        ("search_files", "search file contents by pattern"),
        ("find_files", "find files matching a glob pattern"),
        ("search_web", "search the web via Firecrawl API"),
        ("fetch_url", "fetch a URL and return text content"),
        ("extract_website", "extract readable content from a URL"),
        ("fetch_datetime", "get current date, time, and timezone"),
        (
            "fetch_geolocation",
            "get geolocation data for an IP address",
        ),
    ];
    let max_len = tools.iter().map(|(n, _)| n.len()).max().unwrap_or(0);
    println!();
    for (name, desc) in tools {
        let pad = max_len - name.len() + 2;
        println!("  {}{:pad$}{desc}", name.with(Color::Cyan), "");
    }
    println!();
}

/// Build the display lines for the model menu. Each entry is either a
/// header line (provider name) or a model line with its index into MODELS.
/// Returns `(lines, model_indices)` where `model_indices[i]` maps selectable
/// row `i` to its position in MODELS.
/// A combined model entry used for building the menu (static + dynamic Ollama models).
struct MenuModel {
    provider: String,
    model: String,
    api_key_name: String,
}

fn build_combined_models(ollama_models: &[String]) -> Vec<MenuModel> {
    let mut combined: Vec<MenuModel> = MODELS
        .iter()
        .map(|(prov, model, key)| MenuModel {
            provider: (*prov).to_string(),
            model: (*model).to_string(),
            api_key_name: (*key).to_string(),
        })
        .collect();

    for m in ollama_models {
        combined.push(MenuModel {
            provider: "ollama".to_string(),
            model: m.clone(),
            api_key_name: String::new(),
        });
    }

    combined
}

fn build_menu_lines(
    selected: usize,
    current_model: &str,
    models: &[MenuModel],
) -> (Vec<String>, Vec<usize>) {
    let mut lines = Vec::new();
    let mut model_indices = Vec::new();

    for (sel_row, (i, entry)) in models.iter().enumerate().enumerate() {
        // Print provider header when provider changes
        if i == 0 || models[i - 1].provider != entry.provider {
            let label = match entry.provider.as_str() {
                "anthropic" => "Anthropic:",
                "openai" => "OpenAI:",
                "gemini" => "Gemini:",
                "grok" => "Grok:",
                "mistral" => "Mistral:",
                "deepseek" => "DeepSeek:",
                "zai" => "Z.ai:",
                "ollama" => "Ollama:",
                _ => entry.provider.as_str(),
            };
            lines.push(format!("  {}", label.with(Color::Cyan)));
        }

        let is_selected = sel_row == selected;
        let is_current = entry.model == current_model;

        let marker = if is_current { "●" } else { " " };
        let name = if is_selected {
            format!(
                "       {} {}",
                marker.with(Color::Green),
                entry
                    .model
                    .as_str()
                    .with(Color::White)
                    .attribute(crossterm::style::Attribute::Bold)
            )
        } else {
            format!(
                "       {} {}",
                marker.with(Color::Green),
                entry.model.as_str().with(Color::DarkGrey)
            )
        };

        let line = if is_selected {
            format!("  {} {name}", "›".with(Color::Cyan))
        } else {
            format!("    {name}")
        };

        lines.push(line);
        model_indices.push(i);
    }

    (lines, model_indices)
}

/// Generic arrow-key menu selector.
/// `item_count` is the number of selectable items, `initial_selected` is the
/// starting index, and `build_lines` returns the display lines for a given
/// selected index.  Returns `Some(selected_index)` or `None` if cancelled.
#[allow(clippy::cast_possible_truncation)]
fn select_from_menu<F>(item_count: usize, initial_selected: usize, build_lines: F) -> Option<usize>
where
    F: Fn(usize) -> Vec<String>,
{
    use crossterm::{
        cursor,
        event::{self, Event, KeyCode, KeyEventKind},
        execute,
        terminal::{self, ClearType},
    };

    let mut selected = initial_selected;

    let _ = terminal::enable_raw_mode();
    let mut stdout = std::io::stdout();
    let _ = execute!(stdout, cursor::Hide);

    let lines = build_lines(selected);
    let _ = execute!(stdout, cursor::MoveToColumn(0));
    let _ = write!(stdout, "\r\n");
    for line in &lines {
        let _ = write!(stdout, "{line}\r\n");
    }
    let _ = write!(
        stdout,
        "\r\n  {}\r\n",
        "↑/↓ navigate · enter select · esc cancel".with(Color::DarkGrey)
    );
    let _ = stdout.flush();
    let total_rendered_lines = lines.len() + 2;

    loop {
        if !event::poll(std::time::Duration::from_millis(100)).unwrap_or(false) {
            continue;
        }
        let Ok(ev) = event::read() else { break };

        match ev {
            Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                KeyCode::Up => {
                    selected = selected.saturating_sub(1);
                }
                KeyCode::Down => {
                    if selected + 1 < item_count {
                        selected += 1;
                    }
                }
                KeyCode::Enter => {
                    let _ = execute!(
                        stdout,
                        cursor::MoveUp(total_rendered_lines as u16),
                        terminal::Clear(ClearType::FromCursorDown),
                        cursor::Show,
                    );
                    let _ = terminal::disable_raw_mode();
                    return Some(selected);
                }
                KeyCode::Esc => {
                    let _ = execute!(
                        stdout,
                        cursor::MoveUp(total_rendered_lines as u16),
                        terminal::Clear(ClearType::FromCursorDown),
                        cursor::Show,
                    );
                    let _ = terminal::disable_raw_mode();
                    return None;
                }
                _ => continue,
            },
            _ => continue,
        }

        let lines = build_lines(selected);
        let _ = execute!(
            stdout,
            cursor::MoveUp(total_rendered_lines as u16),
            terminal::Clear(ClearType::FromCursorDown),
        );
        for line in &lines {
            let _ = write!(stdout, "{line}\r\n");
        }
        let _ = write!(
            stdout,
            "\r\n  {}\r\n",
            "↑/↓ navigate · enter select · esc cancel".with(Color::DarkGrey)
        );
        let _ = stdout.flush();
    }

    let _ = execute!(stdout, cursor::Show);
    let _ = terminal::disable_raw_mode();
    None
}

/// Interactively select a model with arrow keys.
/// `ollama_models` are dynamically fetched model names (empty if Ollama is not running).
/// Returns (Provider, `model_name`, `api_key_config_key`) or None if cancelled (Esc).
pub fn select_model(
    current_model: &str,
    ollama_models: &[String],
) -> Option<(Provider, String, String)> {
    let combined = build_combined_models(ollama_models);
    let initial = combined
        .iter()
        .position(|m| m.model == current_model)
        .unwrap_or(0);
    let selected = select_from_menu(combined.len(), initial, |sel| {
        build_menu_lines(sel, current_model, &combined).0
    })?;
    let entry = &combined[selected];
    let provider = match entry.provider.as_str() {
        "openai" => Provider::Openai,
        "anthropic" => Provider::Anthropic,
        "gemini" => Provider::Gemini,
        "grok" => Provider::Grok,
        "mistral" => Provider::Mistral,
        "deepseek" => Provider::Deepseek,
        "zai" => Provider::Zai,
        "ollama" => Provider::Ollama,
        _ => unreachable!(),
    };
    Some((provider, entry.model.clone(), entry.api_key_name.clone()))
}

const BEHAVIORS: &[(&str, &str)] = &[
    (
        "human-in-the-loop",
        "ask confirmation before each tool call",
    ),
    ("auto", "run tools without confirmation"),
];

fn build_behavior_menu_lines(selected: usize, current_auto: bool) -> Vec<String> {
    let mut lines = Vec::new();
    let max_name = BEHAVIORS.iter().map(|(n, _)| n.len()).max().unwrap_or(0);
    for (i, (name, desc)) in BEHAVIORS.iter().enumerate() {
        let is_selected = i == selected;
        let is_current = (*name == "auto") == current_auto;

        let marker = if is_current { "●" } else { " " };
        let padded = format!("{:<max_name$}", *name);
        let name_styled = if is_selected {
            format!(
                "{} {}",
                marker.with(Color::Green),
                padded
                    .with(Color::White)
                    .attribute(crossterm::style::Attribute::Bold)
            )
        } else {
            format!(
                "{} {}",
                marker.with(Color::Green),
                padded.with(Color::DarkGrey)
            )
        };

        let desc_styled = format!("{}", desc.with(Color::DarkGrey));

        let line = if is_selected {
            format!("  {} {name_styled}  {desc_styled}", "›".with(Color::Cyan))
        } else {
            format!("    {name_styled}  {desc_styled}")
        };

        lines.push(line);
    }
    lines
}

/// Interactively select auto/human-in-the-loop behavior with arrow keys.
/// Returns `Some(auto_bool)` or `None` if cancelled (Esc).
pub fn select_behavior(current_auto: bool) -> Option<bool> {
    let initial = usize::from(current_auto);
    let selected = select_from_menu(BEHAVIORS.len(), initial, |sel| {
        build_behavior_menu_lines(sel, current_auto)
    })?;
    Some(BEHAVIORS[selected].0 == "auto")
}

const THINKING_MODES: &[(&str, &str)] = &[
    ("smart", "all messages, no optimization"),
    ("fast", "sliding window with recent messages"),
];

fn build_thinking_menu_lines(selected: usize, current: ThinkingMode) -> Vec<String> {
    let mut lines = Vec::new();
    let max_name = THINKING_MODES
        .iter()
        .map(|(n, _)| n.len())
        .max()
        .unwrap_or(0);
    for (i, (name, desc)) in THINKING_MODES.iter().enumerate() {
        let is_selected = i == selected;
        let is_current = (*name == "smart" && current == ThinkingMode::Smart)
            || (*name == "fast" && current == ThinkingMode::Fast);

        let marker = if is_current { "●" } else { " " };
        let padded = format!("{:<max_name$}", *name);
        let name_styled = if is_selected {
            format!(
                "{} {}",
                marker.with(Color::Green),
                padded
                    .with(Color::White)
                    .attribute(crossterm::style::Attribute::Bold)
            )
        } else {
            format!(
                "{} {}",
                marker.with(Color::Green),
                padded.with(Color::DarkGrey)
            )
        };

        let desc_styled = format!("{}", desc.with(Color::DarkGrey));

        let line = if is_selected {
            format!("  {} {name_styled}  {desc_styled}", "›".with(Color::Cyan))
        } else {
            format!("    {name_styled}  {desc_styled}")
        };

        lines.push(line);
    }
    lines
}

/// Interactively select thinking mode with arrow keys.
/// Returns `Some(ThinkingMode)` or `None` if cancelled (Esc).
pub fn select_thinking(current: ThinkingMode) -> Option<ThinkingMode> {
    let initial = match current {
        ThinkingMode::Smart => 0,
        ThinkingMode::Fast => 1,
    };
    let selected = select_from_menu(THINKING_MODES.len(), initial, |sel| {
        build_thinking_menu_lines(sel, current)
    })?;
    Some(match THINKING_MODES[selected].0 {
        "fast" => ThinkingMode::Fast,
        _ => ThinkingMode::Smart,
    })
}

pub fn print_info(
    provider: &str,
    model: &str,
    auto: bool,
    thinking: ThinkingMode,
    version_info: &str,
) {
    let version = crate::VERSION;
    let behavior = if auto { "auto" } else { "human-in-the-loop" };
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    let binary_size = std::env::current_exe()
        .ok()
        .and_then(|p| std::fs::metadata(p).ok())
        .map_or_else(
            || "unknown".to_string(),
            #[allow(clippy::cast_precision_loss)]
            |m| {
                let bytes = m.len();
                if bytes >= 1_048_576 {
                    format!("{:.1} MB", bytes as f64 / 1_048_576.0)
                } else {
                    format!("{:.1} KB", bytes as f64 / 1_024.0)
                }
            },
        );

    let version_display = if version_info.is_empty() {
        version.to_string()
    } else {
        let version_color = if version_info.contains("latest") {
            Color::Green
        } else {
            Color::Yellow
        };
        format!("{version} {}", version_info.with(version_color))
    };

    println!();
    println!("  {} {version_display}", "version: ".with(Color::Cyan));
    println!("  {} {provider}", "provider:".with(Color::Cyan));
    println!("  {} {model}", "model:   ".with(Color::Cyan));
    println!("  {} {behavior}", "behavior:".with(Color::Cyan));
    println!("  {} {thinking}", "thinking:".with(Color::Cyan));
    println!("  {} {os}/{arch}", "os:      ".with(Color::Cyan));
    println!("  {} {binary_size}", "binary:  ".with(Color::Cyan));
    println!();
}

const ISSUES_URL: &str =
    "https://raw.githubusercontent.com/pwittchen/aictl/refs/heads/master/ISSUES.md";

/// Fetch and display known issues from the remote ISSUES.md.
pub async fn run_issues(show_error: &dyn Fn(&str)) {
    println!();
    println!("  {} fetching issues...", "↓".with(Color::Cyan));

    let client = crate::config::http_client();
    let result = client
        .get(ISSUES_URL)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .ok();

    let Some(response) = result else {
        show_error("Could not fetch ISSUES.md. Please try again later.");
        return;
    };

    let Ok(body) = response.text().await else {
        show_error("Could not read ISSUES.md response body.");
        return;
    };

    let skin = termimad::MadSkin::default();
    let width = crossterm::terminal::size()
        .map_or(80, |(w, _)| w as usize)
        .min(100);
    let rendered = format!(
        "{}",
        termimad::FmtText::from_text(&skin, body.as_str().into(), Some(width))
    );
    println!();
    for line in rendered.lines() {
        println!("  {line}");
    }
    println!();
}

const UPDATE_CMD: &str =
    "curl -sSf https://raw.githubusercontent.com/pwittchen/aictl/master/install.sh | sh";

/// Run the update process interactively (REPL `/update`).
/// Returns `true` if the binary was updated and the REPL should exit.
pub async fn run_update(show_error: &dyn Fn(&str)) -> bool {
    println!();
    println!("  {} checking for updates...", "↓".with(Color::Cyan),);

    let remote = crate::fetch_remote_version().await;
    match &remote {
        Some(v) if v == crate::VERSION => {
            println!(
                "  {} already on latest version ({})",
                "✓".with(Color::Green),
                crate::VERSION,
            );
            println!();
            return false;
        }
        Some(v) => {
            println!(
                "  {} updating {} → {v}...",
                "↓".with(Color::Cyan),
                crate::VERSION,
            );
            println!();
        }
        None => {
            show_error("Could not check remote version. Please try again later.");
            return false;
        }
    }

    let status = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(UPDATE_CMD)
        .status()
        .await;

    match status {
        Ok(s) if s.success() => {
            println!();
            println!(
                "  {} updated successfully. Please restart aictl.",
                "✓".with(Color::Green),
            );
            println!();
            true
        }
        Ok(s) => {
            show_error(&format!(
                "Update failed with exit code: {}",
                s.code().unwrap_or(-1)
            ));
            false
        }
        Err(e) => {
            show_error(&format!("Failed to run update: {e}"));
            false
        }
    }
}

// --- Session management ---

const SESSION_ITEMS: &[(&str, &str)] = &[
    ("current session info", "show id, name, messages, size"),
    ("set session name", "assign a readable name"),
    ("view saved sessions", "load or delete saved sessions"),
    ("clear all sessions", "remove all saved sessions"),
];

fn build_session_menu_lines(selected: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let max_name = SESSION_ITEMS.iter().map(|(n, _)| n.len()).max().unwrap_or(0);
    for (i, (name, desc)) in SESSION_ITEMS.iter().enumerate() {
        let is_selected = i == selected;
        let padded = format!("{:<max_name$}", *name);
        let name_styled = if is_selected {
            format!(
                "  {}",
                padded
                    .with(Color::White)
                    .attribute(crossterm::style::Attribute::Bold)
            )
        } else {
            format!("  {}", padded.with(Color::DarkGrey))
        };
        let desc_styled = format!("{}", desc.with(Color::DarkGrey));
        let line = if is_selected {
            format!("  {} {name_styled}  {desc_styled}", "›".with(Color::Cyan))
        } else {
            format!("    {name_styled}  {desc_styled}")
        };
        lines.push(line);
    }
    lines
}

fn format_size(bytes: u64) -> String {
    #[allow(clippy::cast_precision_loss)]
    if bytes >= 1_048_576 {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

/// Prompt for a y/N confirmation. Returns true if user pressed y.
fn confirm_yn(prompt: &str) -> bool {
    use crossterm::event::{self, Event, KeyCode, KeyEventKind};
    use crossterm::terminal;
    print!("  {} {} ", prompt.with(Color::Yellow), "(y/N):".with(Color::DarkGrey));
    let _ = std::io::stdout().flush();
    let _ = terminal::enable_raw_mode();
    let mut answer = false;
    loop {
        if !event::poll(std::time::Duration::from_millis(200)).unwrap_or(false) {
            continue;
        }
        if let Ok(Event::Key(key)) = event::read()
            && key.kind == KeyEventKind::Press
        {
            match key.code {
                KeyCode::Char('y' | 'Y') => {
                    answer = true;
                    break;
                }
                KeyCode::Char('n' | 'N') | KeyCode::Esc | KeyCode::Enter => break,
                _ => {}
            }
        }
    }
    let _ = terminal::disable_raw_mode();
    println!();
    answer
}

fn show_current_session_info(messages_len: usize) {
    let Some((id, name)) = crate::session::current_info() else {
        println!();
        println!("  {} no active session", "✗".with(Color::Red));
        println!();
        return;
    };
    let size = crate::session::current_file_size();
    println!();
    println!("  {} {id}", "id:      ".with(Color::Cyan));
    println!(
        "  {} {}",
        "name:    ".with(Color::Cyan),
        name.as_deref().unwrap_or("(unset)")
    );
    println!("  {} {messages_len}", "messages:".with(Color::Cyan));
    println!("  {} {}", "size:    ".with(Color::Cyan), format_size(size));
    println!();
}

fn set_session_name_interactive(show_error: &dyn Fn(&str)) {
    let Some((id, _)) = crate::session::current_info() else {
        show_error("no active session");
        return;
    };
    print!("  {} ", "enter session name:".with(Color::Cyan));
    let _ = std::io::stdout().flush();
    let mut buf = String::new();
    if std::io::stdin().read_line(&mut buf).is_err() {
        return;
    }
    let name = buf.trim();
    if name.is_empty() {
        println!();
        return;
    }
    match crate::session::set_name(&id, name) {
        Ok(()) => {
            let stored = crate::session::current_info()
                .and_then(|(_, n)| n)
                .unwrap_or_else(|| name.to_string());
            println!();
            println!(
                "  {} session name set to \"{stored}\"",
                "✓".with(Color::Green)
            );
            println!();
        }
        Err(e) => show_error(&format!("Error: {e}")),
    }
}

fn format_mtime(mtime: std::time::SystemTime) -> String {
    let secs = mtime
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let diff = now.saturating_sub(secs);
    if diff < 60 {
        format!("{diff}s ago")
    } else if diff < 3600 {
        format!("{}m ago", diff / 60)
    } else if diff < 86400 {
        format!("{}h ago", diff / 3600)
    } else {
        format!("{}d ago", diff / 86400)
    }
}

fn build_saved_sessions_lines(
    selected: usize,
    entries: &[crate::session::SessionEntry],
    current_id: Option<&str>,
) -> Vec<String> {
    if entries.is_empty() {
        return vec![format!("  {}", "(no saved sessions)".with(Color::DarkGrey))];
    }
    let mut lines = Vec::new();
    for (i, e) in entries.iter().enumerate() {
        let is_selected = i == selected;
        let is_current = current_id == Some(e.id.as_str());
        let marker = if is_current { "●" } else { " " };
        let name_part = e
            .name
            .as_deref()
            .map(|n| format!(" [{n}]"))
            .unwrap_or_default();
        let meta = format!(" {} · {}", format_size(e.size), format_mtime(e.mtime));
        let body = format!("{}{}{}", e.id, name_part, meta);
        let styled = if is_selected {
            format!(
                "{} {}",
                marker.with(Color::Green),
                body.with(Color::White)
                    .attribute(crossterm::style::Attribute::Bold)
            )
        } else {
            format!("{} {}", marker.with(Color::Green), body.with(Color::DarkGrey))
        };
        let line = if is_selected {
            format!("  {} {styled}", "›".with(Color::Cyan))
        } else {
            format!("    {styled}")
        };
        lines.push(line);
    }
    lines
}

enum SavedAction {
    Load(usize),
    Delete(usize),
    Cancel,
}

#[allow(clippy::cast_possible_truncation)]
fn select_saved_session(entries: &[crate::session::SessionEntry]) -> SavedAction {
    use crossterm::{
        cursor,
        event::{self, Event, KeyCode, KeyEventKind},
        execute,
        terminal::{self, ClearType},
    };

    let current_id = crate::session::current_id();
    let mut selected: usize = 0;
    let _ = terminal::enable_raw_mode();
    let mut stdout = std::io::stdout();
    let _ = execute!(stdout, cursor::Hide);

    let mut lines = build_saved_sessions_lines(selected, entries, current_id.as_deref());
    let _ = write!(stdout, "\r\n");
    for line in &lines {
        let _ = write!(stdout, "{line}\r\n");
    }
    let hint = "↑/↓ navigate · l/enter load · d delete · esc cancel";
    let _ = write!(stdout, "\r\n  {}\r\n", hint.with(Color::DarkGrey));
    let _ = stdout.flush();
    let mut rendered = lines.len() + 2;

    let result = loop {
        if !event::poll(std::time::Duration::from_millis(100)).unwrap_or(false) {
            continue;
        }
        let Ok(ev) = event::read() else {
            break SavedAction::Cancel;
        };
        if let Event::Key(key) = ev
            && key.kind == KeyEventKind::Press
        {
            match key.code {
                KeyCode::Up => selected = selected.saturating_sub(1),
                KeyCode::Down => {
                    if !entries.is_empty() && selected + 1 < entries.len() {
                        selected += 1;
                    }
                }
                KeyCode::Enter | KeyCode::Char('l' | 'L') => {
                    if !entries.is_empty() {
                        break SavedAction::Load(selected);
                    }
                }
                KeyCode::Char('d' | 'D') => {
                    if !entries.is_empty() {
                        break SavedAction::Delete(selected);
                    }
                }
                KeyCode::Esc => break SavedAction::Cancel,
                _ => {}
            }
        } else {
            continue;
        }

        let _ = execute!(
            stdout,
            cursor::MoveUp(rendered as u16),
            terminal::Clear(ClearType::FromCursorDown),
        );
        lines = build_saved_sessions_lines(selected, entries, current_id.as_deref());
        for line in &lines {
            let _ = write!(stdout, "{line}\r\n");
        }
        let _ = write!(stdout, "\r\n  {}\r\n", hint.with(Color::DarkGrey));
        let _ = stdout.flush();
        rendered = lines.len() + 2;
    };

    let _ = execute!(
        stdout,
        cursor::MoveUp(rendered as u16),
        terminal::Clear(ClearType::FromCursorDown),
        cursor::Show,
    );
    let _ = terminal::disable_raw_mode();
    result
}

fn view_saved_sessions(messages: &mut Vec<Message>, show_error: &dyn Fn(&str)) -> bool {
    loop {
        let entries = crate::session::list_sessions();
        match select_saved_session(&entries) {
            SavedAction::Cancel => return false,
            SavedAction::Load(i) => {
                let entry = &entries[i];
                let label = entry.name.as_deref().map_or_else(
                    || entry.id.clone(),
                    |n| format!("{} ({n})", entry.id),
                );
                if !confirm_yn(&format!("load session {label}?")) {
                    continue;
                }
                match crate::session::load_messages(&entry.id) {
                    Ok(loaded) => {
                        *messages = loaded;
                        crate::session::set_current(entry.id.clone(), entry.name.clone());
                        println!();
                        println!("  {} session loaded: {label}", "✓".with(Color::Green));
                        println!();
                        return true;
                    }
                    Err(e) => {
                        show_error(&format!("Failed to load session: {e}"));
                        return false;
                    }
                }
            }
            SavedAction::Delete(i) => {
                let entry = &entries[i];
                let label = entry.name.as_deref().map_or_else(
                    || entry.id.clone(),
                    |n| format!("{} ({n})", entry.id),
                );
                if !confirm_yn(&format!("delete session {label}?")) {
                    continue;
                }
                crate::session::delete_session(&entry.id);
                println!();
                println!("  {} session deleted", "✓".with(Color::Green));
                println!();
            }
        }
    }
}

fn clear_all_sessions_confirm() {
    if !confirm_yn("clear ALL saved sessions?") {
        return;
    }
    crate::session::clear_all();
    // Re-save current session so it persists after clear.
    println!();
    println!("  {} all sessions cleared", "✓".with(Color::Green));
    println!();
}

/// Run the /session menu. Returns true if the conversation messages were replaced
/// (caller should reset context-tracking state).
pub fn run_session_menu(messages: &mut Vec<Message>, show_error: &dyn Fn(&str)) -> bool {
    let Some(sel) = select_from_menu(SESSION_ITEMS.len(), 0, build_session_menu_lines) else {
        return false;
    };
    match sel {
        0 => {
            show_current_session_info(messages.len());
            false
        }
        1 => {
            set_session_name_interactive(show_error);
            false
        }
        2 => view_saved_sessions(messages, show_error),
        3 => {
            clear_all_sessions_confirm();
            false
        }
        _ => false,
    }
}

/// Print saved sessions in non-interactive mode.
pub fn print_sessions_cli() {
    let entries = crate::session::list_sessions();
    if entries.is_empty() {
        println!("(no saved sessions)");
        return;
    }
    for e in &entries {
        let name = e.name.as_deref().unwrap_or("-");
        println!(
            "{}  {}  {}  {}",
            e.id,
            name,
            format_size(e.size),
            format_mtime(e.mtime)
        );
    }
}

/// Run the update process from the CLI (`--update` flag).
pub async fn run_update_cli() {
    eprintln!("Checking for updates...");

    let remote = crate::fetch_remote_version().await;
    match &remote {
        Some(v) if v == crate::VERSION => {
            println!("Already on latest version ({}).", crate::VERSION);
            return;
        }
        Some(v) => {
            eprintln!("Updating {} → {v}...", crate::VERSION);
        }
        None => {
            eprintln!("Error: could not check remote version. Please try again later.");
            std::process::exit(1);
        }
    }

    let status = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(UPDATE_CMD)
        .status()
        .await;

    match status {
        Ok(s) if s.success() => {
            println!("Updated successfully.");
        }
        Ok(s) => {
            eprintln!("Update failed with exit code: {}", s.code().unwrap_or(-1));
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("Failed to run update: {e}");
            std::process::exit(1);
        }
    }
}
