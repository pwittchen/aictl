use crossterm::style::{Color, Stylize};

use crate::llm;
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
            crate::with_esc_cancel(llm::openai::call_openai(api_key, model, &summary_msgs)).await
        }
        Provider::Anthropic => {
            crate::with_esc_cancel(llm::anthropic::call_anthropic(
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
    println!(
        "  {}   Show available tools",
        "/tools".with(Color::Cyan)
    );
    println!("  {}    Exit the REPL", "/exit".with(Color::Cyan));
    println!();
}

fn print_tools() {
    println!();
    println!(
        "  {}      Execute a shell command via sh -c",
        "run_shell".with(Color::Cyan)
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
        "  {} Get current date, time, and timezone",
        "fetch_datetime".with(Color::Cyan)
    );
    println!(
        "  {}      Get geolocation data for an IP address",
        "geolocate".with(Color::Cyan)
    );
    println!();
}
