use crossterm::style::{Color, Stylize};

use super::menu::confirm_yn;

/// Build the list of install locations to check / remove. Mirrors the
/// directories used by `install.sh` and a `cargo install` build, plus
/// `$AICTL_INSTALL_DIR` if the env var is set (deduplicated).
fn uninstall_candidates() -> Vec<std::path::PathBuf> {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();

    if !home.is_empty() {
        candidates.push(std::path::PathBuf::from(format!("{home}/.cargo/bin/aictl")));
        candidates.push(std::path::PathBuf::from(format!("{home}/.local/bin/aictl")));
    }

    if let Ok(custom) = std::env::var("AICTL_INSTALL_DIR")
        && !custom.is_empty()
    {
        let path = std::path::PathBuf::from(custom).join("aictl");
        if !candidates.contains(&path) {
            candidates.push(path);
        }
    }

    candidates
}

/// Perform the actual removal across `candidates`. Prints a status line per
/// path. Returns `(removed, errors)` so callers can decide on their own
/// follow-up behavior. Leaves `~/.aictl/` untouched; the caller prints the
/// "wipe ~/.aictl separately" hint when appropriate.
///
/// On Unix, deleting the currently-running binary is safe: the file is
/// unlinked but the in-memory process keeps running until exit.
fn perform_uninstall(candidates: &[std::path::PathBuf]) -> (u32, u32) {
    let mut removed = 0;
    let mut errors = 0;
    for path in candidates {
        if !path.exists() {
            continue;
        }
        match std::fs::remove_file(path) {
            Ok(()) => {
                println!("  {} removed {}", "✓".with(Color::Green), path.display());
                removed += 1;
            }
            Err(e) => {
                println!(
                    "  {} failed to remove {}: {e}",
                    "✗".with(Color::Red),
                    path.display()
                );
                errors += 1;
            }
        }
    }
    (removed, errors)
}

fn print_uninstall_footer(candidates: &[std::path::PathBuf], removed: u32, errors: u32) {
    if removed == 0 && errors == 0 {
        println!(
            "  {} no aictl binary found in {}",
            "•".with(Color::Yellow),
            candidates
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
    } else {
        println!();
        println!(
            "  {} ~/.aictl/ (config, sessions, models) was left untouched.",
            "→".with(Color::Cyan)
        );
        println!(
            "  {} run {} to remove it as well.",
            "→".with(Color::Cyan),
            "rm -rf ~/.aictl".with(Color::Cyan)
        );
    }
    println!();
}

/// Remove the aictl binary from every install location we know about.
/// Used by the `--uninstall` CLI flag — the explicit flag is treated as
/// consent, so no confirmation is asked. Exits with a non-zero status if
/// any removal failed.
pub fn run_uninstall_cli() {
    let candidates = uninstall_candidates();
    println!();
    let (removed, errors) = perform_uninstall(&candidates);
    print_uninstall_footer(&candidates, removed, errors);
    if errors > 0 {
        std::process::exit(1);
    }
}

/// Interactive `/uninstall` REPL command. Lists what would be removed,
/// asks for y/N confirmation, then deletes the matching binaries.
/// Returns `true` when the REPL should exit (any successful removal
/// makes continuing pointless), `false` otherwise.
pub fn run_uninstall_repl(show_error: &dyn Fn(&str)) -> bool {
    let candidates = uninstall_candidates();
    let existing: Vec<&std::path::PathBuf> = candidates.iter().filter(|p| p.exists()).collect();

    println!();
    if existing.is_empty() {
        println!(
            "  {} no aictl binary found in {}",
            "•".with(Color::Yellow),
            candidates
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
        println!();
        return false;
    }

    println!("  {} would remove:", "→".with(Color::Cyan));
    for path in &existing {
        println!("    {}", path.display().to_string().with(Color::White));
    }
    println!();

    if !confirm_yn("uninstall aictl?") {
        return false;
    }

    println!();
    let (removed, errors) = perform_uninstall(&candidates);
    print_uninstall_footer(&candidates, removed, errors);

    if errors > 0 {
        show_error("uninstall completed with errors — see messages above");
        return false;
    }
    // Any successful removal means the binary the user is running is
    // probably gone; exit so the next launch picks up the absence.
    removed > 0
}
