//! Shared primitives for the interactive slash-command menus: arrow-key
//! selection with viewport scrolling, y/N confirmation, single- and
//! multi-line input readers, and small display helpers. All menus in the
//! sibling command modules are built from these.

use std::io::Write;

use crossterm::style::{Color, Stylize};

/// Visible terminal width in columns, falling back to 80 when the size is
/// unavailable (non-tty, no kernel support, etc.). Menu rendering pre-clips
/// every line to this width so wrapped output never throws off the
/// `MoveUp` cleanup math used between redraws.
pub(super) fn term_width() -> usize {
    crossterm::terminal::size().map_or(80, |(w, _)| w as usize)
}

/// Number of menu rows that fit in the terminal once the scroll indicators
/// (`↑/↓ N more`) and the help line are accounted for. Falls back to a
/// 24-row terminal so the menu still renders something usable when the
/// kernel can't report a size.
pub(super) fn menu_viewport_height() -> usize {
    let h = crossterm::terminal::size().map_or(24, |(_, h)| h as usize);
    h.saturating_sub(4)
}

/// Count visible columns in a string, skipping ANSI CSI escape sequences
/// (the `\x1b[...<letter>` color/attribute codes the menu uses). Treats every
/// non-escape `char` as one column — wide CJK and emoji aren't common in the
/// menus and trying to be exact would drag in `unicode-width`.
fn visible_width(s: &str) -> usize {
    let mut count = 0usize;
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // Skip to the terminator (an alpha char ends a CSI sequence).
            for nc in chars.by_ref() {
                if nc.is_ascii_alphabetic() {
                    break;
                }
            }
            continue;
        }
        count += 1;
    }
    count
}

/// Truncate a styled string so its visible width fits in `max` columns. ANSI
/// escape sequences pass through unchanged; only printable chars are counted.
/// When truncation occurs, appends `…\x1b[0m` so the line ends with a reset.
/// Returns an empty string when `max < 2` because the ellipsis itself needs
/// one column and nothing meaningful fits.
pub(super) fn truncate_to_width(line: &str, max: usize) -> String {
    if max < 2 {
        return String::new();
    }
    if visible_width(line) <= max {
        return line.to_string();
    }
    let limit = max - 1;
    let mut out = String::with_capacity(line.len());
    let mut count = 0usize;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            out.push(c);
            for nc in chars.by_ref() {
                out.push(nc);
                if nc.is_ascii_alphabetic() {
                    break;
                }
            }
            continue;
        }
        if count >= limit {
            break;
        }
        out.push(c);
        count += 1;
    }
    out.push('…');
    out.push_str("\x1b[0m");
    out
}

/// Render `lines` into `stdout` with viewport scrolling, scroll indicators,
/// and a help line, returning the number of physical rows written so the
/// caller can pass it back as `prev_rendered` on the next redraw. Each line
/// is pre-clipped to terminal width so wrapping can't desync the
/// `MoveUp(prev_rendered)` cleanup. The currently-selected entry is
/// detected by the `›` glyph — every menu in this codebase marks selection
/// the same way.
#[allow(clippy::cast_possible_truncation)]
pub(super) fn render_menu_viewport(
    stdout: &mut std::io::Stdout,
    lines: &[String],
    scroll_offset: &mut usize,
    prev_rendered: usize,
    max_visible: usize,
    help_text: &str,
) -> usize {
    use crossterm::{
        cursor, execute,
        terminal::{self, ClearType},
    };

    let cols = term_width();
    let selected_line = lines.iter().position(|l| l.contains('›')).unwrap_or(0);
    let total = lines.len();
    let viewport = max_visible.min(total);

    if viewport < total {
        if selected_line < *scroll_offset {
            *scroll_offset = selected_line;
        } else if selected_line >= *scroll_offset + viewport {
            *scroll_offset = selected_line + 1 - viewport;
        }
        if *scroll_offset + viewport > total {
            *scroll_offset = total - viewport;
        }
    } else {
        *scroll_offset = 0;
    }

    let has_above = *scroll_offset > 0;
    let has_below = *scroll_offset + viewport < total;

    if prev_rendered > 0 {
        let _ = execute!(
            stdout,
            cursor::MoveUp(prev_rendered as u16),
            terminal::Clear(ClearType::FromCursorDown),
        );
    }

    if has_above {
        let line = format!(
            "  {}",
            format!("↑ {} more", *scroll_offset).with(Color::DarkGrey)
        );
        let _ = write!(stdout, "{}\r\n", truncate_to_width(&line, cols));
    }
    for line in &lines[*scroll_offset..*scroll_offset + viewport] {
        let _ = write!(stdout, "{}\r\n", truncate_to_width(line, cols));
    }
    if has_below {
        let remaining = total - (*scroll_offset + viewport);
        let line = format!("  {}", format!("↓ {remaining} more").with(Color::DarkGrey));
        let _ = write!(stdout, "{}\r\n", truncate_to_width(&line, cols));
    }
    let help_line = format!("  {}", help_text.with(Color::DarkGrey));
    let _ = write!(stdout, "\r\n{}\r\n", truncate_to_width(&help_line, cols));
    let _ = stdout.flush();

    viewport + usize::from(has_above) + usize::from(has_below) + 2
}

