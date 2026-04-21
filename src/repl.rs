//! Interactive REPL: input loop, slash-command dispatch, tab completion,
//! prompt rendering, and per-turn driving of [`crate::run::run_agent_turn`].
//!
//! [`run_interactive`] is the entry point — `aictl` invoked without
//! `--message` ends up here. It owns the [`rustyline::Editor`] (with
//! [`SlashCommandHelper`] for tab-completion of `/<cmd>` names), the live
//! conversation `Vec<Message>`, the mutable `provider` / `model` / `api_key`
//! that `/model` and `/config` can swap mid-session, and the persistent
//! session UUID so each turn auto-saves to `~/.aictl/sessions/<uuid>`.
//!
//! [`handle_repl_input`] dispatches one input line: slash commands route to
//! the [`crate::commands`] handlers; anything else (auto-compacting if the
//! context is over the configured threshold) falls through to a
//! [`ReplAction::RunAgentTurn`] that the loop in [`run_interactive`] feeds
//! into [`run_and_display_turn`].

use crossterm::style::{Attribute, Color, Stylize};
use rustyline::error::ReadlineError;

use crate::commands::{self, MemoryMode};
use crate::config::{self, MAX_MESSAGES, auto_compact_threshold, config_get, config_set};
use crate::message::{Message, Role};
use crate::run::{Interrupted, Provider, run_agent_turn, stdout_is_tty};
use crate::skills::Skill;
use crate::ui::{AgentUI, InteractiveUI};
use crate::{
    agents, fetch_remote_version, keys, llm, security, session, skills, stats, tools,
    version_cache, version_info_string,
};

// --- Slash command tab completion ---

pub(crate) struct SlashCommandHelper;

impl rustyline::completion::Completer for SlashCommandHelper {
    type Candidate = rustyline::completion::Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &rustyline::Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Self::Candidate>)> {
        if let Some(prefix) = line[..pos].strip_prefix('/') {
            let matches: Vec<_> = commands::COMMANDS
                .iter()
                .filter(|cmd| cmd.starts_with(prefix))
                .map(|cmd| rustyline::completion::Pair {
                    display: format!("/{cmd}"),
                    replacement: format!("/{cmd}"),
                })
                .collect();
            Ok((0, matches))
        } else {
            Ok((0, vec![]))
        }
    }
}

impl rustyline::hint::Hinter for SlashCommandHelper {
    type Hint = String;

    fn hint(&self, line: &str, pos: usize, _ctx: &rustyline::Context<'_>) -> Option<String> {
        if pos != line.len() {
            return None;
        }
        let prefix = line.strip_prefix('/')?;
        if prefix.is_empty() {
            return None;
        }
        commands::COMMANDS
            .iter()
            .find(|cmd| cmd.starts_with(prefix) && **cmd != prefix)
            .map(|cmd| cmd[prefix.len()..].to_string())
    }
}
impl rustyline::highlight::Highlighter for SlashCommandHelper {
    fn highlight_hint<'h>(&self, hint: &'h str) -> std::borrow::Cow<'h, str> {
        std::borrow::Cow::Owned(format!("\x1b[90m{hint}\x1b[0m"))
    }
}
impl rustyline::validate::Validator for SlashCommandHelper {}
impl rustyline::Helper for SlashCommandHelper {}

enum ReplAction {
    Continue,
    Break,
    RunAgentTurn,
    /// Run an agent turn with this message instead of the typed input
    /// (used by `/retry` to re-submit the previous user prompt).
    RunAgentTurnWith(String),
    /// Invoke a skill with the given task as the user message. The skill
    /// body is injected for this turn only and then dropped.
    InvokeSkill {
        skill: Skill,
        task: String,
    },
}

