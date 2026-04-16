use std::io::Write;

use crossterm::style::{Color, Stylize};

use crate::Message;

use super::menu::{confirm_yn, format_mtime, format_size, select_from_menu};

const SESSION_ITEMS: &[(&str, &str)] = &[
    ("current session info", "show id, name, messages, size"),
    ("set session name", "assign a readable name"),
    ("view saved sessions", "load or delete saved sessions"),
    ("clear all sessions", "remove all saved sessions"),
];

fn build_session_menu_lines(selected: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let max_name = SESSION_ITEMS
        .iter()
        .map(|(n, _)| n.len())
        .max()
        .unwrap_or(0);
    for (i, (name, desc)) in SESSION_ITEMS.iter().enumerate() {
        let is_selected = i == selected;
        let padded = format!("{:<max_name$}", *name);
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
        let line = if is_selected {
            format!("  {} {name_styled}  {desc_styled}", "›".with(Color::Cyan))
        } else {
            format!("    {name_styled}  {desc_styled}")
        };
        lines.push(line);
    }
    lines
}

fn show_current_session_info(messages_len: usize) {
    let Some((id, name)) = crate::session::current_info() else {
        println!();
        println!("  {} no active session", "✗".with(Color::Red));
        println!();
        return;
    };
    let size = crate::session::current_file_size();
    println!();
    println!("  {} {id}", "id:      ".with(Color::Cyan));
    println!(
        "  {} {}",
        "name:    ".with(Color::Cyan),
        name.as_deref().unwrap_or("(unset)")
    );
    println!("  {} {messages_len}", "messages:".with(Color::Cyan));
    println!("  {} {}", "size:    ".with(Color::Cyan), format_size(size));
    println!();
}

fn set_session_name_interactive(show_error: &dyn Fn(&str)) {
    let Some((id, _)) = crate::session::current_info() else {
        show_error("no active session");
        return;
    };
    print!("  {} ", "enter session name:".with(Color::Cyan));
    let _ = std::io::stdout().flush();
    let mut buf = String::new();
    if std::io::stdin().read_line(&mut buf).is_err() {
        return;
    }
    let name = buf.trim();
    if name.is_empty() {
        println!();
        return;
    }
    match crate::session::set_name(&id, name) {
        Ok(()) => {
            let stored = crate::session::current_info()
                .and_then(|(_, n)| n)
                .unwrap_or_else(|| name.to_string());
            println!();
            println!(
                "  {} session name set to \"{stored}\"",
                "✓".with(Color::Green)
            );
            println!();
        }
        Err(e) => show_error(&format!("Error: {e}")),
    }
}

fn build_saved_sessions_lines(
    selected: usize,
    entries: &[crate::session::SessionEntry],
    current_id: Option<&str>,
) -> Vec<String> {
    if entries.is_empty() {
        return vec![format!("  {}", "(no saved sessions)".with(Color::DarkGrey))];
    }
    let mut lines = Vec::new();
    for (i, e) in entries.iter().enumerate() {
        let is_selected = i == selected;
        let is_current = current_id == Some(e.id.as_str());
        let marker = if is_current { "●" } else { " " };
        let name_part = e
            .name
            .as_deref()
            .map(|n| format!(" [{n}]"))
            .unwrap_or_default();
        let meta = format!(" {} · {}", format_size(e.size), format_mtime(e.mtime));
        let body = format!("{}{}{}", e.id, name_part, meta);
        let styled = if is_selected {
            format!(
                "{} {}",
                marker.with(Color::Green),
                body.with(Color::White)
                    .attribute(crossterm::style::Attribute::Bold)
            )
        } else {
            format!(
                "{} {}",
                marker.with(Color::Green),
                body.with(Color::DarkGrey)
            )
        };
        let line = if is_selected {
            format!("  {} {styled}", "›".with(Color::Cyan))
        } else {
            format!("    {styled}")
        };
        lines.push(line);
    }
    lines
}

enum SavedAction {
    Load(usize),
    Delete(usize),
    Cancel,
}

