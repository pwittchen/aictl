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
#[cfg(not(test))]
use std::sync::atomic::{AtomicBool, Ordering};

use clap::ValueEnum;

use crate::commands::MemoryMode;
use crate::config::{
    self, MAX_MESSAGES, SHORT_TERM_MEMORY_WINDOW, SPINNER_PHRASES, SYSTEM_PROMPT, load_prompt_file,
    max_iterations,
};
use crate::message::{Message, Role};
use crate::ui::{self, AgentUI, PlainUI};
use crate::{agents, llm, security, stats, tools};
use llm::{TokenSink, TokenUsage, stream::StreamState};

/// Cached "is stdout a TTY?" check. Computed once at startup to avoid repeated
/// syscalls on every agent turn. Streaming auto-disables when stdout is being
/// piped to a file/pager regardless of `AICTL_STREAMING`, since interleaved
/// progressive output is rarely useful in that case.
static STDOUT_IS_TTY: OnceLock<bool> = OnceLock::new();

pub(crate) fn stdout_is_tty() -> bool {
    *STDOUT_IS_TTY.get_or_init(|| std::io::stdout().is_terminal())
}

/// Result of a single agent turn.
#[cfg_attr(test, derive(Debug))]
pub(crate) struct TurnResult {
    pub answer: String,
    pub usage: TokenUsage,
    pub llm_calls: u32,
    pub tool_calls: u32,
    pub elapsed: std::time::Duration,
    pub last_input_tokens: u64,
}

#[derive(Debug, Clone, ValueEnum)]
pub(crate) enum Provider {
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
    /// Scripted provider used by the integration tests. Hidden from the CLI
    /// via `#[value(skip)]` so users can never select it; the actual dispatch
    /// in `run_agent_turn` is cfg-gated so non-test builds can't route here.
    #[value(skip)]
    #[allow(dead_code)]
    Mock,
}

// --- Esc key interrupt support ---

/// Error type for user-initiated interruption via Esc key.
#[derive(Debug)]
pub(crate) struct Interrupted;

impl std::fmt::Display for Interrupted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "interrupted")
    }
}

impl std::error::Error for Interrupted {}

/// Wrap a future so that pressing Esc cancels it.
///
/// Enables crossterm raw mode for the duration of the call and spawns
/// a blocking listener that polls for Esc key events. Returns
/// `Ok(value)` on normal completion or `Err(Interrupted)` if the user
/// pressed Esc.
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
pub(crate) async fn with_esc_cancel<F: std::future::Future>(
    future: F,
) -> Result<F::Output, Interrupted> {
    Ok(future.await)
}

#[cfg(not(test))]
pub(crate) async fn with_esc_cancel<F: std::future::Future>(
    future: F,
) -> Result<F::Output, Interrupted> {
    use crossterm::event::{self, Event, KeyCode, KeyEventKind};
    use tokio::sync::oneshot;

    if !stdout_is_tty() {
        return Ok(future.await);
    }

    let (tx, rx) = oneshot::channel::<()>();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_clone = stop.clone();

    let listener = tokio::task::spawn_blocking(move || {
        let _ = crossterm::terminal::enable_raw_mode();
        let mut tx = Some(tx);
        loop {
            if stop_clone.load(Ordering::Relaxed) {
                break;
            }
            if event::poll(std::time::Duration::from_millis(50)).unwrap_or(false)
                && let Ok(Event::Key(key)) = event::read()
                && key.code == KeyCode::Esc
                && key.kind == KeyEventKind::Press
            {
                if let Some(tx) = tx.take() {
                    let _ = tx.send(());
                }
                break;
            }
        }
        let _ = crossterm::terminal::disable_raw_mode();
    });

    let result = tokio::select! {
        value = future => Ok(value),
        _ = rx => Err(Interrupted),
    };

    stop.store(true, Ordering::Relaxed);
    let _ = listener.await;

    result
}

/// Build the full system prompt, appending the project prompt file and loaded agent if present.
pub(crate) fn build_system_prompt() -> String {
    let mut prompt = SYSTEM_PROMPT.to_string();
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
        match with_esc_cancel(run_with_streaming(llm_future, rx, ui)).await {
            Ok((value, streamed)) => (Ok(value), streamed),
            Err(e) => (Err(e), false),
        }
    } else {
        (with_esc_cancel(llm_future).await, false)
    }
}

// --- Agent loop ---

enum ToolAction {
    Executed,
    Denied,
}