/// Handle a single REPL input line: dispatch slash commands, auto-compact, etc.
#[allow(
    clippy::too_many_arguments,
    clippy::fn_params_excessive_bools,
    clippy::too_many_lines
)]
async fn handle_repl_input(
    input: &str,
    last_answer: &mut String,
    ui: &InteractiveUI,
    rl: &mut rustyline::Editor<SlashCommandHelper, rustyline::history::DefaultHistory>,
    messages: &mut Vec<Message>,
    last_input_tokens: &mut u64,
    provider: &mut Provider,
    api_key: &mut String,
    model: &mut String,
    auto: &mut bool,
    memory: &mut MemoryMode,
    version_info: &str,
) -> ReplAction {
    if input.is_empty() {
        return ReplAction::Continue;
    }
    if input == "exit" || input == "quit" {
        return ReplAction::Break;
    }

    match commands::handle(input, last_answer, &|msg| ui.show_error(msg)) {
        commands::CommandResult::Exit => return ReplAction::Break,
        commands::CommandResult::Clear => {
            let _ = rl.add_history_entry(input);
            messages.truncate(1);
            tools::clear_call_history();
            last_answer.clear();
            *last_input_tokens = 0;
            println!();
            println!("  {} context cleared", "✓".with(Color::Green));
            println!();
            return ReplAction::Continue;
        }
        commands::CommandResult::Compact => {
            let _ = rl.add_history_entry(input);
            commands::compact(
                provider,
                api_key,
                model,
                messages,
                ui,
                &memory.to_string(),
                false,
            )
            .await;
            *last_input_tokens = 0;
            session::save_current(messages);
            return ReplAction::Continue;
        }
        commands::CommandResult::Session => {
            let _ = rl.add_history_entry(input);
            if session::is_incognito() {
                println!();
                println!(
                    "  {} incognito mode: session functionality is disabled",
                    "⚠".with(Color::Yellow)
                );
                println!();
            } else {
                if commands::run_session_menu(messages, &|msg| ui.show_error(msg)) {
                    *last_input_tokens = 0;
                }
                session::save_current(messages);
            }
            return ReplAction::Continue;
        }
        commands::CommandResult::Agent => {
            let _ = rl.add_history_entry(input);
            commands::run_agent_menu(provider, api_key, model, messages, ui, &|msg| {
                ui.show_error(msg);
            })
            .await;
            return ReplAction::Continue;
        }
        commands::CommandResult::Skills => {
            let _ = rl.add_history_entry(input);
            match commands::run_skills_menu(provider, api_key, model, ui, &|msg| {
                ui.show_error(msg);
            })
            .await
            {
                commands::SkillsMenuOutcome::Nothing => {}
                commands::SkillsMenuOutcome::Invoke { name, task } => {
                    let Some(skill) = skills::find(&name) else {
                        ui.show_error(&format!("Skill '{name}' not found"));
                        return ReplAction::Continue;
                    };
                    return ReplAction::InvokeSkill { skill, task };
                }
            }
            return ReplAction::Continue;
        }
        commands::CommandResult::InvokeSkill { name, task } => {
            let _ = rl.add_history_entry(input);
            let Some(skill) = skills::find(&name) else {
                ui.show_error(&format!("Skill '{name}' not found"));
                return ReplAction::Continue;
            };
            // Task is optional. When absent, the skill body alone drives the
            // turn via a minimal trigger message the LLM sees as the user
            // saying "run this skill."
            return ReplAction::InvokeSkill { skill, task };
        }
        commands::CommandResult::Gguf => {
            let _ = rl.add_history_entry(input);
            commands::run_gguf_menu(&|msg| ui.show_error(msg)).await;
            return ReplAction::Continue;
        }
        commands::CommandResult::Mlx => {
            let _ = rl.add_history_entry(input);
            commands::run_mlx_menu(&|msg| ui.show_error(msg)).await;
            return ReplAction::Continue;
        }
        commands::CommandResult::Context => {
            let _ = rl.add_history_entry(input);
            commands::print_context(model, messages.len(), *last_input_tokens, MAX_MESSAGES);
            return ReplAction::Continue;
        }
        commands::CommandResult::History(args) => {
            let _ = rl.add_history_entry(input);
            commands::print_history(messages, &args);
            return ReplAction::Continue;
        }
        commands::CommandResult::Info => {
            let _ = rl.add_history_entry(input);
            let pname = format!("{provider:?}").to_lowercase();
            let ollama_models = llm::ollama::list_models().await;
            commands::print_info(&pname, model, *auto, *memory, version_info, &ollama_models);
            return ReplAction::Continue;
        }
        commands::CommandResult::Security => {
            let _ = rl.add_history_entry(input);
            commands::print_security();
            return ReplAction::Continue;
        }
        commands::CommandResult::Continue => {
            let _ = rl.add_history_entry(input);
            return ReplAction::Continue;
        }
        commands::CommandResult::Keys => {
            let _ = rl.add_history_entry(input);
            commands::run_keys_menu(&|msg| ui.show_error(msg));
            return ReplAction::Continue;
        }
        commands::CommandResult::Stats => {
            let _ = rl.add_history_entry(input);
            commands::run_stats_menu(&|msg| ui.show_error(msg));
            return ReplAction::Continue;
        }
        commands::CommandResult::Ping => {
            let _ = rl.add_history_entry(input);
            commands::run_ping().await;
            return ReplAction::Continue;
        }
        commands::CommandResult::Retry => {
            let _ = rl.add_history_entry(input);
            let Some(prompt) = commands::retry_last_exchange(messages) else {
                ui.show_error("nothing to retry");
                return ReplAction::Continue;
            };
            tools::clear_call_history();
            last_answer.clear();
            *last_input_tokens = 0;
            let preview: String = prompt.chars().take(80).collect();
            let ellipsis = if prompt.chars().count() > 80 {
                "…"
            } else {
                ""
            };
            println!();
            println!(
                "  {} retry — resending: {}{}",
                "↩".with(Color::Yellow),
                preview.replace('\n', " ").with(Color::DarkGrey),
                ellipsis.with(Color::DarkGrey),
            );
            println!();
            session::save_current(messages);
            return ReplAction::RunAgentTurnWith(prompt);
        }
        commands::CommandResult::Config => {
            let _ = rl.add_history_entry(input);
            commands::run_config_wizard(true);
            // Re-read provider/model/api_key from config so the change takes
            // effect mid-session. The wizard may have been cancelled, in which
            // case the config values are unchanged and these reads are no-ops.
            if let Some(new_prov) = config_get("AICTL_PROVIDER") {
                let resolved = match new_prov.as_str() {
                    "openai" => Some(Provider::Openai),
                    "anthropic" => Some(Provider::Anthropic),
                    "gemini" => Some(Provider::Gemini),
                    "grok" => Some(Provider::Grok),
                    "mistral" => Some(Provider::Mistral),
                    "deepseek" => Some(Provider::Deepseek),
                    "kimi" => Some(Provider::Kimi),
                    "zai" => Some(Provider::Zai),
                    "ollama" => Some(Provider::Ollama),
                    "gguf" => Some(Provider::Gguf),
                    "mlx" => Some(Provider::Mlx),
                    _ => None,
                };
                if let Some(p) = resolved {
                    *provider = p;
                }
            }
            if let Some(new_model) = config_get("AICTL_MODEL") {
                *model = new_model;
            }
            if matches!(
                provider,
                Provider::Ollama | Provider::Gguf | Provider::Mlx | Provider::Mock
            ) {
                *api_key = String::new();
            } else {
                let key_name = match provider {
                    Provider::Openai => "LLM_OPENAI_API_KEY",
                    Provider::Anthropic => "LLM_ANTHROPIC_API_KEY",
                    Provider::Gemini => "LLM_GEMINI_API_KEY",
                    Provider::Grok => "LLM_GROK_API_KEY",
                    Provider::Mistral => "LLM_MISTRAL_API_KEY",
                    Provider::Deepseek => "LLM_DEEPSEEK_API_KEY",
                    Provider::Kimi => "LLM_KIMI_API_KEY",
                    Provider::Zai => "LLM_ZAI_API_KEY",
                    Provider::Ollama | Provider::Gguf | Provider::Mlx | Provider::Mock => {
                        unreachable!()
                    }
                };
                if let Some(k) = keys::get_secret(key_name) {
                    *api_key = k;
                } else {
                    ui.show_error(&format!(
                        "API key for {key_name} is not set — current session may fail until you run /config or /keys"
                    ));
                }
            }
            return ReplAction::Continue;
        }
        commands::CommandResult::Update => {
            let _ = rl.add_history_entry(input);
            if commands::run_update(&|msg| ui.show_error(msg)).await {
                return ReplAction::Break;
            }
            return ReplAction::Continue;
        }
        commands::CommandResult::Uninstall => {
            let _ = rl.add_history_entry(input);
            if commands::run_uninstall_repl(&|msg| ui.show_error(msg)) {
                return ReplAction::Break;
            }
            return ReplAction::Continue;
        }
        commands::CommandResult::Version => {
            let _ = rl.add_history_entry(input);
            commands::run_version(&|msg| ui.show_error(msg)).await;
            return ReplAction::Continue;
        }
        commands::CommandResult::Model => {
            let _ = rl.add_history_entry(input);
            let ollama_models = llm::ollama::list_models().await;
            let local_models = llm::gguf::list_models();
            let mlx_models = llm::mlx::list_models();
            if let Some((new_provider, new_model, api_key_name)) =
                commands::select_model(model, &ollama_models, &local_models, &mlx_models)
            {
                if matches!(
                    new_provider,
                    Provider::Ollama | Provider::Gguf | Provider::Mlx
                ) {
                    let pname = match new_provider {
                        Provider::Ollama => "ollama",
                        Provider::Gguf => "gguf",
                        Provider::Mlx => "mlx",
                        _ => unreachable!(),
                    };
                    config_set("AICTL_PROVIDER", pname);
                    config_set("AICTL_MODEL", &new_model);
                    *provider = new_provider;
                    *model = new_model;
                    *api_key = String::new();
                } else {
                    let Some(new_api_key) = keys::get_secret(&api_key_name) else {
                        ui.show_error(&format!(
                            "API key not found. Set {api_key_name} in ~/.aictl/config or run /keys to migrate from another provider"
                        ));
                        return ReplAction::Continue;
                    };
                    config_set(
                        "AICTL_PROVIDER",
                        &format!("{new_provider:?}").to_lowercase(),
                    );
                    config_set("AICTL_MODEL", &new_model);
                    *provider = new_provider;
                    *model = new_model;
                    *api_key = new_api_key;
                }
                let pname = format!("{provider:?}").to_lowercase();
                println!();
                println!("  {} switched to {pname}/{model}", "✓".with(Color::Green));
                println!();
            }
            return ReplAction::Continue;
        }
        commands::CommandResult::Behavior => {
            let _ = rl.add_history_entry(input);
            if let Some(new_auto) = commands::select_behavior(*auto) {
                *auto = new_auto;
                let behavior = if *auto { "auto" } else { "human-in-the-loop" };
                println!();
                println!(
                    "  {} switched to {behavior} behavior",
                    "✓".with(Color::Green)
                );
                println!();
            }
            return ReplAction::Continue;
        }
        commands::CommandResult::Memory => {
            let _ = rl.add_history_entry(input);
            if let Some(new_memory) = commands::select_memory(*memory) {
                *memory = new_memory;
                config_set("AICTL_MEMORY", &format!("{new_memory}"));
                println!();
                println!(
                    "  {} switched to {new_memory} memory",
                    "✓".with(Color::Green)
                );
                println!();
            }
            return ReplAction::Continue;
        }
        commands::CommandResult::NotACommand => {}
    }

    let _ = rl.add_history_entry(input);

    // Auto-compact if context is >= configured threshold (default 80%)
    let token_pct = llm::pct(*last_input_tokens, llm::context_limit(model));
    let message_pct = llm::pct_usize(messages.len(), MAX_MESSAGES);
    let context_pct = token_pct.max(message_pct);
    if context_pct >= auto_compact_threshold() {
        println!();
        println!(
            "  {} context at {context_pct}%, auto-compacting...",
            "⚠".with(Color::Yellow)
        );
        commands::compact(
            provider,
            api_key,
            model,
            messages,
            ui,
            &memory.to_string(),
            true,
        )
        .await;
        *last_input_tokens = 0;
        session::save_current(messages);
    }

    ReplAction::RunAgentTurn
}

