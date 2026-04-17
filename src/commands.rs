//! REPL slash-command dispatch.
//!
//! [`handle`] parses a `/command` token and returns a [`CommandResult`] that
//! the main REPL loop acts on. Simple commands that only print (help, copy,
//! tools) are handled inline; anything requiring access to the live
//! conversation state (messages, model, provider, UI) returns a variant the
//! REPL consumes.
//!
//! Each slash command lives in its own submodule so this file stays focused
//! on dispatch. Types and helpers that the rest of the crate uses
//! (`MemoryMode`, `compact`, `run_*_menu`, `select_*`, `print_*`, CLI
//! helpers) are re-exported below to preserve the `crate::commands::X`
//! paths that callers use.

mod agent;
mod behavior;
mod clipboard;
mod compact;
mod config_wizard;
mod gguf;
mod help;
mod history;
mod info;
mod keys;
mod memory;
mod menu;
mod mlx;
mod model;
mod retry;
mod security;
mod session;
mod stats;
mod tools;
mod uninstall;
mod update;

pub use agent::{print_agents_cli, run_agent_menu};
pub use behavior::select_behavior;
pub use compact::{compact, print_context};
pub use config_wizard::run_config_wizard;
pub use gguf::run_gguf_menu;
pub use history::print_history;
pub use info::print_info;
pub use keys::{run_clear_keys_unconfirmed, run_keys_menu, run_lock_keys, run_unlock_keys};
pub use memory::{MemoryMode, select_memory};
pub use mlx::run_mlx_menu;
pub use model::select_model;
pub use retry::retry_last_exchange;
pub use security::print_security;
pub use session::{print_sessions_cli, run_session_menu};
pub use stats::run_stats_menu;
pub use uninstall::{run_uninstall_cli, run_uninstall_repl};
pub use update::{run_update, run_update_cli, run_version};

/// All slash command names (without `/`), sorted alphabetically.
/// Used by the REPL tab completer.
pub const COMMANDS: &[&str] = &[
    "agent",
    "behavior",
    "clear",
    "compact",
    "config",
    "context",
    "copy",
    "exit",
    "help",
    "history",
    "info",
    "keys",
    "gguf",
    "memory",
    "mlx",
    "model",
    "retry",
    "security",
    "session",
    "stats",
    "tools",
    "uninstall",
    "update",
    "version",
];

/// Result of handling a slash command.
pub enum CommandResult {
    /// Exit the REPL.
    Exit,
    /// Clear conversation context and continue.
    Clear,
    /// Compact conversation context via LLM summarization.
    Compact,
    /// Show context usage info.
    Context,
    /// View / search the in-memory conversation. Carries the raw arg string
    /// after `/history` (e.g. `"user rust"`) for the consumer to parse.
    History(String),
    /// Show setup info (provider, model, version, etc.).
    Info,
    /// Show security policy status.
    Security,
    /// Switch model interactively.
    Model,
    /// Switch auto/human-in-the-loop behavior.
    Behavior,
    /// Switch memory mode (long-term/short-term).
    Memory,
    /// Update to the latest version.
    Update,
    /// Uninstall the aictl binary from every known install location.
    Uninstall,
    /// Check current version against the latest available.
    Version,
    /// Open the agent management menu.
    Agent,
    /// Open the session management menu.
    Session,
    /// Open the native GGUF model management menu.
    Gguf,
    /// Open the native MLX model management menu (Apple Silicon).
    Mlx,
    /// Open the API key management menu (lock/unlock/clear).
    Keys,
    /// Re-run the interactive configuration wizard.
    Config,
    /// Open the usage statistics menu (view/clear).
    Stats,
    /// Remove the last user/assistant exchange and retry it.
    Retry,
    /// Command handled, continue the loop.
    Continue,
    /// Not a slash command, proceed normally.
    NotACommand,
}

