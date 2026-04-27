//! `/plugins` REPL menu and `--list-plugins` CLI helper.
//!
//! The plugin catalogue is built once at startup; this module only
//! reads it. `view manifest` reaches into [`crate::plugins`] for path
//! info, `enable` / `disable` toggle the `AICTL_PLUGINS_ENABLED` config
//! key (a restart is required to pick up newly dropped-in plugins —
//! we don't hot-reload).

use std::io::Write;

use crossterm::style::{Color, Stylize};

use crate::config::{config_set, config_unset};
use crate::plugins::{self, Plugin};

use super::menu::{build_simple_menu_lines, confirm_yn, select_from_menu};

const PLUGINS_MENU_ITEMS: &[(&str, &str)] = &[
    ("view all plugins", "browse manifest, location, status"),
    ("enable plugins", "set AICTL_PLUGINS_ENABLED=true and exit"),
    ("disable plugins", "unset AICTL_PLUGINS_ENABLED and exit"),
    (
        "show plugins directory",
        "print where plugins are read from",
    ),
];

/// Run the `/plugins` REPL menu.
pub fn run_plugins_menu(show_error: &dyn Fn(&str)) {
    let Some(sel) = select_from_menu(PLUGINS_MENU_ITEMS.len(), 0, |selected| {
        build_simple_menu_lines(PLUGINS_MENU_ITEMS, selected)
    }) else {
        return;
    };
    match PLUGINS_MENU_ITEMS[sel].0 {
        "view all plugins" => view_all_plugins(show_error),
        "enable plugins" => toggle_enabled(true),
        "disable plugins" => toggle_enabled(false),
        "show plugins directory" => print_plugins_dir(),
        _ => {}
    }
}

fn header_status_line() {
    if plugins::enabled() {
        let n = plugins::list().len();
        println!(
            "  {} {} loaded",
            "plugins:".with(Color::Cyan),
            format!("{n}").with(Color::Green)
        );
    } else {
        println!(
            "  {} {} (set AICTL_PLUGINS_ENABLED=true and restart)",
            "plugins:".with(Color::Cyan),
            "disabled".with(Color::Yellow)
        );
    }
}

fn print_plugins_dir() {
    let dir = plugins::plugins_dir();
    println!();
    header_status_line();
    println!(
        "  {} {}",
        "directory:".with(Color::Cyan),
        dir.display().to_string().with(Color::DarkGrey)
    );
    println!();
}

fn toggle_enabled(on: bool) {
    println!();
    if on {
        config_set("AICTL_PLUGINS_ENABLED", "true");
        println!(
            "  {} plugins {}. Restart aictl to pick up plugins on disk.",
            "✓".with(Color::Green),
            "enabled".with(Color::Green)
        );
    } else {
        config_unset("AICTL_PLUGINS_ENABLED");
        println!(
            "  {} plugins {}. Restart aictl to drop them from this session.",
            "✓".with(Color::Green),
            "disabled".with(Color::Yellow)
        );
    }
    println!();
}

fn view_all_plugins(show_error: &dyn Fn(&str)) {
    let plugins_list = plugins::list();
    if plugins_list.is_empty() {
        println!();
        header_status_line();
        if plugins::enabled() {
            println!(
                "  {}",
                "No plugins found in the plugins directory.".with(Color::DarkGrey)
            );
        } else {
            println!(
                "  {}",
                "Enable plugins from the menu to discover them on next startup."
                    .with(Color::DarkGrey)
            );
        }
        println!();
        return;
    }
    loop {
        match select_plugin_action(plugins_list) {
            PluginListAction::Cancel => return,
            PluginListAction::View(i) => view_plugin(&plugins_list[i], show_error),
        }
    }
}

enum PluginListAction {
    View(usize),
    Cancel,
}

fn build_plugins_list_lines(selected: usize, entries: &[Plugin]) -> Vec<String> {
    let max_name = entries.iter().map(|e| e.name.len()).max().unwrap_or(0);
    let mut lines = Vec::new();
    for (i, e) in entries.iter().enumerate() {
        let is_selected = i == selected;
        let padded = format!("{:<max_name$}", e.name);
        let name_styled = if is_selected {
            format!(
                "{}",
                padded
                    .as_str()
                    .with(Color::White)
                    .attribute(crossterm::style::Attribute::Bold)
            )
        } else {
            format!("{}", padded.as_str().with(Color::DarkGrey))
        };
        let desc_styled = format!("{}", e.description.as_str().with(Color::DarkGrey));
        let line = if is_selected {
            format!("  {} {name_styled}  {desc_styled}", "›".with(Color::Cyan))
        } else {
            format!("    {name_styled}  {desc_styled}")
        };
        lines.push(line);
    }
    lines
}

