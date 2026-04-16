use std::io::Write;
use std::sync::atomic::{AtomicU32, Ordering};

use crossterm::style::{Color, Stylize};

static MANUAL_COMPACTIONS: AtomicU32 = AtomicU32::new(0);
static AUTO_COMPACTIONS: AtomicU32 = AtomicU32::new(0);

pub fn compaction_counts() -> (u32, u32) {
    (
        MANUAL_COMPACTIONS.load(Ordering::Relaxed),
        AUTO_COMPACTIONS.load(Ordering::Relaxed),
    )
}

use crate::llm;
use crate::llm::MODELS;
use crate::ui::AgentUI;
use crate::{Message, Provider, Role};

/// Memory mode: controls conversation history optimization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryMode {
    /// All messages, no optimization.
    LongTerm,
    /// Sliding window with most recent messages and optional compaction.
    ShortTerm,
}

impl std::fmt::Display for MemoryMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LongTerm => write!(f, "long-term"),
            Self::ShortTerm => write!(f, "short-term"),
        }
    }
}

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
    "info",
    "issues",
    "keys",
    "gguf",
    "memory",
    "mlx",
    "model",
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
    /// Fetch and display known issues.
    Issues,
    /// Open the API key management menu (lock/unlock/clear).
    Keys,
    /// Re-run the interactive configuration wizard.
    Config,
    /// Open the usage statistics menu (view/clear).
    Stats,
    /// Command handled, continue the loop.
    Continue,
    /// Not a slash command, proceed normally.
    NotACommand,
}