/// Handle a single tool call: display reasoning, get approval, execute, push result.
async fn handle_tool_call(
    tool_call: &tools::ToolCall,
    response: &str,
    auto: &mut bool,
    ui: &dyn AgentUI,
    messages: &mut Vec<Message>,
    streamed: bool,
) -> Result<ToolAction, Interrupted> {
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

    let approval = if *auto {
        ui.show_auto_tool(tool_call);
        ui::ToolApproval::Allow
    } else {
        ui.confirm_tool(tool_call)
    };

    if approval == ui::ToolApproval::AutoAccept {
        *auto = true;
    }

    if approval == ui::ToolApproval::Allow || approval == ui::ToolApproval::AutoAccept {
        ui.start_spinner("running tool...");
        let output = with_esc_cancel(tools::execute_tool(tool_call)).await?;
        ui.stop_spinner();
        ui.show_tool_result(&output.text);
        messages.push(Message {
            role: Role::User,
            content: format!("<tool_result>\n{}\n</tool_result>", output.text),
            images: output.images,
        });
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

/// Build a windowed view of messages for short-term memory mode.
/// Keeps the system prompt (first message) and the most recent `window` messages.
fn windowed_messages(messages: &[Message], window: usize) -> Vec<Message> {
    if messages.len() <= 1 + window {
        return messages.to_vec();
    }
    let mut result = vec![messages[0].clone()];
    result.extend_from_slice(&messages[messages.len() - window..]);
    result
}

/// Run one turn of the agent loop: send `user_message`, handle tool calls,
/// return the final text answer.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(crate) async fn run_agent_turn(
    provider: &Provider,
    api_key: &str,
    model: &str,
    messages: &mut Vec<Message>,
    user_message: &str,
    auto: &mut bool,
    ui: &dyn AgentUI,
    memory: MemoryMode,
    streaming: bool,
) -> Result<TurnResult, Box<dyn std::error::Error>> {
    if security::policy().enabled
        && security::policy().injection_guard
        && let Err(reason) = security::detect_prompt_injection(user_message)
    {
        return Err(format!("blocked: possible prompt injection ({reason})").into());
    }

    messages.push(Message {
        role: Role::User,
        content: user_message.to_string(),
        images: vec![],
    });

    let mut total_usage = TokenUsage::default();
    let mut tool_calls = 0u32;
    let turn_start = std::time::Instant::now();
    #[allow(unused_assignments)]
    let mut last_input_tokens = 0u64;

    let max_iter = max_iterations();
    for llm_calls in 1..=max_iter {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos() as usize;
        let phrase = SPINNER_PHRASES[nanos % SPINNER_PHRASES.len()];
        ui.start_spinner(phrase);

        // In LongTerm mode we pass the history directly as a slice, avoiding a
        // full clone of every message on every loop iteration. ShortTerm mode
        // still materializes a windowed Vec, but `short_term_buf` owns it only
        // when that branch runs.
        let short_term_buf;
        let llm_messages: &[Message] = match memory {
            MemoryMode::LongTerm => messages.as_slice(),
            MemoryMode::ShortTerm => {
                short_term_buf = windowed_messages(messages, SHORT_TERM_MEMORY_WINDOW);
                &short_term_buf
            }
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

        let (result, streamed) = match provider {
            Provider::Openai => {
                run_provider_call(
                    tokio::time::timeout(
                        llm_timeout,
                        llm::openai::call_openai(api_key, model, llm_messages, sink),
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
                        llm::anthropic::call_anthropic(api_key, model, llm_messages, sink),
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
                        llm::gemini::call_gemini(api_key, model, llm_messages, sink),
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
                        llm::grok::call_grok(api_key, model, llm_messages, sink),
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
                        llm::mistral::call_mistral(api_key, model, llm_messages, sink),
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
                        llm::deepseek::call_deepseek(api_key, model, llm_messages, sink),
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
                        llm::kimi::call_kimi(api_key, model, llm_messages, sink),
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
                        llm::zai::call_zai(api_key, model, llm_messages, sink),
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
                        llm::ollama::call_ollama(model, llm_messages, sink),
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
                        llm::gguf::call_gguf(model, llm_messages, sink),
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
                        llm::mlx::call_mlx(model, llm_messages, sink),
                    ),
                    rx_opt,
                    ui,
                )
                .await
            }
            Provider::Mock => {
                #[cfg(test)]
                {
                    run_provider_call(
                        tokio::time::timeout(
                            llm_timeout,
                            llm::mock::call_mock(model, llm_messages, sink),
                        ),
                        rx_opt,
                        ui,
                    )
                    .await
                }
                #[cfg(not(test))]
                {
                    let _ = (llm_timeout, llm_messages, sink, rx_opt);
                    unreachable!("Provider::Mock is test-only and never selected at runtime")
                }
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

        let result = result.map_err(|e| -> Box<dyn std::error::Error> { Box::new(e) })?;
        let result = result.map_err(|_| -> Box<dyn std::error::Error> {
            format!(
                "LLM call exceeded the {}s timeout. Increase AICTL_LLM_TIMEOUT in ~/.aictl/config (seconds, 0 disables) if this is expected on your hardware.",
                llm_timeout.as_secs()
            )
            .into()
        })?;
        let (response, usage) = result?;

        total_usage.input_tokens += usage.input_tokens;
        total_usage.output_tokens += usage.output_tokens;
        total_usage.cache_creation_input_tokens += usage.cache_creation_input_tokens;
        total_usage.cache_read_input_tokens += usage.cache_read_input_tokens;
        last_input_tokens = usage.input_tokens;

        let token_pct = llm::pct(last_input_tokens, llm::context_limit(model));
        let message_pct = llm::pct_usize(messages.len(), MAX_MESSAGES);
        let context_pct = token_pct.max(message_pct);

        messages.push(Message {
            role: Role::Assistant,
            content: response.clone(),
            images: vec![],
        });

        let tool_call = tools::parse_tool_call(&response);
        let malformed_tool_call =
            tool_call.is_none() && tools::looks_like_malformed_tool_call(&response);
        let is_final_answer = tool_call.is_none() && !malformed_tool_call;

        // Helper closure so every exit path shows the same rule+status line.
        // We intentionally defer this past tool execution in the tool-call
        // branch so the status lands below the tool output, matching the
        // "response → rule → status" shape of the final-answer branch.
        let emit_status = |tool_calls: u32| {
            ui.show_token_usage(
                &usage,
                model,
                is_final_answer,
                tool_calls,
                call_elapsed,
                context_pct,
                &memory.to_string(),
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

        let Some(tool_call) = tool_call else {
            // No tool call — this is the final answer
            emit_status(tool_calls);
            return Ok(TurnResult {
                answer: response,
                usage: total_usage,
                #[allow(clippy::cast_possible_truncation)] // max_iter is small (default 20)
                llm_calls: llm_calls as u32,
                tool_calls,
                elapsed: turn_start.elapsed(),
                last_input_tokens,
            });
        };

        // Abort the turn if the model is trying to repeat a tool call it
        // has already made this session. `tools::execute_tool` would reject
        // the duplicate anyway, but continuing the loop just gives the
        // model another chance to emit the same call.
        if tools::is_duplicate_call(&tool_call) {
            emit_status(tool_calls);
            // Only print the leading reasoning when streaming wasn't already
            // showing it. With streaming on, the reasoning is on screen
            // already (the suspend buffer flushed it before catching <tool).
            if !streamed && let Some(idx) = response.find("<tool") {
                let reasoning = response[..idx].trim();
                if !reasoning.is_empty() {
                    ui.show_reasoning(reasoning);
                }
            }
            return Err(format!(
                "Agent stopped: model tried to call `{}` again with the same input — it is looping. Try a stronger model or rephrase the request.",
                tool_call.name
            )
            .into());
        }

        match handle_tool_call(&tool_call, &response, auto, ui, messages, streamed).await {
            Ok(ToolAction::Executed) => {
                tool_calls += 1;
            }
            Ok(ToolAction::Denied) => {}
            Err(e) => return Err(Box::new(e)),
        }
        // Status line goes at the bottom of the iteration — below the tool
        // output, above the next prompt. The counter includes the tool call
        // we just ran so the display tracks progress intuitively.
        emit_status(tool_calls);
    }

    Err(format!(
        "Agent loop reached maximum iterations ({max_iter}) after {:.1}s",
        turn_start.elapsed().as_secs_f64()
    )
    .into())
}

/// Single-shot mode: run one message and print the result.
pub(crate) async fn run_agent_single(
    provider: &Provider,
    api_key: &str,
    model: &str,
    user_message: &str,
    auto: bool,
    quiet: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    use std::cell::Cell;

    let mut messages = vec![Message {
        role: Role::System,
        content: build_system_prompt(),
        images: vec![],
    }];

    let mut auto = auto;
    let ui = PlainUI {
        quiet,
        streamed: Cell::new(false),
    };
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
        &ui,
        MemoryMode::LongTerm,
        streaming,
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
    Ok(())
}
