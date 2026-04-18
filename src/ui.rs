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
const WELCOME_TEXT: &str = "Type a message or /help for commands. Exit with \"exit\" or Ctrl+D";
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

/// Write `text` to `out`, translating every bare `\n` into `\r\n`. Used by the
/// streaming path because [`crate::with_esc_cancel`] holds the terminal in
/// crossterm raw mode for the duration of the LLM call — in raw mode a lone
/// line-feed moves the cursor down but does not reset it to column 0, so
/// multi-line streamed output otherwise marches off to the right.
fn write_with_crlf<W: Write>(out: &mut W, text: &str) {
    // Fast path: no newlines to translate.
    if !text.contains('\n') {
        let _ = out.write_all(text.as_bytes());
        return;
    }
    let bytes = text.as_bytes();
    let mut last = 0usize;
    for (idx, b) in bytes.iter().enumerate() {
        if *b == b'\n' {
            let _ = out.write_all(&bytes[last..idx]);
            let _ = out.write_all(b"\r\n");
            last = idx + 1;
        }
    }
    if last < bytes.len() {
        let _ = out.write_all(&bytes[last..]);
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
    /// Begin a streamed response — called once just before the first
    /// `stream_chunk` of a turn, after the spinner has been stopped.
    /// Used by the interactive UI to draw the top frame; the plain UI
    /// does nothing.
    fn stream_begin(&self) {}
    /// Forward an incremental delta of text from the LLM stream to the user.
    /// Empty deltas should be tolerated (and ignored). The default impl is a
    /// no-op so providers don't need to special-case UIs that don't render
    /// progressively.
    fn stream_chunk(&self, _text: &str) {}
    /// Called once when the streaming state machine has confirmed the start
    /// of a tool call (the `<tool name="…">` prefix matched). The
    /// interactive UI uses this hook to flush any word-wrap buffer — so the
    /// last word of the reasoning appears immediately instead of hanging
    /// until `stream_end` — and to start a "preparing tool call…" spinner
    /// that fills the otherwise-silent gap while the model streams the
    /// (hidden) tool XML. No-op by default.
    fn stream_suspend(&self) {}
    /// End a streamed response — called once after the stream completes
    /// (whether or not a tool call was detected). Draws the bottom frame
    /// in the interactive UI; no-op in plain UI.
    fn stream_end(&self) {}
    #[allow(clippy::too_many_arguments)]
    fn show_token_usage(
        &self,
        usage: &TokenUsage,
        model: &str,
        final_answer: bool,
        tool_calls: u32,
        elapsed: Duration,
        context_pct: u8,
        memory: &str,
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
    /// True once a streamed response has been printed in this turn — tells
    /// the agent loop to skip the trailing `show_answer` re-render.
    pub streamed: Cell<bool>,
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
            if result.starts_with("Security policy denied:") || result.starts_with("Error:") {
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
        if self.streamed.replace(false) {
            // Streamed response already on screen. Just terminate the line so
            // the next prompt isn't glued to the answer's last token.
            println!();
            return;
        }
        println!("{text}");
    }

    fn show_error(&self, text: &str) {
        eprintln!("{text}");
    }

    fn stream_chunk(&self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.streamed.set(true);
        // `with_esc_cancel` puts the terminal in raw mode for the duration of
        // the LLM call, so bare `\n` is a line-feed with no carriage return.
        // Translate embedded newlines to `\r\n` so the cursor snaps back to
        // column 0 on each line — otherwise long responses cascade right
        // across the terminal.
        let mut out = std::io::stdout();
        write_with_crlf(&mut out, text);
        let _ = out.flush();
    }

    fn show_token_usage(
        &self,
        _usage: &TokenUsage,
        _model: &str,
        _final_answer: bool,
        _tool_calls: u32,
        _elapsed: Duration,
        _context_pct: u8,
        _memory: &str,
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
    /// True once any text has been streamed in this turn — flips the trailing
    /// `show_answer` into a no-op so the markdown-rendered answer doesn't
    /// duplicate the raw streamed text.
    streamed: Cell<bool>,
    /// `true` when the streaming cursor is at column 0 — used by
    /// `stream_chunk` to know whether to emit the `PAD` left margin before
    /// the next text byte. Reset to `true` by `stream_begin` and after every
    /// `\r\n` we emit mid-chunk.
    at_line_start: Cell<bool>,
    /// Visible character count on the current streamed line (excluding the
    /// leading `PAD`). Used by the word-wrap logic in `stream_chunk` to
    /// decide when to break to a new line so streamed output matches the
    /// fixed width of tool output blocks.
    stream_col: Cell<usize>,
    /// Buffered run of non-whitespace characters that hasn't been emitted
    /// yet — held until we see whitespace (or end of stream) so we can
    /// word-wrap at a clean boundary instead of mid-token. Survives across
    /// chunks so a word split mid-delta still wraps correctly.
    stream_word: RefCell<String>,
    /// Pending space characters that follow the last emitted word. Held
    /// alongside `stream_word` so they can either ride along with the next
    /// word on the same line or be dropped after a wrap.
    stream_spaces: Cell<usize>,
    /// `true` while a "preparing tool call…" spinner started by
    /// `stream_suspend` is still running. `stream_end` reads this to know
    /// whether to stop a spinner (and reset to `false`).
    suspend_spinner: Cell<bool>,
}

impl InteractiveUI {
    pub fn new() -> Self {
        Self {
            spinner: RefCell::new(ProgressBar::hidden()),
            first_spinner: Cell::new(true),
            streamed: Cell::new(false),
            at_line_start: Cell::new(true),
            stream_col: Cell::new(0),
            stream_word: RefCell::new(String::new()),
            stream_spaces: Cell::new(0),
            suspend_spinner: Cell::new(false),
        }
    }

    #[allow(clippy::too_many_lines)]
    pub fn print_welcome(
        provider: &str,
        model: &str,
        memory: crate::commands::MemoryMode,
        version_info: &str,
    ) {
        const BLANK: &str = "      ";
        const MASCOTS: [[&str; 2]; 6] = [
            ["[o_o] ", " |_|  "],
            ["[^_^] ", " |_|  "],
            ["[>_<] ", " |_|  "],
            ["[-_-] ", " |_|  "],
            ["[o_O] ", " |_|  "],
            ["[._.) ", " |_|  "],
        ];
        const SLEEPY: [&str; 2] = ["[u_u] ", " |_|  "];
        // Small spiral drawn from z letters, rendered in the mascot column
        // below the sleepy face (exactly 3 z letters, one per row, forming
        // a tiny S-curl). Each row is 6 chars wide so the rest of the banner
        // content stays aligned.
        const SLEEPY_SPIRAL: [&str; 3] = [" z    ", "z     ", " z    "];
        const CREAM_CAKE_FAN: [&str; 2] = ["[x_x] ", " |_|  "];
        let now_str = std::process::Command::new("date")
            .arg("+%H:%M")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .unwrap_or_default();
        let (hour_str, minute_str) = now_str.trim().split_once(':').unwrap_or(("", ""));
        let local_hour = hour_str.parse::<u32>().ok();
        let local_minute = minute_str.parse::<u32>().ok();
        let sleepy = matches!(local_hour, Some(h) if !(6..22).contains(&h));
        let eol = local_hour == Some(21) && local_minute == Some(37);
        let face = if eol {
            CREAM_CAKE_FAN
        } else if sleepy {
            SLEEPY
        } else {
            let pick = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis() as usize)
                % MASCOTS.len();
            MASCOTS[pick]
        };
        let face_color = if eol { Color::Yellow } else { Color::Cyan };
        let body_color = if eol { Color::White } else { Color::Cyan };
        let m = if sleepy {
            [
                face[0],
                face[1],
                BLANK,
                SLEEPY_SPIRAL[0],
                SLEEPY_SPIRAL[1],
                SLEEPY_SPIRAL[2],
                BLANK,
                BLANK,
            ]
        } else {
            [face[0], face[1], BLANK, BLANK, BLANK, BLANK, BLANK, BLANK]
        };

        let dashes = "─".repeat(rule_width());
        eprintln!();
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

        // Line 0: title
        if version_info.is_empty() {
            eprintln!(
                "{PAD}{} {}{} {} {}",
                PIPE.with(Color::DarkGrey),
                m[0].with(face_color),
                "aictl".with(Color::Cyan).attribute(Attribute::Bold),
                crate::VERSION.with(Color::DarkGrey),
                "— AI agent in your terminal".with(Color::DarkGrey),
            );
        } else {
            eprintln!(
                "{PAD}{} {}{} {} {} {}",
                PIPE.with(Color::DarkGrey),
                m[0].with(face_color),
                "aictl".with(Color::Cyan).attribute(Attribute::Bold),
                crate::VERSION.with(Color::DarkGrey),
                version_info.with(version_color),
                "— AI agent in your terminal".with(Color::DarkGrey),
            );
        }

        // Line 1: provider · model
        eprintln!(
            "{PAD}{} {}{} {} {}",
            PIPE.with(Color::DarkGrey),
            m[1].with(body_color),
            provider.with(Color::Green),
            "·".with(Color::DarkGrey),
            model.with(Color::Yellow),
        );

        // Line 2: memory · tools · dir
        let cwd = std::env::current_dir()
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_default();
        let tools_info = if crate::tools::tools_enabled() {
            let tools_count =
                crate::tools::TOOL_COUNT - crate::security::policy().disabled_tools.len();
            format!("{tools_count} tools")
        } else {
            "tools disabled".to_string()
        };
        eprintln!(
            "{PAD}{} {}{} {} {} {} {}",
            PIPE.with(Color::DarkGrey),
            m[2].with(Color::Cyan),
            format!("{memory} memory").with(Color::DarkGrey),
            "·".with(Color::DarkGrey),
            tools_info.as_str().with(if crate::tools::tools_enabled() {
                Color::DarkGrey
            } else {
                Color::Yellow
            }),
            "·".with(Color::DarkGrey),
            format!("dir: {cwd}/").as_str().with(Color::DarkGrey),
        );

        // Line 3: security info
        let mut next = 3;
        if crate::security::policy().enabled {
            let pol = crate::security::policy();
            let cwd_jail = if pol.paths.restrict_to_cwd {
                "on"
            } else {
                "off"
            };
            let subshell = if pol.shell.block_subshell {
                "blocked"
            } else {
                "allowed"
            };
            let timeout = if pol.resources.shell_timeout_secs == 0 {
                "none".to_string()
            } else {
                format!("{}s", pol.resources.shell_timeout_secs)
            };
            let disabled = if crate::tools::tools_enabled() {
                pol.disabled_tools.len().to_string()
            } else {
                "all".to_string()
            };
            eprintln!(
                "{PAD}{} {}{}",
                PIPE.with(Color::DarkGrey),
                m[next].with(Color::Cyan),
                format!(
                    "cwd jail: {cwd_jail} · subshell: {subshell} · timeout: {timeout} · disabled tools: {disabled}"
                ).with(Color::DarkGrey),
            );
        } else {
            eprintln!(
                "{PAD}{} {}{}",
                PIPE.with(Color::DarkGrey),
                m[next].with(Color::Cyan),
                "security restrictions disabled (--unrestricted)".with(Color::Red),
            );
        }
        next += 1;

        // Key storage backend line
        let backend = crate::keys::backend_name();
        let (locked, plain, both, _unset) = crate::keys::counts();
        let backend_color = if backend == "plain text" {
            Color::Yellow
        } else {
            Color::Green
        };
        eprintln!(
            "{PAD}{} {}{} {}",
            PIPE.with(Color::DarkGrey),
            m[next].with(Color::Cyan),
            format!(
                "{backend} {}",
                if plain == 0 && both == 0 {
                    "●"
                } else {
                    "○"
                }
            )
            .with(backend_color),
            format!("({locked} locked · {plain} plain · {both} both)").with(Color::DarkGrey),
        );
        next += 1;

        // Line 4: session / incognito info
        if crate::session::is_incognito() {
            eprintln!(
                "{PAD}{} {}{}",
                PIPE.with(Color::DarkGrey),
                m[next].with(Color::Cyan),
                "incognito mode: sessions are not saved".with(Color::Yellow),
            );
        } else if let Some((id, name)) = crate::session::current_info() {
            let label = name
                .as_deref()
                .map_or_else(|| id.clone(), |n| format!("{id} ({n})"));
            eprintln!(
                "{PAD}{} {}{}",
                PIPE.with(Color::DarkGrey),
                m[next].with(Color::Cyan),
                format!("session: {label}").with(Color::DarkGrey),
            );
        }
        next += 1;

        // Welcome text
        eprintln!(
            "{PAD}{} {}{}",
            PIPE.with(Color::DarkGrey),
            m[next].with(Color::Cyan),
            WELCOME_TEXT.with(Color::DarkGrey)
        );
        next += 1;

        // Optional update hint + installation instructions
        if version_info.contains("available") {
            eprintln!(
                "{PAD}{} {}{}",
                PIPE.with(Color::DarkGrey),
                m[next].with(Color::Cyan),
                "Run /update or aictl --update to upgrade, or install manually:"
                    .with(Color::Yellow),
            );
            eprintln!(
                "{PAD}{} {}{}",
                PIPE.with(Color::DarkGrey),
                BLANK.with(Color::Cyan),
                format!("  {}", crate::commands::UPDATE_CMD).with(Color::Yellow),
            );
        }
        eprintln!(
            "{PAD}{}{}",
            "╰".with(Color::DarkGrey),
            dashes.as_str().with(Color::DarkGrey),
        );
        eprintln!();
    }

    /// Horizontal rule drawn above status / summary lines. Dark-grey, left-
    /// aligned with the PAD margin used by the rest of the turn body.
    fn status_rule() {
        let dashes = "─".repeat(rule_width());
        eprintln!("{PAD}{}", dashes.as_str().with(Color::DarkGrey));
    }

    fn pad_line(text: &str, color: Color) {
        eprintln!("{PAD}{}", text.with(color));
    }

    /// Emit the buffered streamed word (with any pending inter-word spaces)
    /// to `out`, wrapping to a fresh `PAD`-prefixed line if it wouldn't fit
    /// inside `max_w` visible columns. Updates `stream_col`, `at_line_start`,
    /// and clears `stream_word` / `stream_spaces`. No-op when the word
    /// buffer is empty (trailing whitespace before a newline is discarded
    /// by the caller's `\r\n` reset).
    fn flush_stream_word<W: Write>(&self, out: &mut W, max_w: usize) {
        let mut word = self.stream_word.borrow_mut();
        if word.is_empty() {
            return;
        }
        let word_len = word.chars().count();
        let spaces = self.stream_spaces.replace(0);
        let col = self.stream_col.get();
        let needs_wrap = col > 0 && col + spaces + word_len > max_w;

        if needs_wrap {
            // Word doesn't fit on the current line — break to a new line and
            // drop the pending inter-word spaces (standard word-wrap).
            let _ = write!(out, "\r\n");
            let _ = write!(out, "{PAD}");
            let _ = out.write_all(word.as_bytes());
            self.stream_col.set(word_len);
            self.at_line_start.set(false);
        } else {
            if self.at_line_start.get() {
                let _ = write!(out, "{PAD}");
            }
            for _ in 0..spaces {
                let _ = out.write_all(b" ");
            }
            let _ = out.write_all(word.as_bytes());
            self.stream_col.set(col + spaces + word_len);
            self.at_line_start.set(false);
        }
        word.clear();
    }

    fn print_block(text: &str, color: Color) {
        let max_w = max_content_width();
        let lines: Vec<&str> = text.lines().collect();
        let total = lines.len();

        if total <= MAX_RESULT_LINES {
            for line in &lines {
                Self::pad_line(&truncate_line(line, max_w), color);
            }
        } else {
            let head = MAX_RESULT_LINES - 3;
            let tail = 2;
            for line in &lines[..head] {
                Self::pad_line(&truncate_line(line, max_w), color);
            }
            let hidden = total - head - tail;
            Self::pad_line(&format!("… {hidden} lines hidden …"), Color::DarkGrey);
            for line in &lines[total - tail..] {
                Self::pad_line(&truncate_line(line, max_w), color);
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
        eprintln!();
        Self::print_block(text, Color::DarkGrey);
    }

    fn show_auto_tool(&self, tool_call: &ToolCall) {
        let max_w = max_content_width();
        let input = first_input_line(&tool_call.input);
        let budget = max_w.saturating_sub(tool_call.name.len() + 13);
        let input = truncate_line(&input, budget);
        eprintln!();
        eprintln!(
            "{PAD}{} {} {} {}",
            tool_call.name.as_str().with(Color::Cyan),
            "──".with(Color::DarkGrey),
            input.with(Color::DarkGrey),
            "(auto)".with(Color::Yellow),
        );
    }

    fn show_tool_result(&self, result: &str) {
        eprintln!();
        if result.starts_with("Security policy denied:") || result.starts_with("Error:") {
            Self::print_block(result, Color::Red);
        } else {
            Self::print_block(result, Color::DarkGrey);
        }
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
        eprintln!();
        eprintln!(
            "{PAD}{} {} {}",
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
        // When the answer was streamed, it's already on screen with the
        // pipe framing — don't re-render. Trying to clear the streamed
        // output and reprint with termimad markdown is unreliable across
        // terminals (line-wrap counting, scrolled viewports). We trade
        // the pretty markdown render for progressive output; documented
        // in CLAUDE.md.
        if self.streamed.replace(false) {
            return;
        }
        // Buffered path (streaming off, or non-TTY). Render markdown but
        // without the old `│` frame — show_token_usage draws the rule.
        let skin = termimad::MadSkin::default();
        let width = max_content_width().min(MAX_ANSWER_WIDTH);
        let rendered = format!(
            "{}",
            termimad::FmtText::from_text(&skin, text.into(), Some(width))
        );
        eprintln!();
        for line in rendered.lines() {
            eprintln!("{PAD}{line}");
        }
    }

    fn show_error(&self, text: &str) {
        self.first_spinner.set(true);
        eprintln!();
        eprintln!("{PAD}{}", text.with(Color::Red).attribute(Attribute::Bold));
        eprintln!();
    }

    fn stream_begin(&self) {
        // Blank spacer line between the prompt and the streamed body — no
        // frame glyphs. `with_esc_cancel` holds the terminal in raw mode for
        // the whole LLM call, so we emit CR+LF explicitly (a bare `\n` only
        // moves the cursor down, without resetting it to column 0).
        self.first_spinner.set(true);
        self.streamed.set(true);
        self.at_line_start.set(true);
        self.stream_col.set(0);
        self.stream_word.borrow_mut().clear();
        self.stream_spaces.set(0);
        let mut out = std::io::stderr();
        let _ = write!(out, "\r\n");
        let _ = out.flush();
    }

    fn stream_chunk(&self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.streamed.set(true);
        let mut out = std::io::stderr();
        let max_w = max_content_width();
        // Walk the chunk char by char: build up the current word, flush at
        // whitespace/newlines. Wrapping happens inside `flush_stream_word`
        // when col + spaces + word would exceed `max_w`, so streamed output
        // aligns with the fixed tool-output width instead of relying on the
        // terminal's soft-wrap.
        for ch in text.chars() {
            match ch {
                '\n' => {
                    self.flush_stream_word(&mut out, max_w);
                    let _ = write!(out, "\r\n");
                    self.at_line_start.set(true);
                    self.stream_col.set(0);
                    self.stream_spaces.set(0);
                }
                ' ' | '\t' => {
                    self.flush_stream_word(&mut out, max_w);
                    self.stream_spaces.set(self.stream_spaces.get() + 1);
                }
                _ => {
                    self.stream_word.borrow_mut().push(ch);
                }
            }
        }
        let _ = out.flush();
    }

    fn stream_suspend(&self) {
        // The state machine just confirmed a tool call. Flush whatever is
        // still buffered in the word-wrap tail so the last word of the
        // reasoning reaches the screen now — otherwise it'd hang until the
        // whole (hidden) tool-XML body finishes streaming. Then drop to a
        // fresh line and start a spinner so the user sees progress instead
        // of a silent terminal while the model emits tool args.
        let mut out = std::io::stderr();
        self.flush_stream_word(&mut out, max_content_width());
        if !self.at_line_start.get() {
            let _ = write!(out, "\r\n");
            self.at_line_start.set(true);
            self.stream_col.set(0);
            self.stream_spaces.set(0);
        }
        let _ = out.flush();
        // The streamed body is PAD-indented, so the spinner that follows
        // should be too — clear first_spinner so start_spinner emits the PAD.
        self.first_spinner.set(false);
        self.start_spinner("preparing tool call...");
        self.suspend_spinner.set(true);
    }

    fn stream_end(&self) {
        // Flush any half-emitted word still pending from the last chunk,
        // then make sure we leave the cursor at column 0 of a fresh line so
        // the caller (show_token_usage, show_auto_tool, etc.) starts cleanly.
        // Skip the trailing CR+LF when the last chunk already left us at
        // column 0 — otherwise we'd inject an extra blank line above the
        // tool call or status rule.
        // The horizontal rule is drawn by show_token_usage, not here — that
        // keeps the rule+status pair together for both streamed final
        // answers and tool-call iterations where show_token_usage runs
        // after the tool output.
        if self.suspend_spinner.replace(false) {
            self.stop_spinner();
        }
        let mut out = std::io::stderr();
        self.flush_stream_word(&mut out, max_content_width());
        if !self.at_line_start.get() {
            let _ = write!(out, "\r\n");
        }
        let _ = out.flush();
    }

    fn show_token_usage(
        &self,
        usage: &TokenUsage,
        model: &str,
        _final_answer: bool,
        tool_calls: u32,
        elapsed: Duration,
        context_pct: u8,
        memory: &str,
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
        let cache_str = if usage.cache_read_input_tokens > 0 {
            format!(" ({}⚡)", usage.cache_read_input_tokens)
        } else {
            String::new()
        };
        let short_mem = memory.strip_suffix("-term").unwrap_or(memory);
        let text = format!(
            "{display_model} · {}↑{cache_str} · {}↓ · {} tool(s){cost_str} · {:.1}s · ctx {context_pct}% · ⛁ {short_mem}",
            usage.input_tokens,
            usage.output_tokens,
            tool_calls,
            elapsed.as_secs_f64(),
        );
        // Rule-above-status so every iteration (streamed final answer or
        // tool-call block) ends in the same visual shape.
        Self::status_rule();
        eprintln!("{PAD}{}", text.with(Color::Green).attribute(Attribute::Dim));
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
        // End-of-turn summary gets its own rule above and a distinct color
        // (Cyan) so it reads differently from the per-call status lines
        // (Green). Same dim attribute keeps it quiet in the terminal.
        Self::status_rule();
        eprintln!("{PAD}{}", text.with(Color::Cyan).attribute(Attribute::Dim));
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
