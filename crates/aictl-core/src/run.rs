//! Core agent loop: provider selection, streaming plumbing, tool dispatch,
//! and the per-turn run/render of a model response.
//!
//! [`run_agent_turn`] is the workhorse — it sends a user message through the
//! configured provider, parses any `<tool>` calls in the response, executes
//! them under the security policy, and loops until the model produces a final
//! answer (or hits [`crate::config::max_iterations`]). [`run_agent_single`]
//! is the single-shot wrapper used by `--message`; the REPL drives
//! [`run_agent_turn`] directly via [`crate::repl::run_and_display_turn`].
//!
//! Also home to [`Provider`] (the runtime-resolved provider tag),
//! [`Interrupted`] / [`with_esc_cancel`] (Esc-key cancellation for any in-flight
//! future), the [`build_stream_sink`] / [`run_with_streaming`] machinery used
//! by every provider call when `AICTL_STREAMING` is on, and
//! [`build_system_prompt`] which assembles the base system prompt + project
//! prompt file + loaded agent prompt.

use std::io::IsTerminal;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;

use clap::ValueEnum;

use crate::config::{
    self, MAX_MESSAGES, SPINNER_PHRASES, SYSTEM_PROMPT, SYSTEM_PROMPT_CHAT_ONLY,
    SYSTEM_PROMPT_CODING, coding_agent_enabled, load_prompt_file, max_iterations,
};
use crate::error::AictlError;
use crate::hooks::{self, HookContext, HookEvent};
use crate::message::{Message, Role};
use crate::security::redaction::{
    self, RedactionDirection, RedactionMode, RedactionPolicy, RedactionResult, RedactionSource,
};
use crate::skills::Skill;
use crate::ui::{self, AgentUI};
use crate::{agents, audit, llm, mcp, plugins, security, stats, tools};
use llm::{TokenSink, TokenUsage, stream::StreamState};

/// Cached "is stdout a TTY?" check. Computed once at startup to avoid repeated
/// syscalls on every agent turn. Streaming auto-disables when stdout is being
/// piped to a file/pager regardless of `AICTL_STREAMING`, since interleaved
/// progressive output is rarely useful in that case.
static STDOUT_IS_TTY: OnceLock<bool> = OnceLock::new();

pub fn stdout_is_tty() -> bool {
    *STDOUT_IS_TTY.get_or_init(|| std::io::stdout().is_terminal())
}

/// Result of a single agent turn.
#[derive(Debug)]
pub struct TurnResult {
    pub answer: String,
    pub usage: TokenUsage,
    pub llm_calls: u32,
    pub tool_calls: u32,
    pub elapsed: std::time::Duration,
    pub last_input_tokens: u64,
}

#[derive(Debug, Clone, ValueEnum)]
pub enum Provider {
    Openai,
    Anthropic,
    Gemini,
    Grok,
    Mistral,
    Deepseek,
    Kimi,
    Zai,
    Ollama,
    Gguf,
    Mlx,
    /// Explicit `aictl-server` upstream. Selected via
    /// `--provider aictl-server` (or `AICTL_PROVIDER=aictl-server`); the
    /// model name is forwarded verbatim and dispatch always goes through
    /// [`crate::llm::server_proxy::call`] using `AICTL_CLIENT_HOST` +
    /// `AICTL_CLIENT_MASTER_KEY`.
    ///
    /// Distinct from the *implicit* routing the same proxy module
    /// supports: setting `AICTL_CLIENT_HOST` while keeping
    /// `--provider openai` (etc.) still routes those calls through the
    /// server, with the model staying part of the upstream provider's
    /// catalogue. This variant is the "use the server's own model
    /// catalogue" mode — shown in `/model` and `/ping` like any other
    /// provider, and the model list comes from `${url}/v1/models`.
    #[value(name = "aictl-server")]
    AictlServer,
    /// Scripted provider used by the integration tests. Hidden from the CLI
    /// via `#[value(skip)]` so users can never select it; the actual dispatch
    /// in `run_agent_turn` is cfg-gated so non-test builds can't route here.
    #[value(skip)]
    #[allow(dead_code)]
    Mock,
}

impl Provider {
    /// True for providers that run on the local machine (`Ollama`, `Gguf`,
    /// `Mlx`). When `aictl-server` routing is configured, local providers
    /// still bypass the server — they live in the same process or on the
    /// same host, so a network round-trip would be pointless and would
    /// also leak the server's identity into traffic that never had to
    /// leave the machine.
    ///
    /// `AictlServer` is **not** local — it speaks HTTP to a separate
    /// process (possibly on a different host) and goes through
    /// [`crate::llm::server_proxy`].
    #[must_use]
    pub fn is_local(&self) -> bool {
        matches!(self, Self::Ollama | Self::Gguf | Self::Mlx)
    }
}

/// Parse the lowercase provider tag the desktop / `/model` menu writes
/// to [`crate::config::AICTL_PROVIDER`] back into a [`Provider`] variant.
/// Returns `None` for unrecognized strings — every caller treats that as
/// "skip the override" rather than a hard error.
fn provider_from_tag(tag: &str) -> Option<Provider> {
    match tag.trim() {
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
        "aictl-server" => Some(Provider::AictlServer),
        _ => None,
    }
}

/// Map a [`Provider`] onto the keyring/plain-config secret name it
/// expects. Local providers and the server proxy return `None` — they
/// don't speak per-provider API keys.
fn api_key_name_for(provider: &Provider) -> Option<&'static str> {
    match provider {
        Provider::Openai => Some("LLM_OPENAI_API_KEY"),
        Provider::Anthropic => Some("LLM_ANTHROPIC_API_KEY"),
        Provider::Gemini => Some("LLM_GEMINI_API_KEY"),
        Provider::Grok => Some("LLM_GROK_API_KEY"),
        Provider::Mistral => Some("LLM_MISTRAL_API_KEY"),
        Provider::Deepseek => Some("LLM_DEEPSEEK_API_KEY"),
        Provider::Kimi => Some("LLM_KIMI_API_KEY"),
        Provider::Zai => Some("LLM_ZAI_API_KEY"),
        Provider::Ollama
        | Provider::Gguf
        | Provider::Mlx
        | Provider::Mock
        | Provider::AictlServer => None,
    }
}

/// True when any message in the current turn carries one or more image
/// attachments. Drives the Settings → Image Models analysis override —
/// a text-only conversation never swaps providers.
fn messages_have_images(msgs: &[Message]) -> bool {
    msgs.iter().any(|m| !m.images.is_empty())
}

/// Render the synthetic `<test_failure>` user turn the agent loop
/// injects after a `test` tool dispatch reports `failed > 0`. The
/// model parses this block on the next iteration to decide what to fix.
///
/// When `terminal` is `true`, the block also tells the model the retry
/// budget has been exhausted so it should surface the remaining
/// failures to the user instead of looping further.
fn format_test_failure_block(
    summary: &tools::TestSummary,
    attempt: u32,
    budget: u32,
    terminal: bool,
) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let tag = if terminal {
        "test_failure_terminal"
    } else {
        "test_failure"
    };
    let _ = writeln!(out, "<{tag}>");
    let _ = writeln!(
        out,
        "Test command exited {} ({} passed, {} failed, {} skipped).",
        summary.exit_code, summary.passed, summary.failed, summary.skipped
    );
    let _ = writeln!(out, "Retry {attempt} of {budget}.");
    if let Some(warn) = &summary.parse_warning {
        let _ = writeln!(out, "Parser note: {warn}");
    }
    if !summary.failures.is_empty() {
        let _ = writeln!(out, "\nFailing tests:");
        for f in &summary.failures {
            let _ = writeln!(out, "- {}", f.name);
            if !f.message.is_empty() {
                for line in f.message.lines() {
                    let _ = writeln!(out, "    {line}");
                }
            }
            if let Some(loc) = &f.location {
                let _ = writeln!(out, "    at {loc}");
            }
        }
    } else if !summary.raw_tail.is_empty() {
        // No structured failures parsed — give the model the tail of
        // the runner output so it has something to act on.
        let _ = writeln!(out, "\nRaw tail:");
        out.push_str(&summary.raw_tail);
        if !out.ends_with('\n') {
            out.push('\n');
        }
    }
    if terminal {
        let _ = writeln!(
            out,
            "\nThe retry budget for the `test` tool has been exhausted. \
             Do not call `test` again this turn. Surface the remaining failures \
             to the user with a brief explanation and a recommendation for the next step."
        );
    } else {
        let _ = writeln!(
            out,
            "\nFix the root cause and re-run the `test` tool. \
             Read the relevant source first to confirm the assumption before editing."
        );
    }
    let _ = write!(out, "</{tag}>");
    out
}

