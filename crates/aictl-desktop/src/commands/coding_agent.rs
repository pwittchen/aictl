//! Coding-agent Tauri commands.
//!
//! The desktop exposes the same master switch the CLI's `/coding-agent`
//! command toggles. Both surfaces (Settings panel + composer-toolbar
//! icon) call these adapters; the engine reads
//! [`aictl_core::config::coding_agent_enabled`] every turn so a flip
//! takes effect on the next message without an app restart.

use aictl_core::config::{AICTL_CODING_AGENT, coding_agent_enabled, config_set};
use serde::Serialize;

#[derive(Serialize)]
pub struct CodingAgentStatus {
    pub enabled: bool,
}

#[tauri::command]
pub fn coding_agent_status() -> CodingAgentStatus {
    CodingAgentStatus {
        enabled: coding_agent_enabled(),
    }
}

#[tauri::command]
pub fn coding_agent_set_enabled(enabled: bool) -> CodingAgentStatus {
    config_set(AICTL_CODING_AGENT, if enabled { "true" } else { "false" });
    coding_agent_status()
}

/// Auto-detected (or user-overridden) build command for the current
/// working directory. `null` means no build command could be detected.
#[tauri::command]
pub fn coding_agent_build_cmd() -> Option<String> {
    let cwd = aictl_core::security::policy().paths.working_dir.clone();
    aictl_core::coding::detect_build_cmd(&cwd)
}

/// Auto-detected (or user-overridden) lint command for the current
/// working directory. `null` means no linter could be detected.
#[tauri::command]
pub fn coding_agent_lint_cmd() -> Option<String> {
    let cwd = aictl_core::security::policy().paths.working_dir.clone();
    aictl_core::coding::detect_linter(&cwd)
}

/// Auto-detected (or user-overridden) test command for the current
/// working directory. `null` means no test command could be detected.
#[tauri::command]
pub fn coding_agent_test_cmd() -> Option<String> {
    let cwd = aictl_core::security::policy().paths.working_dir.clone();
    aictl_core::coding::detect_test_cmd(&cwd)
}
