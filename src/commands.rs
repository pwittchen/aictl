use std::io::Write;

use crossterm::style::{Color, Stylize};

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
    "security", "thinking", "tools", "update",
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
    /// Switch auto/human-in-the-loop mode.
    Mode,
    /// Switch thinking mode (smart/fast).
    Thinking,
    /// Update to the latest version.
    Update,
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
        "behavior" => CommandResult::Mode,
        "thinking" => CommandResult::Thinking,
        "update" => CommandResult::Update,
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

pub async fn compact(
    provider: &Provider,
    api_key: &str,
    model: &str,
    messages: &mut Vec<Message>,
    ui: &dyn AgentUI,
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
            ui.show_token_usage(&usage, model, false, 0, std::time::Duration::ZERO, 0);
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
        "context:".with(Color::Cyan),
        "█".repeat(filled).with(bar_color),
        "░".repeat(empty).with(Color::DarkGrey),
    );
    println!(
        "  {} {last_input_tokens} / {limit}",
        "tokens: ".with(Color::DarkGrey),
    );
    println!(
        "  {} {messages_len} / {max_messages}",
        "messages:".with(Color::DarkGrey),
    );
    println!();
}

fn print_help() {
    println!();
    println!(
        "  {}    clear conversation context",
        "/clear".with(Color::Cyan)
    );
    println!(
        "  {}  compact context into a summary",
        "/compact".with(Color::Cyan)
    );
    println!("  {}  show context usage", "/context".with(Color::Cyan));
    println!(
        "  {}     copy last response to clipboard",
        "/copy".with(Color::Cyan)
    );
    println!("  {}     show this help message", "/help".with(Color::Cyan));
    println!("  {}     show setup info", "/info".with(Color::Cyan));
    println!("  {}   show known issues", "/issues".with(Color::Cyan));
    println!(
        "  {} switch auto/human-in-the-loop behavior",
        "/behavior".with(Color::Cyan)
    );
    println!(
        "  {}    switch model and provider",
        "/model".with(Color::Cyan)
    );
    println!("  {} show security policy", "/security".with(Color::Cyan));
    println!(
        "  {} switch thinking mode (smart/fast)",
        "/thinking".with(Color::Cyan)
    );
    println!("  {}    show available tools", "/tools".with(Color::Cyan));
    println!(
        "  {}   update to the latest version",
        "/update".with(Color::Cyan)
    );
    println!("  {}     exit the REPL", "/exit".with(Color::Cyan));
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
            CommandResult::Mode
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
fn build_menu_lines(selected: usize, current_model: &str) -> (Vec<String>, Vec<usize>) {
    let mut lines = Vec::new();
    let mut model_indices = Vec::new();

    for (sel_row, (i, (prov, model, _))) in MODELS.iter().enumerate().enumerate() {
        // Print provider header when provider changes
        if i == 0 || MODELS[i - 1].0 != *prov {
            let label = match *prov {
                "anthropic" => "Anthropic:",
                "openai" => "OpenAI:",
                _ => prov,
            };
            lines.push(format!("  {}", label.with(Color::Cyan)));
        }

        let is_selected = sel_row == selected;
        let is_current = *model == current_model;

        let marker = if is_current { "●" } else { " " };
        let name = if is_selected {
            format!(
                "    {} {}",
                marker.with(Color::Green),
                model
                    .with(Color::White)
                    .attribute(crossterm::style::Attribute::Bold)
            )
        } else {
            format!(
                "    {} {}",
                marker.with(Color::Green),
                model.with(Color::DarkGrey)
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
/// Returns (Provider, `model_name`, `api_key_config_key`) or None if cancelled (Esc).
pub fn select_model(current_model: &str) -> Option<(Provider, String, String)> {
    let initial = MODELS
        .iter()
        .position(|(_, m, _)| *m == current_model)
        .unwrap_or(0);
    let selected = select_from_menu(MODELS.len(), initial, |sel| {
        build_menu_lines(sel, current_model).0
    })?;
    let (prov_str, model, api_key_name) = MODELS[selected];
    let provider = match prov_str {
        "openai" => Provider::Openai,
        "anthropic" => Provider::Anthropic,
        _ => unreachable!(),
    };
    Some((provider, model.to_string(), api_key_name.to_string()))
}

const MODES: &[(&str, &str)] = &[
    (
        "human-in-the-loop",
        "ask confirmation before each tool call",
    ),
    ("auto", "run tools without confirmation"),
];

fn build_mode_menu_lines(selected: usize, current_auto: bool) -> Vec<String> {
    let mut lines = Vec::new();
    for (i, (name, desc)) in MODES.iter().enumerate() {
        let is_selected = i == selected;
        let is_current = (*name == "auto") == current_auto;

        let marker = if is_current { "●" } else { " " };
        let name_styled = if is_selected {
            format!(
                "{} {}",
                marker.with(Color::Green),
                name.with(Color::White)
                    .attribute(crossterm::style::Attribute::Bold)
            )
        } else {
            format!(
                "{} {}",
                marker.with(Color::Green),
                name.with(Color::DarkGrey)
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

/// Interactively select auto/human-in-the-loop mode with arrow keys.
/// Returns `Some(auto_bool)` or `None` if cancelled (Esc).
pub fn select_mode(current_auto: bool) -> Option<bool> {
    let initial = usize::from(current_auto);
    let selected = select_from_menu(MODES.len(), initial, |sel| {
        build_mode_menu_lines(sel, current_auto)
    })?;
    Some(MODES[selected].0 == "auto")
}

const THINKING_MODES: &[(&str, &str)] = &[
    ("smart", "all messages, no optimization"),
    ("fast", "sliding window with recent messages"),
];

fn build_thinking_menu_lines(selected: usize, current: ThinkingMode) -> Vec<String> {
    let mut lines = Vec::new();
    for (i, (name, desc)) in THINKING_MODES.iter().enumerate() {
        let is_selected = i == selected;
        let is_current = (*name == "smart" && current == ThinkingMode::Smart)
            || (*name == "fast" && current == ThinkingMode::Fast);

        let marker = if is_current { "●" } else { " " };
        let name_styled = if is_selected {
            format!(
                "{} {}",
                marker.with(Color::Green),
                name.with(Color::White)
                    .attribute(crossterm::style::Attribute::Bold)
            )
        } else {
            format!(
                "{} {}",
                marker.with(Color::Green),
                name.with(Color::DarkGrey)
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
