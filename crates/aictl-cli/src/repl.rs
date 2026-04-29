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

use crossterm::style::{Color, Stylize};
use rustyline::error::ReadlineError;

use crate::commands::{self, MemoryMode};
use crate::config::{self, MAX_MESSAGES, auto_compact_threshold, config_get, config_set};
use crate::error::AictlError;
use crate::message::{Message, Role};
use crate::run::{Provider, run_agent_turn, stdout_is_tty};
use crate::skills::Skill;
use crate::ui::{AgentUI, InteractiveUI};
use crate::{
    agents, fetch_remote_version, keys, llm, security, session, skills, stats, tools,
    version_cache, version_info_string,
};

// Codex-style prompt block uses raw SGR codes so the dark-gray bg stays
// active across labels and the typing area. Crossterm's `Stylize` injects a
// full SGR reset after each span which would knock the bg off mid-row.
//   `\x1b[48;5;236m` — 256-color dark gray bg (darker than bright-black).
//   `\x1b[K` — erase from cursor to EOL, painting with the current bg so
//              the row fills to the full terminal width.
//   `\x1b[2A` — move cursor up 2 rows; used to drop rustyline's prompt onto
//               the middle row of the pre-painted 3-row block so the bottom
//               padding is visible while the user is still typing.
//   `\x1b[39m` / `\x1b[22m` — reset only fg / bold so the bg survives.
//   `\x1b[0m` — full reset, used between the block and surrounding output.
const PROMPT_BG: &str = "\x1b[48;5;236m";
const PROMPT_FILL: &str = "\x1b[K";
const PROMPT_RESET: &str = "\x1b[0m";
const CURSOR_UP_2: &str = "\x1b[2A";

// --- Slash command tab completion ---

pub(crate) struct SlashCommandHelper;

/// Names completable after `/` with no space yet — the union of built-in
/// slash commands and user-defined skills. Skills are invoked as
/// `/<skill-name>` so they belong in the same completion bucket. Returned
/// sorted with duplicates dropped so a skill can't shadow a built-in in the
/// list (the dispatcher already prefers the built-in).
fn slash_completion_names() -> Vec<String> {
    let mut names: Vec<String> = commands::COMMANDS
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    for entry in skills::list() {
        if !names.iter().any(|n| n == &entry.name) {
            names.push(entry.name);
        }
    }
    names.sort();
    names
}

/// Names completable after `/agent ` — saved agents by name.
fn agent_completion_names() -> Vec<String> {
    agents::list_agents().into_iter().map(|e| e.name).collect()
}

impl rustyline::completion::Completer for SlashCommandHelper {
    type Candidate = rustyline::completion::Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &rustyline::Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Self::Candidate>)> {
        let head = &line[..pos];
        let Some(rest) = head.strip_prefix('/') else {
            return Ok((0, vec![]));
        };

        // `/agent <prefix>` → complete against saved agent names.
        if let Some(agent_arg) = rest.strip_prefix("agent ") {
            let arg_start = pos - agent_arg.len();
            let matches: Vec<_> = agent_completion_names()
                .into_iter()
                .filter(|n| n.starts_with(agent_arg))
                .map(|n| rustyline::completion::Pair {
                    display: n.clone(),
                    replacement: n,
                })
                .collect();
            return Ok((arg_start, matches));
        }

        // A space elsewhere means the user is typing command arguments for
        // which we don't offer completion.
        if rest.contains(' ') {
            return Ok((0, vec![]));
        }

        // `/<prefix>` → commands + skill names.
        let matches: Vec<_> = slash_completion_names()
            .into_iter()
            .filter(|name| name.starts_with(rest))
            .map(|name| rustyline::completion::Pair {
                display: format!("/{name}"),
                replacement: format!("/{name}"),
            })
            .collect();
        Ok((0, matches))
    }
}

impl rustyline::hint::Hinter for SlashCommandHelper {
    type Hint = String;