/// Handle slash command input. Returns how the REPL should proceed.
pub fn handle(input: &str, last_answer: &str, show_error: &dyn Fn(&str)) -> CommandResult {
    let Some(cmd) = input.strip_prefix('/') else {
        return CommandResult::NotACommand;
    };

    match cmd {
        "exit" => CommandResult::Exit,
        "clear" => CommandResult::Clear,
        "compact" => CommandResult::Compact,
        "context" => CommandResult::Context,
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
        "issues" => CommandResult::Issues,
        "copy" => {
            copy_to_clipboard(last_answer, show_error);
            CommandResult::Continue
        }
        "help" => {
            print_help();
            CommandResult::Continue
        }
        "tools" => {
            print_tools();
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

fn copy_to_clipboard(text: &str, show_error: &dyn Fn(&str)) {
    use std::io::Write;
    use std::process::{Command, Stdio};

    if text.is_empty() {
        show_error("Nothing to copy yet.");
        return;
    }

    match Command::new("pbcopy").stdin(Stdio::piped()).spawn() {
        Ok(mut child) => {
            if let Some(ref mut stdin) = child.stdin {
                let _ = stdin.write_all(text.as_bytes());
            }
            match child.wait() {
                Ok(_) => {
                    println!();
                    println!("  {} copied to clipboard", "✓".with(Color::Green));
                    println!();
                }
                Err(e) => show_error(&format!("Clipboard error: {e}")),
            }
        }
        Err(e) => show_error(&format!("Failed to run pbcopy: {e}")),
    }
}

#[allow(clippy::too_many_lines)]
pub async fn compact(
    provider: &Provider,
    api_key: &str,
    model: &str,
    messages: &mut Vec<Message>,
    ui: &dyn AgentUI,
    memory: &str,
    is_auto: bool,
) {
    if messages.len() <= 1 {
        ui.show_error("Nothing to compact.");
        return;
    }

    ui.start_spinner("compacting context...");

    let mut summary_msgs = messages.clone();
    summary_msgs.push(Message {
        role: Role::User,
        content: "Summarize our conversation so far in a compact form. \
            Include all key facts, decisions, code changes, file paths, \
            and open tasks so we can continue without losing context. \
            Be concise but thorough."
            .to_string(),
        images: vec![],
    });

    let result = match provider {
        Provider::Openai => {
            crate::with_esc_cancel(crate::llm_openai::call_openai(
                api_key,
                model,
                &summary_msgs,
            ))
            .await
        }
        Provider::Anthropic => {
            crate::with_esc_cancel(crate::llm_anthropic::call_anthropic(
                api_key,
                model,
                &summary_msgs,
            ))
            .await
        }
        Provider::Gemini => {
            crate::with_esc_cancel(crate::llm_gemini::call_gemini(
                api_key,
                model,
                &summary_msgs,
            ))
            .await
        }
        Provider::Grok => {
            crate::with_esc_cancel(crate::llm_grok::call_grok(api_key, model, &summary_msgs)).await
        }
        Provider::Mistral => {
            crate::with_esc_cancel(crate::llm_mistral::call_mistral(
                api_key,
                model,
                &summary_msgs,
            ))
            .await
        }
        Provider::Deepseek => {
            crate::with_esc_cancel(crate::llm_deepseek::call_deepseek(
                api_key,
                model,
                &summary_msgs,
            ))
            .await
        }
        Provider::Kimi => {
            crate::with_esc_cancel(crate::llm_kimi::call_kimi(api_key, model, &summary_msgs)).await
        }
        Provider::Zai => {
            crate::with_esc_cancel(crate::llm_zai::call_zai(api_key, model, &summary_msgs)).await
        }
        Provider::Ollama => {
            crate::with_esc_cancel(crate::llm_ollama::call_ollama(model, &summary_msgs)).await
        }
        Provider::Gguf => {
            crate::with_esc_cancel(crate::llm_gguf::call_gguf(model, &summary_msgs)).await
        }
        Provider::Mlx => {
            crate::with_esc_cancel(crate::llm_mlx::call_mlx(model, &summary_msgs)).await
        }
    };

    ui.stop_spinner();

    let result = match result {
        Ok(inner) => inner,
        Err(_interrupted) => {
            println!("\n  {} interrupted\n", "✗".with(Color::Yellow));
            return;
        }
    };

    match result {
        Ok((summary, usage)) => {
            let system = messages[0].clone();
            messages.clear();
            messages.push(system);
            messages.push(Message {
                role: Role::User,
                content: format!("Here is a summary of our conversation so far:\n\n{summary}"),
                images: vec![],
            });
            messages.push(Message {
                role: Role::Assistant,
                content: "Understood. I have the context from our previous \
                    conversation. How can I help you next?"
                    .to_string(),
                images: vec![],
            });
            println!();
            ui.show_token_usage(
                &usage,
                model,
                false,
                0,
                std::time::Duration::ZERO,
                0,
                memory,
            );
            if is_auto {
                AUTO_COMPACTIONS.fetch_add(1, Ordering::Relaxed);
            } else {
                MANUAL_COMPACTIONS.fetch_add(1, Ordering::Relaxed);
            }
            println!("  {} context compacted", "✓".with(Color::Green));
            println!();
        }
        Err(e) => ui.show_error(&format!("Compact failed: {e}")),
    }
}

pub fn print_context(
    model: &str,
    messages_len: usize,
    last_input_tokens: u64,
    max_messages: usize,
) {
    let limit = llm::context_limit(model);
    let token_pct = llm::pct(last_input_tokens, limit);
    let message_pct = llm::pct_usize(messages_len, max_messages);
    let context_pct = token_pct.max(message_pct).min(100);

    let bar_width = 30;
    let filled = (context_pct as usize * bar_width / 100).min(bar_width);
    let empty = bar_width - filled;
    let bar_color = if context_pct >= 80 {
        Color::Red
    } else if context_pct >= 50 {
        Color::Yellow
    } else {
        Color::Green
    };

    println!();
    println!(
        "  {} {}{} {context_pct}%",
        format!("{:<13}", "context:").with(Color::Cyan),
        "█".repeat(filled).with(bar_color),
        "░".repeat(empty).with(Color::DarkGrey),
    );
    println!(
        "  {} {last_input_tokens} / {limit}",
        format!("{:<13}", "tokens:").with(Color::DarkGrey),
    );
    println!(
        "  {} {messages_len} / {max_messages}",
        format!("{:<13}", "messages:").with(Color::DarkGrey),
    );
    let (manual, auto) = compaction_counts();
    println!(
        "  {} manual: {manual}, auto: {auto}",
        format!("{:<13}", "compactions:").with(Color::DarkGrey),
    );
    let threshold = crate::config::auto_compact_threshold();
    let source = if crate::config::config_get("AICTL_AUTO_COMPACT_THRESHOLD")
        .and_then(|v| v.parse::<u8>().ok())
        .filter(|v| (1..=100).contains(v))
        .is_some()
    {
        "config"
    } else {
        "default"
    };
    println!(
        "  {} {threshold}% ({source})",
        format!("{:<13}", "auto-compact:").with(Color::DarkGrey),
    );
    println!();
}

fn print_help() {
    let entries: &[(&str, &str)] = &[
        ("/agent", "manage agents"),
        ("/clear", "clear conversation context"),
        ("/compact", "compact context into a summary"),
        ("/context", "show context usage"),
        ("/copy", "copy last response to clipboard"),
        ("/help", "show this help message"),
        ("/info", "show setup info"),
        ("/issues", "show known issues"),
        ("/gguf", "manage native local GGUF models [experimental]"),
        (
            "/mlx",
            "manage native MLX models (Apple Silicon) [experimental]",
        ),
        ("/behavior", "switch auto/human-in-the-loop behavior"),
        ("/model", "switch model and provider"),
        ("/security", "show security policy"),
        ("/session", "manage sessions"),
        ("/stats", "view and manage usage statistics"),
        ("/memory", "switch memory mode (long-term/short-term)"),
        ("/tools", "show available tools"),
        ("/keys", "manage API keys (lock, unlock, clear)"),
        ("/config", "re-run the configuration wizard"),
        ("/update", "update to the latest version"),
        (
            "/uninstall",
            "remove the aictl binary (asks for confirmation)",
        ),
        ("/version", "check current version against the latest"),
        ("/exit", "exit the REPL"),
    ];
    let max_len = entries.iter().map(|(c, _)| c.len()).max().unwrap_or(0);
    println!();
    for (cmd, desc) in entries {
        let pad = max_len - cmd.len() + 2;
        println!("  {}{:pad$}{desc}", cmd.with(Color::Cyan), "");
    }
    println!();
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
    fn cmd_issues() {
        assert!(matches!(
            handle("/issues", "", &noop_error),
            CommandResult::Issues
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

pub fn print_security() {
    use crate::keys;

    let summary = crate::security::policy_summary();
    let max_label = summary
        .iter()
        .map(|(k, _)| k.len())
        .chain(std::iter::once("key storage".len()))
        .chain(keys::KEY_NAMES.iter().map(|n| n.len()))
        .max()
        .unwrap_or(0);
    println!();
    for (key, value) in &summary {
        let pad = max_label - key.len() + 2;
        println!("  {}:{:pad$}{}", key.as_str().with(Color::Cyan), "", value);
    }
    print_key_storage(max_label);
}

/// Print the current API key storage backend and per-key status.
fn print_key_storage(max_label: usize) {
    use crate::keys::{self, KeyLocation};

    let backend = keys::backend_name();
    let (locked, plain, both, _unset) = keys::counts();
    let key = "key storage";
    let pad = max_label - key.len() + 2;
    println!(
        "  {}:{:pad$}{} {}",
        key.with(Color::Cyan),
        "",
        backend.with(Color::Green),
        format!("({locked} locked · {plain} plain · {both} both)").with(Color::DarkGrey),
    );
    for (name, loc) in keys::all_locations() {
        let label = loc.label();
        let color = match loc {
            KeyLocation::Keyring => Color::Green,
            KeyLocation::Config => Color::Yellow,
            KeyLocation::Both => Color::Red,
            KeyLocation::None => Color::DarkGrey,
        };
        let pad = max_label - name.len() + 3;
        println!(
            "  {}{:pad$}{}",
            name.with(Color::DarkGrey),
            "",
            label.with(color),
        );
    }
    println!();
}

/// Print a single stats section (today / this month / overall).
fn print_stats_section(label: &str, stats: &crate::stats::DayStats) {
    println!(
        "  {}",
        label
            .with(Color::Cyan)
            .attribute(crossterm::style::Attribute::Bold),
    );
    println!(
        "    {} {}",
        format!("{:<15}", "sessions:").with(Color::DarkGrey),
        stats.sessions,
    );
    println!(
        "    {} {}",
        format!("{:<15}", "requests:").with(Color::DarkGrey),
        stats.requests,
    );
    println!(
        "    {} {}",
        format!("{:<15}", "llm calls:").with(Color::DarkGrey),
        stats.llm_calls,
    );
    println!(
        "    {} {}",
        format!("{:<15}", "tool calls:").with(Color::DarkGrey),
        stats.tool_calls,
    );
    println!(
        "    {} {}",
        format!("{:<15}", "input tokens:").with(Color::DarkGrey),
        format_token_count(stats.input_tokens),
    );
    println!(
        "    {} {}",
        format!("{:<15}", "output tokens:").with(Color::DarkGrey),
        format_token_count(stats.output_tokens),
    );
    println!(
        "    {} {}",
        format!("{:<15}", "cost:").with(Color::DarkGrey),
        format_cost(stats.cost_usd),
    );
    if !stats.models.is_empty() {
        let mut models: Vec<_> = stats.models.iter().collect();
        models.sort_by(|a, b| b.1.cmp(a.1));
        println!(
            "    {} {} ({})",
            format!("{:<15}", "models:").with(Color::DarkGrey),
            models[0].0,
            models[0].1,
        );
        for (model, count) in models.iter().skip(1) {
            println!("    {:<15} {} ({})", "", model, count);
        }
    }
}

#[allow(clippy::cast_precision_loss)]
fn format_token_count(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}K", tokens as f64 / 1_000.0)
    } else {
        format!("{tokens}")
    }
}

fn format_cost(cost: f64) -> String {
    if cost < 0.01 {
        format!("${cost:.4}")
    } else {
        format!("${cost:.2}")
    }
}

/// Display usage statistics: today, this month, overall.
pub fn print_stats() {
    let today = crate::stats::today();
    let month = crate::stats::this_month();
    let overall = crate::stats::overall();
    let days = crate::stats::day_count();

    println!();
    print_stats_section("Today", &today);
    println!();
    print_stats_section("This month", &month);
    println!();
    print_stats_section(&format!("Overall ({days} days)"), &overall);
    println!();
}

/// Clear all stats after user confirmation.
pub fn run_clear_stats(_show_error: &dyn Fn(&str)) {
    println!();
    if !confirm_yn("clear ALL usage statistics?") {
        return;
    }
    crate::stats::clear_all();
    println!("  {} statistics cleared", "✓".with(Color::Green));
    println!();
}

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

const STATS_MENU_ITEMS: &[(&str, &str)] = &[
    ("view stats", "show today / this month / overall"),
    ("clear stats", "remove all usage statistics"),
];

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

fn build_simple_menu_lines(items: &[(&str, &str)], selected: usize) -> Vec<String> {
    let max_name = items.iter().map(|(n, _)| n.len()).max().unwrap_or(0);
    items
        .iter()
        .enumerate()
        .map(|(i, (name, desc))| {
            let is_selected = i == selected;
            let padded = format!("{name:<max_name$}");
            let name_styled = if is_selected {
                format!(
                    "  {}",
                    padded
                        .with(Color::White)
                        .attribute(crossterm::style::Attribute::Bold)
                )
            } else {
                format!("  {}", padded.with(Color::DarkGrey))
            };
            let desc_styled = format!("{}", desc.with(Color::DarkGrey));
            if is_selected {
                format!("  {} {name_styled}  {desc_styled}", "›".with(Color::Cyan))
            } else {
                format!("    {name_styled}  {desc_styled}")
            }
        })
        .collect()
}

/// Open the `/stats` interactive menu (view or clear).
pub fn run_stats_menu(show_error: &dyn Fn(&str)) {
    let Some(sel) = select_from_menu(STATS_MENU_ITEMS.len(), 0, |s| {
        build_simple_menu_lines(STATS_MENU_ITEMS, s)
    }) else {
        return;
    };
    match sel {
        0 => print_stats(),
        1 => run_clear_stats(show_error),
        _ => {}
    }
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

fn print_tools() {
    let tools: &[(&str, &str)] = &[
        ("exec_shell", "execute a shell command via sh -c"),
        ("read_file", "read the contents of a file"),
        ("write_file", "write content to a file"),
        ("remove_file", "remove (delete) a file"),
        ("edit_file", "edit a file with find-and-replace"),
        (
            "create_directory",
            "create a directory and any missing parents",
        ),
        ("list_directory", "list files and directories at a path"),
        ("search_files", "search file contents by pattern"),
        ("find_files", "find files matching a glob pattern"),
        ("search_web", "search the web via Firecrawl API"),
        ("fetch_url", "fetch a URL and return text content"),
        ("extract_website", "extract readable content from a URL"),
        ("fetch_datetime", "get current date, time, and timezone"),
        (
            "fetch_geolocation",
            "get geolocation data for an IP address",
        ),
        ("read_image", "read an image from file or URL for analysis"),
        (
            "generate_image",
            "generate an image from text (DALL-E/Imagen/Grok)",
        ),
        (
            "read_document",
            "read a PDF, DOCX, or spreadsheet as markdown",
        ),
    ];
    let enabled = crate::tools::tools_enabled();
    let max_len = tools.iter().map(|(n, _)| n.len()).max().unwrap_or(0);
    println!();
    if !enabled {
        println!(
            "  {}",
            "all tools disabled (AICTL_TOOLS_ENABLED=false)".with(Color::Yellow)
        );
        println!();
    }
    for (name, desc) in tools {
        let pad = max_len - name.len() + 2;
        println!("  {}{:pad$}{desc}", name.with(Color::Cyan), "");
    }
    println!();
}

/// Build the display lines for the model menu. Each entry is either a
/// header line (provider name) or a model line with its index into MODELS.
/// Returns `(lines, model_indices)` where `model_indices[i]` maps selectable
/// row `i` to its position in MODELS.
/// A combined model entry used for building the menu (static + dynamic Ollama models).
struct MenuModel {
    provider: String,
    model: String,
    api_key_name: String,
}

fn build_combined_models(
    ollama_models: &[String],
    local_models: &[String],
    mlx_models: &[String],
) -> Vec<MenuModel> {
    let mut combined: Vec<MenuModel> = MODELS
        .iter()
        .map(|(prov, model, key)| MenuModel {
            provider: (*prov).to_string(),
            model: (*model).to_string(),
            api_key_name: (*key).to_string(),
        })
        .collect();

    for m in ollama_models {
        combined.push(MenuModel {
            provider: "ollama".to_string(),
            model: m.clone(),
            api_key_name: String::new(),
        });
    }

    for m in local_models {
        combined.push(MenuModel {
            provider: "gguf".to_string(),
            model: m.clone(),
            api_key_name: String::new(),
        });
    }

    for m in mlx_models {
        combined.push(MenuModel {
            provider: "mlx".to_string(),
            model: m.clone(),
            api_key_name: String::new(),
        });
    }

    combined
}

fn build_menu_lines(
    selected: usize,
    current_model: &str,
    models: &[MenuModel],
) -> (Vec<String>, Vec<usize>) {
    let mut lines = Vec::new();
    let mut model_indices = Vec::new();

    for (sel_row, (i, entry)) in models.iter().enumerate().enumerate() {
        // Print provider header when provider changes
        if i == 0 || models[i - 1].provider != entry.provider {
            let label = match entry.provider.as_str() {
                "anthropic" => "Anthropic:",
                "openai" => "OpenAI:",
                "gemini" => "Gemini:",
                "grok" => "Grok:",
                "mistral" => "Mistral:",
                "deepseek" => "DeepSeek:",
                "kimi" => "Kimi:",
                "zai" => "Z.ai:",
                "ollama" => "Ollama:",
                "gguf" => "Native GGUF:",
                "mlx" => "MLX (Apple Silicon):",
                _ => entry.provider.as_str(),
            };
            lines.push(format!("  {}", label.with(Color::Cyan)));
        }

        let is_selected = sel_row == selected;
        let is_current = entry.model == current_model;

        let marker = if is_current { "●" } else { " " };
        let name = if is_selected {
            format!(
                "       {} {}",
                marker.with(Color::Green),
                entry
                    .model
                    .as_str()
                    .with(Color::White)
                    .attribute(crossterm::style::Attribute::Bold)
            )
        } else {
            format!(
                "       {} {}",
                marker.with(Color::Green),
                entry.model.as_str().with(Color::DarkGrey)
            )
        };

        let line = if is_selected {
            format!("  {} {name}", "›".with(Color::Cyan))
        } else {
            format!("    {name}")
        };

        lines.push(line);
        model_indices.push(i);
    }

    (lines, model_indices)
}

/// Generic arrow-key menu selector with viewport scrolling.
/// `item_count` is the number of selectable items, `initial_selected` is the
/// starting index, and `build_lines` returns the display lines for a given
/// selected index.  Returns `Some(selected_index)` or `None` if cancelled.
#[allow(clippy::cast_possible_truncation, clippy::too_many_lines)]
fn select_from_menu<F>(item_count: usize, initial_selected: usize, build_lines: F) -> Option<usize>
where
    F: Fn(usize) -> Vec<String>,
{
    use crossterm::{
        cursor,
        event::{self, Event, KeyCode, KeyEventKind},
        execute,
        terminal::{self, ClearType},
    };

    let mut selected = initial_selected;
    let mut scroll_offset: usize = 0;

    let _ = terminal::enable_raw_mode();
    let mut stdout = std::io::stdout();
    let _ = execute!(stdout, cursor::Hide);

    // Determine how many menu lines fit in the terminal.
    // Reserve 4 lines: 1 top blank, 1 bottom blank, 1 help text, 1 safety margin.
    let term_height = terminal::size().map_or(24, |(_, h)| h as usize);
    let max_visible = term_height.saturating_sub(4);

    let render = |stdout: &mut std::io::Stdout,
                  lines: &[String],
                  scroll_offset: &mut usize,
                  prev_rendered: usize| {
        // Find the selected line (marked with ›) and keep it in view.
        let selected_line = lines.iter().position(|l| l.contains('›')).unwrap_or(0);
        let total = lines.len();
        let viewport = max_visible.min(total);

        if viewport < total {
            if selected_line < *scroll_offset {
                *scroll_offset = selected_line;
            } else if selected_line >= *scroll_offset + viewport {
                *scroll_offset = selected_line + 1 - viewport;
            }
            // Clamp
            if *scroll_offset + viewport > total {
                *scroll_offset = total - viewport;
            }
        } else {
            *scroll_offset = 0;
        }

        let has_above = *scroll_offset > 0;
        let has_below = *scroll_offset + viewport < total;

        // Clear previous render
        if prev_rendered > 0 {
            let _ = execute!(
                stdout,
                cursor::MoveUp(prev_rendered as u16),
                terminal::Clear(ClearType::FromCursorDown),
            );
        }

        // Scroll indicator above
        if has_above {
            let _ = write!(
                stdout,
                "  {}\r\n",
                format!("↑ {} more", *scroll_offset).with(Color::DarkGrey)
            );
        }

        // Visible lines
        for line in &lines[*scroll_offset..*scroll_offset + viewport] {
            let _ = write!(stdout, "{line}\r\n");
        }

        // Scroll indicator below
        if has_below {
            let remaining = total - (*scroll_offset + viewport);
            let _ = write!(
                stdout,
                "  {}\r\n",
                format!("↓ {remaining} more").with(Color::DarkGrey)
            );
        }

        // Help text
        let _ = write!(
            stdout,
            "\r\n  {}\r\n",
            "↑/↓ navigate · enter select · esc cancel".with(Color::DarkGrey)
        );
        let _ = stdout.flush();

        // Return number of rendered lines for cleanup
        viewport + usize::from(has_above) + usize::from(has_below) + 2 // blank + help text
    };

    // Initial render
    let lines = build_lines(selected);
    let _ = execute!(stdout, cursor::MoveToColumn(0));
    let _ = write!(stdout, "\r\n");
    let mut total_rendered_lines = render(&mut stdout, &lines, &mut scroll_offset, 0);

    loop {
        if !event::poll(std::time::Duration::from_millis(100)).unwrap_or(false) {
            continue;
        }
        let Ok(ev) = event::read() else { break };

        match ev {
            Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                KeyCode::Up => {
                    selected = selected.saturating_sub(1);
                }
                KeyCode::Down => {
                    if selected + 1 < item_count {
                        selected += 1;
                    }
                }
                KeyCode::Enter => {
                    let _ = execute!(
                        stdout,
                        cursor::MoveUp(total_rendered_lines as u16),
                        terminal::Clear(ClearType::FromCursorDown),
                        cursor::Show,
                    );
                    let _ = terminal::disable_raw_mode();
                    return Some(selected);
                }
                KeyCode::Esc => {
                    let _ = execute!(
                        stdout,
                        cursor::MoveUp(total_rendered_lines as u16),
                        terminal::Clear(ClearType::FromCursorDown),
                        cursor::Show,
                    );
                    let _ = terminal::disable_raw_mode();
                    return None;
                }
                _ => continue,
            },
            _ => continue,
        }

        let lines = build_lines(selected);
        total_rendered_lines = render(
            &mut stdout,
            &lines,
            &mut scroll_offset,
            total_rendered_lines,
        );
    }

    let _ = execute!(stdout, cursor::Show);
    let _ = terminal::disable_raw_mode();
    None
}

/// Interactively select a model with arrow keys.
/// `ollama_models` are dynamically fetched model names (empty if Ollama is not running).
/// Returns (Provider, `model_name`, `api_key_config_key`) or None if cancelled (Esc).
pub fn select_model(
    current_model: &str,
    ollama_models: &[String],
    local_models: &[String],
    mlx_models: &[String],
) -> Option<(Provider, String, String)> {
    let combined = build_combined_models(ollama_models, local_models, mlx_models);
    let initial = combined
        .iter()
        .position(|m| m.model == current_model)
        .unwrap_or(0);
    let selected = select_from_menu(combined.len(), initial, |sel| {
        build_menu_lines(sel, current_model, &combined).0
    })?;
    let entry = &combined[selected];
    let provider = match entry.provider.as_str() {
        "openai" => Provider::Openai,
        "anthropic" => Provider::Anthropic,
        "gemini" => Provider::Gemini,
        "grok" => Provider::Grok,
        "mistral" => Provider::Mistral,
        "deepseek" => Provider::Deepseek,
        "kimi" => Provider::Kimi,
        "zai" => Provider::Zai,
        "ollama" => Provider::Ollama,
        "gguf" => Provider::Gguf,
        "mlx" => Provider::Mlx,
        _ => unreachable!(),
    };
    Some((provider, entry.model.clone(), entry.api_key_name.clone()))
}

const BEHAVIORS: &[(&str, &str)] = &[
    (
        "human-in-the-loop",
        "ask confirmation before each tool call",
    ),
    ("auto", "run tools without confirmation"),
];

fn build_behavior_menu_lines(selected: usize, current_auto: bool) -> Vec<String> {
    let mut lines = Vec::new();
    let max_name = BEHAVIORS.iter().map(|(n, _)| n.len()).max().unwrap_or(0);
    for (i, (name, desc)) in BEHAVIORS.iter().enumerate() {
        let is_selected = i == selected;
        let is_current = (*name == "auto") == current_auto;

        let marker = if is_current { "●" } else { " " };
        let padded = format!("{:<max_name$}", *name);
        let name_styled = if is_selected {
            format!(
                "{} {}",
                marker.with(Color::Green),
                padded
                    .with(Color::White)
                    .attribute(crossterm::style::Attribute::Bold)
            )
        } else {
            format!(
                "{} {}",
                marker.with(Color::Green),
                padded.with(Color::DarkGrey)
            )
        };

        let desc_styled = format!("{}", desc.with(Color::DarkGrey));

        let line = if is_selected {
            format!("  {} {name_styled}  {desc_styled}", "›".with(Color::Cyan))
        } else {
            format!("    {name_styled}  {desc_styled}")
        };

        lines.push(line);
    }
    lines
}

/// Interactively select auto/human-in-the-loop behavior with arrow keys.
/// Returns `Some(auto_bool)` or `None` if cancelled (Esc).
pub fn select_behavior(current_auto: bool) -> Option<bool> {
    let initial = usize::from(current_auto);
    let selected = select_from_menu(BEHAVIORS.len(), initial, |sel| {
        build_behavior_menu_lines(sel, current_auto)
    })?;
    Some(BEHAVIORS[selected].0 == "auto")
}

const MEMORY_MODES: &[(&str, &str)] = &[
    ("long-term", "all messages, no optimization"),
    ("short-term", "sliding window with recent messages"),
];

fn build_memory_menu_lines(selected: usize, current: MemoryMode) -> Vec<String> {
    let mut lines = Vec::new();
    let max_name = MEMORY_MODES.iter().map(|(n, _)| n.len()).max().unwrap_or(0);
    for (i, (name, desc)) in MEMORY_MODES.iter().enumerate() {
        let is_selected = i == selected;
        let is_current = (*name == "long-term" && current == MemoryMode::LongTerm)
            || (*name == "short-term" && current == MemoryMode::ShortTerm);

        let marker = if is_current { "●" } else { " " };
        let padded = format!("{:<max_name$}", *name);
        let name_styled = if is_selected {
            format!(
                "{} {}",
                marker.with(Color::Green),
                padded
                    .with(Color::White)
                    .attribute(crossterm::style::Attribute::Bold)
            )
        } else {
            format!(
                "{} {}",
                marker.with(Color::Green),
                padded.with(Color::DarkGrey)
            )
        };

        let desc_styled = format!("{}", desc.with(Color::DarkGrey));

        let line = if is_selected {
            format!("  {} {name_styled}  {desc_styled}", "›".with(Color::Cyan))
        } else {
            format!("    {name_styled}  {desc_styled}")
        };

        lines.push(line);
    }
    lines
}

/// Interactively select memory mode with arrow keys.
/// Returns `Some(MemoryMode)` or `None` if cancelled (Esc).
pub fn select_memory(current: MemoryMode) -> Option<MemoryMode> {
    let initial = match current {
        MemoryMode::LongTerm => 0,
        MemoryMode::ShortTerm => 1,
    };
    let selected = select_from_menu(MEMORY_MODES.len(), initial, |sel| {
        build_memory_menu_lines(sel, current)
    })?;
    Some(match MEMORY_MODES[selected].0 {
        "short-term" => MemoryMode::ShortTerm,
        _ => MemoryMode::LongTerm,
    })
}

#[allow(clippy::too_many_lines)]
pub fn print_info(
    provider: &str,
    model: &str,
    auto: bool,
    memory: MemoryMode,
    version_info: &str,
    ollama_models: &[String],
) {
    let version = crate::VERSION;
    let behavior = if auto { "auto" } else { "human-in-the-loop" };
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    let current_exe = std::env::current_exe().ok();
    let binary_path = current_exe
        .as_ref()
        .map_or_else(|| "unknown".to_string(), |p| p.display().to_string());
    let binary_size = current_exe
        .as_ref()
        .and_then(|p| std::fs::metadata(p).ok())
        .map_or_else(
            || "unknown".to_string(),
            #[allow(clippy::cast_precision_loss)]
            |m| {
                let bytes = m.len();
                if bytes >= 1_048_576 {
                    format!("{:.1} MB", bytes as f64 / 1_048_576.0)
                } else {
                    format!("{:.1} KB", bytes as f64 / 1_024.0)
                }
            },
        );

    let version_display = if version_info.is_empty() {
        version.to_string()
    } else {
        let version_color = if version_info.contains("latest") {
            Color::Green
        } else {
            Color::Yellow
        };
        format!("{version} {}", version_info.with(version_color))
    };

    println!();
    println!("  {} {version_display}", "version:  ".with(Color::Cyan));
    println!(
        "  {} {}",
        "build:    ".with(Color::Cyan),
        env!("AICTL_BUILD_DATETIME")
    );
    println!("  {} {provider}", "provider: ".with(Color::Cyan));
    println!("  {} {model}", "model:    ".with(Color::Cyan));
    println!("  {} {behavior}", "behavior: ".with(Color::Cyan));
    println!("  {} {memory}", "memory:   ".with(Color::Cyan));
    let prompt_file = crate::config::load_prompt_file();
    let prompt_file_name =
        crate::config::config_get("AICTL_PROMPT_FILE").unwrap_or_else(|| "AICTL.md".to_string());
    let prompt_info = if prompt_file.is_some() {
        format!("{prompt_file_name} (loaded)")
    } else {
        format!("{prompt_file_name} (not found)")
    };

    println!("  {} {os}/{arch}", "os:       ".with(Color::Cyan));
    println!("  {} {binary_size}", "binary:   ".with(Color::Cyan));
    println!("  {} {binary_path}", "path:     ".with(Color::Cyan));
    println!("  {} {prompt_info}", "prompt:   ".with(Color::Cyan));
    let agent_info = agents::loaded_agent_name()
        .map_or_else(|| "(none)".to_string(), |n| format!("{n} (loaded)"));
    println!("  {} {agent_info}", "agent:    ".with(Color::Cyan));

    // Collect unique cloud providers from the static catalog. Anything in
    // MODELS counts as a cloud provider; ollama / native GGUF / native MLX
    // are listed separately under "local:".
    let mut cloud_providers: Vec<&str> = Vec::new();
    for &(prov, _, _) in crate::llm::MODELS {
        if !cloud_providers.contains(&prov) {
            cloud_providers.push(prov);
        }
    }
    let cloud_count = cloud_providers.len();
    let local_count = 3; // ollama + native GGUF + native MLX
    let model_count = crate::llm::MODELS.len();
    println!(
        "  {} {cloud_count} ({})",
        "cloud:    ".with(Color::Cyan),
        cloud_providers.join(", ")
    );
    let ollama_label = if ollama_models.is_empty() {
        "ollama [not running]".to_string()
    } else {
        format!("ollama [{} model(s)]", ollama_models.len())
    };
    println!(
        "  {} {local_count} ({ollama_label}, gguf, mlx)",
        "local:    ".with(Color::Cyan),
    );

    let experimental = "[experimental]".with(Color::Yellow).to_string();

    let gguf_models = crate::llm_gguf::list_models();
    let gguf_available = crate::llm_gguf::is_available();
    let gguf_feature_label = if gguf_available {
        "enabled".with(Color::Green).to_string()
    } else {
        "disabled (rebuild with --features gguf)"
            .with(Color::Yellow)
            .to_string()
    };
    let gguf_info = if gguf_models.is_empty() {
        format!("0 downloaded · inference {gguf_feature_label} {experimental}")
    } else {
        format!(
            "{} downloaded ({}) · inference {gguf_feature_label} {experimental}",
            gguf_models.len(),
            gguf_models.join(", ")
        )
    };
    println!("  {} {gguf_info}", "gguf:     ".with(Color::Cyan));

    let mlx_models = crate::llm_mlx::list_models();
    let mlx_available = crate::llm_mlx::is_available();
    let mlx_host_ok = crate::llm_mlx::host_supports_mlx();
    let mlx_feature_label = if mlx_available {
        "enabled".with(Color::Green).to_string()
    } else if !mlx_host_ok {
        "disabled (requires macOS + Apple Silicon)"
            .with(Color::Yellow)
            .to_string()
    } else {
        "disabled (rebuild with --features mlx)"
            .with(Color::Yellow)
            .to_string()
    };
    let mlx_info = if mlx_models.is_empty() {
        format!("0 downloaded · inference {mlx_feature_label} {experimental}")
    } else {
        format!(
            "{} downloaded ({}) · inference {mlx_feature_label} {experimental}",
            mlx_models.len(),
            mlx_models.join(", ")
        )
    };
    println!("  {} {mlx_info}", "mlx:      ".with(Color::Cyan));

    let total_models = model_count + ollama_models.len() + gguf_models.len() + mlx_models.len();
    println!(
        "  {} {total_models} ({model_count} cataloged, {} ollama, {} gguf, {} mlx)",
        "models:   ".with(Color::Cyan),
        ollama_models.len(),
        gguf_models.len(),
        mlx_models.len()
    );
    let tool_count = crate::tools::TOOL_COUNT;
    let disabled = crate::security::policy().disabled_tools.len();
    let tools_info = if disabled > 0 {
        format!("{tool_count} ({disabled} disabled)")
    } else {
        format!("{tool_count}")
    };
    println!("  {} {tools_info}", "tools:    ".with(Color::Cyan));
    println!();
}

const ISSUES_URL: &str =
    "https://raw.githubusercontent.com/pwittchen/aictl/refs/heads/master/ISSUES.md";

/// Fetch and display known issues from the remote ISSUES.md.
pub async fn run_issues(show_error: &dyn Fn(&str)) {
    println!();
    println!("  {} fetching issues...", "↓".with(Color::Cyan));

    let client = crate::config::http_client();
    let result = client
        .get(ISSUES_URL)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .ok();

    let Some(response) = result else {
        show_error("Could not fetch ISSUES.md. Please try again later.");
        return;
    };

    let Ok(body) = response.text().await else {
        show_error("Could not read ISSUES.md response body.");
        return;
    };

    let skin = termimad::MadSkin::default();
    let width = crossterm::terminal::size()
        .map_or(80, |(w, _)| w as usize)
        .min(100);
    let rendered = format!(
        "{}",
        termimad::FmtText::from_text(&skin, body.as_str().into(), Some(width))
    );
    println!();
    for line in rendered.lines() {
        println!("  {line}");
    }
    println!();
}

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

const UPDATE_CMD: &str =
    "curl -sSf https://aictl.app/install.sh | sh";

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

// --- Session management ---

const SESSION_ITEMS: &[(&str, &str)] = &[
    ("current session info", "show id, name, messages, size"),
    ("set session name", "assign a readable name"),
    ("view saved sessions", "load or delete saved sessions"),
    ("clear all sessions", "remove all saved sessions"),
];

fn build_session_menu_lines(selected: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let max_name = SESSION_ITEMS
        .iter()
        .map(|(n, _)| n.len())
        .max()
        .unwrap_or(0);
    for (i, (name, desc)) in SESSION_ITEMS.iter().enumerate() {
        let is_selected = i == selected;
        let padded = format!("{:<max_name$}", *name);
        let name_styled = if is_selected {
            format!(
                "  {}",
                padded
                    .with(Color::White)
                    .attribute(crossterm::style::Attribute::Bold)
            )
        } else {
            format!("  {}", padded.with(Color::DarkGrey))
        };
        let desc_styled = format!("{}", desc.with(Color::DarkGrey));
        let line = if is_selected {
            format!("  {} {name_styled}  {desc_styled}", "›".with(Color::Cyan))
        } else {
            format!("    {name_styled}  {desc_styled}")
        };
        lines.push(line);
    }
    lines
}

fn format_size(bytes: u64) -> String {
    #[allow(clippy::cast_precision_loss)]
    if bytes >= 1_048_576 {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

/// Prompt for a y/N confirmation. Returns true if user pressed y.
fn confirm_yn(prompt: &str) -> bool {
    use crossterm::event::{self, Event, KeyCode, KeyEventKind};
    use crossterm::terminal;
    print!(
        "  {} {} ",
        prompt.with(Color::Yellow),
        "(y/N):".with(Color::DarkGrey)
    );
    let _ = std::io::stdout().flush();
    let _ = terminal::enable_raw_mode();
    let mut answer = false;
    loop {
        if !event::poll(std::time::Duration::from_millis(200)).unwrap_or(false) {
            continue;
        }
        if let Ok(Event::Key(key)) = event::read()
            && key.kind == KeyEventKind::Press
        {
            match key.code {
                KeyCode::Char('y' | 'Y') => {
                    answer = true;
                    break;
                }
                KeyCode::Char('n' | 'N') | KeyCode::Esc | KeyCode::Enter => break,
                _ => {}
            }
        }
    }
    let _ = terminal::disable_raw_mode();
    println!();
    answer
}

fn show_current_session_info(messages_len: usize) {
    let Some((id, name)) = crate::session::current_info() else {
        println!();
        println!("  {} no active session", "✗".with(Color::Red));
        println!();
        return;
    };
    let size = crate::session::current_file_size();
    println!();
    println!("  {} {id}", "id:      ".with(Color::Cyan));
    println!(
        "  {} {}",
        "name:    ".with(Color::Cyan),
        name.as_deref().unwrap_or("(unset)")
    );
    println!("  {} {messages_len}", "messages:".with(Color::Cyan));
    println!("  {} {}", "size:    ".with(Color::Cyan), format_size(size));
    println!();
}

fn set_session_name_interactive(show_error: &dyn Fn(&str)) {
    let Some((id, _)) = crate::session::current_info() else {
        show_error("no active session");
        return;
    };
    print!("  {} ", "enter session name:".with(Color::Cyan));
    let _ = std::io::stdout().flush();
    let mut buf = String::new();
    if std::io::stdin().read_line(&mut buf).is_err() {
        return;
    }
    let name = buf.trim();
    if name.is_empty() {
        println!();
        return;
    }
    match crate::session::set_name(&id, name) {
        Ok(()) => {
            let stored = crate::session::current_info()
                .and_then(|(_, n)| n)
                .unwrap_or_else(|| name.to_string());
            println!();
            println!(
                "  {} session name set to \"{stored}\"",
                "✓".with(Color::Green)
            );
            println!();
        }
        Err(e) => show_error(&format!("Error: {e}")),
    }
}

fn format_mtime(mtime: std::time::SystemTime) -> String {
    let secs = mtime
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let diff = now.saturating_sub(secs);
    if diff < 60 {
        format!("{diff}s ago")
    } else if diff < 3600 {
        format!("{}m ago", diff / 60)
    } else if diff < 86400 {
        format!("{}h ago", diff / 3600)
    } else {
        format!("{}d ago", diff / 86400)
    }
}

fn build_saved_sessions_lines(
    selected: usize,
    entries: &[crate::session::SessionEntry],
    current_id: Option<&str>,
) -> Vec<String> {
    if entries.is_empty() {
        return vec![format!("  {}", "(no saved sessions)".with(Color::DarkGrey))];
    }
    let mut lines = Vec::new();
    for (i, e) in entries.iter().enumerate() {
        let is_selected = i == selected;
        let is_current = current_id == Some(e.id.as_str());
        let marker = if is_current { "●" } else { " " };
        let name_part = e
            .name
            .as_deref()
            .map(|n| format!(" [{n}]"))
            .unwrap_or_default();
        let meta = format!(" {} · {}", format_size(e.size), format_mtime(e.mtime));
        let body = format!("{}{}{}", e.id, name_part, meta);
        let styled = if is_selected {
            format!(
                "{} {}",
                marker.with(Color::Green),
                body.with(Color::White)
                    .attribute(crossterm::style::Attribute::Bold)
            )
        } else {
            format!(
                "{} {}",
                marker.with(Color::Green),
                body.with(Color::DarkGrey)
            )
        };
        let line = if is_selected {
            format!("  {} {styled}", "›".with(Color::Cyan))
        } else {
            format!("    {styled}")
        };
        lines.push(line);
    }
    lines
}

enum SavedAction {
    Load(usize),
    Delete(usize),
    Cancel,
}

#[allow(clippy::cast_possible_truncation)]
fn select_saved_session(entries: &[crate::session::SessionEntry]) -> SavedAction {
    use crossterm::{
        cursor,
        event::{self, Event, KeyCode, KeyEventKind},
        execute,
        terminal::{self, ClearType},
    };

    let current_id = crate::session::current_id();
    let mut selected: usize = 0;
    let _ = terminal::enable_raw_mode();
    let mut stdout = std::io::stdout();
    let _ = execute!(stdout, cursor::Hide);

    let mut lines = build_saved_sessions_lines(selected, entries, current_id.as_deref());
    let _ = write!(stdout, "\r\n");
    for line in &lines {
        let _ = write!(stdout, "{line}\r\n");
    }
    let hint = "↑/↓ navigate · l/enter load · d delete · esc cancel";
    let _ = write!(stdout, "\r\n  {}\r\n", hint.with(Color::DarkGrey));
    let _ = stdout.flush();
    let mut rendered = lines.len() + 2;

    let result = loop {
        if !event::poll(std::time::Duration::from_millis(100)).unwrap_or(false) {
            continue;
        }
        let Ok(ev) = event::read() else {
            break SavedAction::Cancel;
        };
        if let Event::Key(key) = ev
            && key.kind == KeyEventKind::Press
        {
            match key.code {
                KeyCode::Up => selected = selected.saturating_sub(1),
                KeyCode::Down => {
                    if !entries.is_empty() && selected + 1 < entries.len() {
                        selected += 1;
                    }
                }
                KeyCode::Enter | KeyCode::Char('l' | 'L') => {
                    if !entries.is_empty() {
                        break SavedAction::Load(selected);
                    }
                }
                KeyCode::Char('d' | 'D') => {
                    if !entries.is_empty() {
                        break SavedAction::Delete(selected);
                    }
                }
                KeyCode::Esc => break SavedAction::Cancel,
                _ => {}
            }
        } else {
            continue;
        }

        let _ = execute!(
            stdout,
            cursor::MoveUp(rendered as u16),
            terminal::Clear(ClearType::FromCursorDown),
        );
        lines = build_saved_sessions_lines(selected, entries, current_id.as_deref());
        for line in &lines {
            let _ = write!(stdout, "{line}\r\n");
        }
        let _ = write!(stdout, "\r\n  {}\r\n", hint.with(Color::DarkGrey));
        let _ = stdout.flush();
        rendered = lines.len() + 2;
    };

    let _ = execute!(
        stdout,
        cursor::MoveUp(rendered as u16),
        terminal::Clear(ClearType::FromCursorDown),
        cursor::Show,
    );
    let _ = terminal::disable_raw_mode();
    result
}

fn view_saved_sessions(messages: &mut Vec<Message>, show_error: &dyn Fn(&str)) -> bool {
    loop {
        let entries = crate::session::list_sessions();
        match select_saved_session(&entries) {
            SavedAction::Cancel => return false,
            SavedAction::Load(i) => {
                let entry = &entries[i];
                let label = entry
                    .name
                    .as_deref()
                    .map_or_else(|| entry.id.clone(), |n| format!("{} ({n})", entry.id));
                if !confirm_yn(&format!("load session {label}?")) {
                    continue;
                }
                match crate::session::load_messages(&entry.id) {
                    Ok(loaded) => {
                        *messages = loaded;
                        crate::session::set_current(entry.id.clone(), entry.name.clone());
                        println!();
                        println!("  {} session loaded: {label}", "✓".with(Color::Green));
                        println!();
                        return true;
                    }
                    Err(e) => {
                        show_error(&format!("Failed to load session: {e}"));
                        return false;
                    }
                }
            }
            SavedAction::Delete(i) => {
                let entry = &entries[i];
                let label = entry
                    .name
                    .as_deref()
                    .map_or_else(|| entry.id.clone(), |n| format!("{} ({n})", entry.id));
                if !confirm_yn(&format!("delete session {label}?")) {
                    continue;
                }
                crate::session::delete_session(&entry.id);
                println!();
                println!("  {} session deleted", "✓".with(Color::Green));
                println!();
            }
        }
    }
}

fn clear_all_sessions_confirm() {
    if !confirm_yn("clear ALL saved sessions?") {
        return;
    }
    crate::session::clear_all();
    // Re-save current session so it persists after clear.
    println!();
    println!("  {} all sessions cleared", "✓".with(Color::Green));
    println!();
}

/// Run the /session menu. Returns true if the conversation messages were replaced
/// (caller should reset context-tracking state).
pub fn run_session_menu(messages: &mut Vec<Message>, show_error: &dyn Fn(&str)) -> bool {
    let Some(sel) = select_from_menu(SESSION_ITEMS.len(), 0, build_session_menu_lines) else {
        return false;
    };
    match sel {
        0 => {
            show_current_session_info(messages.len());
            false
        }
        1 => {
            set_session_name_interactive(show_error);
            false
        }
        2 => view_saved_sessions(messages, show_error),
        3 => {
            clear_all_sessions_confirm();
            false
        }
        _ => false,
    }
}

/// Print saved sessions in non-interactive mode.
pub fn print_sessions_cli() {
    let entries = crate::session::list_sessions();
    if entries.is_empty() {
        println!("(no saved sessions)");
        return;
    }
    for e in &entries {
        let name = e.name.as_deref().unwrap_or("-");
        println!(
            "{}  {}  {}  {}",
            e.id,
            name,
            format_size(e.size),
            format_mtime(e.mtime)
        );
    }
}

/// Print saved agents in non-interactive mode.
pub fn print_agents_cli() {
    let entries = crate::agents::list_agents();
    if entries.is_empty() {
        println!("(no saved agents)");
        return;
    }
    for e in &entries {
        println!("{}", e.name);
    }
}

// --- Native GGUF model management ---

const GGUF_MENU_ITEMS: &[(&str, &str)] = &[
    ("view downloaded", "list models in ~/.aictl/models/gguf/"),
    (
        "pull model",
        "download a GGUF model from Hugging Face or URL",
    ),
    ("remove model", "delete a downloaded model"),
    ("clear all", "remove every downloaded model"),
];

fn build_gguf_menu_lines(selected: usize) -> Vec<String> {
    let max = GGUF_MENU_ITEMS
        .iter()
        .map(|(n, _)| n.len())
        .max()
        .unwrap_or(0);
    GGUF_MENU_ITEMS
        .iter()
        .enumerate()
        .map(|(i, (name, desc))| {
            let is_selected = i == selected;
            let padded = format!("{name:<max$}");
            let name_styled = if is_selected {
                format!(
                    "{}",
                    padded
                        .with(Color::White)
                        .attribute(crossterm::style::Attribute::Bold)
                )
            } else {
                format!("{}", padded.with(Color::DarkGrey))
            };
            let desc_styled = format!("{}", desc.with(Color::DarkGrey));
            if is_selected {
                format!("  {} {name_styled}  {desc_styled}", "›".with(Color::Cyan))
            } else {
                format!("    {name_styled}  {desc_styled}")
            }
        })
        .collect()
}

fn print_gguf_models() {
    let models = crate::llm_gguf::list_models();
    println!();
    if !crate::llm_gguf::is_available() {
        println!(
            "  {}",
            "native inference is not compiled in — rebuild with `cargo build --features gguf` to use downloaded models".with(Color::Yellow)
        );
    }
    if models.is_empty() {
        println!("  {}", "no local models downloaded".with(Color::DarkGrey));
        println!();
        return;
    }
    let dir = crate::llm_gguf::models_dir();
    for m in &models {
        let path = dir.join(format!("{m}.gguf"));
        let size = std::fs::metadata(&path)
            .ok()
            .map_or_else(|| "?".to_string(), |meta| format_size(meta.len()));
        println!(
            "  {} {}  {}",
            "●".with(Color::Green),
            m.as_str().with(Color::White),
            size.with(Color::DarkGrey),
        );
    }
    println!();
}

/// Cancellable single-line prompt.
///
/// Returns `Ok(text)` when the user presses Enter (text may be empty) or
/// `Err(())` when the user presses Esc or Ctrl+C. Backspace deletes.
fn prompt_line_cancellable(prompt: &str) -> Result<String, ()> {
    use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
    use crossterm::terminal;

    print!("  {} ", prompt.with(Color::Cyan));
    let _ = std::io::stdout().flush();

    let _ = terminal::enable_raw_mode();
    let mut buf = String::new();
    let result: Result<String, ()> = loop {
        if !event::poll(std::time::Duration::from_millis(200)).unwrap_or(false) {
            continue;
        }
        let Ok(ev) = event::read() else {
            break Err(());
        };
        if let Event::Key(key) = ev
            && key.kind == KeyEventKind::Press
        {
            match key.code {
                KeyCode::Esc => break Err(()),
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    break Err(());
                }
                KeyCode::Enter => break Ok(buf.clone()),
                KeyCode::Backspace => {
                    if buf.pop().is_some() {
                        // Erase last character on screen.
                        print!("\u{8} \u{8}");
                        let _ = std::io::stdout().flush();
                    }
                }
                KeyCode::Char(c) => {
                    buf.push(c);
                    print!("{c}");
                    let _ = std::io::stdout().flush();
                }
                _ => {}
            }
        }
    };
    let _ = terminal::disable_raw_mode();
    println!();
    result
}

/// Curated list of popular small-to-medium GGUF models that run well on
/// consumer hardware. Each entry is (display label, spec, approximate size).
/// Keep this short — it's a starter selection, not a catalog.
/// Curated subset of the LM Studio model catalog (<https://lmstudio.ai/models>).
/// Each entry points at the `lmstudio-community` GGUF mirror on Hugging Face
/// with the `Q4_K_M` quant where available (gpt-oss ships only `MXFP4`).
/// Sizes were read from the HF tree API at the time of selection.
const LMSTUDIO_CATALOG: &[(&str, &str, &str)] = &[
    (
        "Llama 3.2 3B Instruct (Q4_K_M)",
        "lmstudio-community/Llama-3.2-3B-Instruct-GGUF:Llama-3.2-3B-Instruct-Q4_K_M.gguf",
        "~1.9 GB",
    ),
    (
        "Llama 3.1 8B Instruct (Q4_K_M)",
        "lmstudio-community/Meta-Llama-3.1-8B-Instruct-GGUF:Meta-Llama-3.1-8B-Instruct-Q4_K_M.gguf",
        "~4.6 GB",
    ),
    (
        "Qwen3 4B (Q4_K_M)",
        "lmstudio-community/Qwen3-4B-GGUF:Qwen3-4B-Q4_K_M.gguf",
        "~2.3 GB",
    ),
    (
        "Qwen3 8B (Q4_K_M)",
        "lmstudio-community/Qwen3-8B-GGUF:Qwen3-8B-Q4_K_M.gguf",
        "~4.7 GB",
    ),
    (
        "Qwen3 14B (Q4_K_M)",
        "lmstudio-community/Qwen3-14B-GGUF:Qwen3-14B-Q4_K_M.gguf",
        "~8.4 GB",
    ),
    (
        "Qwen3 Coder 30B A3B Instruct (Q4_K_M)",
        "lmstudio-community/Qwen3-Coder-30B-A3B-Instruct-GGUF:Qwen3-Coder-30B-A3B-Instruct-Q4_K_M.gguf",
        "~17.4 GB",
    ),
    (
        "Gemma 3 4B Instruct (Q4_K_M)",
        "lmstudio-community/gemma-3-4b-it-GGUF:gemma-3-4b-it-Q4_K_M.gguf",
        "~2.3 GB",
    ),
    (
        "Gemma 3 12B Instruct (Q4_K_M)",
        "lmstudio-community/gemma-3-12b-it-GGUF:gemma-3-12b-it-Q4_K_M.gguf",
        "~6.8 GB",
    ),
    (
        "Gemma 3 27B Instruct (Q4_K_M)",
        "lmstudio-community/gemma-3-27b-it-GGUF:gemma-3-27b-it-Q4_K_M.gguf",
        "~15.4 GB",
    ),
    (
        "gpt-oss 20B (MXFP4)",
        "lmstudio-community/gpt-oss-20b-GGUF:gpt-oss-20b-MXFP4.gguf",
        "~11.3 GB",
    ),
    (
        "DeepSeek R1 Distill Qwen 7B (Q4_K_M)",
        "lmstudio-community/DeepSeek-R1-Distill-Qwen-7B-GGUF:DeepSeek-R1-Distill-Qwen-7B-Q4_K_M.gguf",
        "~4.4 GB",
    ),
    (
        "DeepSeek R1 Distill Qwen 14B (Q4_K_M)",
        "lmstudio-community/DeepSeek-R1-Distill-Qwen-14B-GGUF:DeepSeek-R1-Distill-Qwen-14B-Q4_K_M.gguf",
        "~8.4 GB",
    ),
    (
        "DeepSeek R1 Distill Qwen 32B (Q4_K_M)",
        "lmstudio-community/DeepSeek-R1-Distill-Qwen-32B-GGUF:DeepSeek-R1-Distill-Qwen-32B-Q4_K_M.gguf",
        "~18.5 GB",
    ),
    (
        "Mistral Small 24B Instruct 2501 (Q4_K_M)",
        "lmstudio-community/Mistral-Small-24B-Instruct-2501-GGUF:Mistral-Small-24B-Instruct-2501-Q4_K_M.gguf",
        "~13.3 GB",
    ),
    (
        "Phi 4 (Q4_K_M)",
        "lmstudio-community/phi-4-GGUF:phi-4-Q4_K_M.gguf",
        "~8.4 GB",
    ),
    (
        "Granite 4.0 H Small (Q4_K_M)",
        "lmstudio-community/granite-4.0-h-small-GGUF:granite-4.0-h-small-Q4_K_M.gguf",
        "~18.1 GB",
    ),
];

fn build_lmstudio_catalog_menu_lines(selected: usize) -> Vec<String> {
    let max_label = LMSTUDIO_CATALOG
        .iter()
        .map(|(label, _, _)| label.len())
        .max()
        .unwrap_or(0);
    let total = LMSTUDIO_CATALOG.len() + 1; // +1 for "custom spec"
    (0..total)
        .map(|i| {
            let is_selected = i == selected;
            let (label, size) = if i < LMSTUDIO_CATALOG.len() {
                let (l, _, s) = LMSTUDIO_CATALOG[i];
                (l.to_string(), s.to_string())
            } else {
                (
                    "custom spec (hf:/owner/repo:/https://...)".to_string(),
                    String::new(),
                )
            };
            let padded = format!("{label:<max_label$}");
            let name_styled = if is_selected {
                padded
                    .with(Color::White)
                    .attribute(crossterm::style::Attribute::Bold)
                    .to_string()
            } else {
                padded.with(Color::DarkGrey).to_string()
            };
            let size_styled = format!("{}", size.with(Color::DarkGrey));
            if is_selected {
                format!("  {} {name_styled}  {size_styled}", "›".with(Color::Cyan))
            } else {
                format!("    {name_styled}  {size_styled}")
            }
        })
        .collect()
}

async fn pull_gguf_model(show_error: &dyn Fn(&str)) {
    println!();
    println!(
        "  {}",
        "curated from the LM Studio catalog (lmstudio.ai/models), hosted on Hugging Face by lmstudio-community"
            .with(Color::DarkGrey)
    );
    let total = LMSTUDIO_CATALOG.len() + 1;
    let Some(sel) = select_from_menu(total, 0, build_lmstudio_catalog_menu_lines) else {
        return;
    };

    let spec = if sel < LMSTUDIO_CATALOG.len() {
        LMSTUDIO_CATALOG[sel].1.to_string()
    } else {
        println!();
        println!("  {}", "spec examples:".with(Color::DarkGrey));
        println!(
            "    {}",
            "hf:TheBloke/Llama-2-7B-Chat-GGUF/llama-2-7b-chat.Q4_K_M.gguf".with(Color::DarkGrey)
        );
        println!(
            "    {}",
            "bartowski/Llama-3.2-3B-Instruct-GGUF:Llama-3.2-3B-Instruct-Q4_K_M.gguf"
                .with(Color::DarkGrey)
        );
        println!(
            "    {}",
            "https://host/path/model.gguf".with(Color::DarkGrey)
        );
        match prompt_line_cancellable("spec:") {
            Ok(s) if s.trim().is_empty() => {
                show_cancelled();
                return;
            }
            Ok(s) => s.trim().to_string(),
            Err(()) => {
                show_cancelled();
                return;
            }
        }
    };

    let name_override = if let Ok(s) =
        prompt_line_cancellable("local name (optional, press enter to use default):")
    {
        let t = s.trim().to_string();
        if t.is_empty() { None } else { Some(t) }
    } else {
        show_cancelled();
        return;
    };

    let download = crate::llm_gguf::download_model(&spec, name_override.as_deref());
    match crate::with_esc_cancel(download).await {
        Ok(Ok(name)) => {
            println!();
            println!(
                "  {} downloaded {}",
                "✓".with(Color::Green),
                name.with(Color::White)
            );
            println!();
        }
        Ok(Err(e)) => show_error(&format!("download failed: {e}")),
        Err(_) => {
            println!();
            println!(
                "  {} download cancelled (partial file removed)",
                "✗".with(Color::Yellow)
            );
            println!();
            // Clean up the leaked .part file so the next attempt starts fresh.
            let _ = cleanup_partial_download(&spec, name_override.as_deref());
        }
    }
}

fn show_cancelled() {
    println!();
    println!("  {} cancelled", "✗".with(Color::Yellow));
    println!();
}

/// Best-effort cleanup of a `<name>.gguf.part` file left behind when a
/// download is cancelled via Esc. Silently ignores failures.
fn cleanup_partial_download(spec: &str, override_name: Option<&str>) -> std::io::Result<()> {
    // Resolve the same name the downloader would have used.
    let name = override_name.map_or_else(
        || {
            spec.rsplit('/')
                .next()
                .and_then(|f| f.split('?').next())
                .map(|f| {
                    std::path::Path::new(f)
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or(f)
                        .to_string()
                })
                .unwrap_or_default()
        },
        String::from,
    );
    if name.is_empty() {
        return Ok(());
    }
    let path = crate::llm_gguf::models_dir().join(format!("{name}.gguf.part"));
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

fn remove_gguf_model_interactive(show_error: &dyn Fn(&str)) {
    let models = crate::llm_gguf::list_models();
    if models.is_empty() {
        println!();
        println!("  {}", "no local models to remove".with(Color::DarkGrey));
        println!();
        return;
    }
    let build = |sel: usize| -> Vec<String> {
        models
            .iter()
            .enumerate()
            .map(|(i, m)| {
                let is_selected = i == sel;
                if is_selected {
                    format!(
                        "  {} {}",
                        "›".with(Color::Cyan),
                        m.as_str()
                            .with(Color::White)
                            .attribute(crossterm::style::Attribute::Bold)
                    )
                } else {
                    format!("    {}", m.as_str().with(Color::DarkGrey))
                }
            })
            .collect()
    };
    let Some(sel) = select_from_menu(models.len(), 0, build) else {
        return;
    };
    let name = &models[sel];
    println!();
    if !confirm_yn(&format!("remove local model '{name}'?")) {
        return;
    }
    match crate::llm_gguf::remove_model(name) {
        Ok(()) => {
            println!();
            println!(
                "  {} removed {}",
                "✓".with(Color::Green),
                name.as_str().with(Color::White)
            );
            println!();
        }
        Err(e) => show_error(&format!("remove failed: {e}")),
    }
}

fn clear_all_gguf_models_confirm() {
    println!();
    if !confirm_yn("remove ALL downloaded local models?") {
        return;
    }
    match crate::llm_gguf::clear_models() {
        Ok(n) => {
            println!();
            println!("  {} removed {n} local model(s)", "✓".with(Color::Green));
            println!();
        }
        Err(e) => {
            println!();
            println!(
                "  {} {}",
                "✗".with(Color::Red),
                e.to_string().with(Color::Red)
            );
            println!();
        }
    }
}

// --- Native MLX model management (Apple Silicon) ---

const MLX_MENU_ITEMS: &[(&str, &str)] = &[
    ("view downloaded", "list models in ~/.aictl/models/mlx/"),
    (
        "pull model",
        "download an MLX model from Hugging Face (mlx-community)",
    ),
    ("remove model", "delete a downloaded model"),
    ("clear all", "remove every downloaded model"),
];

fn build_mlx_menu_lines(selected: usize) -> Vec<String> {
    let max = MLX_MENU_ITEMS
        .iter()
        .map(|(n, _)| n.len())
        .max()
        .unwrap_or(0);
    MLX_MENU_ITEMS
        .iter()
        .enumerate()
        .map(|(i, (name, desc))| {
            let is_selected = i == selected;
            let padded = format!("{name:<max$}");
            let name_styled = if is_selected {
                format!(
                    "{}",
                    padded
                        .with(Color::White)
                        .attribute(crossterm::style::Attribute::Bold)
                )
            } else {
                format!("{}", padded.with(Color::DarkGrey))
            };
            let desc_styled = format!("{}", desc.with(Color::DarkGrey));
            if is_selected {
                format!("  {} {name_styled}  {desc_styled}", "›".with(Color::Cyan))
            } else {
                format!("    {name_styled}  {desc_styled}")
            }
        })
        .collect()
}

fn print_mlx_models() {
    let models = crate::llm_mlx::list_models();
    println!();
    if !crate::llm_mlx::host_supports_mlx() {
        println!(
            "  {}",
            "MLX inference requires macOS + Apple Silicon — downloaded models on this host can't run"
                .with(Color::Yellow)
        );
    } else if !crate::llm_mlx::is_available() {
        println!(
            "  {}",
            "native MLX inference is not compiled in — rebuild with `cargo build --features mlx` to use downloaded models".with(Color::Yellow)
        );
    }
    if models.is_empty() {
        println!("  {}", "no MLX models downloaded".with(Color::DarkGrey));
        println!();
        return;
    }
    for m in &models {
        let size = format_size(crate::llm_mlx::model_size(m));
        println!(
            "  {} {}  {}",
            "●".with(Color::Green),
            m.as_str().with(Color::White),
            size.with(Color::DarkGrey),
        );
    }
    println!();
}

/// Curated starter list of popular MLX-community repos on Hugging Face.
/// Sizes are approximate on-disk footprints for the 4-bit variants; the
/// actual download size will depend on what's in the repo tree.
const MLX_CATALOG: &[(&str, &str, &str)] = &[
    (
        "Llama 3.2 3B Instruct (4-bit)",
        "mlx-community/Llama-3.2-3B-Instruct-4bit",
        "~1.8 GB",
    ),
    (
        "Llama 3.1 8B Instruct (4-bit)",
        "mlx-community/Meta-Llama-3.1-8B-Instruct-4bit",
        "~4.5 GB",
    ),
    (
        "Qwen2.5 7B Instruct (4-bit)",
        "mlx-community/Qwen2.5-7B-Instruct-4bit",
        "~4.3 GB",
    ),
    (
        "Qwen2.5 14B Instruct (4-bit)",
        "mlx-community/Qwen2.5-14B-Instruct-4bit",
        "~8.0 GB",
    ),
    (
        "Qwen2.5 Coder 7B Instruct (4-bit)",
        "mlx-community/Qwen2.5-Coder-7B-Instruct-4bit",
        "~4.3 GB",
    ),
    (
        "Mistral 7B Instruct v0.3 (4-bit)",
        "mlx-community/Mistral-7B-Instruct-v0.3-4bit",
        "~4.1 GB",
    ),
    (
        "Gemma 2 9B Instruct (4-bit)",
        "mlx-community/gemma-2-9b-it-4bit",
        "~5.3 GB",
    ),
    (
        "Phi-3.5 Mini Instruct (4-bit)",
        "mlx-community/Phi-3.5-mini-instruct-4bit",
        "~2.2 GB",
    ),
    (
        "DeepSeek R1 Distill Qwen 7B (4-bit)",
        "mlx-community/DeepSeek-R1-Distill-Qwen-7B-4bit",
        "~4.3 GB",
    ),
    (
        "DeepSeek R1 Distill Qwen 14B (4-bit)",
        "mlx-community/DeepSeek-R1-Distill-Qwen-14B-4bit",
        "~8.0 GB",
    ),
];

fn build_mlx_catalog_menu_lines(selected: usize) -> Vec<String> {
    let max_label = MLX_CATALOG
        .iter()
        .map(|(l, _, _)| l.len())
        .max()
        .unwrap_or(0);
    let max_size = MLX_CATALOG
        .iter()
        .map(|(_, _, s)| s.len())
        .max()
        .unwrap_or(0);
    MLX_CATALOG
        .iter()
        .enumerate()
        .map(|(i, (label, spec, size))| {
            let is_selected = i == selected;
            let padded_label = format!("{label:<max_label$}");
            let padded_size = format!("{size:<max_size$}");
            let label_styled = if is_selected {
                format!(
                    "{}",
                    padded_label
                        .with(Color::White)
                        .attribute(crossterm::style::Attribute::Bold)
                )
            } else {
                format!("{}", padded_label.with(Color::DarkGrey))
            };
            let size_styled = format!("{}", padded_size.with(Color::DarkGrey));
            let spec_styled = format!("{}", spec.with(Color::DarkGrey));
            if is_selected {
                format!(
                    "  {} {label_styled}  {size_styled}  {spec_styled}",
                    "›".with(Color::Cyan)
                )
            } else {
                format!("    {label_styled}  {size_styled}  {spec_styled}")
            }
        })
        .chain(std::iter::once({
            let label = "other (enter a custom spec)";
            let is_selected = selected == MLX_CATALOG.len();
            let padded_label = format!("{label:<max_label$}");
            if is_selected {
                format!(
                    "  {} {}",
                    "›".with(Color::Cyan),
                    padded_label
                        .with(Color::White)
                        .attribute(crossterm::style::Attribute::Bold)
                )
            } else {
                format!("    {}", padded_label.with(Color::DarkGrey))
            }
        }))
        .collect()
}

async fn pull_mlx_model(show_error: &dyn Fn(&str)) {
    println!();
    println!(
        "  {}",
        "curated from mlx-community on Hugging Face (huggingface.co/mlx-community)"
            .with(Color::DarkGrey)
    );
    let total = MLX_CATALOG.len() + 1;
    let Some(sel) = select_from_menu(total, 0, build_mlx_catalog_menu_lines) else {
        return;
    };

    let spec = if sel < MLX_CATALOG.len() {
        MLX_CATALOG[sel].1.to_string()
    } else {
        println!();
        println!("  {}", "spec examples:".with(Color::DarkGrey));
        println!(
            "    {}",
            "mlx:mlx-community/Llama-3.2-3B-Instruct-4bit".with(Color::DarkGrey)
        );
        println!(
            "    {}",
            "mlx-community/Qwen2.5-7B-Instruct-4bit".with(Color::DarkGrey)
        );
        match prompt_line_cancellable("spec:") {
            Ok(s) if s.trim().is_empty() => {
                show_cancelled();
                return;
            }
            Ok(s) => s.trim().to_string(),
            Err(()) => {
                show_cancelled();
                return;
            }
        }
    };

    let name_override = if let Ok(s) =
        prompt_line_cancellable("local name (optional, press enter to use default):")
    {
        let t = s.trim().to_string();
        if t.is_empty() { None } else { Some(t) }
    } else {
        show_cancelled();
        return;
    };

    let download = crate::llm_mlx::download_model(&spec, name_override.as_deref());
    match crate::with_esc_cancel(download).await {
        Ok(Ok(name)) => {
            println!();
            println!(
                "  {} downloaded {}",
                "✓".with(Color::Green),
                name.with(Color::White)
            );
            println!();
        }
        Ok(Err(e)) => show_error(&format!("download failed: {e}")),
        Err(_) => {
            println!();
            println!(
                "  {} download cancelled (partial directory left in place)",
                "✗".with(Color::Yellow)
            );
            println!();
        }
    }
}

fn remove_mlx_model_interactive(show_error: &dyn Fn(&str)) {
    let models = crate::llm_mlx::list_models();
    if models.is_empty() {
        println!();
        println!("  {}", "no MLX models to remove".with(Color::DarkGrey));
        println!();
        return;
    }
    let build = |sel: usize| -> Vec<String> {
        models
            .iter()
            .enumerate()
            .map(|(i, m)| {
                let is_selected = i == sel;
                if is_selected {
                    format!(
                        "  {} {}",
                        "›".with(Color::Cyan),
                        m.as_str()
                            .with(Color::White)
                            .attribute(crossterm::style::Attribute::Bold)
                    )
                } else {
                    format!("    {}", m.as_str().with(Color::DarkGrey))
                }
            })
            .collect()
    };
    let Some(sel) = select_from_menu(models.len(), 0, build) else {
        return;
    };
    let name = &models[sel];
    println!();
    if !confirm_yn(&format!("remove MLX model '{name}'?")) {
        return;
    }
    match crate::llm_mlx::remove_model(name) {
        Ok(()) => {
            println!();
            println!(
                "  {} removed {}",
                "✓".with(Color::Green),
                name.as_str().with(Color::White)
            );
            println!();
        }
        Err(e) => show_error(&format!("remove failed: {e}")),
    }
}

fn clear_all_mlx_models_confirm() {
    println!();
    if !confirm_yn("remove ALL downloaded MLX models?") {
        return;
    }
    match crate::llm_mlx::clear_models() {
        Ok(n) => {
            println!();
            println!("  {} removed {n} MLX model(s)", "✓".with(Color::Green));
            println!();
        }
        Err(e) => {
            println!();
            println!(
                "  {} {}",
                "✗".with(Color::Red),
                e.to_string().with(Color::Red)
            );
            println!();
        }
    }
}

/// Interactive `/mlx` menu: list / pull / remove / clear.
pub async fn run_mlx_menu(show_error: &dyn Fn(&str)) {
    println!();
    println!(
        "  {} {}",
        "⚠".with(Color::Yellow),
        "native MLX model support is experimental — expect rough edges"
            .with(Color::Yellow)
    );
    if !crate::llm_mlx::host_supports_mlx() {
        println!(
            "  {} {}",
            "⚠".with(Color::Yellow),
            "this host is not Apple Silicon — models can be downloaded but not run here"
                .with(Color::Yellow)
        );
    }
    println!();
    let Some(sel) = select_from_menu(MLX_MENU_ITEMS.len(), 0, build_mlx_menu_lines) else {
        return;
    };
    match sel {
        0 => print_mlx_models(),
        1 => pull_mlx_model(show_error).await,
        2 => remove_mlx_model_interactive(show_error),
        3 => clear_all_mlx_models_confirm(),
        _ => {}
    }
}

/// Interactive `/gguf` menu: list / pull / remove / clear.
pub async fn run_gguf_menu(show_error: &dyn Fn(&str)) {
    println!();
    println!(
        "  {} {}",
        "⚠".with(Color::Yellow),
        "native GGUF model support is experimental — expect rough edges".with(Color::Yellow)
    );
    println!();
    let Some(sel) = select_from_menu(GGUF_MENU_ITEMS.len(), 0, build_gguf_menu_lines) else {
        return;
    };
    match sel {
        0 => print_gguf_models(),
        1 => pull_gguf_model(show_error).await,
        2 => remove_gguf_model_interactive(show_error),
        3 => clear_all_gguf_models_confirm(),
        _ => {}
    }
}

// --- Config wizard ---

/// All providers and their API key config key names.
const PROVIDERS: &[(&str, &str)] = &[
    ("anthropic", "LLM_ANTHROPIC_API_KEY"),
    ("openai", "LLM_OPENAI_API_KEY"),
    ("gemini", "LLM_GEMINI_API_KEY"),
    ("grok", "LLM_GROK_API_KEY"),
    ("mistral", "LLM_MISTRAL_API_KEY"),
    ("deepseek", "LLM_DEEPSEEK_API_KEY"),
    ("kimi", "LLM_KIMI_API_KEY"),
    ("zai", "LLM_ZAI_API_KEY"),
    ("ollama", ""),
];

fn build_provider_menu_lines(selected: usize) -> Vec<String> {
    PROVIDERS
        .iter()
        .enumerate()
        .map(|(i, (name, _))| {
            let is_selected = i == selected;
            let label = match *name {
                "anthropic" => "Anthropic",
                "openai" => "OpenAI",
                "gemini" => "Gemini",
                "grok" => "Grok",
                "mistral" => "Mistral",
                "deepseek" => "DeepSeek",
                "kimi" => "Kimi",
                "zai" => "Z.ai",
                "ollama" => "Ollama (local, no API key)",
                _ => name,
            };
            if is_selected {
                format!(
                    "  {} {}",
                    "›".with(Color::Cyan),
                    label
                        .with(Color::White)
                        .attribute(crossterm::style::Attribute::Bold)
                )
            } else {
                format!("    {}", label.with(Color::DarkGrey))
            }
        })
        .collect()
}

fn build_model_select_lines(selected: usize, models: &[&str]) -> Vec<String> {
    models
        .iter()
        .enumerate()
        .map(|(i, name)| {
            if i == selected {
                format!(
                    "  {} {}",
                    "›".with(Color::Cyan),
                    name.with(Color::White)
                        .attribute(crossterm::style::Attribute::Bold)
                )
            } else {
                format!("    {}", name.with(Color::DarkGrey))
            }
        })
        .collect()
}

/// Read a line from stdin with a prompt. Returns None if Esc pressed (via raw mode detection)
/// or empty input. Masks input when `masked` is true.
fn read_input_line(prompt: &str, masked: bool) -> Option<String> {
    use crossterm::event::{self, Event, KeyCode, KeyEventKind};
    use crossterm::terminal;

    print!("  {} ", prompt.with(Color::Cyan));
    let _ = std::io::stdout().flush();

    let _ = terminal::enable_raw_mode();
    let mut buf = String::new();
    let result = loop {
        if !event::poll(std::time::Duration::from_millis(200)).unwrap_or(false) {
            continue;
        }
        if let Ok(Event::Key(key)) = event::read()
            && key.kind == KeyEventKind::Press
        {
            match key.code {
                KeyCode::Esc => break None,
                KeyCode::Enter => break Some(buf.clone()),
                KeyCode::Backspace => {
                    if buf.pop().is_some() {
                        print!("\x08 \x08");
                        let _ = std::io::stdout().flush();
                    }
                }
                KeyCode::Char(c) => {
                    buf.push(c);
                    if masked {
                        print!("*");
                    } else {
                        print!("{c}");
                    }
                    let _ = std::io::stdout().flush();
                }
                _ => {}
            }
        }
    };
    let _ = terminal::disable_raw_mode();
    println!();
    result
}

/// Short status hint for an API key based on its current storage location.
/// Returns `None` when the key is not set anywhere.
fn key_status_hint(key_name: &str) -> Option<&'static str> {
    match crate::keys::location(key_name) {
        crate::keys::KeyLocation::None => None,
        crate::keys::KeyLocation::Config => Some("set in plain-text config"),
        crate::keys::KeyLocation::Keyring => Some("set in system keyring"),
        crate::keys::KeyLocation::Both => Some("set in both config and keyring"),
    }
}

/// Interactive configuration wizard for setting provider, model, and API keys.
///
/// When `from_repl` is true the wizard suppresses the trailing "run aictl..."
/// hint (which doesn't make sense from inside an active REPL session) and lets
/// the caller refresh its in-memory `provider`/`model`/`api_key` after the wizard
/// returns.
#[allow(clippy::too_many_lines)]
pub fn run_config_wizard(from_repl: bool) {
    println!();
    println!(
        "  {}",
        "aictl configuration wizard"
            .with(Color::Cyan)
            .attribute(crossterm::style::Attribute::Bold)
    );
    println!(
        "  {}",
        "You can also edit ~/.aictl/config manually at any time.".with(Color::DarkGrey)
    );
    println!();

    // Step 1: Select provider
    println!("  {}", "Select provider:".with(Color::White));
    let Some(provider_idx) = select_from_menu(PROVIDERS.len(), 0, build_provider_menu_lines) else {
        println!();
        println!("  {} configuration cancelled", "✗".with(Color::Yellow));
        println!();
        return;
    };
    let (provider_name, api_key_name) = PROVIDERS[provider_idx];
    let is_ollama = provider_name == "ollama";

    // Step 2: Select model
    let models_for_provider: Vec<&str> = if is_ollama {
        vec![]
    } else {
        MODELS
            .iter()
            .filter(|(p, _, _)| *p == provider_name)
            .map(|(_, m, _)| *m)
            .collect()
    };

    let model = if is_ollama {
        // For Ollama, ask user to type a model name
        println!();
        println!(
            "  {}",
            "Enter Ollama model name (e.g. llama3, mistral):".with(Color::White)
        );
        let Some(m) = read_input_line("model:", false) else {
            println!();
            println!("  {} configuration cancelled", "✗".with(Color::Yellow));
            println!();
            return;
        };
        let m = m.trim().to_string();
        if m.is_empty() {
            println!();
            println!("  {} no model specified, skipping", "⚠".with(Color::Yellow));
            println!();
            return;
        }
        m
    } else if models_for_provider.is_empty() {
        println!();
        println!(
            "  {} no models available for {provider_name}",
            "✗".with(Color::Red)
        );
        println!();
        return;
    } else {
        println!();
        println!("  {}", "Select model:".with(Color::White));
        let models_clone = models_for_provider.clone();
        let Some(model_idx) = select_from_menu(models_for_provider.len(), 0, |sel| {
            build_model_select_lines(sel, &models_clone)
        }) else {
            println!();
            println!("  {} configuration cancelled", "✗".with(Color::Yellow));
            println!();
            return;
        };
        models_for_provider[model_idx].to_string()
    };

    // Step 3: API key for the selected provider (required for non-Ollama).
    // If a value is already stored (config or keyring), let the user keep it
    // by pressing Enter without typing anything.
    let mut keys_to_save: Vec<(String, String)> = Vec::new();

    if !is_ollama {
        println!();
        if let Some(hint) = key_status_hint(api_key_name) {
            println!(
                "  {} {}",
                format!("Enter API key for {provider_name}:").with(Color::White),
                format!("({hint} — press Enter to keep)").with(Color::DarkGrey),
            );
        } else {
            println!(
                "  {} {}",
                format!("Enter API key for {provider_name}:").with(Color::White),
                "(required)".with(Color::DarkGrey),
            );
        }
        let Some(key) = read_input_line(&format!("{api_key_name}:"), true) else {
            println!();
            println!("  {} configuration cancelled", "✗".with(Color::Yellow));
            println!();
            return;
        };
        let key = key.trim().to_string();
        if key.is_empty() {
            if key_status_hint(api_key_name).is_none() {
                println!();
                println!(
                    "  {} API key for {provider_name} is required, aborting",
                    "✗".with(Color::Red)
                );
                println!();
                return;
            }
            // else: keep existing value, don't queue for save
        } else {
            keys_to_save.push((api_key_name.to_string(), key));
        }
    }

    // Step 4: Ask about other API keys (optional). When a key is already set
    // (either in config or keyring), the prompt shows that and Enter keeps it.
    println!();
    println!(
        "  {}",
        "You can also set API keys for other providers (optional, press Enter to skip):"
            .with(Color::DarkGrey)
    );
    for &(prov, key_name) in PROVIDERS {
        if prov == provider_name || prov == "ollama" || key_name.is_empty() {
            continue;
        }
        let label = match prov {
            "anthropic" => "Anthropic",
            "openai" => "OpenAI",
            "gemini" => "Gemini",
            "grok" => "Grok",
            "mistral" => "Mistral",
            "deepseek" => "DeepSeek",
            "kimi" => "Kimi",
            "zai" => "Z.ai",
            _ => prov,
        };
        let prompt_label = if let Some(hint) = key_status_hint(key_name) {
            format!("{label} ({key_name}, {hint}):")
        } else {
            format!("{label} ({key_name}):")
        };
        let Some(key) = read_input_line(&prompt_label, true) else {
            println!();
            println!("  {} configuration cancelled", "✗".with(Color::Yellow));
            println!();
            return;
        };
        let key = key.trim().to_string();
        if !key.is_empty() {
            keys_to_save.push((key_name.to_string(), key));
        }
    }

    // Step 5: Ollama host (optional)
    if is_ollama {
        println!();
        println!(
            "  {}",
            "Enter Ollama host (press Enter for default http://localhost:11434):"
                .with(Color::DarkGrey)
        );
        if let Some(host) = read_input_line("LLM_OLLAMA_HOST:", false) {
            let host = host.trim().to_string();
            if !host.is_empty() {
                keys_to_save.push(("LLM_OLLAMA_HOST".to_string(), host));
            }
        } else {
            println!();
            println!("  {} configuration cancelled", "✗".with(Color::Yellow));
            println!();
            return;
        }
    }

    // Save everything
    crate::config::config_set("AICTL_PROVIDER", provider_name);
    crate::config::config_set("AICTL_MODEL", &model);
    for (key_name, key_value) in &keys_to_save {
        crate::config::config_set(key_name, key_value);
    }

    println!();
    println!(
        "  {} configuration saved to ~/.aictl/config",
        "✓".with(Color::Green)
    );
    println!();
    println!("  {} {provider_name}", "provider:".with(Color::Cyan));
    println!("  {} {model}", "model:   ".with(Color::Cyan));
    if !keys_to_save.is_empty() {
        let saved_keys: Vec<&str> = keys_to_save.iter().map(|(k, _)| k.as_str()).collect();
        println!(
            "  {} {}",
            "keys:    ".with(Color::Cyan),
            saved_keys.join(", ")
        );
    }
    println!();

    // Step 6: Offer to migrate API keys into the system keyring (filter out
    // non-secret entries like LLM_OLLAMA_HOST by intersecting with KEY_NAMES).
    let lockable: Vec<String> = keys_to_save
        .iter()
        .filter(|(k, _)| crate::keys::KEY_NAMES.contains(&k.as_str()))
        .map(|(k, _)| k.clone())
        .collect();
    if !lockable.is_empty() && crate::keys::backend_available() {
        println!(
            "  {} {} {}",
            "→".with(Color::Cyan),
            format!("Lock {} API key(s) into the system keyring", lockable.len())
                .with(Color::White),
            format!("({})", crate::keys::backend_name()).with(Color::Green),
        );
        println!(
            "  {}",
            "Removes plain-text copies from ~/.aictl/config.".with(Color::DarkGrey),
        );
        if confirm_yn("lock keys now?") {
            for key in &lockable {
                match crate::keys::lock_key(key) {
                    crate::keys::LockOutcome::Locked => println!(
                        "  {} {} → keyring",
                        "✓".with(Color::Green),
                        key.as_str().with(Color::White),
                    ),
                    crate::keys::LockOutcome::AlreadyLocked
                    | crate::keys::LockOutcome::NotInConfig => {}
                    crate::keys::LockOutcome::Error(e) => println!(
                        "  {} {} ({})",
                        "✗".with(Color::Red),
                        key.as_str().with(Color::White),
                        e.with(Color::Red),
                    ),
                }
            }
            println!();
        }
    }

    if from_repl {
        println!(
            "  {} configuration applied — continuing your session",
            "→".with(Color::Cyan),
        );
    } else {
        println!(
            "  {} run {} to start a conversation, or {} for a single query",
            "→".with(Color::Cyan),
            "aictl"
                .with(Color::White)
                .attribute(crossterm::style::Attribute::Bold),
            "aictl -m \"your message\""
                .with(Color::White)
                .attribute(crossterm::style::Attribute::Bold),
        );
    }
    println!();
}

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

// --- Agent management ---

use crate::agents;

fn build_agent_menu_lines(selected: usize) -> Vec<String> {
    let has_loaded = agents::loaded_agent_name().is_some();
    let items = agent_menu_items(has_loaded);
    let max_name = items.iter().map(|(n, _)| n.len()).max().unwrap_or(0);
    let mut lines = Vec::new();
    for (i, (name, desc)) in items.iter().enumerate() {
        let is_selected = i == selected;
        let padded = format!("{:<max_name$}", *name);
        let name_styled = if is_selected {
            format!(
                "  {}",
                padded
                    .with(Color::White)
                    .attribute(crossterm::style::Attribute::Bold)
            )
        } else {
            format!("  {}", padded.with(Color::DarkGrey))
        };
        let desc_styled = format!("{}", desc.with(Color::DarkGrey));
        let line = if is_selected {
            format!("  {} {name_styled}  {desc_styled}", "›".with(Color::Cyan))
        } else {
            format!("    {name_styled}  {desc_styled}")
        };
        lines.push(line);
    }
    lines
}

fn agent_menu_items(has_loaded: bool) -> Vec<(&'static str, &'static str)> {
    let mut items = vec![
        ("create agent manually", "type or paste agent prompt"),
        ("create agent with AI", "describe what the agent should do"),
        ("view all agents", "browse, load, or delete agents"),
    ];
    if has_loaded {
        items.push(("edit agent", "edit currently loaded agent prompt"));
        items.push(("unload agent", "remove currently loaded agent"));
    }
    items
}

/// Run the /agent menu. Returns `true` if the system prompt should be rebuilt.
#[allow(clippy::too_many_arguments)]
pub async fn run_agent_menu(
    provider: &Provider,
    api_key: &str,
    model: &str,
    messages: &mut [Message],
    ui: &dyn AgentUI,
    show_error: &dyn Fn(&str),
) -> bool {
    let has_loaded = agents::loaded_agent_name().is_some();
    let items = agent_menu_items(has_loaded);
    let Some(sel) = select_from_menu(items.len(), 0, build_agent_menu_lines) else {
        return false;
    };
    match items[sel].0 {
        "create agent manually" => create_agent_manually(show_error),
        "create agent with AI" => {
            create_agent_with_ai(provider, api_key, model, ui, show_error).await
        }
        "view all agents" => view_all_agents(messages, show_error),
        "edit agent" => {
            let name = agents::loaded_agent_name().unwrap_or_default();
            edit_agent_prompt(&name, true, messages, show_error).unwrap_or(false)
        }
        "unload agent" => unload_agent_action(messages),
        _ => false,
    }
}

fn create_agent_manually(show_error: &dyn Fn(&str)) -> bool {
    let Some(name) = read_input_line("agent name:", false) else {
        return false;
    };
    let name = name.trim().to_string();
    if name.is_empty() {
        return false;
    }
    if !agents::is_valid_name(&name) {
        show_error("Invalid name. Use only letters, numbers, underscore, or dash.");
        return false;
    }

    println!();
    println!(
        "  {}",
        "Enter agent prompt (multi-line: Ctrl+D to finish, Esc to cancel):".with(Color::DarkGrey)
    );
    let Some(prompt) = read_multiline_input() else {
        return false;
    };
    let prompt = prompt.trim().to_string();
    if prompt.is_empty() {
        show_error("Empty prompt, agent not created.");
        return false;
    }

    if let Err(e) = agents::save_agent(&name, &prompt) {
        show_error(&format!("Failed to save agent: {e}"));
        return false;
    }
    println!();
    println!(
        "  {} agent \"{}\" created",
        "✓".with(Color::Green),
        name.with(Color::Magenta)
    );
    println!();
    false
}

/// Read multi-line input. Ctrl+D finishes input, Esc cancels.
/// Supports bracketed paste mode so pasted text is received as a single event.
fn read_multiline_input() -> Option<String> {
    read_multiline_input_prefilled("")
}

/// Read multi-line input with optional pre-filled content.
/// The initial text is displayed and editable. Ctrl+D finishes, Esc cancels.
fn read_multiline_input_prefilled(initial: &str) -> Option<String> {
    use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
    use crossterm::terminal;

    print!("  ");
    let _ = std::io::stdout().flush();

    let _ = terminal::enable_raw_mode();
    let _ = crossterm::execute!(std::io::stdout(), event::EnableBracketedPaste);
    let mut buf = String::new();

    // Pre-fill buffer and display initial content
    if !initial.is_empty() {
        buf.push_str(initial);
        for ch in initial.chars() {
            if ch == '\n' {
                print!("\r\n  ");
            } else if ch == '\t' {
                print!("    ");
            } else {
                print!("{ch}");
            }
        }
        let _ = std::io::stdout().flush();
    }

    let result = loop {
        if !event::poll(std::time::Duration::from_millis(200)).unwrap_or(false) {
            continue;
        }
        match event::read() {
            Ok(Event::Paste(text)) => {
                let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
                buf.push_str(&normalized);
                for ch in normalized.chars() {
                    if ch == '\n' {
                        print!("\r\n  ");
                    } else if ch == '\t' {
                        print!("    ");
                    } else {
                        print!("{ch}");
                    }
                }
                let _ = std::io::stdout().flush();
            }
            Ok(Event::Key(key)) if key.kind == KeyEventKind::Press => match key.code {
                KeyCode::Esc => break None,
                KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    break Some(buf.clone());
                }
                KeyCode::Enter => {
                    buf.push('\n');
                    print!("\r\n  ");
                    let _ = std::io::stdout().flush();
                }
                KeyCode::Backspace => {
                    if buf.pop().is_some() {
                        print!("\x08 \x08");
                        let _ = std::io::stdout().flush();
                    }
                }
                KeyCode::Tab => {
                    buf.push('\t');
                    print!("    ");
                    let _ = std::io::stdout().flush();
                }
                KeyCode::Char(c) => {
                    buf.push(c);
                    print!("{c}");
                    let _ = std::io::stdout().flush();
                }
                _ => {}
            },
            _ => {}
        }
    };
    let _ = crossterm::execute!(std::io::stdout(), event::DisableBracketedPaste);
    let _ = terminal::disable_raw_mode();
    println!();
    result
}

#[allow(clippy::too_many_lines)]
async fn create_agent_with_ai(
    provider: &Provider,
    api_key: &str,
    model: &str,
    ui: &dyn AgentUI,
    show_error: &dyn Fn(&str),
) -> bool {
    let Some(name) = read_input_line("agent name:", false) else {
        return false;
    };
    let name = name.trim().to_string();
    if name.is_empty() {
        return false;
    }
    if !agents::is_valid_name(&name) {
        show_error("Invalid name. Use only letters, numbers, underscore, or dash.");
        return false;
    }

    println!();
    println!(
        "  {}",
        "Describe what the agent should do:".with(Color::DarkGrey)
    );
    let Some(description) = read_input_line("description:", false) else {
        return false;
    };
    let description = description.trim().to_string();
    if description.is_empty() {
        return false;
    }

    ui.start_spinner("generating agent prompt...");

    let gen_messages = vec![
        Message {
            role: Role::System,
            content: "You are an expert at writing system prompts for AI assistants. \
                Generate a clear, detailed system prompt for an AI agent based on the user's \
                description. The prompt should define the agent's role, capabilities, behavior, \
                and constraints. Output ONLY the prompt text, nothing else."
                .to_string(),
            images: vec![],
        },
        Message {
            role: Role::User,
            content: format!(
                "Create a system prompt for an AI agent named \"{name}\" that does the following: {description}"
            ),
            images: vec![],
        },
    ];

    let result = match provider {
        Provider::Openai => {
            crate::with_esc_cancel(crate::llm_openai::call_openai(
                api_key,
                model,
                &gen_messages,
            ))
            .await
        }
        Provider::Anthropic => {
            crate::with_esc_cancel(crate::llm_anthropic::call_anthropic(
                api_key,
                model,
                &gen_messages,
            ))
            .await
        }
        Provider::Gemini => {
            crate::with_esc_cancel(crate::llm_gemini::call_gemini(
                api_key,
                model,
                &gen_messages,
            ))
            .await
        }
        Provider::Grok => {
            crate::with_esc_cancel(crate::llm_grok::call_grok(api_key, model, &gen_messages)).await
        }
        Provider::Mistral => {
            crate::with_esc_cancel(crate::llm_mistral::call_mistral(
                api_key,
                model,
                &gen_messages,
            ))
            .await
        }
        Provider::Deepseek => {
            crate::with_esc_cancel(crate::llm_deepseek::call_deepseek(
                api_key,
                model,
                &gen_messages,
            ))
            .await
        }
        Provider::Kimi => {
            crate::with_esc_cancel(crate::llm_kimi::call_kimi(api_key, model, &gen_messages)).await
        }
        Provider::Zai => {
            crate::with_esc_cancel(crate::llm_zai::call_zai(api_key, model, &gen_messages)).await
        }
        Provider::Ollama => {
            crate::with_esc_cancel(crate::llm_ollama::call_ollama(model, &gen_messages)).await
        }
        Provider::Gguf => {
            crate::with_esc_cancel(crate::llm_gguf::call_gguf(model, &gen_messages)).await
        }
        Provider::Mlx => {
            crate::with_esc_cancel(crate::llm_mlx::call_mlx(model, &gen_messages)).await
        }
    };

    ui.stop_spinner();

    let result = match result {
        Ok(inner) => inner,
        Err(_interrupted) => {
            println!("\n  {} interrupted\n", "✗".with(Color::Yellow));
            return false;
        }
    };

    let (prompt, _usage) = match result {
        Ok(r) => r,
        Err(e) => {
            show_error(&format!("Failed to generate agent prompt: {e}"));
            return false;
        }
    };

    let prompt = prompt.trim().to_string();
    println!();
    println!("  {}", "Generated agent prompt:".with(Color::Cyan));
    println!();
    for line in prompt.lines() {
        println!("  {}", line.with(Color::DarkGrey));
    }
    println!();

    if !confirm_yn("save this agent?") {
        return false;
    }

    if let Err(e) = agents::save_agent(&name, &prompt) {
        show_error(&format!("Failed to save agent: {e}"));
        return false;
    }
    println!();
    println!(
        "  {} agent \"{}\" created",
        "✓".with(Color::Green),
        name.with(Color::Magenta)
    );
    println!();
    false
}

fn build_agents_list_lines(
    selected: usize,
    entries: &[agents::AgentEntry],
    loaded_name: Option<&str>,
) -> Vec<String> {
    if entries.is_empty() {
        return vec![format!("  {}", "(no agents found)".with(Color::DarkGrey))];
    }
    let mut lines = Vec::new();
    for (i, e) in entries.iter().enumerate() {
        let is_selected = i == selected;
        let is_loaded = loaded_name == Some(e.name.as_str());
        let marker = if is_loaded { "●" } else { " " };
        let body = e.name.as_str();
        let styled = if is_selected {
            format!(
                "{} {}",
                marker.with(Color::Green),
                body.with(Color::White)
                    .attribute(crossterm::style::Attribute::Bold)
            )
        } else {
            format!(
                "{} {}",
                marker.with(Color::Green),
                body.with(Color::DarkGrey)
            )
        };
        let line = if is_selected {
            format!("  {} {styled}", "›".with(Color::Cyan))
        } else {
            format!("    {styled}")
        };
        lines.push(line);
    }
    lines
}

enum AgentListAction {
    Load(usize),
    View(usize),
    Edit(usize),
    Delete(usize),
    Cancel,
}

#[allow(clippy::cast_possible_truncation)]
fn select_agent_from_list(entries: &[agents::AgentEntry]) -> AgentListAction {
    use crossterm::{
        cursor,
        event::{self, Event, KeyCode, KeyEventKind},
        execute,
        terminal::{self, ClearType},
    };

    let loaded_name = agents::loaded_agent_name();
    let mut selected: usize = 0;
    let _ = terminal::enable_raw_mode();
    let mut stdout = std::io::stdout();
    let _ = execute!(stdout, cursor::Hide);

    let mut lines = build_agents_list_lines(selected, entries, loaded_name.as_deref());
    let _ = write!(stdout, "\r\n");
    for line in &lines {
        let _ = write!(stdout, "{line}\r\n");
    }
    let hint = "↑/↓ navigate · l/enter load · v view · e edit · d delete · esc cancel";
    let _ = write!(stdout, "\r\n  {}\r\n", hint.with(Color::DarkGrey));
    let _ = stdout.flush();
    let mut rendered = lines.len() + 2;

    let result = loop {
        if !event::poll(std::time::Duration::from_millis(100)).unwrap_or(false) {
            continue;
        }
        let Ok(ev) = event::read() else {
            break AgentListAction::Cancel;
        };
        if let Event::Key(key) = ev
            && key.kind == KeyEventKind::Press
        {
            match key.code {
                KeyCode::Up => selected = selected.saturating_sub(1),
                KeyCode::Down => {
                    if !entries.is_empty() && selected + 1 < entries.len() {
                        selected += 1;
                    }
                }
                KeyCode::Enter | KeyCode::Char('l' | 'L') => {
                    if !entries.is_empty() {
                        break AgentListAction::Load(selected);
                    }
                }
                KeyCode::Char('v' | 'V') => {
                    if !entries.is_empty() {
                        break AgentListAction::View(selected);
                    }
                }
                KeyCode::Char('e' | 'E') => {
                    if !entries.is_empty() {
                        break AgentListAction::Edit(selected);
                    }
                }
                KeyCode::Char('d' | 'D') => {
                    if !entries.is_empty() {
                        break AgentListAction::Delete(selected);
                    }
                }
                KeyCode::Esc => break AgentListAction::Cancel,
                _ => {}
            }
        } else {
            continue;
        }

        let _ = execute!(
            stdout,
            cursor::MoveUp(rendered as u16),
            terminal::Clear(ClearType::FromCursorDown),
        );
        lines = build_agents_list_lines(selected, entries, loaded_name.as_deref());
        for line in &lines {
            let _ = write!(stdout, "{line}\r\n");
        }
        let _ = write!(stdout, "\r\n  {}\r\n", hint.with(Color::DarkGrey));
        let _ = stdout.flush();
        rendered = lines.len() + 2;
    };

    let _ = execute!(
        stdout,
        cursor::MoveUp(rendered as u16),
        terminal::Clear(ClearType::FromCursorDown),
        cursor::Show,
    );
    let _ = terminal::disable_raw_mode();
    result
}

fn view_all_agents(messages: &mut [Message], show_error: &dyn Fn(&str)) -> bool {
    loop {
        let entries = agents::list_agents();
        if entries.is_empty() {
            println!();
            println!(
                "  {}",
                "No agents found. Create one first.".with(Color::DarkGrey)
            );
            println!();
            return false;
        }
        match select_agent_from_list(&entries) {
            AgentListAction::Cancel => return false,
            AgentListAction::Load(i) => {
                let entry = &entries[i];
                let Ok(prompt) = agents::read_agent(&entry.name) else {
                    show_error("Failed to read agent file.");
                    return false;
                };
                agents::load_agent(&entry.name, &prompt);
                rebuild_system_prompt(messages);
                println!();
                println!(
                    "  {} agent \"{}\" loaded",
                    "✓".with(Color::Green),
                    entry.name.as_str().with(Color::Magenta)
                );
                println!();
                return true;
            }
            AgentListAction::View(i) => {
                let entry = &entries[i];
                let Ok(prompt) = agents::read_agent(&entry.name) else {
                    show_error("Failed to read agent file.");
                    continue;
                };
                println!();
                println!(
                    "  {} {}",
                    "agent:".with(Color::Cyan),
                    entry.name.as_str().with(Color::Magenta)
                );
                println!();
                for line in prompt.lines() {
                    println!("  {}", line.with(Color::DarkGrey));
                }
                println!();
                // After viewing, return to the list
            }
            AgentListAction::Edit(i) => {
                let entry = &entries[i];
                let is_loaded = agents::loaded_agent_name().as_deref() == Some(entry.name.as_str());
                if edit_agent_prompt(&entry.name, is_loaded, messages, show_error) == Some(true) {
                    return true;
                }
                // Return to the list
            }
            AgentListAction::Delete(i) => {
                let entry = &entries[i];
                if !confirm_yn(&format!("delete agent \"{}\"?", entry.name)) {
                    continue;
                }
                // If deleting the currently loaded agent, unload it first
                if agents::loaded_agent_name().as_deref() == Some(entry.name.as_str()) {
                    agents::unload_agent();
                    rebuild_system_prompt(messages);
                }
                if let Err(e) = agents::delete_agent(&entry.name) {
                    show_error(&format!("Failed to delete agent: {e}"));
                } else {
                    println!();
                    println!("  {} agent deleted", "✓".with(Color::Green));
                    println!();
                }
            }
        }
    }
}

/// Edit an agent's prompt. Returns `Some(true)` if the system prompt was rebuilt,
/// `Some(false)` if saved but agent not loaded, `None` if cancelled.
fn edit_agent_prompt(
    name: &str,
    is_loaded: bool,
    messages: &mut [Message],
    show_error: &dyn Fn(&str),
) -> Option<bool> {
    let Ok(current_prompt) = agents::read_agent(name) else {
        show_error("Failed to read agent file.");
        return None;
    };

    println!();
    println!(
        "  {} {}",
        "editing agent:".with(Color::Cyan),
        name.with(Color::Magenta)
    );
    println!();
    println!(
        "  {}",
        "Edit prompt below (Ctrl+D to finish, Esc to cancel):".with(Color::DarkGrey)
    );
    let new_prompt = read_multiline_input_prefilled(&current_prompt)?;
    let new_prompt = new_prompt.trim().to_string();
    if new_prompt.is_empty() {
        show_error("Empty prompt, agent not updated.");
        return None;
    }

    if let Err(e) = agents::save_agent(name, &new_prompt) {
        show_error(&format!("Failed to save agent: {e}"));
        return None;
    }

    let rebuilt = if is_loaded {
        agents::load_agent(name, &new_prompt);
        rebuild_system_prompt(messages);
        true
    } else {
        false
    };

    println!();
    println!(
        "  {} agent \"{}\" updated",
        "✓".with(Color::Green),
        name.with(Color::Magenta)
    );
    println!();
    Some(rebuilt)
}

fn unload_agent_action(messages: &mut [Message]) -> bool {
    if agents::unload_agent() {
        rebuild_system_prompt(messages);
        println!();
        println!("  {} agent unloaded", "✓".with(Color::Green));
        println!();
        true
    } else {
        println!();
        println!("  {} no agent loaded", "✗".with(Color::DarkGrey));
        println!();
        false
    }
}

/// Rebuild the system prompt (messages[0]) including any loaded agent prompt.
fn rebuild_system_prompt(messages: &mut [Message]) {
    if messages.is_empty() {
        return;
    }
    messages[0].content = crate::build_system_prompt();
}