/// Render the synthetic `<review_result>` user turn injected by the
/// structured Review hook when the host's build / per-file lint
/// sequence reports a failure.
fn format_review_result_block(
    build: Option<&crate::coding::StepResult>,
    lints: &[crate::coding::StepResult],
    attempt: u32,
    budget: u32,
) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(out, "<review_result>");
    let _ = writeln!(
        out,
        "Structured Review hook reported a failure (attempt {attempt} of {budget})."
    );

    if let Some(b) = build {
        let _ = writeln!(out, "\nBuild step: `{}` exited {}.", b.command, b.exit_code);
        if b.exit_code != 0 && !b.output_tail.is_empty() {
            let _ = writeln!(out, "Build output (tail):");
            out.push_str(&b.output_tail);
            if !out.ends_with('\n') {
                out.push('\n');
            }
        }
    } else {
        let _ = writeln!(
            out,
            "\nBuild step: skipped (no build command detected for this project)."
        );
    }

    let lint_failures: Vec<&crate::coding::StepResult> =
        lints.iter().filter(|l| l.exit_code == 1).collect();
    let lint_skipped: Vec<&crate::coding::StepResult> =
        lints.iter().filter(|l| l.exit_code == -2).collect();
    if !lint_failures.is_empty() {
        let _ = writeln!(out, "\nLint failures:");
        for l in &lint_failures {
            let _ = writeln!(out, "  {}", l.command);
            if !l.output_tail.is_empty() {
                for line in l.output_tail.lines() {
                    let _ = writeln!(out, "    {line}");
                }
            }
        }
    }
    if !lint_skipped.is_empty() {
        let _ = writeln!(
            out,
            "\n(skipped lint for {} files — no linter configured for those extensions)",
            lint_skipped.len()
        );
    }

    let _ = writeln!(
        out,
        "\nFix the underlying issues and emit a new final answer. The Review hook will re-run automatically."
    );
    let _ = write!(out, "</review_result>");
    out
}

/// Resolve the user-configured image-analysis override into a runtime
/// `(Provider, model, api_key)` tuple. Returns `None` when no override
/// is configured or the configured provider tag is unrecognized.
///
/// Mirrors the per-binary `api_key_for` helpers in the CLI / desktop
/// crates so the engine can swap routing for one call without asking
/// the caller to re-resolve keys.
fn resolve_image_analysis_override() -> Option<(Provider, String, String)> {
    let (prov_tag, model) = config::image_analysis_override()?;
    let provider = provider_from_tag(&prov_tag)?;
    let api_key = api_key_name_for(&provider)
        .and_then(crate::keys::get_secret)
        .unwrap_or_default();
    Some((provider, model, api_key))
}

#[cfg(test)]
mod prompt_tests {
    use super::{build_system_prompt, build_system_prompt_with};

    // The CONFIG global is process-wide and shared across the test
    // suite, so we don't try to assert which of the three base prompts
    // landed — that depends on whatever earlier tests left in
    // `AICTL_CODING_AGENT` / `AICTL_TOOLS_ENABLED`. We only assert the
    // non-empty + phase-hint behaviors that are pure on the inputs.
    #[test]
    fn build_system_prompt_returns_non_empty() {
        let s = build_system_prompt();
        assert!(!s.is_empty(), "system prompt must be non-empty");
    }

    #[test]
    fn build_system_prompt_with_none_matches_build_system_prompt() {
        // Both helpers should hit the same code path when no hint is
        // supplied — `build_system_prompt` is literally
        // `build_system_prompt_with(None)`.
        assert_eq!(build_system_prompt(), build_system_prompt_with(None));
    }
}

#[cfg(test)]
mod provider_tests {
    use super::Provider;

    /// Exhaustive coverage gate for [`Provider::is_local`]. New variants
    /// must explicitly answer the local-vs-remote question here so a
    /// future provider can't accidentally route through `aictl-server`
    /// (or skip it) by default.
    #[test]
    fn is_local_exhaustive_per_variant() {
        for v in [
            Provider::Openai,
            Provider::Anthropic,
            Provider::Gemini,
            Provider::Grok,
            Provider::Mistral,
            Provider::Deepseek,
            Provider::Kimi,
            Provider::Zai,
            // AictlServer speaks HTTP to a separate process — its own
            // dispatch branch routes through `server_proxy::call`, not
            // a local module.
            Provider::AictlServer,
        ] {
            assert!(!v.is_local(), "{v:?} must not be local");
        }
        for v in [Provider::Ollama, Provider::Gguf, Provider::Mlx] {
            assert!(v.is_local(), "{v:?} must be local");
        }
        // Mock is bag-of-tricks for tests; treat it as remote so the
        // routing branch in run_agent_turn handles it like any other
        // non-local provider when active_server is configured.
        assert!(!Provider::Mock.is_local());
    }
}

// --- Esc key interrupt support ---

/// Error type for user-initiated interruption via Esc key.
#[derive(Debug)]
pub struct Interrupted;

impl std::fmt::Display for Interrupted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "interrupted")
    }
}

impl std::error::Error for Interrupted {}

/// Wrap a future so that pressing Esc cancels it.
///
/// The Esc-listening lifecycle (raw-mode toggle, keyboard polling) lives
/// on the [`AgentUI`] implementation: [`AgentUI::interruption`] returns a
/// future that resolves on cancellation; the implementation's `Drop` impl
/// on the listener cleans up regardless of which branch of the `select!`
/// resolves first.
///
/// Skipped entirely in two cases:
///   * Under `#[cfg(test)]` — `cargo test` inherits the shell's TTY on FD 1,
///     so `is_terminal()` still returns `true`, but no test presses Esc. If
///     the listener ran, it would flip the terminal into raw mode and bare
///     `\n` in the test harness output (run concurrently by parallel tests)
///     would stop resetting the cursor to column 0, producing staircase
///     margins and run-together lines in `cargo test` output.
///   * When stdout is not a TTY (piped output, pager) — raw mode and a
///     keyboard poller serve no purpose there either.
#[cfg(test)]
pub async fn with_esc_cancel<F: std::future::Future>(
    _ui: &dyn AgentUI,
    future: F,
) -> Result<F::Output, Interrupted> {
    Ok(future.await)
}

#[cfg(not(test))]
pub async fn with_esc_cancel<F: std::future::Future>(
    ui: &dyn AgentUI,
    future: F,
) -> Result<F::Output, Interrupted> {
    if !stdout_is_tty() {
        return Ok(future.await);
    }
    tokio::select! {
        value = future => Ok(value),
        () = ui.interruption() => Err(Interrupted),
    }
}

/// Build the full system prompt, appending the project prompt file and loaded agent if present.
///
/// When `AICTL_TOOLS_ENABLED=false` the base prompt is swapped for a
/// pure-chat variant that omits the tool catalog entirely and tells the
/// model tools are unavailable. This prevents the model from trying to emit
/// `<tool>` XML (which would be blocked by the execute-tool guard anyway)
/// and stops it hallucinating filesystem or network access.
pub fn build_system_prompt() -> String {
    build_system_prompt_with(None)
}

/// Like [`build_system_prompt`] but appends an optional per-turn phase
/// guidance block. The CLI's REPL hands this in when coding-agent mode
/// is on; every other frontend (and the CLI when coding mode is off)
/// calls [`build_system_prompt`] which passes `None`.
///
/// The hint is appended to the base coding-agent prompt as a
/// `# Phase guidance` section so the model sees it as an authoritative
/// per-turn cue rather than persona drift.
#[must_use]
pub fn build_system_prompt_with(phase_hint: Option<&str>) -> String {
    // Tools-off always wins: a coding agent without tools is a chat-only
    // session that happens to know the word "phase". Fall back to the
    // chat-only prompt and ignore any phase hint in that case.
    let base = match (tools::tools_enabled(), coding_agent_enabled()) {
        (false, _) => SYSTEM_PROMPT_CHAT_ONLY,
        (true, true) => SYSTEM_PROMPT_CODING,
        (true, false) => SYSTEM_PROMPT,
    };
    let mut prompt = base.to_string();
    // Coding-agent only: prepend a snapshot of the working tree
    // (branch, recent commits, dirty files, top-level layout, detected
    // build/lint/test commands). Cached per working directory; busted by
    // `crate::coding::invalidate_repo_context` after every write through
    // `handle_tool_call`.
    if tools::tools_enabled() && coding_agent_enabled() {
        let cwd = security::policy().paths.working_dir.clone();
        let block = crate::coding::format_repo_context(&cwd);
        if !block.is_empty() {
            prompt.push_str(&block);
        }
    }
    if tools::tools_enabled()
        && coding_agent_enabled()
        && let Some(hint) = phase_hint
    {
        prompt.push_str("\n\n# Phase guidance\n\n");
        prompt.push_str(hint);
    }
    if tools::tools_enabled() {
        let plugin_list = plugins::list();
        if !plugin_list.is_empty() {
            use std::fmt::Write as _;
            prompt.push_str("\n\nAdditional tools (plugins):\n");
            for p in &plugin_list {
                let _ = write!(prompt, "\n### {} (plugin)\n{}\n", p.name, p.catalog_body());
            }
        }
        let mcp_servers = mcp::list();
        let any_ready = mcp_servers
            .iter()
            .any(|s| matches!(s.state, mcp::ServerState::Ready) && !s.tools.is_empty());
        if any_ready {
            use std::fmt::Write as _;
            prompt.push_str(
                "\n\nAdditional tools (MCP servers). Each call body must be a JSON object matching the tool's input schema:\n",
            );
            for server in &mcp_servers {
                if !matches!(server.state, mcp::ServerState::Ready) {
                    continue;
                }
                for tool in &server.tools {
                    let qualified = mcp::qualify(&server.name, &tool.name);
                    let desc = if tool.description.trim().is_empty() {
                        "(no description provided)"
                    } else {
                        tool.description.trim()
                    };
                    let _ = write!(prompt, "\n### {qualified} (mcp)\n{desc}\n");
                    if !tool.input_schema.is_null() {
                        let schema = serde_json::to_string_pretty(&tool.input_schema)
                            .unwrap_or_else(|_| tool.input_schema.to_string());
                        let _ = write!(prompt, "\nInput schema:\n```json\n{schema}\n```\n");
                    }
                }
            }
        }
    }
    if let Some((name, content)) = load_prompt_file() {
        prompt.push_str("\n\n# Project prompt file (");
        prompt.push_str(&name);
        prompt.push_str(")\n\n");
        prompt.push_str(&content);
    }
    if let Some((name, agent_prompt)) = agents::loaded_agent() {
        prompt.push_str("\n\n# Agent: ");
        prompt.push_str(&name);
        prompt.push_str("\n\n");
        prompt.push_str(&agent_prompt);
    }
    // Behavior override lives in `~/.aictl/AICTL.md` (user-global,
    // shared between the CLI and the desktop). The legacy
    // `AICTL_BEHAVIOR` config key is consulted as a fallback so existing
    // installs keep working until the user re-saves through the desktop
    // editor, which migrates the value into the file.
    if let Some(behavior) = crate::config::load_behavior()
        .or_else(|| crate::config::config_get("AICTL_BEHAVIOR"))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    {
        prompt.push_str("\n\n# Behavior overrides\n\n");
        prompt.push_str(&behavior);
    }
    // Long-term memory injects past facts the agent has learned about the
    // user. The helper short-circuits to an empty string when memory is
    // disabled or the session is incognito, so an unconditional concat is safe.
    prompt.push_str(&crate::memory::prompt_block());
    prompt
}

