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
mod balance;
mod behavior;
mod clipboard;
mod compact;
mod config_wizard;
mod gguf;
mod help;
mod history;
mod hooks;
mod info;
mod keys;
mod memory;
mod menu;
mod mlx;
mod model;
mod ping;
mod plugins;
mod retry;
mod roadmap;
mod security;
mod session;
mod skills;
mod stats;
mod tools;
mod undo;
mod uninstall;
mod update;

pub use agent::{load_agent_by_name, print_agents_cli, run_agent_menu};
pub use balance::run_balance;
pub use behavior::select_behavior;
pub use compact::{compact, print_context};
pub use config_wizard::run_config_wizard;
pub use gguf::run_gguf_menu;
pub use history::print_history;
pub use hooks::{print_hooks_cli, run_hooks_menu};
pub use info::print_info;
pub use keys::{run_clear_keys_unconfirmed, run_keys_menu, run_lock_keys, run_unlock_keys};
pub use memory::{MemoryMode, select_memory};
pub use mlx::run_mlx_menu;
pub use model::select_model;
pub use ping::run_ping;
pub use plugins::{print_plugins_cli, run_plugins_menu};
pub use retry::retry_last_exchange;
pub use roadmap::run_roadmap;
pub use security::print_security;
pub use session::{print_sessions_cli, run_session_menu};
pub use skills::{SkillsMenuOutcome, print_skills_cli, run_skills_menu};
pub use stats::run_stats_menu;
pub use undo::undo_turns;
pub use uninstall::{run_uninstall_cli, run_uninstall_repl};
pub use update::{run_update, run_update_cli, run_version};

/// All slash command names (without `/`), sorted alphabetically.
/// Used by the REPL tab completer.
pub const COMMANDS: &[&str] = &[
    "agent",
    "balance",
    "behavior",
    "clear",
    "compact",
    "config",
    "context",
    "copy",
    "exit",
    "help",
    "history",
    "hooks",
    "info",
    "keys",
    "gguf",
    "memory",
    "mlx",
    "model",
    "ping",
    "plugins",
    "retry",
    "roadmap",
    "security",
    "session",
    "skills",
    "stats",
    "tools",
    "undo",
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
    /// Switch model interactively. Carries an optional search query: when
    /// `Some(q)`, the handler should skip the top-level Browse/Search menu
    /// and jump straight to filtered results (used for `/model search <q>`
    /// and `/model <q>` scripted selection).
    Model(Option<String>),
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
    /// Open the agent management menu. When the user types `/agent <name>`,
    /// the parsed name is carried here and the REPL loads that agent directly
    /// instead of opening the menu. Name validation/existence checks happen
    /// at dispatch time.
    Agent(Option<String>),
    /// Probe every cloud provider's balance endpoint and print remaining
    /// credit. Async — the REPL drives the future on its tokio runtime.
    Balance,
    /// Open the skills management menu.
    Skills,
    /// Invoke a skill by name. `task` is the inline argument (may be empty —
    /// the REPL will prompt for a task in that case). The REPL looks up the
    /// skill body via [`crate::skills::find`] before handing it to
    /// [`crate::run::run_agent_turn`].
    InvokeSkill { name: String, task: String },
    /// Open the session management menu.
    Session,
    /// Open the native GGUF model management menu.
    Gguf,
    /// Open the native MLX model management menu (Apple Silicon).
    Mlx,
    /// Open the API key management menu (lock/unlock/clear).
    Keys,
    /// Open the plugins management menu (list manifests, toggle the
    /// `AICTL_PLUGINS_ENABLED` master switch).
    Plugins,
    /// Open the hooks management menu (list, toggle, test-fire).
    Hooks,
    /// Check connectivity and API key validity for all providers.
    Ping,
    /// Re-run the interactive configuration wizard.
    Config,
    /// Open the usage statistics menu (view/clear).
    Stats,
    /// Remove the last user/assistant exchange and retry it.
    Retry,
    /// Fetch and render the project roadmap. Carries an optional heading
    /// filter: `/roadmap desktop` jumps to the `## Desktop` section.
    Roadmap(Option<String>),
    /// Drop the last N turns from the conversation without re-running
    /// anything. Carries the requested count (defaults to `1` when the user
    /// typed `/undo` with no argument).
    Undo(usize),
    /// Command handled, continue the loop.
    Continue,
    /// Not a slash command, proceed normally.
    NotACommand,
}