pub(super) fn build_simple_menu_lines(items: &[(&str, &str)], selected: usize) -> Vec<String> {
    let max_name = items.iter().map(|(n, _)| n.len()).max().unwrap_or(0);
    items
        .iter()
        .enumerate()
        .map(|(i, (name, desc))| {
            let is_selected = i == selected;
            let padded = format!("{name:<max_name$}");
            let name_styled = if is_selected {
                format!(
                    "  {}",
                    padded
                        .with(Color::White)
                        .attribute(crossterm::style::Attribute::Bold)
                )
            } else {
                format!("  {}", padded.with(Color::DarkGrey))
            };
            let desc_styled = format!("{}", desc.with(Color::DarkGrey));
            if is_selected {
                format!("  {} {name_styled}  {desc_styled}", "›".with(Color::Cyan))
            } else {
                format!("    {name_styled}  {desc_styled}")
            }
        })
        .collect()
}

/// Generic arrow-key menu selector with viewport scrolling.
/// `item_count` is the number of selectable items, `initial_selected` is the
/// starting index, and `build_lines` returns the display lines for a given
/// selected index.  Returns `Some(selected_index)` or `None` if cancelled.
#[allow(clippy::cast_possible_truncation, clippy::too_many_lines)]
pub(super) fn select_from_menu<F>(
    item_count: usize,
    initial_selected: usize,
    build_lines: F,
) -> Option<usize>
where
    F: Fn(usize) -> Vec<String>,
{
    use crossterm::{
        cursor,
        event::{self, Event, KeyCode, KeyEventKind},
        execute,
        terminal::{self, ClearType},
    };

    let mut selected = initial_selected;
    let mut scroll_offset: usize = 0;

    let _ = terminal::enable_raw_mode();
    let mut stdout = std::io::stdout();
    let _ = execute!(stdout, cursor::Hide);

    let max_visible = menu_viewport_height();
    let help = "↑/↓ navigate · enter select · esc cancel";

    // Initial render: a leading `\r\n` reserves a blank row above the menu so
    // the cleanup `MoveUp(rendered + 1)` lands the cursor exactly where it
    // started. `MoveToColumn(0)` guards against rendering mid-prompt.
    let lines = build_lines(selected);
    let _ = execute!(stdout, cursor::MoveToColumn(0));
    let _ = write!(stdout, "\r\n");
    let mut total_rendered_lines = render_menu_viewport(
        &mut stdout,
        &lines,
        &mut scroll_offset,
        0,
        max_visible,
        help,
    );

    loop {
        if !event::poll(std::time::Duration::from_millis(100)).unwrap_or(false) {
            continue;
        }
        let Ok(ev) = event::read() else { break };

        match ev {
            Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                KeyCode::Up => {
                    selected = selected.saturating_sub(1);
                }
                KeyCode::Down => {
                    if selected + 1 < item_count {
                        selected += 1;
                    }
                }
                KeyCode::Enter => {
                    let _ = execute!(
                        stdout,
                        cursor::MoveUp((total_rendered_lines + 1) as u16),
                        terminal::Clear(ClearType::FromCursorDown),
                        cursor::Show,
                    );
                    let _ = terminal::disable_raw_mode();
                    return Some(selected);
                }
                KeyCode::Esc => {
                    let _ = execute!(
                        stdout,
                        cursor::MoveUp((total_rendered_lines + 1) as u16),
                        terminal::Clear(ClearType::FromCursorDown),
                        cursor::Show,
                    );
                    let _ = terminal::disable_raw_mode();
                    return None;
                }
                _ => continue,
            },
            _ => continue,
        }

        let lines = build_lines(selected);
        total_rendered_lines = render_menu_viewport(
            &mut stdout,
            &lines,
            &mut scroll_offset,
            total_rendered_lines,
            max_visible,
            help,
        );
    }

    let _ = execute!(stdout, cursor::Show);
    let _ = terminal::disable_raw_mode();
    None
}