// --- Streaming plumbing ---

/// One event the streaming sink hands to the UI-drain loop.
///
/// `Delta` carries a chunk of model-visible prose; `Suspend` is a single
/// marker emitted on the delta that completes the `<tool name="…">` prefix
/// match, so the UI can flush any buffered word-wrap tail and swap in a
/// "preparing tool call…" spinner before the (hidden) tool-XML stream.
enum StreamEvent {
    Delta(String),
    Suspend,
    /// Parsed `<phase>NAME</phase>` self-report from the model. Forwarded
    /// to `AgentUI::on_phase_change` so the CLI's REPL can flip the
    /// `[phase]` prompt prefix to the model's currently-claimed phase
    /// before the next prompt renders. Gated downstream on
    /// `config::coding_agent_enabled()` — non-coding sessions ignore.
    PhaseChange(crate::coding::WorkflowPhase),
}

/// Build the [`TokenSink`] callback the agent loop hands to a provider when
/// streaming is on, plus the [`tokio::sync::mpsc::UnboundedReceiver`] the
/// caller drains in lock-step.
///
/// The returned sink:
///   * Feeds every delta through [`StreamState::accept`], which holds back any
///     pending tail that could grow into the `<tool name="…">` prefix.
///   * For deltas the state machine has cleared as not-tool-markup, sends
///     them on the channel as [`StreamEvent::Delta`] so the agent loop can
///     forward them to the UI.
///   * On the delta that completes the prefix match, sends
///     [`StreamEvent::Suspend`] (after any final visible emit) so the UI
///     can flush its word-wrap buffer and show a tool-call spinner.
///   * Drops everything once the prefix has matched (stream is suspended).
///
/// The state is also handed back to the caller so it can grab `state.full`
/// after the stream finishes — that's the single source of truth for
/// `parse_tool_call`, even though every provider also returns the assembled
/// string.
fn build_stream_sink() -> (
    TokenSink,
    tokio::sync::mpsc::UnboundedReceiver<StreamEvent>,
    Arc<Mutex<StreamState>>,
) {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<StreamEvent>();
    let state = Arc::new(Mutex::new(StreamState::new()));
    let state_for_sink = state.clone();
    let sink: TokenSink = Arc::new(move |delta: &str| {
        let Ok(mut s) = state_for_sink.lock() else {
            return;
        };
        let result = s.accept(delta);
        if !result.emit.is_empty() {
            let _ = tx.send(StreamEvent::Delta(result.emit));
        }
        if result.became_suspended {
            let _ = tx.send(StreamEvent::Suspend);
        }
        if let Some(phase) = result.phase {
            let _ = tx.send(StreamEvent::PhaseChange(phase));
        }
    });
    (sink, rx, state)
}

/// Run an LLM call concurrently with a UI-drain loop: as the provider's
/// streaming sink pushes deltas into `rx`, this function forwards them to
/// `ui` (calling `stream_begin` once on the first chunk and `stream_end` once
/// when the stream finishes — but only if anything was actually emitted).
///
/// On the first delta we also stop the spinner so the body doesn't print
/// underneath an active spinner.
async fn run_with_streaming<F, T>(
    llm_future: F,
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<StreamEvent>,
    ui: &dyn AgentUI,
) -> (T, bool)
where
    F: std::future::Future<Output = T>,
{
    tokio::pin!(llm_future);
    let mut began = false;
    let handle = |event: StreamEvent, began: &mut bool, ui: &dyn AgentUI| match event {
        StreamEvent::Delta(chunk) => {
            if !*began {
                ui.stop_spinner();
                ui.stream_begin();
                *began = true;
            }
            ui.stream_chunk(&chunk);
        }
        StreamEvent::PhaseChange(phase) => {
            // Forward unconditionally — the UI impl decides whether to
            // act on it. CLI's `InteractiveUI` stores the latest phase
            // for the next REPL prompt; `PlainUI` (and every other
            // frontend in v1) no-ops.
            ui.on_phase_change(phase);
        }
        StreamEvent::Suspend => {
            // Only meaningful once we've started streaming visible prose —
            // otherwise the tool call arrived before any reasoning and the
            // original "thinking..." spinner is still on screen, which is
            // exactly what we'd show here anyway.
            if *began {
                ui.stream_suspend();
            }
        }
    };

    let result = loop {
        tokio::select! {
            // Bias toward the LLM future so on completion we drop to draining
            // any remaining buffered chunks before returning. (tokio::select!
            // is otherwise fair, which would leave unread chunks in `rx`.)
            biased;
            r = &mut llm_future => break r,
            Some(event) = rx.recv() => {
                handle(event, &mut began, ui);
            }
        }
    };
    // Drain anything the sink pushed after the future resolved but before
    // we got back here.
    while let Ok(event) = rx.try_recv() {
        handle(event, &mut began, ui);
    }
    if began {
        ui.stream_end();
    }
    (result, began)
}

/// Wraps an LLM provider future with the right combination of esc-cancel and
/// (optionally) streaming-drain. Returns `(call_result, streamed)` where
/// `streamed` is `true` if any text was actually pushed to the UI via
/// `stream_chunk` during the call (so the caller can decide whether to skip
/// the duplicate `show_answer` / `show_reasoning` re-renders downstream).
async fn run_provider_call<F, T>(
    llm_future: F,
    rx: Option<&mut tokio::sync::mpsc::UnboundedReceiver<StreamEvent>>,
    ui: &dyn AgentUI,
) -> (Result<T, Interrupted>, bool)
where
    F: std::future::Future<Output = T>,
{
    if let Some(rx) = rx {
        match with_esc_cancel(ui, run_with_streaming(llm_future, rx, ui)).await {
            Ok((value, streamed)) => (Ok(value), streamed),
            Err(e) => (Err(e), false),
        }
    } else {
        (with_esc_cancel(ui, llm_future).await, false)
    }
}

// --- Redaction seams ---

/// Produce a provider-bound view of the message slice with each
/// message's content run through the redactor. Returns `None` when no
/// message needed rewriting (so the caller can pass the original slice
/// straight through without cloning). Returns `Err` if any message
/// tripped a `Blocked` result and the policy is `block`.
///
/// The persisted `messages: &[Message]` in the agent loop is never
/// mutated — we only clone when something actually changed, keeping
/// the common "no secrets detected" path zero-alloc.
pub fn redact_outbound(
    messages: &[Message],
    pol: &RedactionPolicy,
    provider: &Provider,
) -> Result<Option<Vec<Message>>, AictlError> {
    if matches!(pol.mode, RedactionMode::Off) {
        return Ok(None);
    }
    if pol.skip_local && matches!(provider, Provider::Ollama | Provider::Gguf | Provider::Mlx) {
        return Ok(None);
    }

    let mut rewritten: Option<Vec<Message>> = None;
    for (i, msg) in messages.iter().enumerate() {
        let source = match msg.role {
            Role::System => RedactionSource::SystemPrompt,
            Role::User => {
                // Tool-result turns are stuffed under Role::User in
                // this agent loop; distinguish them by the wrapper tag
                // so audit entries are accurately labeled.
                if msg.content.starts_with("<tool_result>") {
                    RedactionSource::ToolResult
                } else {
                    RedactionSource::UserMessage
                }
            }
            Role::Assistant => RedactionSource::AssistantMessage,
        };
        match redaction::redact(&msg.content, pol) {
            RedactionResult::Clean => {}
            RedactionResult::Redacted { text, matches } => {
                audit::log_redaction(
                    RedactionDirection::Outbound,
                    source,
                    pol.mode,
                    &msg.content,
                    &matches,
                );
                let buf = rewritten.get_or_insert_with(|| messages.to_vec());
                buf[i].content = text;
            }
            RedactionResult::Blocked { matches } => {
                audit::log_redaction(
                    RedactionDirection::Outbound,
                    source,
                    pol.mode,
                    &msg.content,
                    &matches,
                );
                return Err(AictlError::Redaction(redaction::describe_matches(
                    &msg.content,
                    &matches,
                )));
            }
        }
    }
    Ok(rewritten)
}

// --- Agent loop ---

enum ToolAction {
    Executed,
    Denied,
}

/// Build a `HookContext` for a tool-event hook call. Centralized so
/// `PreToolUse` / `PostToolUse` share the same shape.
fn tool_hook_ctx<'a>(tool_call: &'a tools::ToolCall, output: Option<&'a str>) -> HookContext<'a> {
    HookContext {
        session_id: crate::session::current_id(),
        cwd: std::env::current_dir().ok(),
        tool_name: Some(&tool_call.name),
        tool_input: Some(&tool_call.input),
        tool_output: output,
        ..Default::default()
    }
}

