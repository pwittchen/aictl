//! `--uninstall` handler for `aictl-server`.
//!
//! Removes the `aictl-server` binary from every install location used by
//! `website/dist/server/install.sh` and a `cargo install` build, plus
//! `$AICTL_INSTALL_DIR` if set. Mirrors the CLI's uninstall behavior but
//! lives here so the server crate stays free of crossterm/styling deps.
//! Leaves `~/.aictl/` (config, sessions, master key) untouched — the
//! server's footer hint tells the operator how to wipe it manually.

use std::path::PathBuf;

fn uninstall_candidates() -> Vec<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut dirs: Vec<PathBuf> = Vec::new();

    if !home.is_empty() {
        dirs.push(PathBuf::from(format!("{home}/.cargo/bin")));
        dirs.push(PathBuf::from(format!("{home}/.local/bin")));
    }
    dirs.push(PathBuf::from("/usr/local/bin"));

    if let Ok(custom) = std::env::var("AICTL_INSTALL_DIR")
        && !custom.is_empty()
    {
        let dir = PathBuf::from(custom);
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }

    dirs.into_iter().map(|d| d.join("aictl-server")).collect()
}

/// Remove the `aictl-server` binary from every known install location.
/// The explicit `--uninstall` flag is treated as consent — no prompt.
/// Exits with status `1` if any removal failed.
///
/// On Unix, deleting the currently-running binary is safe: the file is
/// unlinked but the in-memory process keeps running until exit.
pub fn run() -> ! {
    let candidates = uninstall_candidates();
    let mut removed = 0u32;
    let mut errors = 0u32;

    println!();
    for path in &candidates {
        if !path.exists() {
            continue;
        }
        match std::fs::remove_file(path) {
            Ok(()) => {
                println!("  ✓ removed {}", path.display());
                removed += 1;
            }
            Err(e) => {
                println!("  ✗ failed to remove {}: {e}", path.display());
                errors += 1;
            }
        }
    }

    if removed == 0 && errors == 0 {
        println!(
            "  • no aictl-server binary found in {}",
            candidates
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
    } else {
        println!();
        println!("  → ~/.aictl/ (config, master key) was left untouched.");
        println!("  → run `rm -rf ~/.aictl` to remove it as well.");
    }
    println!();

    std::process::exit(i32::from(errors > 0));
}
