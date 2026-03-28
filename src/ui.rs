use std::cell::{Cell, RefCell};
use std::io::Write;
use std::time::Duration;

use crossterm::style::{Attribute, Color, Stylize};
use indicatif::{ProgressBar, ProgressStyle};

use crate::llm::TokenUsage;
use crate::tools::ToolCall;

const PAD: &str = "  ";
const PIPE: &str = "│";
const WELCOME_TEXT: &str = "Type a message, \"exit\" or Ctrl+D to quit";
const MAX_RESULT_LINES: usize = 15;
const FALLBACK_WIDTH: u16 = 120;

// ── Helpers ──────────────────────────────────────────────────────────

fn term_width() -> usize {
    crossterm::terminal::size()
        .map(|(w, _)| w)
        .unwrap_or(FALLBACK_WIDTH) as usize
}

fn max_content_width() -> usize {
    // "  │ " = 5 visible prefix chars
    term_width().saturating_sub(5)
}

/// Total visual width of the banner rule (starts at column 0).
fn banner_width() -> usize {
    WELCOME_TEXT.len() + 4
}

/// Number of ─ chars in tool box rules.
/// Tool rule is: PAD(2) + ╭(1) + dashes, must end at same column as banner.
fn tool_rule_width() -> usize {
    banner_width().saturating_sub(3)
}

fn truncate_line(line: &str, max: usize) -> String {
    if max < 2 {
        return String::new();
    }
    if line.chars().count() <= max {
        return line.to_string();
    }
    let truncated: String = line.chars().take(max - 1).collect();
    format!("{truncated}…")
}

fn first_input_line(input: &str) -> String {
    let first = input.lines().next().unwrap_or("");
    if input.contains('\n') {
        format!("{first} …")
    } else {
        first.to_string()
    }
}

// ── AgentUI trait ────────────────────────────────────────────────────

pub trait AgentUI {
    fn start_spinner(&self, msg: &str);
    fn stop_spinner(&self);
    fn show_reasoning(&self, text: &str);
    fn show_auto_tool(&self, tool_call: &ToolCall);
    fn show_tool_result(&self, result: &str);
    fn confirm_tool(&self, tool_call: &ToolCall) -> bool;
    fn show_answer(&self, text: &str);
    fn show_error(&self, text: &str);
    fn show_token_usage(&self, usage: &TokenUsage, model: &str, final_answer: bool);
}

// ── PlainUI (single-shot / pipe-friendly) ────────────────────────────

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

    fn show_token_usage(&self, _usage: &TokenUsage, _model: &str, _final_answer: bool) {}
}

// ── InteractiveUI (colors, spinner, markdown) ────────────────────────

pub struct InteractiveUI {
    spinner: RefCell<ProgressBar>,
    first_spinner: Cell<bool>,
}

impl InteractiveUI {
    pub fn new() -> Self {
        Self {
            spinner: RefCell::new(ProgressBar::hidden()),
            first_spinner: Cell::new(true),
        }
    }

    pub fn print_welcome() {
        let rule = "─".repeat(banner_width());
        eprintln!("{}", rule.as_str().with(Color::DarkGrey));
        eprintln!(
            "{PAD}{} {}",
            "aictl".with(Color::Cyan).attribute(Attribute::Bold),
            "— AI agent in your terminal".with(Color::DarkGrey),
        );
        eprintln!("{PAD}{}", WELCOME_TEXT.with(Color::DarkGrey));
        eprintln!("{}", rule.as_str().with(Color::DarkGrey));
        eprintln!();
    }

    fn bottom_rule() {
        let dashes = "─".repeat(tool_rule_width());
        eprintln!(
            "{PAD}{}{}",
            "╰".with(Color::DarkGrey),
            dashes.as_str().with(Color::DarkGrey),
        );
    }

    fn pipe(text: &str, color: Color) {
        eprintln!("{PAD}{} {}", PIPE.with(Color::DarkGrey), text.with(color));
    }

