mod commands;
#[cfg(test)]
mod integration_tests;
mod repl;
mod ui;
mod version_cache;

use clap::Parser;

// CLI is a thin frontend over the aictl-core crate. Re-export the core's
// modules under crate-relative paths so legacy `crate::config::…` /
// `crate::tools::…` references in the REPL and slash-command handlers
// keep working without an enormous import sweep.
pub(crate) use aictl_core::{Message, Provider, Role, build_system_prompt, with_esc_cancel};
pub(crate) use aictl_core::{
    agents, audit, config, error, hooks, keys, llm, mcp, message, plugins, run, security, session,
    skills, stats, tools,
};

use config::{config_get, load_config};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Fetch the version from the remote Cargo.toml on GitHub.
/// Returns `Some(version_string)` on success, `None` on failure.
/// On success the result is also written to the `~/.aictl/version`
/// cache so subsequent startups (and the next TTL window) see it.
pub(crate) async fn fetch_remote_version() -> Option<String> {
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
    let version = body.lines().find_map(|line| {
        let rest = line.strip_prefix("version")?;
        let (_, val) = rest.split_once('=')?;
        Some(val.trim().trim_matches('"').to_string())
    })?;
    version_cache::save(&version);
    Some(version)
}

/// Format a version status string from a remote version check result.
pub(crate) fn version_info_string(remote: Option<&str>) -> String {
    match remote {
        Some(v) if v == VERSION => "(latest)".to_string(),
        Some(v) => format!("({v} available)"),
        None => String::new(),
    }
}

#[derive(Parser)]
#[command(name = "aictl", version = VERSION, disable_version_flag = true, about = "AI agent in your terminal", after_help = "Omit --message to start an interactive REPL with persistent conversation history.")]
#[allow(clippy::struct_excessive_bools)]
struct Cli {
    /// Print version information
    #[arg(short = 'v', long = "version")]
    version: bool,

    /// Update to the latest version
    #[arg(long = "update")]
    update: bool,

    /// Remove the aictl binary from `~/.cargo/bin/` and `~/.local/bin/` (and
    /// `$AICTL_INSTALL_DIR` if set) and exit. Leaves `~/.aictl/` untouched.
    #[arg(long = "uninstall")]
    uninstall: bool,

    /// LLM provider to use (default: `AICTL_PROVIDER` from ~/.aictl/config)
    #[arg(long)]
    provider: Option<Provider>,

    /// Model to use, e.g. gpt-4o, claude-sonnet-4-20250514 (default: `AICTL_MODEL` from ~/.aictl/config)
    #[arg(long)]
    model: Option<String>,

    /// Message to send to the LLM (omit for interactive mode)
    #[arg(long)]
    message: Option<String>,

    /// Run in autonomous mode (skip tool confirmation prompts)
    #[arg(long)]
    auto: bool,

    /// Suppress tool calls and reasoning, only print the final answer (requires --auto)
    #[arg(long, requires = "auto")]
    quiet: bool,

    /// Write the per-line JSON audit log to this path. Intended for
    /// single-shot (`--message`) runs, which otherwise have no session
    /// id to key the default `~/.aictl/audit/<session-id>` file by.
    /// Force-enables audit logging even if `AICTL_SECURITY_AUDIT_LOG`
    /// is `false`. Parent directories are created on demand.
    #[arg(long = "audit-file", value_name = "PATH")]
    audit_file: Option<std::path::PathBuf>,

    /// Disable security restrictions (use with caution)
    #[arg(long)]
    unrestricted: bool,

    /// Load a saved session by uuid or name (interactive mode only)
    #[arg(long = "session")]
    session: Option<String>,

    /// List all saved sessions and exit
    #[arg(long = "list-sessions")]
    list_sessions: bool,

    /// Clear all saved sessions and exit
    #[arg(long = "clear-sessions")]
    clear_sessions: bool,

    /// Start in incognito mode: interactive REPL without saving sessions
    #[arg(long)]
    incognito: bool,