    fn hint(&self, line: &str, pos: usize, _ctx: &rustyline::Context<'_>) -> Option<String> {
        if pos != line.len() {
            return None;
        }
        let rest = line.strip_prefix('/')?;
        if rest.is_empty() {
            return None;
        }
        if let Some(agent_arg) = rest.strip_prefix("agent ") {
            if agent_arg.is_empty() {
                return None;
            }
            return agent_completion_names()
                .into_iter()
                .find(|n| n.starts_with(agent_arg) && n != agent_arg)
                .map(|n| n[agent_arg.len()..].to_string());
        }
        if rest.contains(' ') {
            return None;
        }
        slash_completion_names()
            .into_iter()
            .find(|name| name.starts_with(rest) && name != rest)
            .map(|name| name[rest.len()..].to_string())
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

/// Handle a single REPL input line. Thin coordinator: validates the line,
/// asks [`commands::handle`] to classify it, then either dispatches the slash
/// command (via [`dispatch_slash_command`]) or falls through to the user-turn
/// path (auto-compact, then return [`ReplAction::RunAgentTurn`]).
#[allow(clippy::too_many_arguments, clippy::fn_params_excessive_bools)]
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

    let result = commands::handle(input, last_answer, &|msg| ui.show_error(msg));
    if matches!(result, commands::CommandResult::NotACommand) {
        let _ = rl.add_history_entry(input);
        handle_user_turn(
            provider,
            api_key,
            model,
            messages,
            last_input_tokens,
            ui,
            *memory,
        )
        .await;
        return ReplAction::RunAgentTurn;
    }

    dispatch_slash_command(
        result,
        input,
        last_answer,
        ui,
        rl,
        messages,
        last_input_tokens,
        provider,
        api_key,
        model,
        auto,
        memory,
        version_info,
    )
    .await
}

/// Dispatch a non-NotACommand [`commands::CommandResult`]. Each arm is a
/// one-line call to a focused helper so the diff for any single command stays
/// local. History is added once (except for `Exit`, which exits immediately).
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn dispatch_slash_command(
    result: commands::CommandResult,
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
    if matches!(result, commands::CommandResult::Exit) {
        return ReplAction::Break;
    }
    let _ = rl.add_history_entry(input);

    match result {
        commands::CommandResult::Exit | commands::CommandResult::NotACommand => unreachable!(),
        commands::CommandResult::Clear => {
            handle_clear(messages, last_answer, last_input_tokens);
            ReplAction::Continue
        }
        commands::CommandResult::Compact => {
            handle_compact(
                provider,
                api_key,
                model,
                messages,
                ui,
                *memory,
                last_input_tokens,
            )
            .await;
            ReplAction::Continue
        }
        commands::CommandResult::Session => {
            handle_session(messages, last_input_tokens, ui);
            ReplAction::Continue
        }
        commands::CommandResult::Agent(name) => {
            handle_agent(name.as_deref(), provider, api_key, model, messages, ui).await;
            ReplAction::Continue
        }
        commands::CommandResult::Skills => handle_skills(provider, api_key, model, ui).await,
        commands::CommandResult::InvokeSkill { name, task } => handle_invoke_skill(&name, task, ui),
        commands::CommandResult::Gguf => {
            commands::run_gguf_menu(ui, &|msg| ui.show_error(msg)).await;
            ReplAction::Continue
        }
        commands::CommandResult::Mlx => {
            commands::run_mlx_menu(ui, &|msg| ui.show_error(msg)).await;
            ReplAction::Continue
        }
        commands::CommandResult::Context => {
            commands::print_context(model, messages.len(), *last_input_tokens, MAX_MESSAGES);
            ReplAction::Continue
        }
        commands::CommandResult::History(args) => {
            commands::print_history(messages, &args);
            ReplAction::Continue
        }
        commands::CommandResult::Info => {
            let pname = format!("{provider:?}").to_lowercase();
            let ollama_models = llm::ollama::list_models().await;
            commands::print_info(&pname, model, *auto, *memory, version_info, &ollama_models);
            ReplAction::Continue
        }
        commands::CommandResult::Security => {
            commands::print_security();
            ReplAction::Continue
        }
        commands::CommandResult::Continue => ReplAction::Continue,
        commands::CommandResult::Keys => {
            commands::run_keys_menu(&|msg| ui.show_error(msg));
            ReplAction::Continue
        }
        commands::CommandResult::Stats => {
            commands::run_stats_menu(&|msg| ui.show_error(msg));
            ReplAction::Continue
        }
        commands::CommandResult::Ping => {
            commands::run_ping().await;
            ReplAction::Continue
        }
        commands::CommandResult::Plugins => {
            commands::run_plugins_menu(&|msg| ui.show_error(msg));
            ReplAction::Continue
        }
        commands::CommandResult::Hooks => {
            commands::run_hooks_menu(&|msg| ui.show_error(msg));
            ReplAction::Continue
        }
        commands::CommandResult::Mcp => {
            commands::run_mcp_menu(&|msg| ui.show_error(msg));
            ReplAction::Continue
        }
        commands::CommandResult::Balance => {
            commands::run_balance().await;
            ReplAction::Continue
        }
        commands::CommandResult::Roadmap(query) => {
            commands::run_roadmap(query.as_deref(), &|msg| ui.show_error(msg)).await;
            ReplAction::Continue
        }
        commands::CommandResult::Retry => {
            handle_retry(messages, last_answer, last_input_tokens, ui)
        }
        commands::CommandResult::Undo(n) => {
            handle_undo(n, messages, last_answer, last_input_tokens, ui);
            ReplAction::Continue
        }
        commands::CommandResult::Config => {
            handle_config_command(provider, api_key, model, ui);
            ReplAction::Continue
        }
        commands::CommandResult::Update => {
            if commands::run_update(&|msg| ui.show_error(msg)).await {
                ReplAction::Break
            } else {
                ReplAction::Continue
            }
        }
        commands::CommandResult::Uninstall => {
            if commands::run_uninstall_repl(&|msg| ui.show_error(msg)) {
                ReplAction::Break
            } else {
                ReplAction::Continue
            }
        }
        commands::CommandResult::Version => {
            commands::run_version(&|msg| ui.show_error(msg)).await;
            ReplAction::Continue
        }
        commands::CommandResult::Model(query) => {
            handle_model_switch(query.as_deref(), provider, api_key, model, ui).await;
            ReplAction::Continue
        }
        commands::CommandResult::Behavior => {
            handle_behavior(auto);
            ReplAction::Continue
        }
        commands::CommandResult::Memory => {
            handle_memory(memory);
            ReplAction::Continue
        }
    }
}

// --- Per-command helpers (stateful) ---

fn handle_clear(
    messages: &mut Vec<Message>,
    last_answer: &mut String,
    last_input_tokens: &mut u64,
) {
    messages.truncate(1);
    tools::clear_call_history();
    last_answer.clear();
    *last_input_tokens = 0;
    println!();
    println!("  {} context cleared", "✓".with(Color::Green));
    println!();
}

async fn handle_compact(
    provider: &Provider,
    api_key: &str,
    model: &str,
    messages: &mut Vec<Message>,
    ui: &InteractiveUI,
    memory: MemoryMode,
    last_input_tokens: &mut u64,
) {
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
}

fn handle_session(messages: &mut Vec<Message>, last_input_tokens: &mut u64, ui: &InteractiveUI) {
    if session::is_incognito() {
        println!();
        println!(
            "  {} incognito mode: session functionality is disabled",
            "⚠".with(Color::Yellow)
        );
        println!();
        return;
    }
    if commands::run_session_menu(messages, &|msg| ui.show_error(msg)) {
        *last_input_tokens = 0;
    }
    session::save_current(messages);
}

async fn handle_agent(
    name: Option<&str>,
    provider: &Provider,
    api_key: &str,
    model: &str,
    messages: &mut [Message],
    ui: &InteractiveUI,
) {
    if let Some(name) = name {
        commands::load_agent_by_name(name, messages, &|msg| ui.show_error(msg));
    } else {
        commands::run_agent_menu(provider, api_key, model, messages, ui, &|msg| {
            ui.show_error(msg);
        })
        .await;
    }
}

async fn handle_skills(
    provider: &Provider,
    api_key: &str,
    model: &str,
    ui: &InteractiveUI,
) -> ReplAction {
    match commands::run_skills_menu(provider, api_key, model, ui, &|msg| ui.show_error(msg)).await {
        commands::SkillsMenuOutcome::Nothing => ReplAction::Continue,
        commands::SkillsMenuOutcome::Invoke { name, task } => {
            let Some(skill) = skills::find(&name) else {
                ui.show_error(&format!("Skill '{name}' not found"));
                return ReplAction::Continue;
            };
            ReplAction::InvokeSkill { skill, task }
        }
    }
}

fn handle_invoke_skill(name: &str, task: String, ui: &InteractiveUI) -> ReplAction {
    let Some(skill) = skills::find(name) else {
        ui.show_error(&format!("Skill '{name}' not found"));
        return ReplAction::Continue;
    };
    // Task is optional. When absent, the skill body alone drives the turn via
    // a minimal trigger message the LLM sees as the user saying "run this skill."
    ReplAction::InvokeSkill { skill, task }
}

fn handle_retry(
    messages: &mut Vec<Message>,
    last_answer: &mut String,
    last_input_tokens: &mut u64,
    ui: &InteractiveUI,
) -> ReplAction {
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
    ReplAction::RunAgentTurnWith(prompt)
}

fn handle_undo(
    n: usize,
    messages: &mut Vec<Message>,
    last_answer: &mut String,
    last_input_tokens: &mut u64,
    ui: &InteractiveUI,
) {
    let popped = commands::undo_turns(messages, n);
    if popped == 0 {
        ui.show_error("nothing to undo");
        return;
    }
    tools::clear_call_history();
    last_answer.clear();
    *last_input_tokens = 0;
    let suffix = if popped == 1 { "turn" } else { "turns" };
    println!();
    if popped < n {
        println!(
            "  {} undo — popped {popped} of {n} {suffix} (compaction boundary reached)",
            "↶".with(Color::Yellow),
        );
    } else {
        println!(
            "  {} undo — popped {popped} {suffix}",
            "↶".with(Color::Green)
        );
    }
    println!();
    session::save_current(messages);
}

/// Handle `/config`: run the wizard, then re-read `provider`/`model`/`api_key`
/// from config so the change takes effect mid-session. If the wizard was
/// cancelled the config values are unchanged and these reads are no-ops.
fn handle_config_command(
    provider: &mut Provider,
    api_key: &mut String,
    model: &mut String,
    ui: &InteractiveUI,
) {
    commands::run_config_wizard(true);
    if let Some(new_prov) = config_get("AICTL_PROVIDER")
        && let Some(p) = provider_from_str(&new_prov)
    {
        *provider = p;
    }
    if let Some(new_model) = config_get("AICTL_MODEL") {
        *model = new_model;
    }
    reload_api_key(provider, api_key, ui);
}

/// Reload `api_key` from the keyring/config for the current `provider`. For
/// keyless providers (Ollama/Gguf/Mlx/Mock) this clears it. When the key is
/// missing for a keyed provider we surface a non-fatal error — the next LLM
/// call will fail loudly but the user can still fix it via `/config` or `/keys`.
fn reload_api_key(provider: &Provider, api_key: &mut String, ui: &InteractiveUI) {
    if matches!(
        provider,
        Provider::Ollama | Provider::Gguf | Provider::Mlx | Provider::Mock
    ) {
        *api_key = String::new();
        return;
    }
    let key_name = provider_api_key_name(provider);
    if let Some(k) = keys::get_secret(key_name) {
        *api_key = k;
    } else {
        ui.show_error(&format!(
            "API key for {key_name} is not set — current session may fail until you run /config or /keys"
        ));
    }
}

async fn handle_model_switch(
    query: Option<&str>,
    provider: &mut Provider,
    api_key: &mut String,
    model: &mut String,
    ui: &InteractiveUI,
) {
    let ollama_models = llm::ollama::list_models().await;
    let local_models = llm::gguf::list_models();
    let mlx_models = llm::mlx::list_models();
    let Some((new_provider, new_model, api_key_name)) =
        commands::select_model(model, &ollama_models, &local_models, &mlx_models, query)
    else {
        return;
    };
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
            return;
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

fn handle_behavior(auto: &mut bool) {
    let Some(new_auto) = commands::select_behavior(*auto) else {
        return;
    };
    *auto = new_auto;
    let behavior = if *auto { "auto" } else { "human-in-the-loop" };
    println!();
    println!(
        "  {} switched to {behavior} behavior",
        "✓".with(Color::Green)
    );
    println!();
}

fn handle_memory(memory: &mut MemoryMode) {
    let Some(new_memory) = commands::select_memory(*memory) else {
        return;
    };
    *memory = new_memory;
    config_set("AICTL_MEMORY", &format!("{new_memory}"));
    println!();
    println!(
        "  {} switched to {new_memory} memory",
        "✓".with(Color::Green)
    );
    println!();
}

/// Fall-through path for a non-slash input: auto-compact if we're near the
/// context limit, then leave it to the caller to run the agent turn.
async fn handle_user_turn(
    provider: &Provider,
    api_key: &str,
    model: &str,
    messages: &mut Vec<Message>,
    last_input_tokens: &mut u64,
    ui: &InteractiveUI,
    memory: MemoryMode,
) {
    let token_pct = llm::pct(*last_input_tokens, llm::context_limit(model));
    let message_pct = llm::pct_usize(messages.len(), MAX_MESSAGES);
    let context_pct = token_pct.max(message_pct);
    if context_pct < auto_compact_threshold() {
        return;
    }
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

fn provider_from_str(name: &str) -> Option<Provider> {
    Some(match name {
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
        _ => return None,
    })
}

fn provider_api_key_name(provider: &Provider) -> &'static str {
    match provider {
        Provider::Openai => "LLM_OPENAI_API_KEY",
        Provider::Anthropic => "LLM_ANTHROPIC_API_KEY",
        Provider::Gemini => "LLM_GEMINI_API_KEY",
        Provider::Grok => "LLM_GROK_API_KEY",
        Provider::Mistral => "LLM_MISTRAL_API_KEY",
        Provider::Deepseek => "LLM_DEEPSEEK_API_KEY",
        Provider::Kimi => "LLM_KIMI_API_KEY",
        Provider::Zai => "LLM_ZAI_API_KEY",
        Provider::Ollama | Provider::Gguf | Provider::Mlx | Provider::Mock => unreachable!(),
    }
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
            if matches!(e, AictlError::Interrupted) {
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
) -> Result<(), AictlError> {
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

    let _ = crate::hooks::run_hooks(
        crate::hooks::HookEvent::SessionStart,
        "",
        crate::hooks::HookContext {
            session_id: session::current_id(),
            cwd: std::env::current_dir().ok(),
            trigger: Some(if loaded_ok { "resume" } else { "startup" }),
            ..Default::default()
        },
    )
    .await;

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

    let mut rl = rustyline::Editor::new()
        .map_err(|e| AictlError::Other(format!("readline init failed: {e}")))?;
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
        let agent_prefix =
            agents::loaded_agent_name().map(|name| format!("\x1b[35m[{name}]\x1b[39m "));
        let ap = agent_prefix.as_deref().unwrap_or("");
        let chevron = "\x1b[1;36m❯\x1b[22;39m";
        let auto_label = "\x1b[33m[auto]\x1b[39m";
        let unrestricted_label = "\x1b[31m[unrestricted]\x1b[39m";

        // Pre-paint a 3-row dark-gray block (top padding, blank middle row,
        // bottom padding) and move the cursor up 2 rows. Rustyline then writes
        // the prompt onto the middle row, so the bottom padding is visible
        // while the user is still typing — the user wanted the whole block,
        // including its bottom edge, framed before submission.
        print!(
            "{PROMPT_BG}{PROMPT_FILL}{PROMPT_RESET}\r\n\
             \r\n\
             {PROMPT_BG}{PROMPT_FILL}{PROMPT_RESET}\r\n\
             {CURSOR_UP_2}"
        );
        let _ = std::io::Write::flush(&mut std::io::stdout());

        // Prompt: bg active, line filled with bg, content rendered, no
        // trailing reset so typed chars inherit the dark-gray bg.
        let prompt = match (auto, unrestricted) {
            (true, true) => format!(
                "{PROMPT_BG}{PROMPT_FILL}  {ap}{auto_label} {unrestricted_label} {chevron}  "
            ),
            (true, false) => {
                format!("{PROMPT_BG}{PROMPT_FILL}  {ap}{auto_label} {chevron}  ")
            }
            (false, true) => {
                format!("{PROMPT_BG}{PROMPT_FILL}  {ap}{unrestricted_label} {chevron}  ")
            }
            (false, false) => format!("{PROMPT_BG}{PROMPT_FILL}  {ap}{chevron}  "),
        };
        let line = rl.readline(&prompt);

        // After Enter, cursor sits at column 0 of the bottom-padding row.
        // Reset SGR and step past it onto a fresh line, plus one extra blank
        // row so the spinner / agent reply isn't glued to the prompt block.
        print!("{PROMPT_RESET}\r\n\r\n");
        let _ = std::io::Write::flush(&mut std::io::stdout());
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

    let _ = crate::hooks::run_hooks(
        crate::hooks::HookEvent::SessionEnd,
        "",
        crate::hooks::HookContext {
            session_id: session::current_id(),
            cwd: std::env::current_dir().ok(),
            trigger: Some("repl-exit"),
            ..Default::default()
        },
    )
    .await;

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

#[cfg(test)]
mod tests {
    use super::*;
    use rustyline::completion::Completer;
    use rustyline::hint::Hinter;
    use rustyline::history::DefaultHistory;

    #[test]
    fn completer_lists_builtin_commands_for_slash_prefix() {
        let helper = SlashCommandHelper;
        let history = DefaultHistory::new();
        let ctx = rustyline::Context::new(&history);
        let (start, cands) = helper.complete("/h", 2, &ctx).unwrap();
        assert_eq!(start, 0);
        let replacements: Vec<String> = cands.iter().map(|c| c.replacement.clone()).collect();
        assert!(replacements.iter().any(|r| r == "/help"));
        assert!(replacements.iter().any(|r| r == "/history"));
    }

    #[test]
    fn completer_returns_empty_for_non_slash_input() {
        let helper = SlashCommandHelper;
        let history = DefaultHistory::new();
        let ctx = rustyline::Context::new(&history);
        let (_, cands) = helper.complete("hello", 5, &ctx).unwrap();
        assert!(cands.is_empty());
    }

    #[test]
    fn completer_skips_other_command_args() {
        let helper = SlashCommandHelper;
        let history = DefaultHistory::new();
        let ctx = rustyline::Context::new(&history);
        // `/history` takes free-form args — we don't try to complete them.
        let (_, cands) = helper.complete("/history us", 11, &ctx).unwrap();
        assert!(cands.is_empty());
    }

    #[test]
    fn completer_agent_args_replace_at_arg_start() {
        // Even with no agents on disk the completer must return the correct
        // `start` position so a tab with no matches leaves the user's cursor
        // at the right spot.
        let helper = SlashCommandHelper;
        let history = DefaultHistory::new();
        let ctx = rustyline::Context::new(&history);
        let line = "/agent foo";
        let (start, _) = helper.complete(line, line.len(), &ctx).unwrap();
        assert_eq!(start, "/agent ".len());
    }

    #[test]
    fn hinter_suggests_completion_suffix() {
        let helper = SlashCommandHelper;
        let history = DefaultHistory::new();
        let ctx = rustyline::Context::new(&history);
        // "/he" → "help" → suffix "lp"
        let hint = helper.hint("/he", 3, &ctx);
        assert_eq!(hint.as_deref(), Some("lp"));
    }

    #[test]
    fn hinter_returns_none_for_exact_match() {
        let helper = SlashCommandHelper;
        let history = DefaultHistory::new();
        let ctx = rustyline::Context::new(&history);
        // Exact match of a built-in — nothing to hint.
        let hint = helper.hint("/help", 5, &ctx);
        assert!(hint.is_none());
    }

    #[test]
    fn hinter_returns_none_mid_line() {
        let helper = SlashCommandHelper;
        let history = DefaultHistory::new();
        let ctx = rustyline::Context::new(&history);
        // Cursor not at end — no hint.
        let hint = helper.hint("/help extra", 3, &ctx);
        assert!(hint.is_none());
    }

    #[test]
    fn slash_completion_names_includes_builtins() {
        let names = slash_completion_names();
        for required in ["help", "history", "agent", "skills", "exit"] {
            assert!(
                names.iter().any(|n| n == required),
                "expected {required:?} in slash completion names"
            );
        }
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
    }
}