#[allow(clippy::cast_possible_truncation)]
fn select_saved_session(entries: &[crate::session::SessionEntry]) -> SavedAction {
    use crossterm::{
        cursor,
        event::{self, Event, KeyCode, KeyEventKind},
        execute,
        terminal::{self, ClearType},
    };

    let current_id = crate::session::current_id();
    let mut selected: usize = 0;
    let _ = terminal::enable_raw_mode();
    let mut stdout = std::io::stdout();
    let _ = execute!(stdout, cursor::Hide);

    let mut lines = build_saved_sessions_lines(selected, entries, current_id.as_deref());
    let _ = write!(stdout, "\r\n");
    for line in &lines {
        let _ = write!(stdout, "{line}\r\n");
    }
    let hint = "↑/↓ navigate · l/enter load · d delete · esc cancel";
    let _ = write!(stdout, "\r\n  {}\r\n", hint.with(Color::DarkGrey));
    let _ = stdout.flush();
    let mut rendered = lines.len() + 2;

    let result = loop {
        if !event::poll(std::time::Duration::from_millis(100)).unwrap_or(false) {
            continue;
        }
        let Ok(ev) = event::read() else {
            break SavedAction::Cancel;
        };
        if let Event::Key(key) = ev
            && key.kind == KeyEventKind::Press
        {
            match key.code {
                KeyCode::Up => selected = selected.saturating_sub(1),
                KeyCode::Down => {
                    if !entries.is_empty() && selected + 1 < entries.len() {
                        selected += 1;
                    }
                }
                KeyCode::Enter | KeyCode::Char('l' | 'L') => {
                    if !entries.is_empty() {
                        break SavedAction::Load(selected);
                    }
                }
                KeyCode::Char('d' | 'D') => {
                    if !entries.is_empty() {
                        break SavedAction::Delete(selected);
                    }
                }
                KeyCode::Esc => break SavedAction::Cancel,
                _ => {}
            }
        } else {
            continue;
        }

        let _ = execute!(
            stdout,
            cursor::MoveUp(rendered as u16),
            terminal::Clear(ClearType::FromCursorDown),
        );
        lines = build_saved_sessions_lines(selected, entries, current_id.as_deref());
        for line in &lines {
            let _ = write!(stdout, "{line}\r\n");
        }
        let _ = write!(stdout, "\r\n  {}\r\n", hint.with(Color::DarkGrey));
        let _ = stdout.flush();
        rendered = lines.len() + 2;
    };

    let _ = execute!(
        stdout,
        cursor::MoveUp(rendered as u16),
        terminal::Clear(ClearType::FromCursorDown),
        cursor::Show,
    );
    let _ = terminal::disable_raw_mode();
    result
}

fn view_saved_sessions(messages: &mut Vec<Message>, show_error: &dyn Fn(&str)) -> bool {
    loop {
        let entries = crate::session::list_sessions();
        match select_saved_session(&entries) {
            SavedAction::Cancel => return false,
            SavedAction::Load(i) => {
                let entry = &entries[i];
                let label = entry
                    .name
                    .as_deref()
                    .map_or_else(|| entry.id.clone(), |n| format!("{} ({n})", entry.id));
                if !confirm_yn(&format!("load session {label}?")) {
                    continue;
                }
                match crate::session::load_messages(&entry.id) {
                    Ok(loaded) => {
                        *messages = loaded;
                        crate::session::set_current(entry.id.clone(), entry.name.clone());
                        println!();
                        println!("  {} session loaded: {label}", "✓".with(Color::Green));
                        println!();
                        return true;
                    }
                    Err(e) => {
                        show_error(&format!("Failed to load session: {e}"));
                        return false;
                    }
                }
            }
            SavedAction::Delete(i) => {
                let entry = &entries[i];
                let label = entry
                    .name
                    .as_deref()
                    .map_or_else(|| entry.id.clone(), |n| format!("{} ({n})", entry.id));
                if !confirm_yn(&format!("delete session {label}?")) {
                    continue;
                }
                crate::session::delete_session(&entry.id);
                println!();
                println!("  {} session deleted", "✓".with(Color::Green));
                println!();
            }
        }
    }
}

fn clear_all_sessions_confirm() {
    if !confirm_yn("clear ALL saved sessions?") {
        return;
    }
    crate::session::clear_all();
    // Re-save current session so it persists after clear.
    println!();
    println!("  {} all sessions cleared", "✓".with(Color::Green));
    println!();
}

/// Run the /session menu. Returns true if the conversation messages were replaced
/// (caller should reset context-tracking state).
pub fn run_session_menu(messages: &mut Vec<Message>, show_error: &dyn Fn(&str)) -> bool {
    let Some(sel) = select_from_menu(SESSION_ITEMS.len(), 0, build_session_menu_lines) else {
        return false;
    };
    match sel {
        0 => {
            show_current_session_info(messages.len());
            false
        }
        1 => {
            set_session_name_interactive(show_error);
            false
        }
        2 => view_saved_sessions(messages, show_error),
        3 => {
            clear_all_sessions_confirm();
            false
        }
        _ => false,
    }
}

/// Print saved sessions in non-interactive mode.
pub fn print_sessions_cli() {
    let entries = crate::session::list_sessions();
    if entries.is_empty() {
        println!("(no saved sessions)");
        return;
    }
    for e in &entries {
        let name = e.name.as_deref().unwrap_or("-");
        println!(
            "{}  {}  {}  {}",
            e.id,
            name,
            format_size(e.size),
            format_mtime(e.mtime)
        );
    }
}