/// Result of a tool-dispatch decision passed back to the agent loop.
///
/// Carries both the executed-call count (for stats) and the names of every
/// call that landed in history (so the loop's post-dispatch hooks — the
/// `test`-retry block in particular — can fire whether the call ran solo or
/// as part of a parallel batch).
struct DispatchResult {
    executed: u32,
    dispatched_names: Vec<String>,
}

/// Outcome of one tool call inside a parallel batch.
struct ParallelCallOutcome {
    idx: usize,
    call: tools::ToolCall,
    result_body: String,
    images: Vec<crate::ImageData>,
    /// `additional_context` lines from the `PostToolUse` hook plus the
    /// hook's `blocked` reason (if any). Appended as a single
    /// `hook_context` user turn after the batch's `<tool_results>` lands.
    hook_extras: Vec<String>,
    /// `true` when the call was rejected by the `PreToolUse` hook, never
    /// dispatched. Doesn't count toward the executed `tool_calls` counter.
    denied: bool,
}

/// Execute one tool call inside a parallel batch. Runs `PreToolUse` → dispatch →
/// `PostToolUse` → optional redaction-block seam, and packages everything into
/// a [`ParallelCallOutcome`] so the orchestrator can join results in source
/// order.
async fn run_parallel_call(idx: usize, call: tools::ToolCall) -> ParallelCallOutcome {
    let pre_outcome = hooks::run_hooks(
        HookEvent::PreToolUse,
        &call.name,
        tool_hook_ctx(&call, None),
    )
    .await;
    if let Some(reason) = pre_outcome.blocked {
        crate::audit::log_tool(
            &call,
            crate::audit::Outcome::DeniedByPolicy {
                reason: &format!("hook: {reason}"),
            },
        );
        return ParallelCallOutcome {
            idx,
            call,
            result_body: format!("Tool call blocked by hook: {reason}"),
            images: vec![],
            hook_extras: vec![],
            denied: true,
        };
    }

    let output = tools::execute_tool(&call).await;

    // Coding-agent workspace tracking. Reads / list / search never land
    // here in a parallel batch (they're parallelizable and side-effect
    // calls aren't), but the partial-rejection path dispatches a single
    // side-effect call through this helper too, so keep the invariant
    // honest.
    if matches!(
        call.name.as_str(),
        "write_file" | "edit_file" | "remove_file" | "create_directory"
    ) {
        crate::coding::invalidate_repo_context();
        if let Some(first) = call.input.lines().next() {
            let path = first.trim();
            if !path.is_empty() {
                crate::coding::record_workspace_change(std::path::Path::new(path));
            }
        }
    }

    let pol = redaction::policy();
    let mut result_content = output.text.clone();
    if matches!(pol.mode, RedactionMode::Block)
        && let RedactionResult::Blocked { matches } = redaction::redact(&output.text, &pol)
    {
        audit::log_redaction(
            RedactionDirection::Inbound,
            RedactionSource::ToolResult,
            pol.mode,
            &output.text,
            &matches,
        );
        result_content = format!(
            "[tool result blocked by redaction policy — {} matches detected]",
            matches.len()
        );
    }

    let post = hooks::run_hooks(
        HookEvent::PostToolUse,
        &call.name,
        tool_hook_ctx(&call, Some(&result_content)),
    )
    .await;
    let mut hook_extras = post.additional_context;
    if let Some(reason) = post.blocked {
        hook_extras.push(format!("hook objection (post): {reason}"));
    }

    ParallelCallOutcome {
        idx,
        call,
        result_body: result_content,
        images: output.images,
        hook_extras,
        denied: false,
    }
}

/// Render a `<tool_results>` user turn aggregating multiple per-call result
/// bodies. The body lists `<tool_result name="…">` blocks in source order so
/// the model reads them in the order it emitted the calls — regardless of
/// completion order during parallel dispatch.
fn build_tool_results_block(outcomes: &[ParallelCallOutcome]) -> String {
    use std::fmt::Write as _;
    let mut body = String::from("<tool_results>\n");
    for o in outcomes {
        let _ = writeln!(body, "<tool_result name=\"{}\">", o.call.name);
        body.push_str(&o.result_body);
        if !body.ends_with('\n') {
            body.push('\n');
        }
        body.push_str("</tool_result>\n");
    }
    body.push_str("</tool_results>");
    body
}

/// Append a single `<tool_results>` user turn carrying rejection messages for
/// every call that was *not* dispatched (because the host short-circuited a
/// mixed batch or `AICTL_CODING_PARALLEL_TOOLS_MAX=0`).
fn push_rejection_block(messages: &mut Vec<Message>, rejected: &[(&tools::ToolCall, String)]) {
    use std::fmt::Write as _;
    if rejected.is_empty() {
        return;
    }
    let mut body = String::from("<tool_results>\n");
    for (call, reason) in rejected {
        let _ = writeln!(body, "<tool_result name=\"{}\">", call.name);
        body.push_str(reason);
        if !body.ends_with('\n') {
            body.push('\n');
        }
        body.push_str("</tool_result>\n");
    }
    body.push_str("</tool_results>");
    messages.push(Message {
        role: Role::User,
        content: body,
        images: vec![],
    });
}

