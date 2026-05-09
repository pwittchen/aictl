//! `/memory` and `/remember` slash-command surface.
//!
//! `/remember <fact>` is a one-shot writer — it goes straight through
//! [`crate::memory::add`] and prints the outcome, mirroring how the
//! `save_memory` tool reports back when the agent calls it.
//!
//! `/memory` opens a small menu: toggle the master switch, browse saved
//! entries, delete one or all. The browse view uses the same arrow-key
//! selector pattern as `/skills` and `/agent` so the UX stays uniform.

use std::io::Write;

use crossterm::style::{Color, Stylize};

use crate::memory::{self, AddOutcome, MemoryEntry};
use crate::session;

use super::menu::{
    build_simple_menu_lines, confirm_yn, menu_viewport_height, render_menu_viewport,
    select_from_menu,
};

/// Handle `/remember <fact>` directly. The REPL passes the trimmed
/// argument; an empty argument prints a usage hint instead of writing
/// an empty entry.
pub fn run_remember(fact: &str, show_error: &dyn Fn(&str)) {
    if session::is_incognito() {
        println!();
        println!(
            "  {} incognito mode: memory is disabled for this session",
            "⚠".with(Color::Yellow)
        );
        println!();
        return;
    }
    let trimmed = fact.trim();
    if trimmed.is_empty() {
        show_error("/remember requires text — e.g. /remember user prefers terse responses");
        return;
    }
    match memory::add(trimmed) {
        AddOutcome::Saved(entry) => {
            println!();
            println!(
                "  {} memory saved: {}",
                "✓".with(Color::Green),
                entry.text.with(Color::DarkGrey)
            );
            println!();
        }
        AddOutcome::Disabled => {
            println!();
            println!(
                "  {} memory is disabled — turn it on via /memory or set AICTL_MEMORY_ENABLED=true",
                "⚠".with(Color::Yellow)
            );
            println!();
        }
        AddOutcome::Empty => {
            show_error("/remember requires a non-empty fact");
        }
        AddOutcome::IoError(e) => {
            show_error(&format!("Failed to save memory: {e}"));
        }
    }
}

const MEMORY_MENU_ITEMS: &[(&str, &str)] = &[
    ("toggle memory", "enable or disable long-term memory"),
    ("view memories", "browse, view, or delete saved memories"),
    ("add memory", "type a new fact to remember"),
    ("clear all", "delete every stored memory"),
];

/// Open the `/memory` interactive menu.
pub fn run_memory_menu(show_error: &dyn Fn(&str)) {
    if session::is_incognito() {
        println!();
        println!(
            "  {} incognito mode: memory functionality is disabled",
            "⚠".with(Color::Yellow)
        );
        println!();
        return;
    }
    let Some(sel) = select_from_menu(MEMORY_MENU_ITEMS.len(), 0, |s| {
        build_simple_menu_lines(MEMORY_MENU_ITEMS, s)
    }) else {
        return;
    };
    match MEMORY_MENU_ITEMS[sel].0 {
        "toggle memory" => toggle_memory(),
        "view memories" => view_memories(show_error),
        "add memory" => add_memory(show_error),
        "clear all" => clear_all(show_error),
        _ => {}
    }
}

fn toggle_memory() {
    let now_on = !memory::enabled();
    memory::set_enabled(now_on);
    println!();
    if now_on {
        println!("  {} memory enabled", "✓".with(Color::Green));
    } else {
        println!(
            "  {} memory disabled (saved entries kept on disk; not loaded into prompts)",
            "✓".with(Color::Yellow)
        );
    }
    println!();
}

fn add_memory(show_error: &dyn Fn(&str)) {
    use super::menu::prompt_line_cancellable;
    let Ok(fact) = prompt_line_cancellable("fact:") else {
        return;
    };
    run_remember(&fact, show_error);
}

fn clear_all(show_error: &dyn Fn(&str)) {
    let entries = memory::load();
    if entries.is_empty() {
        println!();
        println!("  {}", "(no memories to clear)".with(Color::DarkGrey));
        println!();
        return;
    }
    println!();
    if !confirm_yn(&format!("delete ALL {} memories?", entries.len())) {
        return;
    }
    if let Err(e) = memory::clear_all() {
        show_error(&format!("Failed to clear memories: {e}"));
        return;
    }
    println!("  {} all memories cleared", "✓".with(Color::Green));
    println!();
}

