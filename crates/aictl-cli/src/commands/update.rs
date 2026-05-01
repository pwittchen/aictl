use std::io::IsTerminal;

use crossterm::style::{Color, Stylize};

pub const UPDATE_CMD: &str = "curl -sSf https://aictl.app/install.sh | sh";

/// Server install one-liner — kept in sync with the URL `--serve` prints
/// when `aictl-server` is missing and with `SERVER.md`.
pub const SERVER_UPDATE_CMD: &str = "curl -fsSL https://aictl.app/server/install.sh | sh";

/// Check the current version against the latest available (REPL `/version`).
pub async fn run_version(show_error: &dyn Fn(&str)) {
    println!();
    println!("  {} checking latest version...", "↓".with(Color::Cyan),);

    let remote = crate::fetch_remote_version().await;
    match &remote {
        Some(v) if v == crate::VERSION => {
            println!(
                "  {} aictl {} (latest)",
                "✓".with(Color::Green),
                crate::VERSION,
            );
        }
        Some(v) => {
            println!(
                "  {} aictl {} → {v} available",
                "!".with(Color::Yellow),
                crate::VERSION,
            );
            println!("  run {} to update", "/update".with(Color::Cyan),);
        }
        None => {
            show_error("Could not check remote version. Please try again later.");
        }
    }
    println!();
}

/// Run the update process interactively (REPL `/update`).
/// Returns `true` if the binary was updated and the REPL should exit.
pub async fn run_update(show_error: &dyn Fn(&str)) -> bool {
    println!();
    println!("  {} checking for updates...", "↓".with(Color::Cyan),);

    let remote = crate::fetch_remote_version().await;
    match &remote {
        Some(v) if v == crate::VERSION => {
            println!(
                "  {} already on latest version ({})",
                "✓".with(Color::Green),
                crate::VERSION,
            );
            println!();
            return false;
        }
        Some(v) => {
            println!(
                "  {} updating {} → {v}...",
                "↓".with(Color::Cyan),
                crate::VERSION,
            );
            println!();
        }
        None => {
            show_error("Could not check remote version. Please try again later.");
            return false;
        }
    }

    let status = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(UPDATE_CMD)
        .status()
        .await;

    match status {
        Ok(s) if s.success() => {
            println!();
            println!(
                "  {} updated successfully. Please restart aictl.",
                "✓".with(Color::Green),
            );
            println!();
            maybe_update_server(show_error).await;
            true
        }
        Ok(s) => {
            show_error(&format!(
                "Update failed with exit code: {}",
                s.code().unwrap_or(-1)
            ));
            false
        }
        Err(e) => {
            show_error(&format!("Failed to run update: {e}"));
            false
        }
    }
}

/// Run the update process from the CLI (`--update` flag).
pub async fn run_update_cli() {
    eprintln!("Checking for updates...");

    let remote = crate::fetch_remote_version().await;
    match &remote {
        Some(v) if v == crate::VERSION => {
            println!("Already on latest version ({}).", crate::VERSION);
            return;
        }
        Some(v) => {
            eprintln!("Updating {} → {v}...", crate::VERSION);
        }
        None => {
            eprintln!("Error: could not check remote version. Please try again later.");
            std::process::exit(1);
        }
    }

    let status = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(UPDATE_CMD)
        .status()
        .await;

    match status {
        Ok(s) if s.success() => {
            println!("Updated successfully.");
            maybe_update_server(&|msg| eprintln!("Error: {msg}")).await;
        }
        Ok(s) => {
            eprintln!("Update failed with exit code: {}", s.code().unwrap_or(-1));
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("Failed to run update: {e}");
            std::process::exit(1);
        }
    }
}

/// After the CLI has been updated, detect whether the operator also has
/// `aictl-server` installed and offer to refresh it through the server
/// install script. Any sub-failure is reported via `show_error` so the
/// caller can render it in its preferred style — the CLI update itself
/// already succeeded, so server hiccups must not look fatal.
///
/// The prompt is skipped silently when stdout is not a terminal (CI,
/// pipes, automation) so unattended runs never block waiting for input.
async fn maybe_update_server(show_error: &dyn Fn(&str)) {
    let Some(server_bin) = super::serve::find_server_binary() else {
        return;
    };
    if !std::io::stdout().is_terminal() {
        return;
    }

    println!();
    println!(
        "  {} aictl-server detected at {}",
        "ℹ".with(Color::Cyan),
        server_bin.display(),
    );
    if !super::menu::confirm_yn("update aictl-server as well?") {
        println!();
        return;
    }

    println!();
    println!("  {} updating aictl-server...", "↓".with(Color::Cyan),);
    println!();

    let status = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(SERVER_UPDATE_CMD)
        .status()
        .await;

    match status {
        Ok(s) if s.success() => {
            println!();
            println!(
                "  {} aictl-server updated successfully.",
                "✓".with(Color::Green),
            );
            println!();
        }
        Ok(s) => {
            show_error(&format!(
                "aictl-server update failed with exit code: {}",
                s.code().unwrap_or(-1)
            ));
        }
        Err(e) => {
            show_error(&format!("Failed to run aictl-server update: {e}"));
        }
    }
}