/// Dispatch a multi-call batch.
///
/// Three sub-paths, decided up-front:
///   * `AICTL_CODING_PARALLEL_TOOLS_MAX=0` — parallel dispatch kill switch.
///     Run the first call only via [`handle_tool_call`]; reject the rest with
///     a "serialize" message so the model re-emits them one at a time.
///   * Batch contains any side-effect call — partial rejection: run the first
///     side-effect call (serially) via [`handle_tool_call`]; everything else
///     gets a per-call rejection so the model knows to re-emit reads on a
///     fresh turn.
///   * Pure read-only batch — parallel dispatch via [`tokio::task::JoinSet`],
///     chunked by the configured cap; per-call results join into one
///     `<tool_results>` user turn in source order.
#[allow(clippy::too_many_lines)]
async fn handle_tool_batch(
    calls: Vec<tools::ToolCall>,
    response: &str,
    auto: &mut bool,
    ui: &dyn AgentUI,
    messages: &mut Vec<Message>,
    streamed: bool,
) -> Result<DispatchResult, AictlError> {
    debug_assert!(
        calls.len() > 1,
        "handle_tool_batch should not be called with <2 tool calls"
    );

    // Reasoning that preceded the first tool tag is shown once for the
    // whole batch — same rule as the single-call path: skip when streaming
    // already forwarded it to the UI.
    if !streamed && let Some(idx) = response.find("<tool") {
        let reasoning = response[..idx].trim();
        if !reasoning.is_empty() {
            ui.show_reasoning(reasoning);
        }
    }

    let cap = config::coding_parallel_tools_max();

    // Kill switch: cap == 0. Run the first call only via the single-call
    // path (so its approval, security gate, hooks, and audit all run
    // unchanged) and queue the rest as rejections in a single
    // `<tool_results>` block.
    if cap == 0 {
        let action = handle_tool_call(&calls[0], response, auto, ui, messages, streamed).await?;
        let executed = match action {
            ToolAction::Executed => 1u32,
            ToolAction::Denied => 0,
        };
        let rejected: Vec<(&tools::ToolCall, String)> = calls[1..]
            .iter()
            .map(|c| {
                (
                    c,
                    "Parallel dispatch is disabled (AICTL_CODING_PARALLEL_TOOLS_MAX=0). \
                     Re-emit this call alone in a separate response."
                        .to_string(),
                )
            })
            .collect();
        push_rejection_block(messages, &rejected);
        return Ok(DispatchResult {
            executed,
            dispatched_names: vec![calls[0].name.clone()],
        });
    }

    // Partial rejection: any side-effect call shoves the batch onto the
    // serial path. The first side-effect dispatches via the single-call
    // handler; every other call (read-only or side-effect) is rejected.
    if let Some(se_idx) = calls.iter().position(|c| !tools::is_parallelizable(c)) {
        let action =
            handle_tool_call(&calls[se_idx], response, auto, ui, messages, streamed).await?;
        let executed = match action {
            ToolAction::Executed => 1u32,
            ToolAction::Denied => 0,
        };
        let se_name = calls[se_idx].name.clone();
        let rejected: Vec<(&tools::ToolCall, String)> = calls
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != se_idx)
            .map(|(_, c)| {
                let reason = if tools::is_parallelizable(c) {
                    format!(
                        "Rejected: batched alongside side-effect call `{se_name}`. \
                         Emit read-only calls in a separate turn so they can run in parallel."
                    )
                } else {
                    format!(
                        "Rejected: only one side-effect call per response. The host ran `{se_name}` \
                         instead; re-emit this call alone in a follow-up turn."
                    )
                };
                (c, reason)
            })
            .collect();
        push_rejection_block(messages, &rejected);
        return Ok(DispatchResult {
            executed,
            dispatched_names: vec![se_name],
        });
    }

    // Data-dependency short-circuit: a batch that pairs a context producer
    // (`fetch_geolocation` / `fetch_datetime`) with a consumer that builds a
    // network request from its output (`fetch_url` / `extract_website`) can't
    // run in parallel correctly — the consumer's input was committed as fixed
    // text before the producer ran, so concurrent dispatch races it and bakes
    // in stale or guessed context (e.g. "search the web near me" emitted
    // alongside "look up my location"). Run the producer(s) and any
    // independent reads now; defer the consumers so the model re-emits them
    // next turn with the produced value in hand. Mirrors the side-effect path:
    // dispatch a subset, reject the rest. Opt out via
    // `AICTL_CODING_PARALLEL_TOOL_DEPS=false`.
    let mut deferred_consumers: Vec<tools::ToolCall> = Vec::new();
    let calls = if config::coding_parallel_tool_deps() {
        match tools::split_context_dependency(calls) {
            tools::BatchPlan::Independent(c) => c,
            tools::BatchPlan::Dependency { run_now, deferred } => {
                deferred_consumers = deferred;
                run_now
            }
        }
    } else {
        calls
    };

    // The split can leave a single producer behind; the single-call path keeps
    // its approval/audit/redaction seams in one place, so route there and
    // still defer the consumers.
    if calls.len() == 1 {
        let action = handle_tool_call(&calls[0], response, auto, ui, messages, streamed).await?;
        let executed = match action {
            ToolAction::Executed => 1u32,
            ToolAction::Denied => 0,
        };
        push_dependency_rejections(messages, &deferred_consumers);
        return Ok(DispatchResult {
            executed,
            dispatched_names: vec![calls[0].name.clone()],
        });
    }

    // Pure read-only batch — parallel dispatch path.
    //
    // Approval gate: in v1 we ask once (on the first call) and apply the
    // decision to the whole batch. Concurrent UI prompts would be a UX
    // mess and `confirm_tool_async` is awkward to fan out anyway. The
    // user sees every batched call via `show_reasoning` before the prompt
    // so they know what they're approving.
    let approval = if *auto {
        for c in &calls {
            ui.show_auto_tool(c);
        }
        ui::ToolApproval::Allow
    } else {
        let summary = calls
            .iter()
            .map(|c| {
                let snip = c.input.lines().next().unwrap_or("").trim();
                format!("  • {} : {snip}", c.name)
            })
            .collect::<Vec<_>>()
            .join("\n");
        ui.show_reasoning(&format!(
            "running {} parallel tool calls (approval applies to all):\n{summary}",
            calls.len()
        ));
        ui.confirm_tool_async(&calls[0]).await
    };

    if approval == ui::ToolApproval::AutoAccept {
        *auto = true;
    }

    if approval != ui::ToolApproval::Allow && approval != ui::ToolApproval::AutoAccept {
        crate::audit::log_tool(&calls[0], crate::audit::Outcome::DeniedByUser);
        messages.push(Message {
            role: Role::User,
            content: "Tool call denied by user. Try a different approach or answer without tools."
                .to_string(),
            images: vec![],
        });
        return Ok(DispatchResult {
            executed: 0,
            dispatched_names: calls.into_iter().map(|c| c.name).collect(),
        });
    }

    let mut all_outcomes: Vec<ParallelCallOutcome> = Vec::with_capacity(calls.len());

    // Chunk the batch by the configured cap. Each chunk dispatches
    // concurrently; the next chunk starts only after the previous one
    // drains.
    for chunk in calls.chunks(cap) {
        let label = if chunk.len() > 1 {
            format!("running {} tools in parallel...", chunk.len())
        } else {
            "running tool...".to_string()
        };
        ui.start_spinner(&label);

        let chunk_start = all_outcomes.len();
        let mut set: tokio::task::JoinSet<ParallelCallOutcome> = tokio::task::JoinSet::new();
        for (i, call) in chunk.iter().enumerate() {
            let owned = call.clone();
            let idx = chunk_start + i;
            set.spawn(async move { run_parallel_call(idx, owned).await });
        }

        let drain = async {
            let mut collected = Vec::with_capacity(chunk.len());
            while let Some(joined) = set.join_next().await {
                collected.push(
                    joined.map_err(|e| AictlError::Other(format!("tool task panicked: {e}")))?,
                );
            }
            Ok::<_, AictlError>(collected)
        };

        let mut collected = with_esc_cancel(ui, drain).await??;
        ui.stop_spinner();

        collected.sort_by_key(|o| o.idx);
        for o in &collected {
            ui.show_tool_result(&o.result_body);
        }
        all_outcomes.extend(collected);
    }

    // Build the joined results in source order and push as a single
    // `<tool_results>` user turn.
    let body = build_tool_results_block(&all_outcomes);
    let mut combined_images: Vec<crate::ImageData> = vec![];
    let mut combined_extras: Vec<String> = vec![];
    let mut executed = 0u32;
    let mut dispatched_names: Vec<String> = Vec::with_capacity(all_outcomes.len());
    for o in &all_outcomes {
        combined_images.extend(o.images.iter().cloned());
        combined_extras.extend(o.hook_extras.iter().cloned());
        dispatched_names.push(o.call.name.clone());
        if !o.denied {
            executed += 1;
        }
    }
    messages.push(Message {
        role: Role::User,
        content: body,
        images: combined_images,
    });
    if !combined_extras.is_empty() {
        messages.push(Message {
            role: Role::User,
            content: format!(
                "<hook_context>\n{}\n</hook_context>",
                combined_extras.join("\n\n")
            ),
            images: vec![],
        });
    }

    push_dependency_rejections(messages, &deferred_consumers);

    Ok(DispatchResult {
        executed,
        dispatched_names,
    })
}

/// Push a `<tool_results>` user turn deferring the consumer calls that were
/// held back by the data-dependency split: the matching context producer ran
/// in this batch, so the model should re-emit each consumer now using that
/// result. No-op when nothing was deferred.
fn push_dependency_rejections(messages: &mut Vec<Message>, deferred: &[tools::ToolCall]) {
    if deferred.is_empty() {
        return;
    }
    let rejected: Vec<(&tools::ToolCall, String)> = deferred
        .iter()
        .map(|c| {
            (
                c,
                "Deferred: this call was batched with a context provider \
                 (fetch_geolocation / fetch_datetime) whose output it likely needs. \
                 The provider ran in this turn — re-emit this call now, folding that \
                 result into the request."
                    .to_string(),
            )
        })
        .collect();
    push_rejection_block(messages, &rejected);
}

/// Handle a single tool call: display reasoning, get approval, execute, push result.
#[allow(clippy::too_many_lines)]
async fn handle_tool_call(
    tool_call: &tools::ToolCall,
    response: &str,
    auto: &mut bool,
    ui: &dyn AgentUI,
    messages: &mut Vec<Message>,
    streamed: bool,
) -> Result<ToolAction, AictlError> {
    // Print the LLM's reasoning (text before the tool tag).
    // Skip when streaming was active for this LLM call: the same reasoning
    // text was already forwarded to the UI by stream_chunk before the
    // suspend buffer caught the `<tool name="` prefix.
    if !streamed && let Some(idx) = response.find("<tool") {
        let reasoning = response[..idx].trim();
        if !reasoning.is_empty() {
            ui.show_reasoning(reasoning);
        }
    }

    // PreToolUse hook fires before approval/execution. Hooks can:
    //   - Block the call entirely (decision: "block") — surfaces the reason
    //     to the LLM as the tool result so the model can pivot.
    //   - Pre-approve it (decision: "approve") — skip the user prompt in
    //     human-in-the-loop mode but never override an explicit `--auto`
    //     decision (auto stays auto either way).
    let pre_outcome = hooks::run_hooks(
        HookEvent::PreToolUse,
        &tool_call.name,
        tool_hook_ctx(tool_call, None),
    )
    .await;
    if let Some(reason) = pre_outcome.blocked {
        ui.show_reasoning(&format!("(tool blocked by hook: {reason})"));
        crate::audit::log_tool(
            tool_call,
            crate::audit::Outcome::DeniedByPolicy {
                reason: &format!("hook: {reason}"),
            },
        );
        messages.push(Message {
            role: Role::User,
            content: format!("<tool_result>\nTool call blocked by hook: {reason}\n</tool_result>"),
            images: vec![],
        });
        return Ok(ToolAction::Denied);
    }
    let hook_pre_approved = pre_outcome.approved.is_some();

    let approval = if *auto || hook_pre_approved {
        ui.show_auto_tool(tool_call);
        ui::ToolApproval::Allow
    } else {
        ui.confirm_tool_async(tool_call).await
    };

    if approval == ui::ToolApproval::AutoAccept {
        *auto = true;
    }

    if approval == ui::ToolApproval::Allow || approval == ui::ToolApproval::AutoAccept {
        ui.start_spinner("running tool...");
        let output = with_esc_cancel(ui, tools::execute_tool(tool_call)).await?;
        ui.stop_spinner();
        ui.show_tool_result(&output.text);

        // Coding-agent mode: track workspace mutations so the
        // `<repo_context>` cache stays fresh and the structured Review
        // hook knows whether there's anything to review. We only need
        // the *first line* of the tool body for path extraction — that
        // matches the body grammar of every mutating tool.
        if matches!(
            tool_call.name.as_str(),
            "write_file" | "edit_file" | "remove_file" | "create_directory"
        ) {
            crate::coding::invalidate_repo_context();
            if let Some(first) = tool_call.input.lines().next() {
                let path = first.trim();
                if !path.is_empty() {
                    crate::coding::record_workspace_change(std::path::Path::new(path));
                }
            }
        }

        // Seam 2: tool result about to join history. Only `Block` mode
        // needs to intercept here — for `Redact`, the outbound seam on
        // the next iteration rewrites the tool result before it leaves
        // for the provider, and the persisted history keeps the
        // original (plan §6). For `Off`, this is a no-op.
        let pol = redaction::policy();
        let mut result_content = output.text.clone();
        if matches!(pol.mode, RedactionMode::Block)
            && let RedactionResult::Blocked { matches } = redaction::redact(&output.text, &pol)
        {
            audit::log_redaction(
                RedactionDirection::Inbound,
                RedactionSource::ToolResult,
                pol.mode,
                &output.text,
                &matches,
            );
            let desc = redaction::describe_matches(&output.text, &matches);
            ui.show_reasoning(&format!(
                "(tool result blocked by redaction policy: {desc})"
            ));
            // Hand the model a stub so the turn can continue without
            // giving it anything sensitive. It keeps looping limits
            // honest — the model sees the stub and should pivot.
            result_content = format!(
                "[tool result blocked by redaction policy — {} matches detected]",
                matches.len()
            );
        }

        messages.push(Message {
            role: Role::User,
            content: format!("<tool_result>\n{result_content}\n</tool_result>"),
            images: output.images,
        });

        // PostToolUse hook fires after the result has joined history. It
        // can append guidance for the next iteration via additionalContext
        // (e.g. a formatter result, a "tests passed" note); blocking here
        // would be too late to rewind the side effect, so a `block`
        // decision is treated as additionalContext for that reason.
        let post = hooks::run_hooks(
            HookEvent::PostToolUse,
            &tool_call.name,
            tool_hook_ctx(tool_call, Some(&result_content)),
        )
        .await;
        let mut extras: Vec<String> = post.additional_context;
        if let Some(reason) = post.blocked {
            extras.push(format!("hook objection (post): {reason}"));
        }
        if !extras.is_empty() {
            messages.push(Message {
                role: Role::User,
                content: format!("<hook_context>\n{}\n</hook_context>", extras.join("\n\n")),
                images: vec![],
            });
        }

        Ok(ToolAction::Executed)
    } else {
        crate::audit::log_tool(tool_call, crate::audit::Outcome::DeniedByUser);
        messages.push(Message {
            role: Role::User,
            content: "Tool call denied by user. Try a different approach or answer without tools."
                .to_string(),
            images: vec![],
        });
        Ok(ToolAction::Denied)
    }
}

