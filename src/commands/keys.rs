use crossterm::style::{Color, Stylize};

use super::menu::{build_simple_menu_lines, confirm_yn, select_from_menu};

const KEYS_MENU_ITEMS: &[(&str, &str)] = &[
    (
        "lock keys",
        "migrate API keys from config to system keyring",
    ),
    (
        "unlock keys",
        "migrate API keys from system keyring to config",
    ),
    ("clear keys", "remove API keys from both config and keyring"),
];

/// Migrate all plain-text keys from the config file to the system keyring.
pub fn run_lock_keys(show_error: &dyn Fn(&str)) {
    use crate::keys::{self, LockOutcome};

    if !keys::backend_available() {
        show_error(&format!(
            "System keyring is not available on this platform. Keys remain in ~/.aictl/config. (backend: {})",
            keys::backend_name()
        ));
        return;
    }

    println!();
    println!(
        "  {} migrating keys to {}...",
        "→".with(Color::Cyan),
        keys::backend_name().with(Color::Green),
    );
    println!();

    let mut any = false;
    for name in keys::KEY_NAMES {
        match keys::lock_key(name) {
            LockOutcome::Locked => {
                any = true;
                println!(
                    "  {} {} → keyring",
                    "✓".with(Color::Green),
                    name.with(Color::White),
                );
            }
            LockOutcome::AlreadyLocked => {
                any = true;
                println!(
                    "  {} {} already in keyring",
                    "·".with(Color::DarkGrey),
                    name.with(Color::DarkGrey),
                );
            }
            LockOutcome::NotInConfig => {}
            LockOutcome::Error(e) => {
                println!(
                    "  {} {} ({})",
                    "✗".with(Color::Red),
                    name.with(Color::White),
                    e.with(Color::Red),
                );
            }
        }
    }
    if !any {
        println!("  {}", "no API keys found to lock".with(Color::DarkGrey));
    }
    println!();
}

/// Migrate all keys currently in the system keyring back to the plain-text config.
pub fn run_unlock_keys(show_error: &dyn Fn(&str)) {
    use crate::keys::{self, UnlockOutcome};

    if !keys::backend_available() {
        show_error(&format!(
            "System keyring is not available on this platform. (backend: {})",
            keys::backend_name()
        ));
        return;
    }

    println!();
    println!(
        "  {} migrating keys from keyring to config...",
        "→".with(Color::Cyan),
    );
    println!();

    let mut any = false;
    for name in keys::KEY_NAMES {
        match keys::unlock_key(name) {
            UnlockOutcome::Unlocked => {
                any = true;
                println!(
                    "  {} {} → config",
                    "✓".with(Color::Green),
                    name.with(Color::White),
                );
            }
            UnlockOutcome::AlreadyUnlocked => {
                any = true;
                println!(
                    "  {} {} already in config",
                    "·".with(Color::DarkGrey),
                    name.with(Color::DarkGrey),
                );
            }
            UnlockOutcome::NotInKeyring => {}
            UnlockOutcome::Error(e) => {
                println!(
                    "  {} {} ({})",
                    "✗".with(Color::Red),
                    name.with(Color::White),
                    e.with(Color::Red),
                );
            }
        }
    }
    if !any {
        println!("  {}", "no API keys found to unlock".with(Color::DarkGrey));
    }
    println!();
}

/// Inner loop for `clear_key`: walks `KEY_NAMES`, prints per-key results.
/// Caller is responsible for any confirmation prompt.
fn clear_keys_inner() {
    use crate::keys::{self, ClearOutcome};

    let mut any = false;
    for name in keys::KEY_NAMES {
        match keys::clear_key(name) {
            ClearOutcome::Cleared => {
                any = true;
                println!(
                    "  {} {} cleared",
                    "✓".with(Color::Green),
                    name.with(Color::White),
                );
            }
            ClearOutcome::NotPresent => {}
            ClearOutcome::Error(e) => {
                println!(
                    "  {} {} ({})",
                    "✗".with(Color::Red),
                    name.with(Color::White),
                    e.with(Color::Red),
                );
            }
        }
    }
    if !any {
        println!("  {}", "no API keys found to clear".with(Color::DarkGrey));
    }
    println!();
}

/// Remove all known API keys from both config and keyring. Asks for confirmation.
/// Used by the `/keys` REPL menu's "clear keys" entry.
pub fn run_clear_keys(_show_error: &dyn Fn(&str)) {
    println!();
    if !confirm_yn("remove ALL API keys from config AND keyring?") {
        return;
    }
    clear_keys_inner();
}

/// Remove all known API keys from both config and keyring without prompting.
/// Used by the `--clear-keys` CLI flag, where the explicit flag is treated as
/// the user's confirmation (matching `--clear-sessions`).
pub fn run_clear_keys_unconfirmed() {
    println!();
    clear_keys_inner();
}

/// Open the `/keys` interactive menu (lock, unlock, clear).
pub fn run_keys_menu(show_error: &dyn Fn(&str)) {
    let Some(sel) = select_from_menu(KEYS_MENU_ITEMS.len(), 0, |s| {
        build_simple_menu_lines(KEYS_MENU_ITEMS, s)
    }) else {
        return;
    };
    match sel {
        0 => run_lock_keys(show_error),
        1 => run_unlock_keys(show_error),
        2 => run_clear_keys(show_error),
        _ => {}
    }
}
