//! `/coding` slash command — toggles coding-agent mode and reports
//! state. The legacy names `/coding-agent` and `/coding_agent` route
//! here too (see `commands.rs`); only `/coding` is documented.
//!
//! Two entry shapes:
//!   * `/coding` (bare) → opens an interactive menu with the current
//!     state and a toggle action. The discoverability path, matching
//!     `/memory`'s shape so the two surfaces feel the same.
//!   * `/coding on | off | toggle | status` → scripted subcommands that
//!     act immediately and print a one-line confirmation. Useful for
//!     users who already know what they want, and for documenting in
//!     `/help`-style cheat sheets.
//!
//! Both paths persist through [`crate::config::config_set`] so the new
//! value survives across launches and is visible to the desktop app on
//! next launch. The in-memory cache is updated by the same call, so
//! the very next `run::run_agent_turn` re-reads the new value via
//! `build_system_prompt`.

use crossterm::style::{Color, Stylize};

use crate::config::{AICTL_CODING_AGENT, coding_agent_enabled, config_set};

use super::menu::{build_simple_menu_lines, select_from_menu};

/// What the user asked us to do with `/coding`.
pub enum Action {
    /// No argument — open the interactive menu.
    Menu,
    /// `status` / `show` — print current state without opening a menu.
    Show,
    SetOn,
    SetOff,
    Toggle,
}

/// Parse the argument portion of a `/coding` invocation. Empty arg
/// opens the menu; explicit subcommands act directly. Unknown args
/// print usage.
pub fn parse_action(args: &str) -> Result<Action, String> {
    match args.trim().to_ascii_lowercase().as_str() {
        "" => Ok(Action::Menu),
        "status" | "show" => Ok(Action::Show),
        "on" | "enable" | "enabled" | "true" => Ok(Action::SetOn),
        "off" | "disable" | "disabled" | "false" => Ok(Action::SetOff),
        "toggle" => Ok(Action::Toggle),
        other => Err(format!(
            "/coding: unknown argument '{other}' (expected on / off / toggle / status)"
        )),
    }
}

/// Handle the `/coding` slash command. Returns once any printing is
/// done — no message-state mutation, the next turn picks up the new
/// system prompt automatically via `build_system_prompt`.
pub fn run(args: &str, show_error: &dyn Fn(&str)) {
    let action = match parse_action(args) {
        Ok(a) => a,
        Err(msg) => {
            show_error(&msg);
            return;
        }
    };

    let current = coding_agent_enabled();
    let next = match action {
        Action::Menu => {
            run_menu();
            return;
        }
        Action::Show => {
            print_status(current);
            return;
        }
        Action::SetOn => true,
        Action::SetOff => false,
        Action::Toggle => !current,
    };

    apply(current, next);
}

/// Open the interactive menu. Renders the current state above an
/// arrow-key list with a single toggle action plus a Cancel row, then
/// applies the flip when the user hits Enter on the toggle. Esc /
/// Cancel both leave the state untouched.
fn run_menu() {
    let enabled = coding_agent_enabled();
    let state_label = if enabled { "ON" } else { "OFF" };
    let toggle_label = if enabled {
        "turn off coding-agent mode"
    } else {
        "turn on coding-agent mode"
    };
    let toggle_desc = if enabled {
        "next turn reverts to the general-purpose system prompt"
    } else {
        "next turn uses the coding-specialist system prompt"
    };

    println!();
    let coloured_state = if enabled {
        state_label.with(Color::Green).to_string()
    } else {
        state_label.with(Color::DarkGrey).to_string()
    };
    println!(
        "  {} coding-agent is currently {coloured_state} {}",
        "·".with(Color::Cyan),
        "[experimental]".with(Color::Yellow),
    );
    print_experimental_note();

    let items: Vec<(&str, &str)> = vec![(toggle_label, toggle_desc), ("cancel", "leave as-is")];
    let Some(sel) = select_from_menu(items.len(), 0, |s| build_simple_menu_lines(&items, s)) else {
        return;
    };
    if sel == 0 {
        apply(enabled, !enabled);
    }
}

/// One-liner shown above the menu and the status display so the user
/// sees the experimental warning whenever they touch the slash command.
/// Production-grade alternatives are listed so a user who hits the mode's
/// limits can switch without further hunting.
fn print_experimental_note() {
    println!(
        "  {} for production coding work prefer Claude Code, OpenAI Codex, or opencode",
        "→".with(Color::DarkGrey),
    );
}

/// Persist `next` and print a confirmation line. Called by both the
/// subcommand path (`/coding on`) and the menu's toggle action so the
/// user-visible feedback stays in one place.
fn apply(current: bool, next: bool) {
    config_set(AICTL_CODING_AGENT, if next { "true" } else { "false" });

    println!();
    if next == current {
        let same_label = if next { "on" } else { "off" };
        println!(
            "  {} coding-agent already {same_label}",
            "·".with(Color::DarkGrey),
        );
    } else {
        let label = if next {
            "on".with(Color::Green).to_string()
        } else {
            "off".with(Color::DarkGrey).to_string()
        };
        let glyph = if next {
            "✓".with(Color::Green).to_string()
        } else {
            "·".with(Color::DarkGrey).to_string()
        };
        println!("  {glyph} coding-agent: {label}");
    }
    println!(
        "  {} {}",
        "→".with(Color::Cyan),
        "the next turn uses the new base system prompt".with(Color::DarkGrey)
    );
    println!();
}

fn print_status(enabled: bool) {
    let label = if enabled {
        "on".with(Color::Green).to_string()
    } else {
        "off".with(Color::DarkGrey).to_string()
    };
    println!();
    println!(
        "  {} coding-agent: {label} {}",
        "·".with(Color::Cyan),
        "[experimental]".with(Color::Yellow),
    );
    print_experimental_note();
    println!(
        "  {} flip with: {}",
        "→".with(Color::Cyan),
        "/coding on | off | toggle".with(Color::DarkGrey)
    );
    println!();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_action_empty_opens_menu() {
        assert!(matches!(parse_action(""), Ok(Action::Menu)));
        assert!(matches!(parse_action("  "), Ok(Action::Menu)));
    }

    #[test]
    fn parse_action_status_is_show() {
        assert!(matches!(parse_action("status"), Ok(Action::Show)));
        assert!(matches!(parse_action("show"), Ok(Action::Show)));
    }

    #[test]
    fn parse_action_on_off_toggle() {
        assert!(matches!(parse_action("on"), Ok(Action::SetOn)));
        assert!(matches!(parse_action("OFF"), Ok(Action::SetOff)));
        assert!(matches!(parse_action("toggle"), Ok(Action::Toggle)));
    }

    #[test]
    fn parse_action_aliases() {
        assert!(matches!(parse_action("enable"), Ok(Action::SetOn)));
        assert!(matches!(parse_action("disabled"), Ok(Action::SetOff)));
    }

    #[test]
    fn parse_action_unknown_is_err() {
        assert!(parse_action("maybe").is_err());
    }
}