/// Run one turn of the agent loop: send `user_message`, handle tool calls,
/// return the final text answer.
///
/// `skill`, when `Some`, is injected as a transient system message at index 1
/// of the provider-bound view — right after the base system prompt — for
/// every LLM call in this turn. It is never written into `messages`, so the
/// persisted session history contains only the user message and the final
/// assistant reply; the skill body vanishes once the turn completes.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub async fn run_agent_turn(
    provider: &Provider,
    api_key: &str,
    model: &str,
    messages: &mut Vec<Message>,
    user_message: &str,
    auto: &mut bool,
    ui: &dyn AgentUI,
    streaming: bool,
    skill: Option<&Skill>,
) -> Result<TurnResult, AictlError> {
    // UserPromptSubmit hook runs before the injection guard so a hook can
    // sanitize ("rewrittenPrompt") an otherwise-blocked phrase or block
    // outright with a custom reason. Empty string match_target — only `*`
    // matchers fire for prompt events.
    let prompt_outcome = hooks::run_hooks(
        HookEvent::UserPromptSubmit,
        "",
        HookContext {
            session_id: crate::session::current_id(),
            cwd: std::env::current_dir().ok(),
            prompt: Some(user_message),
            ..Default::default()
        },
    )
    .await;
    if let Some(reason) = &prompt_outcome.blocked {
        return Err(AictlError::Other(format!(
            "prompt blocked by hook: {reason}"
        )));
    }
    let owned_prompt: Option<String> = prompt_outcome.rewritten_prompt.clone();
    let user_message: &str = owned_prompt.as_deref().unwrap_or(user_message);

    if security::policy().enabled
        && security::policy().injection_guard
        && let Err(reason) = security::detect_prompt_injection(user_message)
    {
        return Err(AictlError::Injection(reason));
    }

    // The duplicate-call guard only blocks consecutive repeats. A new
    // user message advances the conversation past whatever tool-only
    // turn ran last, so reset the slot — otherwise the first tool call
    // of this turn could still collide with the trailing call from the
    // previous turn.
    tools::clear_call_history();

    messages.push(Message {
        role: Role::User,
        content: user_message.to_string(),
        images: vec![],
    });

    // Any additional context lines the UserPromptSubmit hook returned go in
    // as a separate user turn so the model sees them as authoritative
    // out-of-band info rather than part of the user's question.
    if let Some(ctx) = prompt_outcome.merged_context() {
        messages.push(Message {
            role: Role::User,
            content: format!("<hook_context>\n{ctx}\n</hook_context>"),
            images: vec![],
        });
    }

    let mut total_usage = TokenUsage::default();
    let mut tool_calls = 0u32;
    let turn_start = std::time::Instant::now();
    #[allow(unused_assignments)]
    let mut last_input_tokens = 0u64;

    // Coding-agent only: bound the `test`-tool failure retry loop and
    // the host-driven Review hook retry loop. Both increment when the
    // host injects a synthetic failure turn; both stop the corresponding
    // loop when they exceed the user's budget.
    let mut test_retry_count: u32 = 0;
    let mut review_retry_count: u32 = 0;

    let max_iter = max_iterations();
    // `0` is the documented sentinel for unlimited — drive the loop bound
    // up to `usize::MAX` so the existing for-range still works, and skip
    // the MaxIterations error after the loop. The runaway guard is opt-out
    // by user choice in this mode.
    let unlimited_iterations = max_iter == 0;
    let iter_bound = if unlimited_iterations {
        usize::MAX
    } else {
        max_iter
    };
    for llm_calls in 1..=iter_bound {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos() as usize;
        let phrase = SPINNER_PHRASES[nanos % SPINNER_PHRASES.len()];
        ui.start_spinner(phrase);

        let raw_slice: &[Message] = messages.as_slice();

        // Merge the skill body into the base system prompt for this call
        // only. Anthropic and Gemini keep just the last System message they
        // see, so a second system block would *replace* the tool catalog
        // rather than add to it — a bug that makes the LLM hallucinate
        // around random projects instead of using tools on the real CWD.
        // Concatenation is portable across every provider and keeps the
        // persisted `messages` Vec untouched.
        let skill_buf: Option<Vec<Message>> = skill.map(|s| {
            let mut buf = raw_slice.to_vec();
            let skill_block = format!("\n\n# Skill: {}\n\n{}", s.name, s.body);
            if let Some(first) = buf.first_mut()
                && matches!(first.role, Role::System)
            {
                first.content.push_str(&skill_block);
            } else {
                buf.insert(
                    0,
                    Message {
                        role: Role::System,
                        content: skill_block.trim_start().to_string(),
                        images: vec![],
                    },
                );
            }
            buf
        });
        let base_messages: &[Message] = skill_buf.as_deref().unwrap_or(raw_slice);

        // Seam 1: redaction at the network boundary. When the policy
        // is `off`, or the provider is local and `skip_local` is on,
        // this is a zero-cost no-op (`redacted_buf` stays None and we
        // pass the original slice straight through). Only when a
        // match is actually found do we clone the slice and rewrite
        // the hit message content. The persisted `messages` Vec is
        // never mutated — redaction is a transient, per-call
        // transformation.
        let redaction_pol = redaction::policy();
        let redacted_buf = match redact_outbound(base_messages, &redaction_pol, provider) {
            Ok(buf) => buf,
            Err(err) => {
                ui.stop_spinner();
                return Err(err);
            }
        };
        let llm_messages: &[Message] = redacted_buf.as_deref().unwrap_or(base_messages);

        // Settings → Image Models override: when the turn carries any
        // image attachments AND the user pinned a separate analysis
        // provider/model, route this single call through that
        // provider/model. Recomputed every iteration in case `read_image`
        // produced new image-bearing messages mid-turn. Owned tuple lives
        // for the iteration scope; the &Provider / &str borrowed below
        // either point at the caller's args or into this local.
        let analysis_override: Option<(Provider, String, String)> =
            if messages_have_images(llm_messages) {
                resolve_image_analysis_override()
            } else {
                None
            };
        let (eff_provider, eff_model, eff_api_key): (&Provider, &str, &str) =
            match &analysis_override {
                Some((p, m, k)) => (p, m.as_str(), k.as_str()),
                None => (provider, model, api_key),
            };

        let call_start = std::time::Instant::now();
        let llm_timeout = config::llm_timeout();

        // Build a streaming sink + receiver for this iteration when streaming
        // is enabled. Each iteration gets fresh state — the suspend buffer
        // must reset every LLM call.
        let mut stream_ctx: Option<(
            tokio::sync::mpsc::UnboundedReceiver<StreamEvent>,
            Arc<Mutex<StreamState>>,
        )> = None;
        let sink: Option<TokenSink> = if streaming {
            let (s, rx, state) = build_stream_sink();
            stream_ctx = Some((rx, state));
            Some(s)
        } else {
            None
        };
        let rx_opt = stream_ctx.as_mut().map(|(rx, _)| rx);

        // Routing decision: only `Provider::AictlServer` dispatches
        // through the proxy. Picking any other provider (including
        // when `AICTL_CLIENT_HOST` happens to be set in config) goes
        // straight to the per-provider module — the user explicitly
        // chose that provider, so the request belongs there. To use
        // the proxy, switch to `--provider aictl-server` (or the
        // matching `/model` entry).
        let server_route = if matches!(eff_provider, Provider::AictlServer) {
            if let Some(pair) = config::active_server() {
                Some(pair)
            } else {
                ui.stop_spinner();
                return Err(AictlError::Other(
                    "provider 'aictl-server' selected but AICTL_CLIENT_HOST and/or AICTL_CLIENT_MASTER_KEY are not configured. Set both via /config (or --client-url and --client-master-key) and try again.".to_string(),
                ));
            }
        } else {
            None
        };

        let (result, streamed) = if let Some((server_url, master_key)) = server_route {
            run_provider_call(
                tokio::time::timeout(
                    llm_timeout,
                    llm::server_proxy::call(
                        &server_url,
                        &master_key,
                        eff_model,
                        llm_messages,
                        sink,
                    ),
                ),
                rx_opt,
                ui,
            )
            .await
        } else {
            match eff_provider {
                Provider::Openai => {
                    run_provider_call(
                        tokio::time::timeout(
                            llm_timeout,
                            llm::openai::call_openai(eff_api_key, eff_model, llm_messages, sink),
                        ),
                        rx_opt,
                        ui,
                    )
                    .await
                }
                Provider::Anthropic => {
                    run_provider_call(
                        tokio::time::timeout(
                            llm_timeout,
                            llm::anthropic::call_anthropic(
                                eff_api_key,
                                eff_model,
                                llm_messages,
                                sink,
                            ),
                        ),
                        rx_opt,
                        ui,
                    )
                    .await
                }
                Provider::Gemini => {
                    run_provider_call(
                        tokio::time::timeout(
                            llm_timeout,
                            llm::gemini::call_gemini(eff_api_key, eff_model, llm_messages, sink),
                        ),
                        rx_opt,
                        ui,
                    )
                    .await
                }
                Provider::Grok => {
                    run_provider_call(
                        tokio::time::timeout(
                            llm_timeout,
                            llm::grok::call_grok(eff_api_key, eff_model, llm_messages, sink),
                        ),
                        rx_opt,
                        ui,
                    )
                    .await
                }
                Provider::Mistral => {
                    run_provider_call(
                        tokio::time::timeout(
                            llm_timeout,
                            llm::mistral::call_mistral(eff_api_key, eff_model, llm_messages, sink),
                        ),
                        rx_opt,
                        ui,
                    )
                    .await
                }
                Provider::Deepseek => {
                    run_provider_call(
                        tokio::time::timeout(
                            llm_timeout,
                            llm::deepseek::call_deepseek(
                                eff_api_key,
                                eff_model,
                                llm_messages,
                                sink,
                            ),
                        ),
                        rx_opt,
                        ui,
                    )
                    .await
                }
                Provider::Kimi => {
                    run_provider_call(
                        tokio::time::timeout(
                            llm_timeout,
                            llm::kimi::call_kimi(eff_api_key, eff_model, llm_messages, sink),
                        ),
                        rx_opt,
                        ui,
                    )
                    .await
                }
                Provider::Zai => {
                    run_provider_call(
                        tokio::time::timeout(
                            llm_timeout,
                            llm::zai::call_zai(eff_api_key, eff_model, llm_messages, sink),
                        ),
                        rx_opt,
                        ui,
                    )
                    .await
                }
                Provider::Ollama => {
                    run_provider_call(
                        tokio::time::timeout(
                            llm_timeout,
                            llm::ollama::call_ollama(eff_model, llm_messages, sink),
                        ),
                        rx_opt,
                        ui,
                    )
                    .await
                }
                Provider::Gguf => {
                    run_provider_call(
                        tokio::time::timeout(
                            llm_timeout,
                            llm::gguf::call_gguf(eff_model, llm_messages, sink),
                        ),
                        rx_opt,
                        ui,
                    )
                    .await
                }
                Provider::Mlx => {
                    run_provider_call(
                        tokio::time::timeout(
                            llm_timeout,
                            llm::mlx::call_mlx(eff_model, llm_messages, sink),
                        ),
                        rx_opt,
                        ui,
                    )
                    .await
                }
                Provider::Mock => {
                    run_provider_call(
                        tokio::time::timeout(
                            llm_timeout,
                            llm::mock::call_mock(eff_model, llm_messages, sink),
                        ),
                        rx_opt,
                        ui,
                    )
                    .await
                }
                // Handled at the top of the routing decision above —
                // `Provider::AictlServer` always takes the server route
                // and never falls through to the per-provider match.
                Provider::AictlServer => unreachable!(
                    "Provider::AictlServer is dispatched via the explicit_server branch above"
                ),
            }
        };
        let call_elapsed = call_start.elapsed();

        if !streamed {
            ui.stop_spinner();
        }
        // Done with the streaming machinery for this iteration. The receiver
        // and state aren't needed once the call returns — the provider
        // already returned the full assembled string.
        drop(stream_ctx);

        // Peel the three layers the provider call accumulates:
        //   * outer `Interrupted` from `with_esc_cancel`
        //   * middle `tokio::time::error::Elapsed` from `tokio::time::timeout`
        //   * inner `AictlError` from the provider itself
        // Keeping them as distinct variants lets `run_and_display_turn` in
        // the REPL branch on `AictlError::Interrupted` without string matching
        // and lets future retry logic fire on `AictlError::Timeout`.
        let result = result?;
        let result = result.map_err(|_| AictlError::Timeout {
            secs: llm_timeout.as_secs(),
        })?;
        let (response, usage) = result?;

        total_usage.input_tokens += usage.input_tokens;
        total_usage.output_tokens += usage.output_tokens;
        total_usage.cache_creation_input_tokens += usage.cache_creation_input_tokens;
        total_usage.cache_read_input_tokens += usage.cache_read_input_tokens;
        last_input_tokens = usage.input_tokens;

        let token_pct = llm::pct(last_input_tokens, llm::context_limit(eff_model));
        let message_pct = llm::pct_usize(messages.len(), MAX_MESSAGES);
        let context_pct = token_pct.max(message_pct);

        messages.push(Message {
            role: Role::Assistant,
            content: response.clone(),
            images: vec![],
        });

        let calls = tools::parse_tool_calls(&response);
        let malformed_tool_call =
            calls.is_empty() && tools::looks_like_malformed_tool_call(&response);
        let is_final_answer = calls.is_empty() && !malformed_tool_call;

        // Helper closure so every exit path shows the same rule+status line.
        // We intentionally defer this past tool execution in the tool-call
        // branch so the status lands below the tool output, matching the
        // "response → rule → status" shape of the final-answer branch.
        let emit_status = |tool_calls: u32| {
            ui.show_token_usage(
                &usage,
                eff_model,
                is_final_answer,
                tool_calls,
                call_elapsed,
                context_pct,
            );
        };

        if malformed_tool_call {
            emit_status(tool_calls);
            // The model tried to emit a tool call but produced invalid XML.
            // Ask it to retry instead of surfacing raw markup as a final answer.
            ui.show_reasoning(
                "(detected a malformed <tool> tag — asking the model to retry with valid syntax)",
            );
            messages.push(Message {
                role: Role::User,
                content: "Your previous response contained a `<tool>` tag that could not be parsed. Retry using exactly this syntax: `<tool name=\"<tool_name>\">input</tool>`. If you did not intend to call a tool, reply with your final answer without any `<tool>` tags.".to_string(),
                images: vec![],
            });
            continue;
        }

        if calls.is_empty() {
            // No tool call — candidate final answer.
            //
            // Coding-agent only: before releasing, run the host-driven
            // Review hook against the changed paths. On Fail it pushes a
            // synthetic `<review_result>` user turn and continues the
            // loop so the model can re-edit; on Pass it lets the answer
            // through with a banner.
            let review_budget = config::coding_review_retries();
            let mut release_with_banner: Option<String> = None;
            if coding_agent_enabled() && !crate::coding::changed_paths().is_empty() {
                if review_retry_count >= review_budget {
                    // Exhausted — release the answer with a "failures
                    // remain" banner so the user sees the unresolved
                    // state. The list of failures already lives in the
                    // prior `<review_result>` block in messages.
                    release_with_banner = Some(format!(
                        "[review: {review_retry_count} attempt(s); failures may remain]"
                    ));
                } else {
                    ui.start_spinner("running structured review...");
                    let outcome = crate::coding::run_structured_review().await;
                    ui.stop_spinner();
                    match outcome {
                        crate::coding::ReviewOutcome::Pass { reason } => {
                            release_with_banner = Some(format!("[review: clean — {reason}]"));
                        }
                        crate::coding::ReviewOutcome::Skipped { reason } => {
                            release_with_banner = Some(format!("[review: skipped — {reason}]"));
                        }
                        crate::coding::ReviewOutcome::Fail { build, lints } => {
                            review_retry_count += 1;
                            let block = format_review_result_block(
                                build.as_ref(),
                                &lints,
                                review_retry_count,
                                review_budget,
                            );
                            messages.push(Message {
                                role: Role::User,
                                content: block,
                                images: vec![],
                            });
                            ui.show_reasoning(&format!(
                                "(structured review failed — attempt {review_retry_count} of {review_budget}; asking the model to fix)"
                            ));
                            // Do not emit status here — the loop continues
                            // and the next iteration's tool call (or final
                            // answer) will emit it.
                            continue;
                        }
                    }
                }
            }

            emit_status(tool_calls);

            // Stop hook fires once per turn after the final answer. It
            // can't influence the answer the user already sees, but it
            // can append additionalContext to the next turn or block
            // future progress with a logged reason.
            let stop = hooks::run_hooks(
                HookEvent::Stop,
                "",
                HookContext {
                    session_id: crate::session::current_id(),
                    cwd: std::env::current_dir().ok(),
                    prompt: Some(&response),
                    ..Default::default()
                },
            )
            .await;
            if let Some(reason) = &stop.blocked {
                ui.show_reasoning(&format!("(Stop hook objection: {reason})"));
            }
            if let Some(ctx) = stop.merged_context() {
                messages.push(Message {
                    role: Role::User,
                    content: format!("<hook_context>\n{ctx}\n</hook_context>"),
                    images: vec![],
                });
            }

            let final_answer = if let Some(banner) = release_with_banner {
                format!("{banner}\n\n{response}")
            } else {
                response
            };

            return Ok(TurnResult {
                answer: final_answer,
                usage: total_usage,
                #[allow(clippy::cast_possible_truncation)] // max_iter is small (default 20)
                llm_calls: llm_calls as u32,
                tool_calls,
                elapsed: turn_start.elapsed(),
                last_input_tokens,
            });
        }

        // Duplicate-call guard runs on the single-call shape only — for
        // batches each call has its own duplicate check inside
        // `tools::execute_tool`, and a batch by definition isn't a
        // back-to-back repeat of one call.
        if calls.len() == 1 && tools::is_duplicate_call(&calls[0]) {
            emit_status(tool_calls);
            if !streamed && let Some(idx) = response.find("<tool") {
                let reasoning = response[..idx].trim();
                if !reasoning.is_empty() {
                    ui.show_reasoning(reasoning);
                }
            }
            return Err(AictlError::Other(format!(
                "Agent stopped: model tried to call `{}` again with the same input — it is looping. Try a stronger model or rephrase the request.",
                calls[0].name
            )));
        }

        let dispatch = if calls.len() == 1 {
            let action =
                handle_tool_call(&calls[0], &response, auto, ui, messages, streamed).await?;
            DispatchResult {
                executed: match action {
                    ToolAction::Executed => 1,
                    ToolAction::Denied => 0,
                },
                dispatched_names: vec![calls[0].name.clone()],
            }
        } else {
            handle_tool_batch(calls, &response, auto, ui, messages, streamed).await?
        };
        tool_calls += dispatch.executed;

        // Coding-agent: host-driven test-retry loop. After a `test`
        // tool dispatch the tool stores a structured `TestSummary` on a
        // private slot; we drain it here and, on `failed > 0`, append a
        // synthetic `<test_failure>` user turn carrying the structured
        // failures so the model can plan a fix on the next iteration.
        // The retry budget caps how many times the host will keep
        // injecting the failure before letting the model produce a
        // terminal answer.
        if coding_agent_enabled()
            && dispatch.dispatched_names.iter().any(|n| n == "test")
            && let Some(summary) = tools::take_last_test_summary().await
            && summary.failed > 0
        {
            let budget = config::coding_test_retries();
            if test_retry_count < budget {
                test_retry_count += 1;
                let synthetic =
                    format_test_failure_block(&summary, test_retry_count, budget, false);
                messages.push(Message {
                    role: Role::User,
                    content: synthetic,
                    images: vec![],
                });
            } else {
                let synthetic = format_test_failure_block(&summary, test_retry_count, budget, true);
                messages.push(Message {
                    role: Role::User,
                    content: synthetic,
                    images: vec![],
                });
            }
        }

        // Status line goes at the bottom of the iteration — below the tool
        // output, above the next prompt. The counter includes the tool call
        // we just ran so the display tracks progress intuitively.
        emit_status(tool_calls);
    }

    if unlimited_iterations {
        // Reaching here means the loop counted all the way to `usize::MAX`,
        // which is unreachable on any real machine in any reasonable lifetime.
        // Surface a generic terminator so the type system stays happy.
        Err(AictlError::Other(
            "agent loop exhausted iteration counter in unlimited mode".to_string(),
        ))
    } else {
        Err(AictlError::MaxIterations {
            #[allow(clippy::cast_possible_truncation)]
            iters: max_iter as u32,
            elapsed_secs: turn_start.elapsed().as_secs_f64(),
        })
    }
}