    fn print_block(text: &str, color: Color) {
        let max_w = max_content_width();
        let lines: Vec<&str> = text.lines().collect();
        let total = lines.len();

        if total <= MAX_RESULT_LINES {
            for line in &lines {
                Self::pipe(&truncate_line(line, max_w), color);
            }
        } else {
            let head = MAX_RESULT_LINES - 3;
            let tail = 2;
            for line in &lines[..head] {
                Self::pipe(&truncate_line(line, max_w), color);
            }
            let hidden = total - head - tail;
            Self::pipe(&format!("… {hidden} lines hidden …"), Color::DarkGrey);
            for line in &lines[total - tail..] {
                Self::pipe(&truncate_line(line, max_w), color);
            }
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
        let prefix = if self.first_spinner.get() {
            self.first_spinner.set(false);
            ""
        } else {
            PAD
        };
        pb.set_message(format!("{prefix}{}", msg.with(Color::DarkGrey)));
        pb.enable_steady_tick(Duration::from_millis(80));
        *self.spinner.borrow_mut() = pb;
    }

    fn stop_spinner(&self) {
        self.spinner.borrow().finish_and_clear();
    }

    fn show_reasoning(&self, text: &str) {
        Self::print_block(text, Color::DarkGrey);
    }

    fn show_auto_tool(&self, tool_call: &ToolCall) {
        let max_w = max_content_width();
        let input = first_input_line(&tool_call.input);
        let budget = max_w.saturating_sub(tool_call.name.len() + 13);
        let input = truncate_line(&input, budget);
        eprintln!(
            "{PAD}{} {} {} {} {}",
            PIPE.with(Color::DarkGrey),
            tool_call.name.as_str().with(Color::Cyan),
            "──".with(Color::DarkGrey),
            input.with(Color::DarkGrey),
            "(auto)".with(Color::Yellow),
        );
    }

    fn show_tool_result(&self, result: &str) {
        eprintln!("{PAD}{}", PIPE.with(Color::DarkGrey));
        Self::print_block(result, Color::DarkGrey);
        Self::bottom_rule();
    }

    fn confirm_tool(&self, tool_call: &ToolCall) -> bool {
        let max_w = max_content_width();
        let input = first_input_line(&tool_call.input);
        let budget = max_w.saturating_sub(tool_call.name.len() + 5);
        let input = truncate_line(&input, budget);
        eprintln!(
            "{PAD}{} {} {} {}",
            PIPE.with(Color::DarkGrey),
            tool_call.name.as_str().with(Color::Cyan),
            "──".with(Color::DarkGrey),
            input.with(Color::DarkGrey),
        );
        eprint!(
            "{PAD}{} {} ",
            PIPE.with(Color::DarkGrey),
            "allow? [y/N]".with(Color::Yellow),
        );
        std::io::stderr().flush().ok();
        let mut buf = String::new();
        if std::io::stdin().read_line(&mut buf).is_err() {
            return false;
        }
        matches!(buf.trim(), "y" | "Y" | "yes" | "Yes")
    }

    fn show_answer(&self, text: &str) {
        self.first_spinner.set(true);
        let skin = termimad::MadSkin::default();
        let rendered = format!("{}", skin.term_text(text));
        eprintln!();
        for line in rendered.lines() {
            eprintln!("{PAD}{line}");
        }
        eprintln!();
    }

    fn show_error(&self, text: &str) {
        self.first_spinner.set(true);
        eprintln!("{PAD}{}", text.with(Color::Red).attribute(Attribute::Bold));
    }

    fn show_token_usage(&self, usage: &TokenUsage, model: &str, final_answer: bool) {
        let total = usage.input_tokens + usage.output_tokens;
        let cost_str = match usage.estimate_cost(model) {
            Some(cost) => format!(" · ${cost:.4}"),
            None => String::new(),
        };
        let text = format!(
            "tokens: {} in / {} out / {} total{cost_str}",
            usage.input_tokens, usage.output_tokens, total,
        );
        if final_answer {
            eprintln!("{PAD}{}", text.with(Color::DarkGreen));
        } else {
            eprintln!(
                "{PAD}{} {}",
                "╭".with(Color::DarkGrey),
                text.with(Color::DarkGreen),
            );
        }
    }
}