/// Run an agent turn and display the result, updating REPL state.
#[allow(clippy::too_many_arguments)]
async fn run_and_display_turn(
    provider: &Provider,
    api_key: &str,
    model: &str,
    messages: &mut Vec<Message>,
    input: &str,
    auto: &mut bool,
    ui: &InteractiveUI,
    last_answer: &mut String,
    last_input_tokens: &mut u64,
    memory: MemoryMode,
    skill: Option<&Skill>,
) {
    let msg_len_before = messages.len();
    // Interactive REPL is always a TTY; honor user's AICTL_STREAMING preference.
    let streaming = stdout_is_tty() && config::streaming_enabled();
    match run_agent_turn(
        provider, api_key, model, messages, input, auto, ui, memory, streaming, skill,
    )
    .await
    {
        Ok(turn) => {
            stats::record(model, turn.llm_calls, turn.tool_calls, &turn.usage);
            ui.show_answer(&turn.answer);
            *last_answer = turn.answer;
            *last_input_tokens = turn.last_input_tokens;
            if turn.llm_calls > 1 {
                let tp = llm::pct(turn.last_input_tokens, llm::context_limit(model));
                let mp = llm::pct_usize(messages.len(), MAX_MESSAGES);
                let cp = tp.max(mp);
                ui.show_summary(
                    &turn.usage,
                    model,
                    turn.llm_calls,
                    turn.tool_calls,
                    turn.elapsed,
                    cp,
                );
            } else {
                // First LLM response was the final answer — no summary will
                // run to add the trailing blank line, so emit one here so the
                // next prompt isn't glued to the status line.
                eprintln!();
            }
        }
        Err(e) => {
            if e.downcast_ref::<Interrupted>().is_some() {
                messages.truncate(msg_len_before);
                println!("\n  {} interrupted\n", "✗".with(Color::Yellow));
            } else {
                ui.show_error(&format!("Error: {e}"));
            }
        }
    }
}