/// Single-shot mode: run one message and print the result.
///
/// The frontend supplies the [`AgentUI`] (e.g. `PlainUI` for the CLI's
/// `--message` path) plus a `quiet` flag — the agent loop uses both to
/// decide whether streaming is worth enabling.
#[allow(clippy::too_many_arguments)] // each parameter is independent state from the CLI
pub async fn run_agent_single(
    provider: &Provider,
    api_key: &str,
    model: &str,
    user_message: &str,
    auto: bool,
    quiet: bool,
    skill: Option<&Skill>,
    ui: &dyn AgentUI,
) -> Result<(), AictlError> {
    // SessionStart for single-shot runs. There's no session id (audit logs
    // need one), but hooks still run — useful for "log every aictl
    // invocation" style observability.
    let _ = hooks::run_hooks(
        HookEvent::SessionStart,
        "",
        HookContext {
            session_id: crate::session::current_id(),
            cwd: std::env::current_dir().ok(),
            trigger: Some("single-shot"),
            ..Default::default()
        },
    )
    .await;

    let mut messages = vec![Message {
        role: Role::System,
        content: build_system_prompt(),
        images: vec![],
    }];

    let mut auto = auto;
    // Stream in single-shot non-quiet mode when stdout is a TTY and the user
    // hasn't disabled streaming. Quiet mode pipes a single final answer; a
    // non-TTY stdout (file/pager) gets nothing useful from raw deltas.
    let streaming = !quiet && stdout_is_tty() && config::streaming_enabled();
    let turn = run_agent_turn(
        provider,
        api_key,
        model,
        &mut messages,
        user_message,
        &mut auto,
        ui,
        streaming,
        skill,
    )
    .await?;
    stats::record(model, turn.llm_calls, turn.tool_calls, &turn.usage);
    ui.show_answer(&turn.answer);
    if turn.llm_calls > 1 {
        ui.show_summary(
            &turn.usage,
            model,
            turn.llm_calls,
            turn.tool_calls,
            turn.elapsed,
            0,
        );
    }

    let _ = hooks::run_hooks(
        HookEvent::SessionEnd,
        "",
        HookContext {
            session_id: crate::session::current_id(),
            cwd: std::env::current_dir().ok(),
            trigger: Some("single-shot"),
            ..Default::default()
        },
    )
    .await;
    Ok(())
}

