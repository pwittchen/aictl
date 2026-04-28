//! `/mcp` REPL menu and `--list-mcp` CLI helper.
//!
//! The MCP catalogue is built once at startup; this module only reads it.
//! `view all` lets the user browse the per-server tool list with schemas;
//! `enable` / `disable` toggle the `AICTL_MCP_ENABLED` master switch (a
//! restart is required to spawn newly-enabled servers — the catalogue is
//! not hot-reloaded).

use std::fmt::Write as _;
use std::io::Write;

use crossterm::style::{Color, Stylize};

use crate::config::{config_set, config_unset};
use crate::mcp::{self, ServerState, ServerSummary};

use super::menu::{build_simple_menu_lines, select_from_menu};

const MCP_MENU_ITEMS: &[(&str, &str)] = &[
    ("view all servers", "browse servers, tools, schemas, status"),
    ("enable mcp", "set AICTL_MCP_ENABLED=true and exit"),
    ("disable mcp", "unset AICTL_MCP_ENABLED and exit"),
    ("show config path", "print where mcp.json is read from"),
];

/// Run the `/mcp` REPL menu.
pub fn run_mcp_menu(_show_error: &dyn Fn(&str)) {
    let Some(sel) = select_from_menu(MCP_MENU_ITEMS.len(), 0, |selected| {
        build_simple_menu_lines(MCP_MENU_ITEMS, selected)
    }) else {
        return;
    };
    match MCP_MENU_ITEMS[sel].0 {
        "view all servers" => view_all(),
        "enable mcp" => toggle_enabled(true),
        "disable mcp" => toggle_enabled(false),
        "show config path" => print_config_path(),
        _ => {}
    }
}

fn header_status_line() {
    if mcp::enabled() {
        let n_servers = mcp::list().len();
        let n_tools = mcp::total_tools();
        let n_failed = mcp::failed_count();
        let mut summary = format!("{n_servers} configured, {n_tools} tools");
        if n_failed > 0 {
            let _ = write!(summary, ", {n_failed} failed");
        }
        println!(
            "  {} {}",
            "mcp:".with(Color::Cyan),
            summary.with(Color::Green)
        );
    } else {
        println!(
            "  {} {} (set AICTL_MCP_ENABLED=true and restart)",
            "mcp:".with(Color::Cyan),
            "disabled".with(Color::Yellow)
        );
    }
}

fn print_config_path() {
    let path = mcp::config::config_path();
    println!();
    header_status_line();
    println!(
        "  {} {}",
        "config:".with(Color::Cyan),
        path.display().to_string().with(Color::DarkGrey)
    );
    println!();
}

fn toggle_enabled(on: bool) {
    println!();
    if on {
        config_set("AICTL_MCP_ENABLED", "true");
        println!(
            "  {} mcp {}. Restart aictl to spawn configured servers.",
            "✓".with(Color::Green),
            "enabled".with(Color::Green)
        );
    } else {
        config_unset("AICTL_MCP_ENABLED");
        println!(
            "  {} mcp {}. Restart aictl to drop them from this session.",
            "✓".with(Color::Green),
            "disabled".with(Color::Yellow)
        );
    }
    println!();
}

fn view_all() {
    let servers = mcp::list();
    if servers.is_empty() {
        println!();
        header_status_line();
        if mcp::enabled() {
            println!(
                "  {}",
                "No servers configured. Drop entries into ~/.aictl/mcp.json.".with(Color::DarkGrey)
            );
        } else {
            println!(
                "  {}",
                "Enable mcp from the menu and add entries to ~/.aictl/mcp.json."
                    .with(Color::DarkGrey)
            );
        }
        println!();
        return;
    }
    loop {
        match select_server(&servers) {
            ServerListAction::Cancel => return,
            ServerListAction::View(i) => view_server(&servers[i]),
        }
    }
}

enum ServerListAction {
    View(usize),
    Cancel,
}

fn state_label(state: &ServerState) -> (String, Color) {
    match state {
        ServerState::Ready => ("ready".to_string(), Color::Green),
        ServerState::Disabled => ("disabled".to_string(), Color::DarkGrey),
        ServerState::Failed(reason) => (format!("failed ({reason})"), Color::Red),
    }
}

fn build_server_lines(selected: usize, entries: &[ServerSummary]) -> Vec<String> {
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
        let (state_text, state_color) = state_label(&e.state);
        let state_styled = format!("{}", state_text.with(state_color));
        let count_styled = format!(
            "{}",
            format!("{} tools", e.tools.len()).with(Color::DarkGrey)
        );
        let line = if is_selected {
            format!(
                "  {} {name_styled}  {state_styled}  {count_styled}",
                "›".with(Color::Cyan)
            )
        } else {
            format!("    {name_styled}  {state_styled}  {count_styled}")
        };
        lines.push(line);
    }
    lines
}

#[allow(clippy::cast_possible_truncation)]
fn select_server(entries: &[ServerSummary]) -> ServerListAction {
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

    let mut lines = build_server_lines(selected, entries);
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
            break ServerListAction::Cancel;
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
                        break ServerListAction::View(selected);
                    }
                }
                KeyCode::Esc => break ServerListAction::Cancel,
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
        lines = build_server_lines(selected, entries);
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

fn view_server(server: &ServerSummary) {
    println!();
    println!(
        "  {} {}",
        "server:".with(Color::Cyan),
        server.name.as_str().with(Color::Magenta)
    );
    let cmd_line = if server.args.is_empty() {
        server.command.clone()
    } else {
        format!("{} {}", server.command, server.args.join(" "))
    };
    println!(
        "  {} {}",
        "command:".with(Color::Cyan),
        cmd_line.with(Color::DarkGrey)
    );
    let (state_text, state_color) = state_label(&server.state);
    println!(
        "  {} {}",
        "state:  ".with(Color::Cyan),
        state_text.with(state_color)
    );
    if matches!(server.state, ServerState::Ready) && !server.tools.is_empty() {
        println!();
        println!("  {}", "tools:".with(Color::Cyan));
        for tool in &server.tools {
            let qualified = mcp::qualify(&server.name, &tool.name);
            println!(
                "    {} — {}",
                qualified.with(Color::Cyan),
                tool.description.as_str().with(Color::DarkGrey)
            );
        }
    }
    println!();
}

/// `--list-mcp` non-interactive output.
pub fn print_mcp_cli() {
    if !mcp::enabled() {
        println!("(mcp disabled — set AICTL_MCP_ENABLED=true in ~/.aictl/config to opt in)");
        return;
    }
    let servers = mcp::list();
    if servers.is_empty() {
        println!(
            "(no servers configured in {})",
            mcp::config::config_path().display()
        );
        return;
    }
    let max_name = servers.iter().map(|s| s.name.len()).max().unwrap_or(0);
    for s in &servers {
        let (state_text, _) = state_label(&s.state);
        println!(
            "{:<max_name$}  {state_text:<12}  {} tools  ({})",
            s.name,
            s.tools.len(),
            s.command,
        );
        for t in &s.tools {
            println!("  - {}: {}", mcp::qualify(&s.name, &t.name), t.description);
        }
    }
}
