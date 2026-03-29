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
        "copy" => {
            copy_to_clipboard(last_answer, show_error);
            CommandResult::Continue
        }
        "help" => {
            print_help();
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
    println!(
        "  {}    Copy last response to clipboard",
        "/copy".with(Color::Cyan)
    );
    println!("  {}    Show this help message", "/help".with(Color::Cyan));
    println!("  {}    Exit the REPL", "/exit".with(Color::Cyan));
    println!();
}
