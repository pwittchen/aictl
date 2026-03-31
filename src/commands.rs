use std::io::Write;

use crossterm::style::{Color, Stylize};

use crate::llm;
use crate::llm::MODELS;
use crate::ui::AgentUI;
use crate::{Message, Provider, Role};

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
    /// Switch model interactively.
    Model,
    /// Switch auto/human-in-the-loop mode.
    Mode,
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
        "model" => CommandResult::Model,
        "mode" => CommandResult::Mode,
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
    if text.is_empty() {
        show_error("Nothing to copy yet.");
        return;
    }

    use std::io::Write;
    use std::process::{Command, Stdio};

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
            crate::with_esc_cancel(crate::llm_openai::call_openai(api_key, model, &summary_msgs)).await
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
    let token_pct = if last_input_tokens > 0 {
        (last_input_tokens as f64 / limit as f64 * 100.0) as u8
    } else {
        0
    };
    let message_pct = (messages_len as f64 / max_messages as f64 * 100.0) as u8;
    let context_pct = token_pct.max(message_pct).min(100);

    println!();
    println!("  {} {context_pct}%", "context:".with(Color::Cyan),);
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
        "  {}   Clear conversation context",
        "/clear".with(Color::Cyan)
    );
    println!(
        "  {} Compact context into a summary",
        "/compact".with(Color::Cyan)
    );
    println!("  {} Show context usage", "/context".with(Color::Cyan));
    println!(
        "  {}    Copy last response to clipboard",
        "/copy".with(Color::Cyan)
    );
    println!("  {}    Show this help message", "/help".with(Color::Cyan));
    println!("  {}    Show setup info", "/info".with(Color::Cyan));
    println!(
        "  {}    Switch auto/human-in-the-loop mode",
        "/mode".with(Color::Cyan)
    );
    println!(
        "  {}   Switch model and provider",
        "/model".with(Color::Cyan)
    );
    println!("  {}   Show available tools", "/tools".with(Color::Cyan));
    println!("  {}    Exit the REPL", "/exit".with(Color::Cyan));
    println!();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noop_error(_msg: &str) {}

    #[test]
    fn cmd_exit() {
        assert!(matches!(handle("/exit", "", &noop_error), CommandResult::Exit));
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
    fn cmd_model() {
        assert!(matches!(
            handle("/model", "", &noop_error),
            CommandResult::Model
        ));
    }

    #[test]
    fn cmd_mode() {
        assert!(matches!(
            handle("/mode", "", &noop_error),
            CommandResult::Mode
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
}

fn print_tools() {
    println!();
    println!(
        "  {}      Execute a shell command via sh -c",
        "exec_shell".with(Color::Cyan)
    );
    println!(
        "  {}      Read the contents of a file",
        "read_file".with(Color::Cyan)
    );
    println!(
        "  {}     Write content to a file",
        "write_file".with(Color::Cyan)
    );
    println!(
        "  {}      Edit a file with find-and-replace",
        "edit_file".with(Color::Cyan)
    );
    println!(
        "  {} List files and directories at a path",
        "list_directory".with(Color::Cyan)
    );
    println!(
        "  {}   Search file contents by pattern",
        "search_files".with(Color::Cyan)
    );
    println!(
        "  {}     Find files matching a glob pattern",
        "find_files".with(Color::Cyan)
    );
    println!(
        "  {}     Search the web via Firecrawl API",
        "search_web".with(Color::Cyan)
    );
    println!(
        "  {}      Fetch a URL and return text content",
        "fetch_url".with(Color::Cyan)
    );
    println!(
        "  {} Extract readable content from a URL",
        "extract_website".with(Color::Cyan)
    );
    println!(
        "  {} Get current date, time, and timezone",
        "fetch_datetime".with(Color::Cyan)
    );
    println!(
        "  {}      Get geolocation data for an IP address",
        "fetch_geolocation".with(Color::Cyan)
    );
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

/// Interactively select a model with arrow keys.
/// Returns (Provider, model_name, api_key_config_key) or None if cancelled (Esc).
pub fn select_model(current_model: &str) -> Option<(Provider, String, String)> {
    use crossterm::{
        cursor,
        event::{self, Event, KeyCode, KeyEventKind},
        execute,
        terminal::{self, ClearType},
    };

    // Find the index of the current model in the selectable rows
    let mut selected: usize = MODELS
        .iter()
        .position(|(_, m, _)| *m == current_model)
        .unwrap_or(0);

    let total = MODELS.len();

    // Enter raw mode for key capture, hide cursor
    let _ = terminal::enable_raw_mode();
    let mut stdout = std::io::stdout();
    let _ = execute!(stdout, cursor::Hide);

    // Draw initial menu (leading \r\n moves past the prompt line)
    let (lines, _) = build_menu_lines(selected, current_model);
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
    // menu lines + blank line before hint + hint line
    let total_rendered_lines = lines.len() + 2;

    loop {
        if !event::poll(std::time::Duration::from_millis(100)).unwrap_or(false) {
            continue;
        }
        let ev = match event::read() {
            Ok(ev) => ev,
            Err(_) => break,
        };

        match ev {
            Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                KeyCode::Up => {
                    selected = selected.saturating_sub(1);
                }
                KeyCode::Down => {
                    if selected + 1 < total {
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
                    let (prov_str, model, api_key_name) = MODELS[selected];
                    let provider = match prov_str {
                        "openai" => Provider::Openai,
                        "anthropic" => Provider::Anthropic,
                        _ => unreachable!(),
                    };
                    return Some((provider, model.to_string(), api_key_name.to_string()));
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

        // Redraw menu in place (no leading \r\n — cursor is already at the first menu row)
        let (lines, _) = build_menu_lines(selected, current_model);
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

const MODES: &[(&str, &str)] = &[
    ("human-in-the-loop", "Ask confirmation before each tool call"),
    ("auto", "Run tools without confirmation"),
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
    use crossterm::{
        cursor,
        event::{self, Event, KeyCode, KeyEventKind},
        execute,
        terminal::{self, ClearType},
    };

    let mut selected: usize = if current_auto { 1 } else { 0 };
    let total = MODES.len();

    let _ = terminal::enable_raw_mode();
    let mut stdout = std::io::stdout();
    let _ = execute!(stdout, cursor::Hide);

    let lines = build_mode_menu_lines(selected, current_auto);
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
        let ev = match event::read() {
            Ok(ev) => ev,
            Err(_) => break,
        };

        match ev {
            Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                KeyCode::Up => {
                    selected = selected.saturating_sub(1);
                }
                KeyCode::Down => {
                    if selected + 1 < total {
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
                    return Some(MODES[selected].0 == "auto");
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

        let lines = build_mode_menu_lines(selected, current_auto);
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

pub fn print_info(provider: &str, model: &str, auto: bool) {
    let version = crate::VERSION;
    let mode = if auto { "auto" } else { "human-in-the-loop" };
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    let binary_size = std::env::current_exe()
        .ok()
        .and_then(|p| std::fs::metadata(p).ok())
        .map(|m| {
            let bytes = m.len();
            if bytes >= 1_048_576 {
                format!("{:.1} MB", bytes as f64 / 1_048_576.0)
            } else {
                format!("{:.1} KB", bytes as f64 / 1_024.0)
            }
        })
        .unwrap_or_else(|| "unknown".to_string());

    println!();
    println!("  {} {version}", "version: ".with(Color::Cyan));
    println!("  {} {provider}", "provider:".with(Color::Cyan));
    println!("  {} {model}", "model:   ".with(Color::Cyan));
    println!("  {} {mode}", "mode:    ".with(Color::Cyan));
    println!("  {} {os}/{arch}", "os:      ".with(Color::Cyan));
    println!("  {} {binary_size}", "binary:  ".with(Color::Cyan));
    println!();
}
