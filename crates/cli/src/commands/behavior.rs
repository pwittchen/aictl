use crossterm::style::{Color, Stylize};

use super::menu::select_from_menu;

const BEHAVIORS: &[(&str, &str)] = &[
    (
        "human-in-the-loop",
        "ask confirmation before each tool call",
    ),
    ("auto", "run tools without confirmation"),
];

fn build_behavior_menu_lines(selected: usize, current_auto: bool) -> Vec<String> {
    let mut lines = Vec::new();
    let max_name = BEHAVIORS.iter().map(|(n, _)| n.len()).max().unwrap_or(0);
    for (i, (name, desc)) in BEHAVIORS.iter().enumerate() {
        let is_selected = i == selected;
        let is_current = (*name == "auto") == current_auto;

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

/// Interactively select auto/human-in-the-loop behavior with arrow keys.
/// Returns `Some(auto_bool)` or `None` if cancelled (Esc).
pub fn select_behavior(current_auto: bool) -> Option<bool> {
    let initial = usize::from(current_auto);
    let selected = select_from_menu(BEHAVIORS.len(), initial, |sel| {
        build_behavior_menu_lines(sel, current_auto)
    })?;
    Some(BEHAVIORS[selected].0 == "auto")
}
