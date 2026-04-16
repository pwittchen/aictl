//! Shared primitives for the interactive slash-command menus: arrow-key
//! selection with viewport scrolling, y/N confirmation, single- and
//! multi-line input readers, and small display helpers. All menus in the
//! sibling command modules are built from these.

use std::io::Write;

use crossterm::style::{Color, Stylize};

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

    // Determine how many menu lines fit in the terminal.
    // Reserve 4 lines: 1 top blank, 1 bottom blank, 1 help text, 1 safety margin.
    let term_height = terminal::size().map_or(24, |(_, h)| h as usize);
    let max_visible = term_height.saturating_sub(4);

    let render = |stdout: &mut std::io::Stdout,
                  lines: &[String],
                  scroll_offset: &mut usize,
                  prev_rendered: usize| {
        // Find the selected line (marked with ›) and keep it in view.
        let selected_line = lines.iter().position(|l| l.contains('›')).unwrap_or(0);
        let total = lines.len();
        let viewport = max_visible.min(total);

        if viewport < total {
            if selected_line < *scroll_offset {
                *scroll_offset = selected_line;
            } else if selected_line >= *scroll_offset + viewport {
                *scroll_offset = selected_line + 1 - viewport;
            }
            // Clamp
            if *scroll_offset + viewport > total {
                *scroll_offset = total - viewport;
            }
        } else {
            *scroll_offset = 0;
        }

        let has_above = *scroll_offset > 0;
        let has_below = *scroll_offset + viewport < total;

        // Clear previous render
        if prev_rendered > 0 {
            let _ = execute!(
                stdout,
                cursor::MoveUp(prev_rendered as u16),
                terminal::Clear(ClearType::FromCursorDown),
            );
        }

        // Scroll indicator above
        if has_above {
            let _ = write!(
                stdout,
                "  {}\r\n",
                format!("↑ {} more", *scroll_offset).with(Color::DarkGrey)
            );
        }

        // Visible lines
        for line in &lines[*scroll_offset..*scroll_offset + viewport] {
            let _ = write!(stdout, "{line}\r\n");
        }

        // Scroll indicator below
        if has_below {
            let remaining = total - (*scroll_offset + viewport);
            let _ = write!(
                stdout,
                "  {}\r\n",
                format!("↓ {remaining} more").with(Color::DarkGrey)
            );
        }

        // Help text
        let _ = write!(
            stdout,
            "\r\n  {}\r\n",
            "↑/↓ navigate · enter select · esc cancel".with(Color::DarkGrey)
        );
        let _ = stdout.flush();

        // Return number of rendered lines for cleanup
        viewport + usize::from(has_above) + usize::from(has_below) + 2 // blank + help text
    };

    // Initial render
    let lines = build_lines(selected);
    let _ = execute!(stdout, cursor::MoveToColumn(0));
    let _ = write!(stdout, "\r\n");
    let mut total_rendered_lines = render(&mut stdout, &lines, &mut scroll_offset, 0);

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
                        cursor::MoveUp(total_rendered_lines as u16),
                        terminal::Clear(ClearType::FromCursorDown),
                        cursor::Show,
                    );
                    let _ = terminal::disable_raw_mode();
                    return Some(selected);
                }
                KeyCode::Esc => {
                    let _ = execute!(
                        stdout,
                        cursor::MoveUp(total_rendered_lines as u16),
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
        total_rendered_lines = render(
            &mut stdout,
            &lines,
            &mut scroll_offset,
            total_rendered_lines,
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
