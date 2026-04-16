use crossterm::style::{Color, Stylize};

pub(super) fn copy_to_clipboard(text: &str, show_error: &dyn Fn(&str)) {
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