/// Prompt for a y/N confirmation. Returns true if user pressed y.
pub(super) fn confirm_yn(prompt: &str) -> bool {
    use crossterm::event::{self, Event, KeyCode, KeyEventKind};
    use crossterm::terminal;
    print!(
        "  {} {} ",
        prompt.with(Color::Yellow),
        "(y/N):".with(Color::DarkGrey)
    );
    let _ = std::io::stdout().flush();
    let _ = terminal::enable_raw_mode();
    let mut answer = false;
    loop {
        if !event::poll(std::time::Duration::from_millis(200)).unwrap_or(false) {
            continue;
        }
        if let Ok(Event::Key(key)) = event::read()
            && key.kind == KeyEventKind::Press
        {
            match key.code {
                KeyCode::Char('y' | 'Y') => {
                    answer = true;
                    break;
                }
                KeyCode::Char('n' | 'N') | KeyCode::Esc | KeyCode::Enter => break,
                _ => {}
            }
        }
    }
    let _ = terminal::disable_raw_mode();
    println!();
    answer
}

/// Read a line from stdin with a prompt. Returns None if Esc pressed (via raw mode detection)
/// or empty input. Masks input when `masked` is true.
pub(super) fn read_input_line(prompt: &str, masked: bool) -> Option<String> {
    use crossterm::event::{self, Event, KeyCode, KeyEventKind};
    use crossterm::terminal;

    print!("  {} ", prompt.with(Color::Cyan));
    let _ = std::io::stdout().flush();

    let _ = terminal::enable_raw_mode();
    let mut buf = String::new();
    let result = loop {
        if !event::poll(std::time::Duration::from_millis(200)).unwrap_or(false) {
            continue;
        }
        if let Ok(Event::Key(key)) = event::read()
            && key.kind == KeyEventKind::Press
        {
            match key.code {
                KeyCode::Esc => break None,
                KeyCode::Enter => break Some(buf.clone()),
                KeyCode::Backspace => {
                    if buf.pop().is_some() {
                        print!("\x08 \x08");
                        let _ = std::io::stdout().flush();
                    }
                }
                KeyCode::Char(c) => {
                    buf.push(c);
                    if masked {
                        print!("*");
                    } else {
                        print!("{c}");
                    }
                    let _ = std::io::stdout().flush();
                }
                _ => {}
            }
        }
    };
    let _ = terminal::disable_raw_mode();
    println!();
    result
}

/// Cancellable single-line prompt.
///
/// Returns `Ok(text)` when the user presses Enter (text may be empty) or
/// `Err(())` when the user presses Esc or Ctrl+C. Backspace deletes.
pub(super) fn prompt_line_cancellable(prompt: &str) -> Result<String, ()> {
    use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
    use crossterm::terminal;

    print!("  {} ", prompt.with(Color::Cyan));
    let _ = std::io::stdout().flush();

    let _ = terminal::enable_raw_mode();
    let mut buf = String::new();
    let result: Result<String, ()> = loop {
        if !event::poll(std::time::Duration::from_millis(200)).unwrap_or(false) {
            continue;
        }
        let Ok(ev) = event::read() else {
            break Err(());
        };
        if let Event::Key(key) = ev
            && key.kind == KeyEventKind::Press
        {
            match key.code {
                KeyCode::Esc => break Err(()),
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    break Err(());
                }
                KeyCode::Enter => break Ok(buf.clone()),
                KeyCode::Backspace => {
                    if buf.pop().is_some() {
                        // Erase last character on screen.
                        print!("\u{8} \u{8}");
                        let _ = std::io::stdout().flush();
                    }
                }
                KeyCode::Char(c) => {
                    buf.push(c);
                    print!("{c}");
                    let _ = std::io::stdout().flush();
                }
                _ => {}
            }
        }
    };
    let _ = terminal::disable_raw_mode();
    println!();
    result
}

/// Read multi-line input. Ctrl+D finishes input, Esc cancels.
/// Supports bracketed paste mode so pasted text is received as a single event.
pub(super) fn read_multiline_input() -> Option<String> {
    read_multiline_input_prefilled("")
}