/// Handle slash command input. Returns how the REPL should proceed.
#[allow(clippy::too_many_lines)]
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
        "agent" => {
            let name = if args.is_empty() {
                None
            } else {
                Some(args.to_string())
            };
            CommandResult::Agent(name)
        }
        "balance" => CommandResult::Balance,
        "security" => CommandResult::Security,
        "model" => {
            // `/model` → top-level Browse/Search menu.
            // `/model search <query>` or `/model <query>` → jump to filtered
            // results. The leading `search` token is optional — either form
            // is accepted so scripted callers can pick whichever reads best.
            let query = if args.is_empty() {
                None
            } else {
                let q = args
                    .strip_prefix("search")
                    .and_then(|rest| {
                        // Only strip the prefix when it was its own token,
                        // so `/model searchlight` stays as a literal query.
                        if rest.is_empty() || rest.starts_with(char::is_whitespace) {
                            Some(rest.trim())
                        } else {
                            None
                        }
                    })
                    .unwrap_or(args)
                    .trim();
                if q.is_empty() {
                    None
                } else {
                    Some(q.to_string())
                }
            };
            CommandResult::Model(query)
        }
        "behavior" => CommandResult::Behavior,
        "memory" => CommandResult::Memory,
        "update" => CommandResult::Update,
        "uninstall" => CommandResult::Uninstall,
        "version" => CommandResult::Version,
        "session" => CommandResult::Session,
        "gguf" => CommandResult::Gguf,
        "mlx" => CommandResult::Mlx,
        "ping" => CommandResult::Ping,
        "plugins" => CommandResult::Plugins,
        "hooks" => CommandResult::Hooks,
        "retry" => CommandResult::Retry,
        "roadmap" => {
            let query = if args.is_empty() {
                None
            } else {
                Some(args.to_string())
            };
            CommandResult::Roadmap(query)
        }
        "undo" => {
            let count = if args.is_empty() {
                1
            } else {
                match args.parse::<usize>() {
                    Ok(0) => {
                        show_error("/undo: count must be at least 1");
                        return CommandResult::Continue;
                    }
                    Ok(n) => n,
                    Err(_) => {
                        show_error("/undo: invalid count (expected a positive integer)");
                        return CommandResult::Continue;
                    }
                }
            };
            CommandResult::Undo(count)
        }
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
        "skills" => CommandResult::Skills,
        _ => {
            // Fall through to a user-defined skill with this name. The
            // dispatcher only checks whether the SKILL.md exists — the REPL
            // reloads the body before the turn so edits take effect live.
            if crate::skills::find(cmd).is_some() {
                return CommandResult::InvokeSkill {
                    name: cmd.to_string(),
                    task: args.to_string(),
                };
            }
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
            CommandResult::Model(None)
        ));
    }

    #[test]
    fn cmd_model_with_search_prefix_strips_the_keyword() {
        match handle("/model search opus", "", &noop_error) {
            CommandResult::Model(Some(q)) => assert_eq!(q, "opus"),
            _ => panic!("expected Model(Some(_))"),
        }
    }

    #[test]
    fn cmd_model_with_bare_query_is_treated_as_search() {
        match handle("/model claude", "", &noop_error) {
            CommandResult::Model(Some(q)) => assert_eq!(q, "claude"),
            _ => panic!("expected Model(Some(_))"),
        }
    }

    #[test]
    fn cmd_model_does_not_strip_prefix_when_token_is_glued() {
        // `/model searchlight` should search for "searchlight", not "light".
        match handle("/model searchlight", "", &noop_error) {
            CommandResult::Model(Some(q)) => assert_eq!(q, "searchlight"),
            _ => panic!("expected Model(Some(_))"),
        }
    }

    #[test]
    fn cmd_model_multi_word_query() {
        match handle("/model anthropic opus", "", &noop_error) {
            CommandResult::Model(Some(q)) => assert_eq!(q, "anthropic opus"),
            _ => panic!("expected Model(Some(_))"),
        }
    }

    #[test]
    fn cmd_model_search_with_empty_query_is_treated_as_no_query() {
        assert!(matches!(
            handle("/model search", "", &noop_error),
            CommandResult::Model(None)
        ));
        assert!(matches!(
            handle("/model search   ", "", &noop_error),
            CommandResult::Model(None)
        ));
    }

    #[test]
    fn cmd_agent() {
        assert!(matches!(
            handle("/agent", "", &noop_error),
            CommandResult::Agent(None)
        ));
    }

    #[test]
    fn cmd_agent_with_name_loads_direct() {
        match handle("/agent reviewer", "", &noop_error) {
            CommandResult::Agent(Some(name)) => assert_eq!(name, "reviewer"),
            _ => panic!("expected Agent(Some(_))"),
        }
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
    fn cmd_ping() {
        assert!(matches!(
            handle("/ping", "", &noop_error),
            CommandResult::Ping
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
    fn cmd_roadmap_no_args() {
        assert!(matches!(
            handle("/roadmap", "", &noop_error),
            CommandResult::Roadmap(None)
        ));
    }

    #[test]
    fn cmd_roadmap_with_section_arg() {
        match handle("/roadmap desktop", "", &noop_error) {
            CommandResult::Roadmap(Some(q)) => assert_eq!(q, "desktop"),
            _ => panic!("expected Roadmap(Some(_))"),
        }
    }

    #[test]
    fn cmd_undo_no_args_defaults_to_one() {
        assert!(matches!(
            handle("/undo", "", &noop_error),
            CommandResult::Undo(1)
        ));
    }

    #[test]
    fn cmd_undo_with_count() {
        assert!(matches!(
            handle("/undo 3", "", &noop_error),
            CommandResult::Undo(3)
        ));
    }

    #[test]
    fn cmd_undo_zero_is_rejected() {
        assert!(matches!(
            handle("/undo 0", "", &noop_error),
            CommandResult::Continue
        ));
    }

    #[test]
    fn cmd_undo_non_numeric_is_rejected() {
        assert!(matches!(
            handle("/undo foo", "", &noop_error),
            CommandResult::Continue
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
