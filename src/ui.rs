use std::io::Write;
use std::time::Duration;

use crossterm::style::{Attribute, Color, Stylize};
use indicatif::{ProgressBar, ProgressStyle};

use crate::tools::ToolCall;

pub trait AgentUI {
    fn start_spinner(&self, msg: &str);
    fn stop_spinner(&self);
    fn show_reasoning(&self, text: &str);
    fn show_auto_tool(&self, tool_call: &ToolCall);
    fn show_tool_result(&self, result: &str);
    fn confirm_tool(&self, tool_call: &ToolCall) -> bool;
    fn show_answer(&self, text: &str);
    fn show_error(&self, text: &str);
}

// --- PlainUI: no colors, no spinner (single-shot / pipe-friendly) ---

pub struct PlainUI;

impl AgentUI for PlainUI {
    fn start_spinner(&self, _msg: &str) {}
    fn stop_spinner(&self) {}

    fn show_reasoning(&self, text: &str) {
        eprintln!("{text}");
    }

    fn show_auto_tool(&self, tool_call: &ToolCall) {
        eprintln!("[auto] Running: {}", tool_call.input);
    }

    fn show_tool_result(&self, result: &str) {
        eprintln!("{result}");
    }

    fn confirm_tool(&self, tool_call: &ToolCall) -> bool {
        crate::tools::confirm_tool_call(tool_call)
    }

    fn show_answer(&self, text: &str) {
        println!("{text}");
    }

    fn show_error(&self, text: &str) {
        eprintln!("{text}");
    }
}

// --- InteractiveUI: colored output, spinner, markdown rendering ---

pub struct InteractiveUI {
    spinner: ProgressBar,
}

impl InteractiveUI {
    pub fn new() -> Self {
        let spinner = ProgressBar::hidden();
        Self { spinner }
    }

    pub fn render_markdown(text: &str) {
        let skin = termimad::MadSkin::default();
        // Print with a leading blank line for visual separation
        eprintln!();
        skin.print_text(text);
    }

    pub fn print_welcome() {
        eprintln!(
            "{}",
            "aictl interactive mode"
                .with(Color::Cyan)
                .attribute(Attribute::Bold)
        );
        eprintln!(
            "{}",
            "Type your message, or \"exit\" / Ctrl+D to quit.".with(Color::DarkGrey)
        );
        eprintln!();
    }
}

impl AgentUI for InteractiveUI {
    fn start_spinner(&self, msg: &str) {
        self.spinner.set_style(
            ProgressStyle::with_template("{spinner} {msg}")
                .unwrap()
                .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏ "),
        );
        self.spinner
            .set_message(msg.to_string().with(Color::DarkGrey).to_string());
        self.spinner.enable_steady_tick(Duration::from_millis(80));
    }

    fn stop_spinner(&self) {
        self.spinner.finish_and_clear();
    }

    fn show_reasoning(&self, text: &str) {
        eprintln!("{}", text.with(Color::DarkGrey));
    }

    fn show_auto_tool(&self, tool_call: &ToolCall) {
        eprintln!(
            "{} {} {}",
            "[auto]".with(Color::Yellow).attribute(Attribute::Bold),
            tool_call.name.as_str().with(Color::Cyan),
            tool_call.input.as_str().with(Color::DarkGrey),
        );
    }

    fn show_tool_result(&self, result: &str) {
        // Truncate long results in interactive mode
        let display = if result.len() > 2000 {
            format!("{}...(truncated)", &result[..2000])
        } else {
            result.to_string()
        };
        eprintln!("{}", display.with(Color::DarkGrey));
    }

    fn confirm_tool(&self, tool_call: &ToolCall) -> bool {
        eprint!(
            "{} [{}]: {}\n{} ",
            "Tool call".with(Color::Yellow),
            tool_call.name.as_str().with(Color::Cyan),
            tool_call.input.as_str().with(Color::DarkGrey),
            "Allow? [y/N]".with(Color::Yellow),
        );
        std::io::stderr().flush().ok();
        let mut input = String::new();
        if std::io::stdin().read_line(&mut input).is_err() {
            return false;
        }
        matches!(input.trim(), "y" | "Y" | "yes" | "Yes")
    }

    fn show_answer(&self, text: &str) {
        Self::render_markdown(text);
    }

    fn show_error(&self, text: &str) {
        eprintln!("{}", text.with(Color::Red).attribute(Attribute::Bold));
    }
}