/// Read multi-line input with optional pre-filled content.
/// The initial text is displayed and editable. Ctrl+D finishes, Esc cancels.
pub(super) fn read_multiline_input_prefilled(initial: &str) -> Option<String> {
    use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
    use crossterm::terminal;

    print!("  ");
    let _ = std::io::stdout().flush();

    let _ = terminal::enable_raw_mode();
    let _ = crossterm::execute!(std::io::stdout(), event::EnableBracketedPaste);
    let mut buf = String::new();

    // Pre-fill buffer and display initial content
    if !initial.is_empty() {
        buf.push_str(initial);
        for ch in initial.chars() {
            if ch == '\n' {
                print!("\r\n  ");
            } else if ch == '\t' {
                print!("    ");
            } else {
                print!("{ch}");
            }
        }
        let _ = std::io::stdout().flush();
    }

    let result = loop {
        if !event::poll(std::time::Duration::from_millis(200)).unwrap_or(false) {
            continue;
        }
        match event::read() {
            Ok(Event::Paste(text)) => {
                let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
                buf.push_str(&normalized);
                for ch in normalized.chars() {
                    if ch == '\n' {
                        print!("\r\n  ");
                    } else if ch == '\t' {
                        print!("    ");
                    } else {
                        print!("{ch}");
                    }
                }
                let _ = std::io::stdout().flush();
            }
            Ok(Event::Key(key)) if key.kind == KeyEventKind::Press => match key.code {
                KeyCode::Esc => break None,
                KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    break Some(buf.clone());
                }
                KeyCode::Enter => {
                    buf.push('\n');
                    print!("\r\n  ");
                    let _ = std::io::stdout().flush();
                }
                KeyCode::Backspace => {
                    if buf.pop().is_some() {
                        print!("\x08 \x08");
                        let _ = std::io::stdout().flush();
                    }
                }
                KeyCode::Tab => {
                    buf.push('\t');
                    print!("    ");
                    let _ = std::io::stdout().flush();
                }
                KeyCode::Char(c) => {
                    buf.push(c);
                    print!("{c}");
                    let _ = std::io::stdout().flush();
                }
                _ => {}
            },
            _ => {}
        }
    };
    let _ = crossterm::execute!(std::io::stdout(), event::DisableBracketedPaste);
    let _ = terminal::disable_raw_mode();
    println!();
    result
}

pub(super) fn show_cancelled() {
    println!();
    println!("  {} cancelled", "✗".with(Color::Yellow));
    println!();
}

pub(super) fn format_size(bytes: u64) -> String {
    #[allow(clippy::cast_precision_loss)]
    if bytes >= 1_048_576 {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

pub(super) fn format_mtime(mtime: std::time::SystemTime) -> String {
    let secs = mtime
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let diff = now.saturating_sub(secs);
    if diff < 60 {
        format!("{diff}s ago")
    } else if diff < 3600 {
        format!("{}m ago", diff / 60)
    } else if diff < 86400 {
        format!("{}h ago", diff / 3600)
    } else {
        format!("{}d ago", diff / 86400)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visible_width_skips_ansi() {
        assert_eq!(visible_width("hello"), 5);
        assert_eq!(visible_width("\x1b[31mhello\x1b[0m"), 5);
        assert_eq!(visible_width("\x1b[1;32mfoo\x1b[0mbar"), 6);
        assert_eq!(visible_width(""), 0);
    }

    #[test]
    fn truncate_to_width_passes_short_strings_through() {
        assert_eq!(truncate_to_width("hello", 10), "hello");
        assert_eq!(truncate_to_width("hello", 5), "hello");
    }

    #[test]
    fn truncate_to_width_clips_long_strings_with_ellipsis() {
        let result = truncate_to_width("hello world", 8);
        // 7 visible chars + '…' + reset
        assert_eq!(result, "hello w…\x1b[0m");
    }

    #[test]
    fn truncate_to_width_preserves_ansi_in_kept_prefix() {
        // Color codes don't count toward visible width.
        let styled = "\x1b[31mhello world\x1b[0m";
        let result = truncate_to_width(styled, 8);
        assert_eq!(result, "\x1b[31mhello w…\x1b[0m");
    }

    #[test]
    fn truncate_to_width_below_two_returns_empty() {
        assert_eq!(truncate_to_width("anything", 0), "");
        assert_eq!(truncate_to_width("anything", 1), "");
    }

    #[test]
    fn truncate_to_width_two_keeps_one_char_plus_ellipsis() {
        let result = truncate_to_width("hello", 2);
        assert_eq!(result, "h…\x1b[0m");
    }
}