    /// Load a saved agent by name
    #[arg(long = "agent")]
    agent: Option<String>,

    /// List all saved agents and exit
    #[arg(long = "list-agents")]
    list_agents: bool,

    /// When used with `--list-agents` or `--list-skills`, show only entries
    /// whose frontmatter has a matching `category:` value (case-insensitive,
    /// e.g. `dev`, `ops`).
    #[arg(long = "category", value_name = "NAME")]
    category: Option<String>,

    /// Download an official agent from the aictl repo into ~/.aictl/agents/.
    /// Prompts before overwriting an existing file unless `--force` is set.
    #[arg(long = "pull-agent", value_name = "NAME")]
    pull_agent: Option<String>,

    /// With `--pull-agent` or `--pull-skill`, skip the overwrite prompt and
    /// replace any existing file with the upstream version.
    #[arg(long = "force")]
    force: bool,

    /// Invoke a saved skill by name for this turn (single-shot or REPL).
    /// In single-shot mode the skill body is injected as a transient system
    /// message for the `--message` call only; it is never persisted.
    #[arg(long = "skill")]
    skill: Option<String>,

    /// List all saved skills and exit
    #[arg(long = "list-skills")]
    list_skills: bool,

    /// Download an official skill from the aictl repo into
    /// ~/.aictl/skills/<name>/SKILL.md. Prompts before overwriting unless
    /// `--force` is set.
    #[arg(long = "pull-skill", value_name = "NAME")]
    pull_skill: Option<String>,

    /// Interactive configuration wizard for provider, model, and API keys
    #[arg(long = "config")]
    config: bool,

    /// Migrate API keys from ~/.aictl/config into the system keyring and exit
    #[arg(long = "lock-keys")]
    lock_keys: bool,

    /// Migrate API keys from the system keyring back into ~/.aictl/config and exit
    #[arg(long = "unlock-keys")]
    unlock_keys: bool,

    /// Remove API keys from both ~/.aictl/config and the system keyring and exit
    #[arg(long = "clear-keys")]
    clear_keys: bool,

    /// [experimental] Download a native local GGUF model (spec: hf:owner/repo/file.gguf,
    /// owner/repo:file.gguf, or an https:// URL). Saved under ~/.aictl/models/gguf/.
    #[arg(long = "pull-gguf-model", value_name = "SPEC")]
    pull_gguf_model: Option<String>,

    /// [experimental] List all downloaded native local GGUF models and exit.
    #[arg(long = "list-gguf-models")]
    list_gguf_models: bool,

    /// [experimental] Remove a downloaded native local GGUF model by name and exit.
    #[arg(long = "remove-gguf-model", value_name = "NAME")]
    remove_gguf_model: Option<String>,

    /// [experimental] Remove every downloaded native local GGUF model and exit.
    #[arg(long = "clear-gguf-models")]
    clear_gguf_models: bool,

    /// [experimental] Download a native MLX model from Hugging Face (spec:
    /// mlx:owner/repo or owner/repo). Saved under ~/.aictl/models/mlx/.
    #[arg(long = "pull-mlx-model", value_name = "SPEC")]
    pull_mlx_model: Option<String>,

    /// [experimental] List all downloaded MLX models and exit.
    #[arg(long = "list-mlx-models")]
    list_mlx_models: bool,

    /// [experimental] Remove a downloaded MLX model by name and exit.
    #[arg(long = "remove-mlx-model", value_name = "NAME")]
    remove_mlx_model: Option<String>,

    /// [experimental] Remove every downloaded MLX model and exit.
    #[arg(long = "clear-mlx-models")]
    clear_mlx_models: bool,

    /// Download a Named Entity Recognition model for the redaction
    /// layer (spec: `owner/repo` or `hf:owner/repo`; default shape is
    /// `onnx-community/gliner_small-v2.1`). Saved under
    /// `~/.aictl/models/ner/<repo>/`. Management commands always work;
    /// running inference requires the `redaction-ner` cargo feature.
    #[arg(long = "pull-ner-model", value_name = "SPEC")]
    pull_ner_model: Option<String>,

