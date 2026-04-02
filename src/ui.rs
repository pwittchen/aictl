use std::cell::{Cell, RefCell};
use std::io::Write;
use std::time::Duration;

use crossterm::style::{Attribute, Color, Stylize};
use indicatif::{ProgressBar, ProgressStyle};

use crate::llm::TokenUsage;
use crate::tools::ToolCall;

/// Result of a tool-call confirmation prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolApproval {
    /// Allow this single tool call.
    Allow,
    /// Deny this tool call.
    Deny,
    /// Allow this call and switch to auto mode for the rest of the session.
    AutoAccept,
}

const PAD: &str = "  ";
const PIPE: &str = "│";
const WELCOME_TEXT: &str = "Type a message, \"exit\" or Ctrl+D to quit";
const MAX_RESULT_LINES: usize = 15;
const MAX_ANSWER_WIDTH: usize = 76;
const FALLBACK_WIDTH: u16 = 80;

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
/// Align right edge with content: PAD(2)+╭(1)+dashes = PAD(2)+│(1)+space(1)+content.
fn rule_width() -> usize {
    max_content_width().min(MAX_ANSWER_WIDTH) + 1
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
    fn confirm_tool(&self, tool_call: &ToolCall) -> ToolApproval;
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
        context_pct: u8,
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
            if result.starts_with("Security policy denied:") {
                eprintln!("{}", result.with(Color::Red));
            } else {
                eprintln!("{result}");
            }
        }
    }

    fn confirm_tool(&self, tool_call: &ToolCall) -> ToolApproval {
        if crate::tools::confirm_tool_call(tool_call) {
            ToolApproval::Allow
        } else {
            ToolApproval::Deny
        }
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
        _context_pct: u8,
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

    pub fn print_welcome(provider: &str, model: &str, version_info: &str) {
        let dashes = "─".repeat(rule_width());
        eprintln!(
            "{PAD}{}{}",
            "╭".with(Color::DarkGrey),
            dashes.as_str().with(Color::DarkGrey),
        );
        let version_color = if version_info.contains("latest") {
            Color::Green
        } else {
            Color::Yellow
        };
        if version_info.is_empty() {
            eprintln!(
                "{PAD}{} {} {} {}",
                PIPE.with(Color::DarkGrey),
                "aictl".with(Color::Cyan).attribute(Attribute::Bold),
                crate::VERSION.with(Color::DarkGrey),
                "— AI agent in your terminal".with(Color::DarkGrey),
            );
        } else {
            eprintln!(
                "{PAD}{} {} {} {} {}",
                PIPE.with(Color::DarkGrey),
                "aictl".with(Color::Cyan).attribute(Attribute::Bold),
                crate::VERSION.with(Color::DarkGrey),
                version_info.with(version_color),
                "— AI agent in your terminal".with(Color::DarkGrey),
            );
        }
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
        if version_info.contains("available") {
            eprintln!(
                "{PAD}{} {}",
                PIPE.with(Color::DarkGrey),
                "Run /update or aictl --update to upgrade".with(Color::Yellow),
            );
        }
        if !crate::security::policy().enabled {
            eprintln!(
                "{PAD}{} {}",
                PIPE.with(Color::DarkGrey),
                "security restrictions disabled (--unrestricted)".with(Color::Yellow),
            );
        }
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
        if result.starts_with("Security policy denied:") {
            Self::print_block(result, Color::Red);
        } else {
            Self::print_block(result, Color::DarkGrey);
        }
        Self::bottom_rule();
    }

    fn confirm_tool(&self, tool_call: &ToolCall) -> ToolApproval {
        use crossterm::{
            cursor,
            event::{self, Event, KeyCode, KeyEventKind},
            execute,
            terminal::{self, ClearType},
        };

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

        let mut selected: usize = 0;
        let options = ["Allow", "Deny", "Auto-accept"];

        let draw = |sel: usize, out: &mut std::io::Stdout| {
            for (i, label) in options.iter().enumerate() {
                if i == sel {
                    let _ = write!(
                        out,
                        "  {} {}\r\n",
                        "›".with(Color::Green).attribute(Attribute::Bold),
                        label.with(Color::White).attribute(Attribute::Bold),
                    );
                } else {
                    let _ = write!(out, "    {}\r\n", label.with(Color::DarkGrey));
                }
            }
            let _ = write!(
                out,
                "\r\n  {}\r\n",
                "↑/↓ navigate · enter select".with(Color::DarkGrey)
            );
            let _ = out.flush();
        };

        let total_lines = options.len() + 2; // options + blank + hint

        let _ = terminal::enable_raw_mode();
        let mut stdout = std::io::stdout();
        let _ = execute!(stdout, cursor::Hide);
        let _ = write!(stdout, "\r\n");
        draw(selected, &mut stdout);

        let result = loop {
            if !event::poll(std::time::Duration::from_millis(100)).unwrap_or(false) {
                continue;
            }
            let Ok(ev) = event::read() else {
                break ToolApproval::Deny;
            };
            match ev {
                Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                    KeyCode::Up | KeyCode::Left => {
                        selected = selected.saturating_sub(1);
                    }
                    KeyCode::Down | KeyCode::Right => {
                        if selected + 1 < options.len() {
                            selected += 1;
                        }
                    }
                    KeyCode::Enter => {
                        break match selected {
                            0 => ToolApproval::Allow,
                            2 => ToolApproval::AutoAccept,
                            _ => ToolApproval::Deny,
                        };
                    }
                    KeyCode::Esc | KeyCode::Char('n' | 'N') => break ToolApproval::Deny,
                    KeyCode::Char('y' | 'Y') => break ToolApproval::Allow,
                    KeyCode::Char('a' | 'A') => break ToolApproval::AutoAccept,
                    _ => continue,
                },
                _ => continue,
            }

            // Redraw
            #[allow(clippy::cast_possible_truncation)]
            let _ = execute!(
                stdout,
                cursor::MoveUp(total_lines as u16),
                terminal::Clear(ClearType::FromCursorDown),
            );
            draw(selected, &mut stdout);
        };

        // Clean up: erase the selector and restore terminal
        #[allow(clippy::cast_possible_truncation)]
        let _ = execute!(
            stdout,
            cursor::MoveUp(total_lines as u16 + 1),
            terminal::Clear(ClearType::FromCursorDown),
            cursor::Show,
        );
        let _ = terminal::disable_raw_mode();

        result
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
        // Shorten claude model names by stripping the date suffix
        let display_model = if model.starts_with("claude-") {
            model.rsplit_once('-').map_or(model, |(prefix, _)| prefix)
        } else {
            model
        };
        let cost_str = match usage.estimate_cost(model) {
            Some(cost) => format!(" · ${cost:.4}"),
            None => String::new(),
        };
        let cwd = std::env::current_dir()
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
            .unwrap_or_default();
        let cwd_str = if cwd.is_empty() {
            String::new()
        } else {
            format!(" · {cwd}")
        };
        let text = format!(
            "{display_model} · {}↑ · {}↓ · {} tool(s){cost_str} · {:.1}s · ctx {context_pct}%{cwd_str}",
            usage.input_tokens,
            usage.output_tokens,
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
        context_pct: u8,
    ) {
        let cost_str = match usage.estimate_cost(model) {
            Some(cost) => format!(" · ${cost:.4}"),
            None => String::new(),
        };
        let text = format!(
            "{llm_calls} reqs · {tool_calls} tool(s) · {}↑ · {}↓{cost_str} · {:.1}s · ctx {context_pct}%",
            usage.input_tokens,
            usage.output_tokens,
            elapsed.as_secs_f64(),
        );
        eprintln!("{PAD}{}", text.with(Color::Green).attribute(Attribute::Dim));
        eprintln!();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- truncate_line ---

    #[test]
    fn truncate_fits_within_limit() {
        assert_eq!(truncate_line("hello", 10), "hello");
    }

    #[test]
    fn truncate_exact_limit() {
        assert_eq!(truncate_line("hello", 5), "hello");
    }

    #[test]
    fn truncate_exceeds_limit() {
        let result = truncate_line("hello world", 6);
        assert_eq!(result, "hello…");
    }

    #[test]
    fn truncate_empty_string() {
        assert_eq!(truncate_line("", 10), "");
    }

    #[test]
    fn truncate_max_less_than_2() {
        assert_eq!(truncate_line("hello", 1), "");
        assert_eq!(truncate_line("hello", 0), "");
    }

    #[test]
    fn truncate_unicode() {
        // 4 chars: café
        assert_eq!(truncate_line("café", 4), "café");
        assert_eq!(truncate_line("café", 3), "ca…");
    }

    // --- first_input_line ---

    #[test]
    fn first_input_single_line() {
        assert_eq!(first_input_line("hello"), "hello");
    }

    #[test]
    fn first_input_multiline() {
        assert_eq!(first_input_line("first\nsecond\nthird"), "first …");
    }

    #[test]
    fn first_input_empty() {
        assert_eq!(first_input_line(""), "");
    }
}