#[allow(clippy::cast_possible_truncation)]
fn select_plugin_action(entries: &[Plugin]) -> PluginListAction {
    use crossterm::{
        cursor,
        event::{self, Event, KeyCode, KeyEventKind},
        execute,
        terminal::{self, ClearType},
    };

    let mut selected: usize = 0;
    let _ = terminal::enable_raw_mode();
    let mut stdout = std::io::stdout();
    let _ = execute!(stdout, cursor::Hide);

    let hint = "↑/↓ navigate · enter/v view · esc cancel";

    let mut lines = build_plugins_list_lines(selected, entries);
    let _ = write!(stdout, "\r\n");
    for line in &lines {
        let _ = write!(stdout, "{line}\r\n");
    }
    let _ = write!(stdout, "\r\n  {}\r\n", hint.with(Color::DarkGrey));
    let _ = stdout.flush();
    let mut rendered = lines.len() + 2;

    let result = loop {
        if !event::poll(std::time::Duration::from_millis(100)).unwrap_or(false) {
            continue;
        }
        let Ok(ev) = event::read() else {
            break PluginListAction::Cancel;
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
                        break PluginListAction::View(selected);
                    }
                }
                KeyCode::Esc => break PluginListAction::Cancel,
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
        lines = build_plugins_list_lines(selected, entries);
        for line in &lines {
            let _ = write!(stdout, "{line}\r\n");
        }
        let _ = write!(stdout, "\r\n  {}\r\n", hint.with(Color::DarkGrey));
        let _ = stdout.flush();
        rendered = lines.len() + 2;
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

fn view_plugin(plugin: &Plugin, show_error: &dyn Fn(&str)) {
    println!();
    println!(
        "  {} {}",
        "plugin:".with(Color::Cyan),
        plugin.name.as_str().with(Color::Magenta)
    );
    println!(
        "  {} {}",
        "description:".with(Color::Cyan),
        plugin.description.as_str().with(Color::DarkGrey)
    );
    println!(
        "  {} {}",
        "directory:  ".with(Color::Cyan),
        plugin.dir.display().to_string().with(Color::DarkGrey)
    );
    println!(
        "  {} {}",
        "entrypoint: ".with(Color::Cyan),
        plugin
            .entrypoint
            .display()
            .to_string()
            .with(Color::DarkGrey)
    );
    println!(
        "  {} {}",
        "confirm:    ".with(Color::Cyan),
        format!("{}", plugin.requires_confirmation).with(Color::DarkGrey)
    );
    if let Some(t) = plugin.timeout_secs {
        println!(
            "  {} {}",
            "timeout:    ".with(Color::Cyan),
            format!("{t}s").with(Color::DarkGrey)
        );
    }
    println!();
    if confirm_yn("show manifest contents?") {
        let manifest_path = plugin.dir.join("plugin.toml");
        match std::fs::read_to_string(&manifest_path) {
            Ok(text) => {
                println!();
                for line in text.lines() {
                    println!("  {}", line.with(Color::DarkGrey));
                }
                println!();
            }
            Err(e) => show_error(&format!("Failed to read manifest: {e}")),
        }
    }
}

/// `--list-plugins` non-interactive output.
pub fn print_plugins_cli() {
    if !plugins::enabled() {
        println!(
            "(plugins disabled — set AICTL_PLUGINS_ENABLED=true in ~/.aictl/config to opt in)"
        );
        return;
    }
    let plugins_list = plugins::list();
    if plugins_list.is_empty() {
        println!("(no plugins found in {})", plugins::plugins_dir().display());
        return;
    }
    let max_name = plugins_list.iter().map(|p| p.name.len()).max().unwrap_or(0);
    for p in plugins_list {
        println!(
            "{:<max_name$}  {}  ({})",
            p.name,
            p.description,
            p.dir.display()
        );
    }
}