/// Interactive REPL mode: multi-turn conversation with persistent history.
///
/// `initial_skill`, when `Some`, applies to the first user turn only — one-turn
/// scope is the defining property of skills, so CLI `--skill` in REPL mode is
/// treated as "run the next turn with this skill, then revert."
#[allow(clippy::too_many_lines, clippy::too_many_arguments)]
pub(crate) async fn run_interactive(
    mut provider: Provider,
    mut api_key: String,
    mut model: String,
    auto: bool,
    session_key: Option<String>,
    initial_skill: Option<Skill>,
) -> Result<(), Box<dyn std::error::Error>> {
    // Check the local cache first. If `~/.aictl/version` exists and its
    // timestamp is less than 24h old, we already know the latest upstream
    // version and skip the network call entirely. On a miss (or stale entry)
    // we kick the remote fetch off on another worker so startup (config,
    // session init, file I/O) proceeds in parallel; `fetch_remote_version`
    // writes the result back into the cache so the *next* run shows the
    // banner notice instantly even if this launch's fetch didn't complete
    // before banner render.
    let cached_version = version_cache::cached_fresh();
    let version_fetch = if cached_version.is_none() {
        Some(tokio::spawn(async { fetch_remote_version().await }))
    } else {
        None
    };

    let mut auto = auto;
    let mut memory = match config_get("AICTL_MEMORY").as_deref() {
        Some("short-term") => MemoryMode::ShortTerm,
        _ => MemoryMode::LongTerm,
    };
    let ui = InteractiveUI::new();

    let mut messages = vec![Message {
        role: Role::System,
        content: crate::run::build_system_prompt(),
        images: vec![],
    }];

    // Initialize session: load if requested, otherwise create a new one.
    // Skipped entirely in incognito mode.
    let mut loaded_ok = false;
    if session::is_incognito() {
        if session_key.is_some() {
            ui.show_error("--session is ignored in incognito mode");
        }
    } else if let Some(key) = session_key.as_ref() {
        if let Some(id) = session::resolve(key) {
            match session::load_messages(&id) {
                Ok(loaded) => {
                    let name = session::name_for(&id);
                    messages = loaded;
                    let label = name
                        .as_deref()
                        .map_or_else(|| id.clone(), |n| format!("{id} ({n})"));
                    session::set_current(id, name);
                    println!("  {} loaded session: {label}", "✓".with(Color::Green));
                    loaded_ok = true;
                }
                Err(e) => {
                    ui.show_error(&format!("Failed to load session '{key}': {e}"));
                }
            }
        } else {
            ui.show_error(&format!(
                "Session '{key}' not found. Starting a new session."
            ));
        }
    }
    if !loaded_ok && !session::is_incognito() {
        let id = session::generate_uuid();
        session::set_current(id, None);
    }
    if !session::is_incognito() {
        stats::record_session();
    }
    session::save_current(&messages);

    // Prefer the cached result if we had one; otherwise consume the live
    // fetch if it's already completed. If neither is ready, fall back to an
    // empty string so the banner prints immediately — the background task
    // still runs to completion and populates the cache for the next launch.
    let version_info = if let Some(cached) = cached_version.as_deref() {
        version_info_string(Some(cached))
    } else if let Some(fetch) = version_fetch {
        if fetch.is_finished() {
            match fetch.await {
                Ok(remote) => version_info_string(remote.as_deref()),
                Err(_) => String::new(),
            }
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    InteractiveUI::print_welcome(
        &format!("{provider:?}").to_lowercase(),
        &model,
        memory,
        &version_info,
    );

    let mut rl = rustyline::Editor::new()?;
    rl.set_helper(Some(SlashCommandHelper));

    // Load history
    let history_path = std::env::var("HOME")
        .map(|h| format!("{h}/.aictl/history"))
        .unwrap_or_default();
    if !history_path.is_empty() {
        let _ = rl.load_history(&history_path);
    }

    let mut last_answer = String::new();
    let mut last_input_tokens: u64 = 0;
    // `--skill` in REPL mode applies to the first user turn only. After
    // consuming it once the REPL reverts to normal behavior.
    let mut pending_skill: Option<Skill> = initial_skill;

    loop {
        let unrestricted = !security::policy().enabled;
        let agent_prefix = agents::loaded_agent_name()
            .map(|name| format!("{} ", format!("[{name}]").with(Color::Magenta)));
        let ap = agent_prefix.as_deref().unwrap_or("");
        let prompt = match (auto, unrestricted) {
            (true, true) => format!(
                "{ap}{} {} {} ",
                "[auto]".with(Color::Yellow),
                "[unrestricted]".with(Color::Red),
                "❯".with(Color::Cyan).attribute(Attribute::Bold),
            ),
            (true, false) => format!(
                "{ap}{} {} ",
                "[auto]".with(Color::Yellow),
                "❯".with(Color::Cyan).attribute(Attribute::Bold),
            ),
            (false, true) => format!(
                "{ap}{} {} ",
                "[unrestricted]".with(Color::Red),
                "❯".with(Color::Cyan).attribute(Attribute::Bold),
            ),
            (false, false) => {
                format!("{ap}{} ", "❯".with(Color::Cyan).attribute(Attribute::Bold))
            }
        };
        let line = rl.readline(&prompt);
        match line {
            Ok(input) => {
                let input = input.trim().to_string();

                let (retry_input, turn_skill): (Option<String>, Option<Skill>) =
                    match handle_repl_input(
                        &input,
                        &mut last_answer,
                        &ui,
                        &mut rl,
                        &mut messages,
                        &mut last_input_tokens,
                        &mut provider,
                        &mut api_key,
                        &mut model,
                        &mut auto,
                        &mut memory,
                        &version_info,
                    )
                    .await
                    {
                        ReplAction::Continue => continue,
                        ReplAction::Break => break,
                        ReplAction::RunAgentTurn => (None, pending_skill.take()),
                        ReplAction::RunAgentTurnWith(s) => (Some(s), pending_skill.take()),
                        // A skill invocation wins over any pending `--skill`;
                        // the latter is dropped so it doesn't leak into the
                        // next turn either. When the inline task is empty,
                        // fall back to a generic trigger so the skill body
                        // alone drives the turn.
                        ReplAction::InvokeSkill { skill, task } => {
                            pending_skill = None;
                            let message = if task.is_empty() {
                                format!("Run the \"{}\" skill.", skill.name)
                            } else {
                                task
                            };
                            (Some(message), Some(skill))
                        }
                    };

                let turn_input = retry_input.as_deref().unwrap_or(input.as_str());
                run_and_display_turn(
                    &provider,
                    &api_key,
                    &model,
                    &mut messages,
                    turn_input,
                    &mut auto,
                    &ui,
                    &mut last_answer,
                    &mut last_input_tokens,
                    memory,
                    turn_skill.as_ref(),
                )
                .await;
                session::save_current(&messages);
            }
            Err(ReadlineError::Interrupted) => {
                // Ctrl+C: cancel current line
            }
            Err(ReadlineError::Eof) => {
                // Ctrl+D: exit
                break;
            }
            Err(e) => {
                ui.show_error(&format!("Input error: {e}"));
                break;
            }
        }
    }

    // Save history
    if !history_path.is_empty() {
        let _ = rl.save_history(&history_path);
    }

    // Final save and exit notification.
    session::save_current(&messages);
    if let Some((id, name)) = session::current_info() {
        let label = name
            .as_deref()
            .map_or_else(|| id.clone(), |n| format!("{id} ({n})"));
        let resume_arg = name.as_deref().unwrap_or(&id);
        println!();
        println!("  {} session saved: {label}", "✓".with(Color::Green));
        println!(
            "  {} resume with: {} {}",
            "→".with(Color::Cyan),
            "aictl --session".with(Color::Cyan),
            resume_arg.with(Color::Cyan)
        );
        println!();
    }

    Ok(())
}
