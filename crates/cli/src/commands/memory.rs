use crossterm::style::{Color, Stylize};

use super::menu::select_from_menu;

// Type definition lives in the engine crate alongside the agent-loop
// code that consumes it; this module only adds the interactive picker.
pub use engine::run::MemoryMode;

const MEMORY_MODES: &[(&str, &str)] = &[
    ("long-term", "all messages, no optimization"),
    ("short-term", "sliding window with recent messages"),
];

fn build_memory_menu_lines(selected: usize, current: MemoryMode) -> Vec<String> {
    let mut lines = Vec::new();
    let max_name = MEMORY_MODES.iter().map(|(n, _)| n.len()).max().unwrap_or(0);
    for (i, (name, desc)) in MEMORY_MODES.iter().enumerate() {
        let is_selected = i == selected;
        let is_current = (*name == "long-term" && current == MemoryMode::LongTerm)
            || (*name == "short-term" && current == MemoryMode::ShortTerm);

        let marker = if is_current { "●" } else { " " };
        let padded = format!("{:<max_name$}", *name);
        let name_styled = if is_selected {
            format!(
                "{} {}",
                marker.with(Color::Green),
                padded
                    .with(Color::White)
                    .attribute(crossterm::style::Attribute::Bold)
            )
        } else {
            format!(
                "{} {}",
                marker.with(Color::Green),
                padded.with(Color::DarkGrey)
            )
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

/// Interactively select memory mode with arrow keys.
/// Returns `Some(MemoryMode)` or `None` if cancelled (Esc).
pub fn select_memory(current: MemoryMode) -> Option<MemoryMode> {
    let initial = match current {
        MemoryMode::LongTerm => 0,
        MemoryMode::ShortTerm => 1,
    };
    let selected = select_from_menu(MEMORY_MODES.len(), initial, |sel| {
        build_memory_menu_lines(sel, current)
    })?;
    Some(match MEMORY_MODES[selected].0 {
        "short-term" => MemoryMode::ShortTerm,
        _ => MemoryMode::LongTerm,
    })
}
