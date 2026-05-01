//! Helpers around `AICTL_WORKING_DIR_DESKTOP` — the desktop's pinned
//! workspace folder.
//!
//! The CLI's `apply_cwd_override` (`crates/aictl-cli/src/main.rs`) does
//! the same job at process boot: resolve a configured path, validate
//! it's a directory, and `set_current_dir` so the security policy's
//! CWD jail anchors there. The desktop differs in two ways:
//!
//! * The path is **required** — there is no launch-CWD fallback,
//!   because launching from `/Applications/aictl.app` produces a
//!   useless anchor (see plan §5.4).
//! * The path can change *during* a session via the Settings →
//!   Workspace pane, so the helpers here re-resolve every time rather
//!   than caching.

use std::path::{Path, PathBuf};

use aictl_core::config::{self, AICTL_WORKING_DIR_DESKTOP};

/// Resolve and canonicalize the configured desktop workspace path.
///
/// Returns `Ok(Some(path))` when the user has picked a workspace and the
/// path still points at an existing directory; `Ok(None)` when no
/// workspace is set; `Err(reason)` when the configured path is set but
/// has gone stale (deleted, moved, replaced by a file). The desktop
/// surfaces the `Err` arm as a re-pick prompt — see the risks table in
/// the plan §11.
pub fn resolve() -> Result<Option<PathBuf>, String> {
    let Some(raw) = config::config_get(AICTL_WORKING_DIR_DESKTOP) else {
        return Ok(None);
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let path = expand(trimmed);
    if !path.exists() {
        return Err(format!(
            "configured workspace '{}' no longer exists",
            path.display()
        ));
    }
    if !path.is_dir() {
        return Err(format!(
            "configured workspace '{}' is not a directory",
            path.display()
        ));
    }
    let canonical = path
        .canonicalize()
        .map_err(|e| format!("failed to canonicalize '{}': {e}", path.display()))?;
    Ok(Some(canonical))
}

/// Persist a freshly-picked workspace path. Validates the target is an
/// existing directory and writes through to `~/.aictl/config`. The
/// security policy picks the new value up automatically on its next
/// `policy()` read because `working_dir_for_role` consults `config_get`
/// every call.
pub fn set(raw_path: &str) -> Result<PathBuf, String> {
    let trimmed = raw_path.trim();
    if trimmed.is_empty() {
        return Err("workspace path is empty".to_string());
    }
    let path = expand(trimmed);
    if !path.exists() {
        return Err(format!("path '{}' does not exist", path.display()));
    }
    if !path.is_dir() {
        return Err(format!("path '{}' is not a directory", path.display()));
    }
    let canonical = path
        .canonicalize()
        .map_err(|e| format!("failed to canonicalize '{}': {e}", path.display()))?;
    let stored = canonical.to_string_lossy().to_string();
    config::config_set(AICTL_WORKING_DIR_DESKTOP, &stored);
    Ok(canonical)
}

/// Whether the desktop has a usable workspace right now. The composer
/// is gated on this — every CWD-relative tool call would otherwise hit
/// the security sentinel and surface a confusing rejection.
pub fn is_set() -> bool {
    matches!(resolve(), Ok(Some(_)))
}

fn expand(raw: &str) -> PathBuf {
    if let Some(rest) = raw.strip_prefix("~/")
        && let Ok(home) = std::env::var("HOME")
    {
        return Path::new(&home).join(rest);
    }
    if raw == "~"
        && let Ok(home) = std::env::var("HOME")
    {
        return PathBuf::from(home);
    }
    PathBuf::from(raw)
}
