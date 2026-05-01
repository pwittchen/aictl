//! `--version` enrichment and `--update` for `aictl-server`.
//!
//! Mirrors the CLI's update flow (see `aictl-cli/src/commands/update.rs`):
//! pull the upstream version straight from the project's root
//! `Cargo.toml`, compare against the embedded `aictl_core::VERSION`, and
//! either announce that the operator is on the latest release or shell
//! out to the published install script. Both binaries track the same
//! workspace version so a single source of truth is enough.

use aictl_core::config;

/// Server install one-liner — must stay in sync with the URL printed by
/// `aictl-cli`'s `--serve` "not installed" hint and with `SERVER.md`.
pub const UPDATE_CMD: &str = "curl -fsSL https://aictl.app/server/install.sh | sh";

/// Fetch the current upstream version from `Cargo.toml` on master.
/// Returns `Some(version_string)` on success, `None` on any network /
/// parse failure. Three-second timeout matches the CLI so a flaky
/// connection never stalls `--version`.
pub async fn fetch_remote_version() -> Option<String> {
    let url = "https://raw.githubusercontent.com/pwittchen/aictl/refs/heads/master/Cargo.toml";
    let client = config::http_client();
    let body = client
        .get(url)
        .timeout(std::time::Duration::from_secs(3))
        .send()
        .await
        .ok()?
        .text()
        .await
        .ok()?;
    body.lines().find_map(|line| {
        let rest = line.strip_prefix("version")?;
        let (_, val) = rest.split_once('=')?;
        Some(val.trim().trim_matches('"').to_string())
    })
}

/// Handle `aictl-server --version`. Prints the embedded version and, if
/// the upstream check succeeds, appends `(latest)` or `(<v> available)`.
pub async fn run_version() {
    let version = aictl_core::VERSION;
    match fetch_remote_version().await {
        Some(v) if v == version => println!("aictl-server {version} (latest)"),
        Some(v) => println!("aictl-server {version} ({v} available)"),
        None => println!("aictl-server {version}"),
    }
}

/// Handle `aictl-server --update`. Confirms an upgrade is needed, then
/// shells out to `install.sh`. Exits the process with status `1` on any
/// failure so scripted invocations propagate the error.
pub async fn run_update_cli() {
    eprintln!("Checking for updates...");
    let version = aictl_core::VERSION;
    let remote = fetch_remote_version().await;
    match &remote {
        Some(v) if v == version => {
            println!("Already on latest version ({version}).");
            return;
        }
        Some(v) => {
            eprintln!("Updating {version} → {v}...");
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
