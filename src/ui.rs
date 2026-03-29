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
const MAX_ANSWER_WIDTH: usize = 80;
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

/// Number of ─ chars in box rules.
/// Rule is: PAD(2) + ╭/╰(1) + dashes, matching answer content width.
fn rule_width() -> usize {
    MAX_ANSWER_WIDTH
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
    fn show_token_usage(
        &self,
        usage: &TokenUsage,
        model: &str,
        final_answer: bool,
        tool_calls: u32,
        elapsed: Duration,
        context_pct: u8,
    );
    fn show_summary(
        &self,
        usage: &TokenUsage,
        model: &str,
        llm_calls: u32,
        tool_calls: u32,
        elapsed: Duration,
    );
}

// ── PlainUI (single-shot / pipe-friendly) ────────────────────────────

pub struct PlainUI {
    pub quiet: bool,
}

impl AgentUI for PlainUI {
    fn start_spinner(&self, _msg: &str) {}
    fn stop_spinner(&self) {}

    fn show_reasoning(&self, text: &str) {
        if !self.quiet {
            eprintln!("{text}");
        }
    }

    fn show_auto_tool(&self, tool_call: &ToolCall) {
        if !self.quiet {
            eprintln!("[auto] Running: {}", tool_call.input);
        }
    }

    fn show_tool_result(&self, result: &str) {
        if !self.quiet {
            eprintln!("{result}");
        }
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

    fn show_token_usage(
        &self,
        _usage: &TokenUsage,
        _model: &str,
        _final_answer: bool,
        _tool_calls: u32,
        _elapsed: Duration,
        _context_pct: u8,
    ) {
    }

    fn show_summary(
        &self,
        _usage: &TokenUsage,
        _model: &str,
        _llm_calls: u32,
        _tool_calls: u32,
        _elapsed: Duration,
    ) {
    }
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

    pub fn print_welcome(provider: &str, model: &str) {
        let dashes = "─".repeat(rule_width());
        eprintln!(
            "{PAD}{}{}",
            "╭".with(Color::DarkGrey),
            dashes.as_str().with(Color::DarkGrey),
        );
        eprintln!(
            "{PAD}{} {} {} {}",
            PIPE.with(Color::DarkGrey),
            "aictl".with(Color::Cyan).attribute(Attribute::Bold),
            crate::VERSION.with(Color::DarkGrey),
            "— AI agent in your terminal".with(Color::DarkGrey),
        );
        eprintln!(
            "{PAD}{} {} {} {}",
            PIPE.with(Color::DarkGrey),
            provider.with(Color::Green),
            "·".with(Color::DarkGrey),
            model.with(Color::Yellow),
        );
        eprintln!(
            "{PAD}{} {}",
            PIPE.with(Color::DarkGrey),
            WELCOME_TEXT.with(Color::DarkGrey)
        );
        eprintln!(
            "{PAD}{}{}",
            "╰".with(Color::DarkGrey),
            dashes.as_str().with(Color::DarkGrey),
        );
        eprintln!();
    }

    fn bottom_rule() {
        let dashes = "─".repeat(rule_width());
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
        eprintln!("{PAD}{}", "│".with(Color::DarkGrey));
        Self::print_block(text, Color::DarkGrey);
    }

    fn show_auto_tool(&self, tool_call: &ToolCall) {
        let max_w = max_content_width();
        let input = first_input_line(&tool_call.input);
        let budget = max_w.saturating_sub(tool_call.name.len() + 13);
        let input = truncate_line(&input, budget);
        eprintln!("{PAD}{}", PIPE.with(Color::DarkGrey));
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
        eprintln!("{PAD}{}", PIPE.with(Color::DarkGrey));
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
        let width = max_content_width().min(MAX_ANSWER_WIDTH);
        let rendered = format!(
            "{}",
            termimad::FmtText::from_text(&skin, text.into(), Some(width))
        );
        eprintln!("{PAD}{}", PIPE.with(Color::DarkGrey));
        for line in rendered.lines() {
            eprintln!("{PAD}{} {line}", PIPE.with(Color::DarkGrey));
        }
        Self::bottom_rule();
        eprintln!();
    }

    fn show_error(&self, text: &str) {
        self.first_spinner.set(true);
        eprintln!();
        eprintln!("{PAD}{}", text.with(Color::Red).attribute(Attribute::Bold));
        eprintln!();
    }

    fn show_token_usage(
        &self,
        usage: &TokenUsage,
        model: &str,
        _final_answer: bool,
        tool_calls: u32,
        elapsed: Duration,
        context_pct: u8,
    ) {
        let total = usage.input_tokens + usage.output_tokens;
        let cost_str = match usage.estimate_cost(model) {
            Some(cost) => format!(" · ${cost:.4}"),
            None => String::new(),
        };
        let text = format!(
            "tokens: {}↑ · {}↓ · {} total · {} tool(s){cost_str} · {:.1}s · ctx: {context_pct}%",
            usage.input_tokens,
            usage.output_tokens,
            total,
            tool_calls,
            elapsed.as_secs_f64(),
        );
        eprintln!(
            "{PAD}{} {}",
            "╭".with(Color::DarkGrey),
            text.with(Color::Green).attribute(Attribute::Dim),
        );
    }

    fn show_summary(
        &self,
        usage: &TokenUsage,
        model: &str,
        llm_calls: u32,
        tool_calls: u32,
        elapsed: Duration,
    ) {
        let total = usage.input_tokens + usage.output_tokens;
        let cost_str = match usage.estimate_cost(model) {
            Some(cost) => format!(" · ${cost:.4}"),
            None => String::new(),
        };
        let text = format!(
            "total: {llm_calls} request(s) · {tool_calls} tool call(s) · {}↑ · {}↓ · {} total{cost_str} · {:.1}s",
            usage.input_tokens,
            usage.output_tokens,
            total,
            elapsed.as_secs_f64(),
        );
        eprintln!("{PAD}{}", text.with(Color::Green).attribute(Attribute::Dim));
        eprintln!();
    }
}