/// Handle slash command input. Returns how the REPL should proceed.
pub fn handle(input: &str, last_answer: &str, show_error: &dyn Fn(&str)) -> CommandResult {
    let Some(rest) = input.strip_prefix('/') else {
        return CommandResult::NotACommand;
    };
    let (cmd, args) = rest
        .split_once(char::is_whitespace)
        .map_or((rest, ""), |(c, a)| (c, a.trim()));

    match cmd {
        "exit" => CommandResult::Exit,
        "clear" => CommandResult::Clear,
        "compact" => CommandResult::Compact,
        "context" => CommandResult::Context,
        "history" => CommandResult::History(args.to_string()),
        "info" => CommandResult::Info,
        "agent" => CommandResult::Agent,
        "security" => CommandResult::Security,
        "model" => CommandResult::Model,
        "behavior" => CommandResult::Behavior,
        "memory" => CommandResult::Memory,
        "update" => CommandResult::Update,
        "uninstall" => CommandResult::Uninstall,
        "version" => CommandResult::Version,
        "session" => CommandResult::Session,
        "gguf" => CommandResult::Gguf,
        "mlx" => CommandResult::Mlx,
        "retry" => CommandResult::Retry,
        "copy" => {
            clipboard::copy_to_clipboard(last_answer, show_error);
            CommandResult::Continue
        }
        "help" => {
            help::print_help();
            CommandResult::Continue
        }
        "tools" => {
            tools::print_tools();
            CommandResult::Continue
        }
        "keys" => CommandResult::Keys,
        "config" => CommandResult::Config,
        "stats" => CommandResult::Stats,
        _ => {
            show_error("Unknown command. Type /help for available commands.");
            CommandResult::Continue
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noop_error(_msg: &str) {}

    #[test]
    fn cmd_exit() {
        assert!(matches!(
            handle("/exit", "", &noop_error),
            CommandResult::Exit
        ));
    }

    #[test]
    fn cmd_clear() {
        assert!(matches!(
            handle("/clear", "", &noop_error),
            CommandResult::Clear
        ));
    }

    #[test]
    fn cmd_compact() {
        assert!(matches!(
            handle("/compact", "", &noop_error),
            CommandResult::Compact
        ));
    }

    #[test]
    fn cmd_context() {
        assert!(matches!(
            handle("/context", "", &noop_error),
            CommandResult::Context
        ));
    }

    #[test]
    fn cmd_info() {
        assert!(matches!(
            handle("/info", "", &noop_error),
            CommandResult::Info
        ));
    }

    #[test]
    fn cmd_model() {
        assert!(matches!(
            handle("/model", "", &noop_error),
            CommandResult::Model
        ));
    }

    #[test]
    fn cmd_agent() {
        assert!(matches!(
            handle("/agent", "", &noop_error),
            CommandResult::Agent
        ));
    }

    #[test]
    fn cmd_behavior() {
        assert!(matches!(
            handle("/behavior", "", &noop_error),
            CommandResult::Behavior
        ));
    }

    #[test]
    fn cmd_memory() {
        assert!(matches!(
            handle("/memory", "", &noop_error),
            CommandResult::Memory
        ));
    }

    #[test]
    fn cmd_retry() {
        assert!(matches!(
            handle("/retry", "", &noop_error),
            CommandResult::Retry
        ));
    }

    #[test]
    fn cmd_history_no_args() {
        assert!(matches!(
            handle("/history", "", &noop_error),
            CommandResult::History(ref a) if a.is_empty()
        ));
    }

    #[test]
    fn cmd_history_with_args() {
        let res = handle("/history user rust", "", &noop_error);
        match res {
            CommandResult::History(args) => assert_eq!(args, "user rust"),
            _ => panic!("expected History variant"),
        }
    }

    #[test]
    fn cmd_unknown() {
        assert!(matches!(
            handle("/foo", "", &noop_error),
            CommandResult::Continue
        ));
    }

    #[test]
    fn cmd_not_a_command() {
        assert!(matches!(
            handle("hello", "", &noop_error),
            CommandResult::NotACommand
        ));
    }

    #[test]
    fn cmd_help_returns_continue() {
        assert!(matches!(
            handle("/help", "", &noop_error),
            CommandResult::Continue
        ));
    }

    #[test]
    fn cmd_tools_returns_continue() {
        assert!(matches!(
            handle("/tools", "", &noop_error),
            CommandResult::Continue
        ));
    }

    #[test]
    fn commands_list_matches_handler() {
        for cmd in COMMANDS {
            let input = format!("/{cmd}");
            assert!(
                !matches!(handle(&input, "", &noop_error), CommandResult::NotACommand),
                "/{cmd} should be recognized as a command"
            );
        }
    }
}