    /// List all downloaded NER models and exit.
    #[arg(long = "list-ner-models")]
    list_ner_models: bool,

    /// Remove a downloaded NER model by name and exit.
    #[arg(long = "remove-ner-model", value_name = "NAME")]
    remove_ner_model: Option<String>,

    /// Remove every downloaded NER model and exit.
    #[arg(long = "clear-ner-models")]
    clear_ner_models: bool,

    /// List installed plugins (name, description, location, status) and exit.
    /// Plugins live under `~/.aictl/plugins/<name>/` (override via
    /// `AICTL_PLUGINS_DIR`). Output reflects the catalogue at startup —
    /// plugins are not hot-reloaded.
    #[arg(long = "list-plugins")]
    list_plugins: bool,

    /// List configured hooks (event, matcher, command, status) and exit.
    /// Hooks live in `~/.aictl/hooks.json` (override via `AICTL_HOOKS_FILE`).
    #[arg(long = "list-hooks")]
    list_hooks: bool,

    /// List configured MCP servers (name, transport, state, tool count)
    /// and exit. MCP servers live in `~/.aictl/mcp.json` (override via
    /// `AICTL_MCP_CONFIG`) and require `AICTL_MCP_ENABLED=true`.
    #[arg(long = "list-mcp")]
    list_mcp: bool,

    /// Restrict this session to only the named MCP server (others are
    /// disabled). Useful for scripted runs that should only touch one
    /// integration. Effective only when `AICTL_MCP_ENABLED=true`.
    #[arg(long = "mcp-server", value_name = "NAME")]
    mcp_server: Option<String>,

    /// Show remaining credit / quota for each configured cloud provider and
    /// exit. Local providers (Ollama/GGUF/MLX) are out of scope. Providers
    /// without a public balance API are reported as "unknown" with a hint
    /// pointing at their billing dashboard. Synonym: `--list-balances`.
    #[arg(long = "balance")]
    balance: bool,

    /// Synonym for `--balance`: list remaining credit / quota for every
    /// configured cloud provider and exit.
    #[arg(long = "list-balances")]
    list_balances: bool,

    /// Internal: route all LLM calls to the scripted mock provider. Used by
    /// the end-to-end smoke tests under `tests/`. Scripted responses are read
    /// from `AICTL_MOCK_RESPONSES_FILE` on first call; hidden from `--help`
    /// because it is not a user-facing flag.
    #[arg(long = "mock", hide = true)]
    mock: bool,

    /// Route every non-local LLM call through this `aictl-server` URL for
    /// this invocation. Overrides `AICTL_CLIENT_HOST`. Empty string ("")
    /// disables routing for this run even if `AICTL_CLIENT_HOST` is set.
    /// Not persisted.
    #[arg(long = "client-url", value_name = "URL")]
    client_url: Option<String>,

    /// Master key the CLI presents to the configured `aictl-server` for
    /// this invocation. Overrides `AICTL_CLIENT_MASTER_KEY` from config or
    /// the keyring. Not persisted. Visible in shell history and `ps`; the
    /// persistent path is `/keys` or `--config`.
    #[arg(long = "client-master-key", value_name = "KEY")]
    client_master_key: Option<String>,

    /// Launch the bundled `aictl-server` HTTP LLM proxy if it's installed.
    /// Convenience shortcut so users don't have to remember the second
    /// binary name. Forwards any trailing args to the server, e.g.
    /// `aictl --serve -- --bind 0.0.0.0:7878 --quiet`. See SERVER.md.
    #[arg(long = "serve")]
    serve: bool,

    /// Trailing args forwarded verbatim to `aictl-server` when `--serve`
    /// is set. Anything after `--` lands here.
    #[arg(last = true)]
    serve_args: Vec<String>,
}

