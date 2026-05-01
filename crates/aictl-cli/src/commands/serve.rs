//! `--serve` convenience flag.
//!
//! Locates a sibling `aictl-server` binary and execs it, forwarding any
//! pass-through args. The two binaries are independent — the CLI does
//! not link `aictl-server` — but a single `aictl --serve` shortcut
//! saves users having to remember the second binary name.
//!
//! Resolution order for the server binary:
//!
//! 1. A binary sitting next to the current `aictl` executable
//!    (e.g. both at `/usr/local/bin/`).
//! 2. The first `aictl-server` on `$PATH`.
//! 3. `~/.cargo/bin/aictl-server`.
//! 4. `~/.local/bin/aictl-server`.
//! 5. `$AICTL_INSTALL_DIR/aictl-server` if the env var is set.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Entry point wired up by `--serve`. Locates the server binary,
/// spawns it, propagates stdin/stdout/stderr, waits for it to exit,
/// and exits with the same status. Never returns.
pub fn run_serve_cli(args: &[String]) -> ! {
    let Some(bin) = find_server_binary() else {
        eprintln!("aictl-server is not installed.");
        eprintln!();
        eprintln!("Install it with one of:");
        eprintln!("  curl -fsSL https://aictl.app/server/install.sh | sh");
        eprintln!(
            "  cargo install --git https://github.com/pwittchen/aictl.git --bin aictl-server"
        );
        eprintln!("  cargo install --path crates/aictl-server   # from the cloned repo");
        eprintln!();
        eprintln!("See https://aictl.app/server for more details.");
        std::process::exit(127);
    };

    let mut cmd = Command::new(&bin);
    cmd.args(args);

    match cmd.status() {
        Ok(status) => {
            if let Some(code) = status.code() {
                std::process::exit(code);
            }
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("aictl: failed to launch {}: {e}", bin.display());
            std::process::exit(1);
        }
    }
}

pub(super) fn find_server_binary() -> Option<PathBuf> {
    if let Some(p) = sibling_of_current_exe() {
        return Some(p);
    }
    if let Some(p) = on_path("aictl-server") {
        return Some(p);
    }
    let home = std::env::var("HOME").ok();
    if let Some(ref h) = home {
        for prefix in ["/.cargo/bin/", "/.local/bin/"] {
            let candidate = PathBuf::from(format!("{h}{prefix}aictl-server"));
            if is_executable_file(&candidate) {
                return Some(candidate);
            }
        }
    }
    if let Ok(custom) = std::env::var("AICTL_INSTALL_DIR")
        && !custom.is_empty()
    {
        let candidate = PathBuf::from(custom).join("aictl-server");
        if is_executable_file(&candidate) {
            return Some(candidate);
        }
    }
    None
}

fn sibling_of_current_exe() -> Option<PathBuf> {
    let current = std::env::current_exe().ok()?;
    let parent = current.parent()?;
    let candidate = parent.join("aictl-server");
    if is_executable_file(&candidate) {
        Some(candidate)
    } else {
        None
    }
}

fn on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if is_executable_file(&candidate) {
            return Some(candidate);
        }
    }
    None
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn is_executable_file_rejects_missing_path() {
        assert!(!is_executable_file(Path::new("/nonexistent/path/xyz")));
    }

    #[cfg(unix)]
    #[test]
    fn is_executable_file_rejects_non_executable() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path();
        let mut perms = fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o644);
        fs::set_permissions(path, perms).unwrap();
        assert!(!is_executable_file(path));
    }

    #[cfg(unix)]
    #[test]
    fn is_executable_file_accepts_executable() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path();
        let mut perms = fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).unwrap();
        assert!(is_executable_file(path));
    }
}