fn build_memory_list_lines(selected: usize, entries: &[MemoryEntry]) -> Vec<String> {
    if entries.is_empty() {
        return vec![format!("  {}", "(no memories saved)".with(Color::DarkGrey))];
    }
    let max_idx_width = entries.len().to_string().len();
    let mut lines = Vec::new();
    for (i, e) in entries.iter().enumerate() {
        let is_selected = i == selected;
        let idx = format!("{:>max_idx_width$}.", i + 1);
        let text = if e.text.chars().count() > 100 {
            let prefix: String = e.text.chars().take(99).collect();
            format!("{prefix}…")
        } else {
            e.text.clone()
        };
        let line = if is_selected {
            format!(
                "  {} {} {}",
                "›".with(Color::Cyan),
                idx.with(Color::Cyan),
                text.as_str()
                    .with(Color::White)
                    .attribute(crossterm::style::Attribute::Bold),
            )
        } else {
            format!(
                "    {} {}",
                idx.with(Color::DarkGrey),
                text.as_str().with(Color::DarkGrey),
            )
        };
        lines.push(line);
    }
    lines
}

enum MemoryListAction {
    View(usize),
    Delete(usize),
    Cancel,
}

#[allow(clippy::cast_possible_truncation)]
fn select_memory_from_list(entries: &[MemoryEntry]) -> MemoryListAction {
    use crossterm::{
        cursor,
        event::{self, Event, KeyCode, KeyEventKind},
        execute,
        terminal::{self, ClearType},
    };

    let mut selected: usize = 0;
    let mut scroll_offset: usize = 0;
    let _ = terminal::enable_raw_mode();
    let mut stdout = std::io::stdout();
    let _ = execute!(stdout, cursor::Hide);

    let max_visible = menu_viewport_height();
    let hint = "↑/↓ navigate · enter/v view · d delete · esc back";

    let lines = build_memory_list_lines(selected, entries);
    let _ = write!(stdout, "\r\n");
    let mut rendered = render_menu_viewport(
        &mut stdout,
        &lines,
        &mut scroll_offset,
        0,
        max_visible,
        hint,
    );

    let result = loop {
        if !event::poll(std::time::Duration::from_millis(100)).unwrap_or(false) {
            continue;
        }
        let Ok(ev) = event::read() else {
            break MemoryListAction::Cancel;
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
                KeyCode::Enter | KeyCode::Char('v' | 'V') => {
                    if !entries.is_empty() {
                        break MemoryListAction::View(selected);
                    }
                }
                KeyCode::Char('d' | 'D') => {
                    if !entries.is_empty() {
                        break MemoryListAction::Delete(selected);
                    }
                }
                KeyCode::Esc => break MemoryListAction::Cancel,
                _ => {}
            }
        } else {
            continue;
        }

        let lines = build_memory_list_lines(selected, entries);
        rendered = render_menu_viewport(
            &mut stdout,
            &lines,
            &mut scroll_offset,
            rendered,
            max_visible,
            hint,
        );
    };

    let _ = execute!(
        stdout,
        cursor::MoveUp((rendered + 1) as u16),
        terminal::Clear(ClearType::FromCursorDown),
        cursor::Show,
    );
    let _ = terminal::disable_raw_mode();
    result
}

fn view_memories(show_error: &dyn Fn(&str)) {
    loop {
        let entries = memory::load();
        if entries.is_empty() {
            println!();
            println!(
                "  {}",
                "No memories saved yet. Use /remember <fact> to add one.".with(Color::DarkGrey)
            );
            println!();
            return;
        }
        match select_memory_from_list(&entries) {
            MemoryListAction::Cancel => return,
            MemoryListAction::View(i) => {
                let entry = &entries[i];
                println!();
                println!(
                    "  {} {}",
                    format!("memory #{}:", i + 1).with(Color::Cyan),
                    entry.id.as_str().with(Color::DarkGrey),
                );
                println!();
                for line in entry.text.lines() {
                    println!("  {line}");
                }
                println!();
            }
            MemoryListAction::Delete(i) => {
                let entry = &entries[i];
                let preview: String = entry.text.chars().take(60).collect();
                if !confirm_yn(&format!("delete memory \"{preview}\"?")) {
                    continue;
                }
                if memory::remove(&entry.id) {
                    println!();
                    println!("  {} memory deleted", "✓".with(Color::Green));
                    println!();
                } else {
                    show_error("Failed to delete memory");
                }
            }
        }
    }
}

/// Print all stored memories in non-interactive mode (`--list-memories`).
pub fn print_memories_cli() {
    let entries = memory::load();
    if entries.is_empty() {
        println!("(no memories saved)");
        return;
    }
    let max_idx_width = entries.len().to_string().len();
    for (i, e) in entries.iter().enumerate() {
        println!("{:>max_idx_width$}. {}", i + 1, e.text);
    }
}