/// Replace `messages` with a one-line summary plus its system prompt.
///
/// Asks the active provider to produce a concise summary of the
/// conversation so far, then collapses the transcript to
/// `[system, user(summary), assistant(ack)]`. Surfaces are responsible
/// for any UI scaffolding (spinner, hooks, counters, cancellation) —
/// this function is purely the LLM call plus the in-place rewrite so
/// the CLI's `/compact` slash command and the desktop's "Compact"
/// button can share the dispatch matrix.
///
/// Returns the token usage of the summary call. Returns
/// [`AictlError::Other`] when there is nothing to compact (only the
/// system prompt) or when the `aictl-server` provider is selected
/// without `AICTL_CLIENT_HOST` / `AICTL_CLIENT_MASTER_KEY` set.
pub async fn compact_messages(
    provider: &Provider,
    api_key: &str,
    model: &str,
    messages: &mut Vec<Message>,
) -> Result<llm::TokenUsage, AictlError> {
    if messages.len() <= 1 {
        return Err(AictlError::Other("nothing to compact".to_string()));
    }

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

    let llm_timeout = config::llm_timeout();
    let server_route = if matches!(provider, Provider::AictlServer) {
        Some(config::active_server().ok_or_else(|| {
            AictlError::Other(
                "provider 'aictl-server' selected but AICTL_CLIENT_HOST and/or AICTL_CLIENT_MASTER_KEY are not configured".to_string(),
            )
        })?)
    } else {
        None
    };

    let call_result = tokio::time::timeout(llm_timeout, async {
        if let Some((url, key)) = server_route.as_ref() {
            return llm::server_proxy::call(url, key, model, &summary_msgs, None).await;
        }
        match provider {
            Provider::Openai => llm::openai::call_openai(api_key, model, &summary_msgs, None).await,
            Provider::Anthropic => {
                llm::anthropic::call_anthropic(api_key, model, &summary_msgs, None).await
            }
            Provider::Gemini => llm::gemini::call_gemini(api_key, model, &summary_msgs, None).await,
            Provider::Grok => llm::grok::call_grok(api_key, model, &summary_msgs, None).await,
            Provider::Mistral => {
                llm::mistral::call_mistral(api_key, model, &summary_msgs, None).await
            }
            Provider::Deepseek => {
                llm::deepseek::call_deepseek(api_key, model, &summary_msgs, None).await
            }
            Provider::Kimi => llm::kimi::call_kimi(api_key, model, &summary_msgs, None).await,
            Provider::Zai => llm::zai::call_zai(api_key, model, &summary_msgs, None).await,
            Provider::Ollama => llm::ollama::call_ollama(model, &summary_msgs, None).await,
            Provider::Gguf => llm::gguf::call_gguf(model, &summary_msgs, None).await,
            Provider::Mlx => llm::mlx::call_mlx(model, &summary_msgs, None).await,
            Provider::Mock => llm::mock::call_mock(model, &summary_msgs, None).await,
            Provider::AictlServer => {
                unreachable!("server_route covers Provider::AictlServer")
            }
        }
    })
    .await;

    let (summary, usage) = match call_result {
        Ok(Ok(pair)) => pair,
        Ok(Err(e)) => return Err(e),
        Err(_) => {
            return Err(AictlError::Timeout {
                secs: llm_timeout.as_secs(),
            });
        }
    };

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
        content: "Understood. I have the context from our previous conversation. How can I help you next?".to_string(),
        images: vec![],
    });
    Ok(usage)
}