#[tokio::main]
async fn main() {
    if let Err(msg) = load_config() {
        eprintln!("Error: {msg}");
        std::process::exit(1);
    }

    let cli = Cli::parse();

    // Handle `--serve` before any heavyweight init: the server has
    // its own startup (security, redaction, MCP guards don't apply
    // to a pure proxy) and we don't want to double-initialize them.
    if cli.serve {
        commands::run_serve_cli(&cli.serve_args);
    }

    // Apply per-process overrides for the aictl-server client routing
    // before anything reads config — `config_set` writes through to the
    // in-memory cache. The flags do not persist to disk between runs:
    // the overlay is wiped when the process exits.
    apply_client_overrides(cli.client_url.as_deref(), cli.client_master_key.as_deref());

    let redaction_warnings = security::init(cli.unrestricted);
    if cli.unrestricted {
        eprintln!("Warning: security restrictions disabled (--unrestricted)");
    }
    for w in redaction_warnings {
        eprintln!("Warning: {w}");
    }
    plugins::init();
    hooks::init();

    mcp::init_with(cli.mcp_server.as_deref()).await;

    if handle_management_flags(&cli).await {
        mcp::shutdown().await;
        return;
    }

    let (provider, model, api_key) = if cli.mock {
        (
            Provider::Mock,
            cli.model
                .clone()
                .unwrap_or_else(|| "mock-model".to_string()),
            String::new(),
        )
    } else {
        let provider = resolve_provider(cli.provider.clone());
        let model = resolve_model(cli.model.clone());
        let api_key = resolve_api_key(&provider);
        (provider, model, api_key)
    };

    session::set_incognito(resolve_incognito(cli.incognito));
    load_agent_if_requested(cli.agent.as_deref());
    let loaded_skill = resolve_skill(cli.skill.as_deref());

    if let Some(path) = cli.audit_file.as_deref() {
        audit::set_file_override(path);
    }

    let result = match cli.message {
        Some(ref msg) => {
            let plain_ui = ui::PlainUI {
                quiet: cli.quiet,
                streamed: std::cell::Cell::new(false),
            };
            run::run_agent_single(
                &provider,
                &api_key,
                &model,
                msg,
                cli.auto,
                cli.quiet,
                loaded_skill.as_ref(),
                &plain_ui,
            )
            .await
        }
        None => {
            repl::run_interactive(
                provider,
                api_key,
                model,
                cli.auto,
                cli.session.clone(),
                loaded_skill,
            )
            .await
        }
    };

    if let Err(e) = result {
        mcp::shutdown().await;
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
    mcp::shutdown().await;
}

/// Handle CLI flags that short-circuit normal execution (version checks,
/// administrative commands, model management). Returns `true` when one of
/// these flags was recognized and handled so the caller should return without
/// starting a session. Each subsystem has its own helper below.
async fn handle_management_flags(cli: &Cli) -> bool {
    if handle_info_flags(cli).await {
        return true;
    }
    if handle_session_flags(cli) {
        return true;
    }
    if handle_agent_skill_flags(cli).await {
        return true;
    }
    if handle_wizard_and_key_flags(cli) {
        return true;
    }
    if handle_gguf_flags(cli).await {
        return true;
    }
    if handle_mlx_flags(cli).await {
        return true;
    }
    if handle_ner_flags(cli).await {
        return true;
    }
    if cli.balance || cli.list_balances {
        commands::run_balance().await;
        return true;
    }
    if cli.list_plugins {
        commands::print_plugins_cli();
        return true;
    }
    if cli.list_hooks {
        commands::print_hooks_cli();
        return true;
    }
    if cli.list_mcp {
        commands::print_mcp_cli();
        return true;
    }
    false
}

async fn handle_info_flags(cli: &Cli) -> bool {
    if cli.version {
        let version_info = version_info_string(fetch_remote_version().await.as_deref());
        if version_info.is_empty() {
            println!("aictl {VERSION}");
        } else {
            println!("aictl {VERSION} {version_info}");
        }
        return true;
    }
    if cli.update {
        commands::run_update_cli().await;
        return true;
    }
    if cli.uninstall {
        commands::run_uninstall_cli();
        return true;
    }
    false
}

fn handle_session_flags(cli: &Cli) -> bool {
    if cli.list_sessions {
        commands::print_sessions_cli();
        return true;
    }
    if cli.clear_sessions {
        session::clear_all();
        println!("All saved sessions cleared.");
        return true;
    }
    false
}

async fn handle_agent_skill_flags(cli: &Cli) -> bool {
    if cli.list_agents {
        commands::print_agents_cli(cli.category.as_deref());
        return true;
    }
    if let Some(name) = cli.pull_agent.as_deref() {
        pull_remote_agent(name, cli.force).await;
        return true;
    }
    if cli.list_skills {
        commands::print_skills_cli(cli.category.as_deref());
        return true;
    }
    if let Some(name) = cli.pull_skill.as_deref() {
        pull_remote_skill(name, cli.force).await;
        return true;
    }
    false
}

async fn pull_remote_agent(name: &str, force: bool) {
    let outcome = agents::remote::pull(name, || {
        if force {
            return true;
        }
        // Non-interactive CLI: without --force, don't silently clobber.
        eprintln!("Error: agent '{name}' already exists. Re-run with --force to overwrite.");
        false
    })
    .await;
    match outcome {
        Ok(agents::remote::PullOutcome::Installed) => {
            println!("pulled official agent: {name}");
        }
        Ok(agents::remote::PullOutcome::Overwritten) => {
            println!("updated official agent: {name}");
        }
        Ok(agents::remote::PullOutcome::SkippedExisting) => {
            // Message already printed by the decider above.
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}

async fn pull_remote_skill(name: &str, force: bool) {
    let outcome = skills::remote::pull(name, || {
        if force {
            return true;
        }
        eprintln!("Error: skill '{name}' already exists. Re-run with --force to overwrite.");
        false
    })
    .await;
    match outcome {
        Ok(skills::remote::PullOutcome::Installed) => {
            println!("pulled official skill: {name}");
        }
        Ok(skills::remote::PullOutcome::Overwritten) => {
            println!("updated official skill: {name}");
        }
        Ok(skills::remote::PullOutcome::SkippedExisting) => {
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}

fn handle_wizard_and_key_flags(cli: &Cli) -> bool {
    if cli.config {
        commands::run_config_wizard(false);
        return true;
    }
    if cli.lock_keys {
        commands::run_lock_keys(&|msg| eprintln!("Error: {msg}"));
        return true;
    }
    if cli.unlock_keys {
        commands::run_unlock_keys(&|msg| eprintln!("Error: {msg}"));
        return true;
    }
    if cli.clear_keys {
        commands::run_clear_keys_unconfirmed();
        return true;
    }
    false
}

async fn handle_gguf_flags(cli: &Cli) -> bool {
    if let Some(spec) = cli.pull_gguf_model.as_deref() {
        let ui = ui::InteractiveUI::new();
        match llm::gguf::download_model(&ui, spec, None).await {
            Ok(name) => println!("downloaded GGUF model: {name}"),
            Err(e) => exit_with_error(&e.to_string()),
        }
        return true;
    }
    if cli.list_gguf_models {
        let models = llm::gguf::list_models();
        if models.is_empty() {
            println!(
                "No GGUF models downloaded. Use `aictl --pull-gguf-model <spec>` to fetch one."
            );
        } else {
            for m in models {
                println!("{m}");
            }
        }
        return true;
    }
    if let Some(name) = cli.remove_gguf_model.as_deref() {
        match llm::gguf::remove_model(name) {
            Ok(()) => println!("removed GGUF model: {name}"),
            Err(e) => exit_with_error(&e.to_string()),
        }
        return true;
    }
    if cli.clear_gguf_models {
        match llm::gguf::clear_models() {
            Ok(n) => println!("removed {n} GGUF model(s)"),
            Err(e) => exit_with_error(&e.to_string()),
        }
        return true;
    }
    false
}

async fn handle_mlx_flags(cli: &Cli) -> bool {
    if let Some(spec) = cli.pull_mlx_model.as_deref() {
        let ui = ui::InteractiveUI::new();
        match llm::mlx::download_model(&ui, spec, None).await {
            Ok(name) => println!("downloaded MLX model: {name}"),
            Err(e) => exit_with_error(&e.to_string()),
        }
        return true;
    }
    if cli.list_mlx_models {
        let models = llm::mlx::list_models();
        if models.is_empty() {
            println!("No MLX models downloaded. Use `aictl --pull-mlx-model <spec>` to fetch one.");
        } else {
            for m in models {
                println!("{m}");
            }
        }
        return true;
    }
    if let Some(name) = cli.remove_mlx_model.as_deref() {
        match llm::mlx::remove_model(name) {
            Ok(()) => println!("removed MLX model: {name}"),
            Err(e) => exit_with_error(&e.to_string()),
        }
        return true;
    }
    if cli.clear_mlx_models {
        match llm::mlx::clear_models() {
            Ok(n) => println!("removed {n} MLX model(s)"),
            Err(e) => exit_with_error(&e.to_string()),
        }
        return true;
    }
    false
}

async fn handle_ner_flags(cli: &Cli) -> bool {
    if let Some(spec) = cli.pull_ner_model.as_deref() {
        let ui = ui::InteractiveUI::new();
        match security::redaction::ner::download_model(&ui, spec, None).await {
            Ok(name) => {
                println!("downloaded NER model: {name}");
                if !security::redaction::ner::is_available() {
                    eprintln!(
                        "note: this build lacks the `redaction-ner` feature, so the \
                         pulled model cannot be used for inference yet. Rebuild with \
                         `cargo build --features redaction-ner`."
                    );
                }
            }
            Err(e) => exit_with_error(&e.to_string()),
        }
        return true;
    }
    if cli.list_ner_models {
        let models = security::redaction::ner::list_models();
        if models.is_empty() {
            println!(
                "No NER models downloaded. Use `aictl --pull-ner-model {}` to fetch one.",
                security::redaction::ner::DEFAULT_NER_MODEL
            );
        } else {
            for m in models {
                println!("{m}");
            }
        }
        return true;
    }
    if let Some(name) = cli.remove_ner_model.as_deref() {
        match security::redaction::ner::remove_model(name) {
            Ok(()) => println!("removed NER model: {name}"),
            Err(e) => exit_with_error(&e.to_string()),
        }
        return true;
    }
    if cli.clear_ner_models {
        match security::redaction::ner::clear_models() {
            Ok(n) => println!("removed {n} NER model(s)"),
            Err(e) => exit_with_error(&e.to_string()),
        }
        return true;
    }
    false
}

fn exit_with_error(msg: &str) -> ! {
    eprintln!("Error: {msg}");
    std::process::exit(1);
}

/// Apply ephemeral `--client-url` / `--client-master-key` overrides
/// for this process. Both feed the routing decision in
/// `aictl_core::run::run_agent_turn` only when the user picks
/// `--provider aictl-server` (otherwise the values are inert). Neither
/// is persisted — the URL goes into the in-memory config cache only,
/// and the master key into the keys-module override map (which beats
/// both keyring and plain-config lookup).
///
/// We deliberately don't error out here when only one of the two is
/// set: the dispatch site only consults `active_server()` when
/// `Provider::AictlServer` is the active provider, and surfaces a
/// clear error from there if the pair is incomplete. Treating the
/// keys as inert for every other provider keeps `AICTL_CLIENT_HOST`
/// from becoming effectively obligatory once configured.
fn apply_client_overrides(client_url: Option<&str>, client_master_key: Option<&str>) {
    if let Some(url) = client_url {
        let trimmed = url.trim().trim_end_matches('/');
        config::config_overlay("AICTL_CLIENT_HOST", trimmed);
    }
    if let Some(key) = client_master_key {
        keys::override_secret("AICTL_CLIENT_MASTER_KEY", key.trim());
    }
}

fn resolve_provider(override_: Option<Provider>) -> Provider {
    override_.unwrap_or_else(|| {
        match config_get("AICTL_PROVIDER").as_deref() {
            Some("openai") => Provider::Openai,
            Some("anthropic") => Provider::Anthropic,
            Some("gemini") => Provider::Gemini,
            Some("grok") => Provider::Grok,
            Some("mistral") => Provider::Mistral,
            Some("deepseek") => Provider::Deepseek,
            Some("kimi") => Provider::Kimi,
            Some("zai") => Provider::Zai,
            Some("ollama") => Provider::Ollama,
            Some("gguf") => Provider::Gguf,
            Some("mlx") => Provider::Mlx,
            Some("aictl-server") => Provider::AictlServer,
            Some(other) => {
                exit_with_error(&format!(
                    "invalid AICTL_PROVIDER value '{other}' (expected 'openai', 'anthropic', 'gemini', 'grok', 'mistral', 'deepseek', 'kimi', 'zai', 'ollama', 'gguf', 'mlx', or 'aictl-server')"
                ));
            }
            None => exit_with_error(
                "no provider specified. Use --provider, set AICTL_PROVIDER in ~/.aictl/config, or run aictl --config",
            ),
        }
    })
}

fn resolve_model(override_: Option<String>) -> String {
    override_.unwrap_or_else(|| {
        config_get("AICTL_MODEL").unwrap_or_else(|| {
            exit_with_error(
                "no model specified. Use --model, set AICTL_MODEL in ~/.aictl/config, or run aictl --config",
            )
        })
    })
}

fn resolve_api_key(provider: &Provider) -> String {
    if matches!(
        provider,
        Provider::Ollama | Provider::Gguf | Provider::Mlx | Provider::Mock
    ) {
        return String::new();
    }
    // `Provider::AictlServer` carries the master key separately (via
    // `config::active_server()`), not as a per-provider API key. The
    // dispatch branch in `run::run_agent_turn` reads it directly; the
    // generic `api_key` argument is unused on that path.
    if matches!(provider, Provider::AictlServer) {
        return String::new();
    }
    let key_name = match provider {
        Provider::Openai => "LLM_OPENAI_API_KEY",
        Provider::Anthropic => "LLM_ANTHROPIC_API_KEY",
        Provider::Gemini => "LLM_GEMINI_API_KEY",
        Provider::Grok => "LLM_GROK_API_KEY",
        Provider::Mistral => "LLM_MISTRAL_API_KEY",
        Provider::Deepseek => "LLM_DEEPSEEK_API_KEY",
        Provider::Kimi => "LLM_KIMI_API_KEY",
        Provider::Zai => "LLM_ZAI_API_KEY",
        Provider::Ollama
        | Provider::Gguf
        | Provider::Mlx
        | Provider::Mock
        | Provider::AictlServer => unreachable!(),
    };
    keys::get_secret(key_name).unwrap_or_else(|| {
        exit_with_error(&format!(
            "API key not provided. Set {key_name} in ~/.aictl/config (or use /lock-keys to store it in the system keyring), or run aictl --config"
        ))
    })
}

fn resolve_incognito(cli_flag: bool) -> bool {
    if cli_flag {
        return true;
    }
    match config_get("AICTL_INCOGNITO").as_deref() {
        Some("true") => true,
        Some("false") | None => false,
        Some(other) => exit_with_error(&format!(
            "invalid AICTL_INCOGNITO value '{other}' (expected 'true' or 'false')"
        )),
    }
}

fn load_agent_if_requested(name: Option<&str>) {
    let Some(name) = name else { return };
    if !agents::is_valid_name(name) {
        exit_with_error(&format!(
            "invalid agent name '{name}' (use only letters, numbers, underscore, or dash)"
        ));
    }
    match agents::read_agent(name) {
        Ok(prompt) => agents::load_agent(name, &prompt),
        Err(_) => exit_with_error(&format!("agent '{name}' not found")),
    }
}

fn resolve_skill(name: Option<&str>) -> Option<skills::Skill> {
    name.map(|name| {
        skills::find(name).unwrap_or_else(|| exit_with_error(&format!("skill '{name}' not found")))
    })
}
