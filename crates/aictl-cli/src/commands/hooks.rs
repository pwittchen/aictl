//! `/hooks` REPL menu and `--list-hooks` CLI helper.
//!
//! Reads the in-memory hook table built by [`crate::hooks::init`] / [`reload`]
//! and lets the user browse, enable / disable, and test-fire individual
//! hooks. Edits land in `~/.aictl/hooks.json` (or `AICTL_HOOKS_FILE`); the
//! REPL reloads the table after every save so toggles take effect mid-session.

use std::io::Write;

use crossterm::style::{Color, Stylize};

use crate::hooks::{self, Hook, HookEvent};

use super::menu::{build_simple_menu_lines, confirm_yn, select_from_menu};

const HOOKS_MENU_ITEMS: &[(&str, &str)] = &[
    ("view all hooks", "browse by event, see matcher / command"),
    (
        "toggle a hook",
        "enable or disable an entry without removing it",
    ),
    ("test-fire a hook", "run a hook with a synthetic payload"),
    ("show hooks file", "print the path of ~/.aictl/hooks.json"),
    ("reload hooks", "re-read hooks.json from disk"),
];

/// Run the `/hooks` REPL menu.
pub fn run_hooks_menu(show_error: &dyn Fn(&str)) {
    loop {
        let Some(sel) = select_from_menu(HOOKS_MENU_ITEMS.len(), 0, |selected| {
            build_simple_menu_lines(HOOKS_MENU_ITEMS, selected)
        }) else {
            return;
        };
        match HOOKS_MENU_ITEMS[sel].0 {
            "view all hooks" => view_all_hooks(),
            "toggle a hook" => toggle_hook(show_error),
            "test-fire a hook" => test_fire_hook(show_error),
            "show hooks file" => print_hooks_file(),
            "reload hooks" => {
                hooks::reload();
                println!();
                println!("  {} hooks reloaded", "✓".with(Color::Green));
                println!();
            }
            _ => {}
        }
    }
}

fn header_status_line() {
    let total: usize = HookEvent::ALL
        .iter()
        .map(|ev| hooks::list_for(*ev).len())
        .sum();
    let path =
        hooks::hooks_file().map_or_else(|| "(unset)".to_string(), |p| p.display().to_string());
    println!(
        "  {} {} hooks across {} events  {}",
        "hooks:".with(Color::Cyan),
        format!("{total}").with(if total == 0 {
            Color::Yellow
        } else {
            Color::Green
        }),
        HookEvent::ALL.len(),
        format!("({path})").with(Color::DarkGrey),
    );
}

fn print_hooks_file() {
    println!();
    header_status_line();
    println!();
}

fn view_all_hooks() {
    println!();
    header_status_line();
    let mut any = false;
    for ev in HookEvent::ALL {
        let hooks = hooks::list_for(*ev);
        if hooks.is_empty() {
            continue;
        }
        any = true;
        println!();
        println!("  {}", ev.as_str().with(Color::Cyan));
        for h in &hooks {
            print_hook_row(h);
        }
    }
    if !any {
        println!(
            "  {}",
            "No hooks configured. Drop a JSON file at the path above.".with(Color::DarkGrey)
        );
    }
    println!();
}

fn print_hook_row(h: &Hook) {
    let status = if h.enabled {
        "on".with(Color::Green)
    } else {
        "off".with(Color::DarkGrey)
    };
    println!(
        "    [{status}] {}  {}  ({}s)",
        h.matcher.as_str().with(Color::Magenta),
        h.command.as_str().with(Color::DarkGrey),
        h.timeout_secs,
    );
}

fn toggle_hook(show_error: &dyn Fn(&str)) {
    let entries = hooks::list_all();
    if entries.is_empty() {
        println!();
        println!("  {}", "No hooks to toggle.".with(Color::DarkGrey));
        println!();
        return;
    }
    let labels: Vec<(String, String)> = entries
        .iter()
        .map(|h| {
            let label = format!(
                "{} / {}",
                h.event.as_str(),
                truncate_for_label(&h.matcher, 24)
            );
            let desc = format!(
                "[{}] {}",
                if h.enabled { "on" } else { "off" },
                truncate_for_label(&h.command, 60)
            );
            (label, desc)
        })
        .collect();
    let label_refs: Vec<(&str, &str)> = labels
        .iter()
        .map(|(l, d)| (l.as_str(), d.as_str()))
        .collect();

    let Some(idx) = select_from_menu(label_refs.len(), 0, |selected| {
        build_simple_menu_lines(&label_refs, selected)
    }) else {
        return;
    };
    let target = entries[idx].clone();

    // Apply toggle to the snapshot, save, reload.
    let mut snap = hooks::snapshot();
    let bucket = snap.entry(target.event).or_default();
    // Match by ordinal position within the event bucket — the flat
    // `entries` index isn't directly usable as a per-event index.
    let same_event_index = entries[..idx]
        .iter()
        .filter(|h| h.event == target.event)
        .count();
    if same_event_index >= bucket.len() {
        show_error("hook table changed under us, reload and retry");
        return;
    }
    bucket[same_event_index].enabled = !bucket[same_event_index].enabled;
    let new_state = bucket[same_event_index].enabled;

    if let Err(e) = hooks::save(&snap) {
        show_error(&format!("failed to save hooks: {e}"));
        return;
    }
    hooks::replace(snap);

    println!();
    println!(
        "  {} {} / {} now {}",
        "✓".with(Color::Green),
        target.event,
        target.matcher,
        if new_state { "enabled" } else { "disabled" },
    );
    println!();
}

