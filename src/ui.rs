use std::cell::RefCell;
use std::io::Write;
use std::time::Duration;

use crossterm::style::{Attribute, Color, Stylize};
use indicatif::{ProgressBar, ProgressStyle};

use crate::tools::ToolCall;

const PAD: &str = "  ";

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
    spinner: RefCell<ProgressBar>,
}

impl InteractiveUI {
    pub fn new() -> Self {
        let spinner = RefCell::new(ProgressBar::hidden());
        Self { spinner }
    }

    pub fn print_welcome() {
        let bar = "─".repeat(40);
        eprintln!("{}", bar.as_str().with(Color::DarkGrey));
        eprintln!(
            "{}{}",
            PAD,
            "aictl".with(Color::Cyan).attribute(Attribute::Bold)
        );
        eprintln!(
            "{}{}",
            PAD,
            "Type a message, \"exit\" or Ctrl+D to quit".with(Color::DarkGrey)
        );
        eprintln!("{}", bar.as_str().with(Color::DarkGrey));
        eprintln!();
    }

    fn padded_lines(text: &str, prefix: &str) {
        for line in text.lines() {
            eprintln!("{prefix}{line}");
        }
    }
}

impl AgentUI for InteractiveUI {
    fn start_spinner(&self, msg: &str) {
        let pb = ProgressBar::new_spinner();
        pb.set_style(
            ProgressStyle::with_template("{spinner} {msg}")
                .unwrap()
                .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏ "),
        );
        pb.set_message(format!("{PAD}{}", msg.with(Color::DarkGrey)));
        pb.enable_steady_tick(Duration::from_millis(80));
        *self.spinner.borrow_mut() = pb;
    }

    fn stop_spinner(&self) {
        self.spinner.borrow().finish_and_clear();
    }

    fn show_reasoning(&self, text: &str) {
        let prefix = format!("{PAD}{} ", "│".with(Color::DarkGrey));
        for line in text.lines() {
            eprintln!("{prefix}{}", line.with(Color::DarkGrey));
        }
    }

    fn show_auto_tool(&self, tool_call: &ToolCall) {
        eprintln!(
            "{PAD}{} {} {} {}",
            "│".with(Color::DarkGrey),
            "[auto]".with(Color::Yellow).attribute(Attribute::Bold),
            tool_call.name.as_str().with(Color::Cyan),
            tool_call.input.as_str().with(Color::DarkGrey),
        );
    }

    fn show_tool_result(&self, result: &str) {
        let display = if result.len() > 2000 {
            format!("{}...(truncated)", &result[..2000])
        } else {
            result.to_string()
        };
        let prefix = format!("{PAD}{} ", "│".with(Color::DarkGrey));
        for line in display.lines() {
            eprintln!("{prefix}{}", line.with(Color::DarkGrey));
        }
    }

    fn confirm_tool(&self, tool_call: &ToolCall) -> bool {
        let pipe = "│".with(Color::DarkGrey);
        eprintln!(
            "{PAD}{} {} [{}]: {}",
            pipe,
            "Tool call".with(Color::Yellow),
            tool_call.name.as_str().with(Color::Cyan),
            tool_call.input.as_str().with(Color::DarkGrey),
        );
        eprint!("{PAD}{} {} ", pipe, "Allow? [y/N]".with(Color::Yellow),);
        std::io::stderr().flush().ok();
        let mut input = String::new();
        if std::io::stdin().read_line(&mut input).is_err() {
            return false;
        }
        matches!(input.trim(), "y" | "Y" | "yes" | "Yes")
    }

    fn show_answer(&self, text: &str) {
        let skin = termimad::MadSkin::default();
        let rendered = skin.term_text(text);
        let rendered = format!("{rendered}");
        eprintln!();
        Self::padded_lines(&rendered, PAD);
        eprintln!();
    }

    fn show_error(&self, text: &str) {
        eprintln!("{PAD}{}", text.with(Color::Red).attribute(Attribute::Bold));
    }
}
