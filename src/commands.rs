use crossterm::style::{Color, Stylize};

/// Result of handling a slash command.
pub enum CommandResult {
    /// Exit the REPL.
    Exit,
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
                Ok(_) => println!("  {} copied to clipboard", "✓".with(Color::Green)),
                Err(e) => show_error(&format!("Clipboard error: {e}")),
            }
        }
        Err(e) => show_error(&format!("Failed to run pbcopy: {e}")),
    }
}

fn print_help() {
    println!();
    println!(
        "  {}  Copy last response to clipboard",
        "/copy".with(Color::Cyan)
    );
    println!(
        "  {}  Show this help message",
        "/help".with(Color::Cyan)
    );
    println!(
        "  {}  Exit the REPL",
        "/exit".with(Color::Cyan)
    );
    println!();
}