fn test_fire_hook(show_error: &dyn Fn(&str)) {
    let entries = hooks::list_all();
    if entries.is_empty() {
        println!();
        println!("  {}", "No hooks to test.".with(Color::DarkGrey));
        println!();
        return;
    }
    let labels: Vec<(String, String)> = entries
        .iter()
        .map(|h| {
            (
                format!(
                    "{} / {}",
                    h.event.as_str(),
                    truncate_for_label(&h.matcher, 24)
                ),
                truncate_for_label(&h.command, 60),
            )
        })
        .collect();
    let label_refs: Vec<(&str, &str)> = labels
        .iter()
        .map(|(l, d)| (l.as_str(), d.as_str()))
        .collect();

    let Some(idx) = select_from_menu(label_refs.len(), 0, |selected| {
        build_simple_menu_lines(&label_refs, selected)
    }) else {
        return;
    };
    let hook = entries[idx].clone();

    if !confirm_yn(&format!(
        "fire `{}` (matcher `{}`) once?",
        hook.command, hook.matcher
    )) {
        return;
    }

    // Hand the hook a representative payload for its event kind.
    let ctx = test_payload_for(hook.event);
    let payload = serde_json::to_string(&hooks::build_payload(hook.event, "", &ctx))
        .unwrap_or_else(|_| "{}".to_string());

    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            show_error(&format!("failed to build tokio runtime: {e}"));
            return;
        }
    };
    let decision = rt.block_on(hooks::execute_hook(&hook, &payload));

    println!();
    match decision {
        hooks::HookDecision::Continue => {
            println!(
                "  {} hook returned {}",
                "✓".with(Color::Green),
                "continue".with(Color::DarkGrey)
            );
        }
        hooks::HookDecision::Block(reason) => {
            println!(
                "  {} hook returned {}: {}",
                "⚠".with(Color::Yellow),
                "block".with(Color::Red),
                reason.with(Color::DarkGrey)
            );
        }
        hooks::HookDecision::Approve(reason) => {
            println!(
                "  {} hook returned {}: {}",
                "✓".with(Color::Green),
                "approve".with(Color::Green),
                reason.with(Color::DarkGrey)
            );
        }
        hooks::HookDecision::AddContext(text) => {
            println!(
                "  {} hook returned {}:",
                "✓".with(Color::Green),
                "additionalContext".with(Color::Cyan)
            );
            for line in text.lines() {
                println!("    {}", line.with(Color::DarkGrey));
            }
        }
        hooks::HookDecision::RewritePrompt(text) => {
            println!(
                "  {} hook returned {}:",
                "✓".with(Color::Green),
                "rewrittenPrompt".with(Color::Magenta)
            );
            for line in text.lines() {
                println!("    {}", line.with(Color::DarkGrey));
            }
        }
    }
    println!();
}

fn test_payload_for(event: HookEvent) -> hooks::HookContext<'static> {
    let mut ctx = hooks::HookContext {
        session_id: crate::session::current_id(),
        cwd: std::env::current_dir().ok(),
        ..Default::default()
    };
    match event {
        HookEvent::PreToolUse | HookEvent::PostToolUse => {
            ctx.tool_name = Some("exec_shell");
            ctx.tool_input = Some("echo hello");
        }
        HookEvent::UserPromptSubmit | HookEvent::Stop => {
            ctx.prompt = Some("synthetic test prompt");
        }
        HookEvent::Notification => {
            ctx.notification = Some("synthetic test notification");
        }
        HookEvent::SessionStart | HookEvent::SessionEnd | HookEvent::PreCompact => {
            ctx.trigger = Some("test");
        }
    }
    ctx
}

fn truncate_for_label(s: &str, max: usize) -> String {
    let one_line: String = s.chars().take_while(|c| *c != '\n').collect();
    if one_line.chars().count() <= max {
        one_line
    } else {
        let kept: String = one_line.chars().take(max.saturating_sub(1)).collect();
        format!("{kept}…")
    }
}

/// `--list-hooks` non-interactive output.
pub fn print_hooks_cli() {
    let path =
        hooks::hooks_file().map_or_else(|| "(unset)".to_string(), |p| p.display().to_string());
    let mut printed = false;
    for ev in HookEvent::ALL {
        let entries = hooks::list_for(*ev);
        if entries.is_empty() {
            continue;
        }
        printed = true;
        println!("[{}]", ev.as_str());
        for h in entries {
            let status = if h.enabled { "on " } else { "off" };
            println!(
                "  {status}  {:<24}  {}  ({}s)",
                h.matcher, h.command, h.timeout_secs
            );
        }
    }
    if !printed {
        println!("(no hooks configured in {path})");
    }
}

/// `/hooks list` — print to stderr/stdout for the current REPL view.
/// Convenience helper; the menu version is interactive.
#[allow(dead_code)]
pub fn print_hooks_repl() {
    let mut stdout = std::io::stdout();
    let _ = writeln!(stdout);
    header_status_line();
    let _ = writeln!(stdout);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_for_label_short_unchanged() {
        assert_eq!(truncate_for_label("ls -la", 20), "ls -la");
    }

    #[test]
    fn truncate_for_label_long_clipped() {
        let s = "echo this is a very long command line that should be truncated";
        let out = truncate_for_label(s, 10);
        assert!(out.ends_with('…'));
        // Visible char count cap: 9 kept chars + ellipsis = 10.
        assert_eq!(out.chars().count(), 10);
    }

    #[test]
    fn truncate_for_label_drops_after_newline() {
        let s = "line1\nline2";
        assert_eq!(truncate_for_label(s, 50), "line1");
    }
}
