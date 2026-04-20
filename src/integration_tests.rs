//! End-to-end tests for [`crate::run::run_agent_turn`] driven by a scripted
//! mock LLM. Covers the full loop: message accumulation, tool-call parsing,
//! dispatch, duplicate detection, malformed-tag retry, denial handling, auto
//! mode, memory-window windowing, and iteration cap.
//!
//! The mock lives at [`crate::llm::mock`]; its [`MockGuard`] serializes these
//! tests across parallel `cargo test` threads and resets shared state
//! (response queue, call log, `tools::CALL_HISTORY`) on each construction.

use std::cell::Cell;

use regex::Regex;

use crate::commands::MemoryMode;
use crate::llm::TokenUsage;
use crate::llm::mock::MockGuard;
use crate::message::{Message, Role};
use crate::run::{Provider, redact_outbound, run_agent_turn};
use crate::security::redaction::{DetectorKind, RedactionMode, RedactionPolicy, RedactionResult};
use crate::tools::ToolCall;
use crate::ui::{AgentUI, PlainUI, ToolApproval};

// --- Helpers ---

fn make_system_messages() -> Vec<Message> {
    vec![Message {
        role: Role::System,
        content: "You are a test assistant.".to_string(),
        images: vec![],
    }]
}

fn quiet_ui() -> PlainUI {
    PlainUI {
        quiet: true,
        streamed: Cell::new(false),
    }
}

/// Test UI that records tool-call confirmation decisions from a fixed
/// sequence. Used by the denial tests where the real `PlainUI::confirm_tool`
/// would block on stdin.
struct ScriptedUI {
    approvals: std::cell::RefCell<Vec<ToolApproval>>,
}

impl ScriptedUI {
    fn new(approvals: Vec<ToolApproval>) -> Self {
        Self {
            approvals: std::cell::RefCell::new(approvals),
        }
    }
}

impl AgentUI for ScriptedUI {
    fn start_spinner(&self, _msg: &str) {}
    fn stop_spinner(&self) {}
    fn show_reasoning(&self, _text: &str) {}
    fn show_auto_tool(&self, _tool_call: &ToolCall) {}
    fn show_tool_result(&self, _result: &str) {}
    fn confirm_tool(&self, _tool_call: &ToolCall) -> ToolApproval {
        // Pop from the scripted approvals; default to Deny if we run out so
        // the test fails fast instead of hanging on an unexpected call.
        self.approvals
            .borrow_mut()
            .drain(..1)
            .next()
            .unwrap_or(ToolApproval::Deny)
    }
    fn show_answer(&self, _text: &str) {}
    fn show_error(&self, _text: &str) {}
    fn show_token_usage(
        &self,
        _usage: &TokenUsage,
        _model: &str,
        _final_answer: bool,
        _tool_calls: u32,
        _elapsed: std::time::Duration,
        _context_pct: u8,
        _memory: &str,
    ) {
    }
    fn show_summary(
        &self,
        _usage: &TokenUsage,
        _model: &str,
        _llm_calls: u32,
        _tool_calls: u32,
        _elapsed: std::time::Duration,
        _context_pct: u8,
    ) {
    }
}

// --- Simple final-answer path ---

#[tokio::test]
async fn plain_final_answer_no_tool_calls() {
    let guard = MockGuard::new();
    guard.push_response("The answer is 42.");

    let mut messages = make_system_messages();
    let mut auto = false;
    let ui = quiet_ui();

    let turn = run_agent_turn(
        &Provider::Mock,
        "",
        "mock-model",
        &mut messages,
        "what is the answer?",
        &mut auto,
        &ui,
        MemoryMode::LongTerm,
        false,
    )
    .await
    .expect("turn should succeed");

    assert_eq!(turn.answer, "The answer is 42.");
    assert_eq!(turn.llm_calls, 1);
    assert_eq!(turn.tool_calls, 0);
    // history = [system, user, assistant]
    assert_eq!(messages.len(), 3);
    assert!(matches!(messages[0].role, Role::System));
    assert!(matches!(messages[1].role, Role::User));
    assert_eq!(messages[1].content, "what is the answer?");
    assert!(matches!(messages[2].role, Role::Assistant));
    assert_eq!(messages[2].content, "The answer is 42.");
}

#[tokio::test]
async fn token_usage_accumulates_across_turn() {
    let guard = MockGuard::new();
    // Use `calculate` with a tagged, test-unique input so the call doesn't
    // collide with pre-existing tool tests' call-history entries even though
    // both share the global `tools::CALL_HISTORY`.
    guard.push_response_with_usage(
        "<tool name=\"calculate\">42 + 1001</tool>",
        TokenUsage {
            input_tokens: 100,
            output_tokens: 50,
            ..TokenUsage::default()
        },
    );
    guard.push_response_with_usage(
        "Done.",
        TokenUsage {
            input_tokens: 200,
            output_tokens: 30,
            ..TokenUsage::default()
        },
    );

    let mut messages = make_system_messages();
    let mut auto = true;
    let ui = quiet_ui();

    let turn = run_agent_turn(
        &Provider::Mock,
        "",
        "mock-model",
        &mut messages,
        "time?",
        &mut auto,
        &ui,
        MemoryMode::LongTerm,
        false,
    )
    .await
    .unwrap();

    assert_eq!(turn.usage.input_tokens, 300);
    assert_eq!(turn.usage.output_tokens, 80);
    assert_eq!(turn.last_input_tokens, 200);
    assert_eq!(turn.llm_calls, 2);
    assert_eq!(turn.tool_calls, 1);
}

// --- Tool-call dispatch ---

#[tokio::test]
async fn tool_call_executes_and_result_is_injected() {
    let guard = MockGuard::new();
    // `calculate` with a test-unique input sidesteps call-history collisions
    // with pre-existing tool tests.
    guard.push_response("Let me compute.\n<tool name=\"calculate\">111 + 222</tool>");
    guard.push_response("Sum recorded.");

    let mut messages = make_system_messages();
    let mut auto = true;
    let ui = quiet_ui();

    let turn = run_agent_turn(
        &Provider::Mock,
        "",
        "mock-model",
        &mut messages,
        "add it",
        &mut auto,
        &ui,
        MemoryMode::LongTerm,
        false,
    )
    .await
    .unwrap();

    assert_eq!(turn.answer, "Sum recorded.");
    assert_eq!(turn.llm_calls, 2);
    assert_eq!(turn.tool_calls, 1);

    // The mock's second call should have received the tool result in its
    // messages slice — system, user, assistant (tool call), user (tool_result),
    // then we ask for the next turn.
    let calls = guard.calls();
    assert_eq!(calls.len(), 2);
    let second = &calls[1];
    // Find the tool_result message.
    let has_result = second
        .iter()
        .any(|m| matches!(m.role, Role::User) && m.content.starts_with("<tool_result>"));
    assert!(
        has_result,
        "second LLM call should see the <tool_result> message in history"
    );
}

#[tokio::test]
async fn tool_call_denied_by_user_continues_loop_with_denial_message() {
    let guard = MockGuard::new();
    // First turn: model emits a (test-unique) tool call; user denies.
    guard.push_response("Let me compute.\n<tool name=\"calculate\">333 + 444</tool>");
    // Second turn: model gives a direct answer.
    guard.push_response("Alright, answering without tools.");

    let mut messages = make_system_messages();
    let mut auto = false; // force interactive approval path
    let ui = ScriptedUI::new(vec![ToolApproval::Deny]);

    let turn = run_agent_turn(
        &Provider::Mock,
        "",
        "mock-model",
        &mut messages,
        "hi",
        &mut auto,
        &ui,
        MemoryMode::LongTerm,
        false,
    )
    .await
    .unwrap();

    assert_eq!(turn.answer, "Alright, answering without tools.");
    assert_eq!(turn.llm_calls, 2);
    // Denied calls don't count as executed.
    assert_eq!(turn.tool_calls, 0);

    // The denial message must appear in the conversation history before the
    // second LLM call.
    let calls = guard.calls();
    let second_call = &calls[1];
    let denial_found = second_call
        .iter()
        .any(|m| matches!(m.role, Role::User) && m.content.contains("Tool call denied by user"));
    assert!(
        denial_found,
        "denial message should be injected into history"
    );
}

#[tokio::test]
async fn auto_accept_flips_auto_mode_for_rest_of_turn() {
    let guard = MockGuard::new();
    // Two tool calls, then a final answer. First approval is AutoAccept.
    // Test-unique `calculate` inputs avoid call-history collisions.
    guard.push_response("<tool name=\"calculate\">555 + 666</tool>");
    guard.push_response("<tool name=\"calculate\">777 + 888</tool>");
    guard.push_response("Done.");

    let mut messages = make_system_messages();
    let mut auto = false;
    // Only the first confirmation is consulted; subsequent tool calls
    // bypass confirm_tool entirely once auto flips on.
    let ui = ScriptedUI::new(vec![ToolApproval::AutoAccept]);

    let turn = run_agent_turn(
        &Provider::Mock,
        "",
        "mock-model",
        &mut messages,
        "hi",
        &mut auto,
        &ui,
        MemoryMode::LongTerm,
        false,
    )
    .await
    .unwrap();

    assert_eq!(turn.answer, "Done.");
    assert_eq!(turn.tool_calls, 2);
    assert!(auto, "AutoAccept must flip the auto flag on");
}

// --- Tool-call loop control ---

#[tokio::test]
async fn duplicate_tool_call_aborts_turn_with_clear_error() {
    let guard = MockGuard::new();
    // Same tool + same normalized input emitted twice — second one should
    // trip `is_duplicate_call` and abort.
    guard.push_response("<tool name=\"calculate\">1 + 1</tool>");
    guard.push_response("<tool name=\"calculate\">1 + 1</tool>");

    let mut messages = make_system_messages();
    let mut auto = true;
    let ui = quiet_ui();

    let err = run_agent_turn(
        &Provider::Mock,
        "",
        "mock-model",
        &mut messages,
        "math please",
        &mut auto,
        &ui,
        MemoryMode::LongTerm,
        false,
    )
    .await
    .expect_err("duplicate tool call should abort the turn");

    let msg = err.to_string();
    assert!(
        msg.contains("looping") && msg.contains("calculate"),
        "unexpected error message: {msg}"
    );
}

#[tokio::test]
async fn malformed_tool_call_triggers_retry_prompt_then_recovers() {
    let guard = MockGuard::new();
    // First response has `<tool name=` but no closing quote / tag — loud
    // malformed signal, should not be surfaced as a final answer.
    guard.push_response("I think I should <tool name=\"read_file");
    // After the retry prompt is injected, the model returns a proper answer.
    guard.push_response("Sorry, here is the real answer.");

    let mut messages = make_system_messages();
    let mut auto = true;
    let ui = quiet_ui();

    let turn = run_agent_turn(
        &Provider::Mock,
        "",
        "mock-model",
        &mut messages,
        "hi",
        &mut auto,
        &ui,
        MemoryMode::LongTerm,
        false,
    )
    .await
    .unwrap();

    assert_eq!(turn.answer, "Sorry, here is the real answer.");
    assert_eq!(turn.llm_calls, 2);
    assert_eq!(turn.tool_calls, 0);

    // The second LLM call should see the retry instruction in history.
    let calls = guard.calls();
    let second = &calls[1];
    let retry_msg_present = second
        .iter()
        .any(|m| matches!(m.role, Role::User) && m.content.contains("could not be parsed"));
    assert!(
        retry_msg_present,
        "retry instruction should be injected before the second LLM call"
    );
}

#[tokio::test]
async fn unknown_tool_returns_message_and_keeps_looping() {
    let guard = MockGuard::new();
    guard.push_response("<tool name=\"nonexistent_tool\">nope</tool>");
    guard.push_response("Moving on.");

    let mut messages = make_system_messages();
    let mut auto = true;
    let ui = quiet_ui();

    let turn = run_agent_turn(
        &Provider::Mock,
        "",
        "mock-model",
        &mut messages,
        "try a weird tool",
        &mut auto,
        &ui,
        MemoryMode::LongTerm,
        false,
    )
    .await
    .unwrap();

    assert_eq!(turn.answer, "Moving on.");
    assert_eq!(turn.tool_calls, 1);

    // The tool_result injected into the second call should contain the
    // "Unknown tool" message from execute_tool's dispatch.
    let calls = guard.calls();
    let tool_result = calls[1]
        .iter()
        .find(|m| matches!(m.role, Role::User) && m.content.starts_with("<tool_result>"))
        .expect("tool_result message should be present");
    assert!(
        tool_result
            .content
            .contains("Unknown tool: nonexistent_tool")
    );
}

// --- Memory modes ---

#[tokio::test]
async fn short_term_memory_windows_the_history() {
    use crate::config::SHORT_TERM_MEMORY_WINDOW;

    let guard = MockGuard::new();
    // Push enough tool calls that the total history (system + user + N *
    // (assistant + tool_result)) exceeds `1 + window`, so the windowing
    // branch actually trims messages. Each input must be unique to dodge
    // the duplicate-call guard. We stay well under the 20-iteration cap.
    let tool_rounds = SHORT_TERM_MEMORY_WINDOW / 2 + 2;
    for i in 0..tool_rounds {
        guard.push_response(format!("<tool name=\"calculate\">{i} + 1</tool>"));
    }
    guard.push_response("Done.");

    let mut messages = make_system_messages();
    let mut auto = true;
    let ui = quiet_ui();

    let turn = run_agent_turn(
        &Provider::Mock,
        "",
        "mock-model",
        &mut messages,
        "run many tools",
        &mut auto,
        &ui,
        MemoryMode::ShortTerm,
        false,
    )
    .await
    .unwrap();

    assert_eq!(turn.answer, "Done.");

    // The last LLM call should see a windowed view: system message + the
    // tail of the conversation, capped at `1 + SHORT_TERM_MEMORY_WINDOW`.
    let calls = guard.calls();
    let last = calls.last().expect("at least one call");
    assert!(
        last.len() <= 1 + SHORT_TERM_MEMORY_WINDOW,
        "short-term view too large: {} messages (limit {})",
        last.len(),
        1 + SHORT_TERM_MEMORY_WINDOW
    );
    assert!(matches!(last[0].role, Role::System));
    // Sanity: the full `messages` vec at turn end must be larger than the
    // windowed view — otherwise the windowing branch wasn't actually
    // exercised and the test would pass trivially.
    assert!(
        messages.len() > 1 + SHORT_TERM_MEMORY_WINDOW,
        "full history ({}) must exceed window ({}) to exercise windowing",
        messages.len(),
        1 + SHORT_TERM_MEMORY_WINDOW
    );
}

#[tokio::test]
async fn long_term_memory_keeps_full_history() {
    let guard = MockGuard::new();
    guard.push_response("<tool name=\"calculate\">1 + 1</tool>");
    guard.push_response("<tool name=\"calculate\">2 + 2</tool>");
    guard.push_response("Done.");

    let mut messages = make_system_messages();
    let mut auto = true;
    let ui = quiet_ui();

    run_agent_turn(
        &Provider::Mock,
        "",
        "mock-model",
        &mut messages,
        "run two tools",
        &mut auto,
        &ui,
        MemoryMode::LongTerm,
        false,
    )
    .await
    .unwrap();

    // Third LLM call receives the full history: system + user + (assistant +
    // tool_result) * 2 = 6 messages.
    let calls = guard.calls();
    let last = calls.last().unwrap();
    assert_eq!(
        last.len(),
        6,
        "long-term history should not be windowed (got {} messages)",
        last.len()
    );
}

// --- Iteration cap ---

#[tokio::test]
async fn max_iterations_cap_terminates_the_loop() {
    use crate::config::DEFAULT_MAX_ITERATIONS;

    let guard = MockGuard::new();
    // Always return a (unique) tool call — never a final answer. The default
    // cap must terminate the loop with an error. Each input is unique to
    // dodge the duplicate-call guard. We intentionally avoid `config_set` so
    // the test doesn't pollute the user's real ~/.aictl/config on disk.
    for i in 0..DEFAULT_MAX_ITERATIONS + 5 {
        guard.push_response(format!("<tool name=\"calculate\">{i} * 2</tool>"));
    }

    let mut messages = make_system_messages();
    let mut auto = true;
    let ui = quiet_ui();

    let result = run_agent_turn(
        &Provider::Mock,
        "",
        "mock-model",
        &mut messages,
        "loop forever",
        &mut auto,
        &ui,
        MemoryMode::LongTerm,
        false,
    )
    .await;

    let err = result.expect_err("runaway loop should hit the iteration cap");
    let msg = err.to_string();
    assert!(
        msg.contains("maximum iterations"),
        "unexpected error: {msg}"
    );
    // Exactly `DEFAULT_MAX_ITERATIONS` LLM calls should have happened.
    assert_eq!(guard.call_count(), DEFAULT_MAX_ITERATIONS);
}

// --- Streaming path ---

#[tokio::test]
async fn streaming_flag_is_wired_through_to_the_mock() {
    // The mock's `call_mock` emits the scripted response as a single chunk
    // to the sink when streaming is on. This test just verifies that
    // enabling `streaming=true` does not break the turn — the provider
    // call path through `run_with_streaming` should still resolve cleanly.
    let guard = MockGuard::new();
    guard.push_response("Hello streaming world.");

    let mut messages = make_system_messages();
    let mut auto = true;
    // Use `ScriptedUI` instead of `PlainUI` so the streamed chunk isn't
    // written to stdout — otherwise the delta would land inline with the
    // `cargo test` harness output (`...ok` concatenated on the same line).
    let ui = ScriptedUI::new(vec![]);

    let turn = run_agent_turn(
        &Provider::Mock,
        "",
        "mock-model",
        &mut messages,
        "hi",
        &mut auto,
        &ui,
        MemoryMode::LongTerm,
        true, // streaming on
    )
    .await
    .unwrap();

    assert_eq!(turn.answer, "Hello streaming world.");
}

// --- Provider errors ---

#[tokio::test]
async fn provider_error_propagates_out_of_the_turn() {
    let guard = MockGuard::new();
    guard.push_error("fake upstream error");

    let mut messages = make_system_messages();
    let mut auto = true;
    let ui = quiet_ui();

    let err = run_agent_turn(
        &Provider::Mock,
        "",
        "mock-model",
        &mut messages,
        "hi",
        &mut auto,
        &ui,
        MemoryMode::LongTerm,
        false,
    )
    .await
    .expect_err("provider error should surface");

    assert!(err.to_string().contains("fake upstream error"));
}

// --- Redaction: outbound seam (Seam 1) integration ---

fn build_redact_policy(mode: RedactionMode) -> RedactionPolicy {
    RedactionPolicy {
        mode,
        skip_local: true,
        enabled_detectors: vec![],
        extra_patterns: vec![],
        allowlist: vec![],
        ner_requested: false,
    }
}

fn sample_history(user_text: &str) -> Vec<Message> {
    vec![
        Message {
            role: Role::System,
            content: "You are a test assistant.".to_string(),
            images: vec![],
        },
        Message {
            role: Role::User,
            content: user_text.to_string(),
            images: vec![],
        },
    ]
}

#[test]
fn outbound_redact_clones_only_affected_messages() {
    // The canonical assertion: the persisted history the caller owns
    // is never mutated. `redact_outbound` returns a new owned slice
    // only when redaction was actually needed.
    let pol = build_redact_policy(RedactionMode::Redact);
    let messages =
        sample_history("my key is sk-proj-aaaaaaaaaaaaaaaaaaaaaaaabbbbbbbbbbbbbbbbbbbb, help");
    let rewritten = redact_outbound(&messages, &pol, &Provider::Mock)
        .expect("redact should succeed")
        .expect("redact should clone when there is a match");
    assert!(rewritten[1].content.contains("[REDACTED:API_KEY]"));
    assert!(!rewritten[1].content.contains("sk-proj-aaaaa"));
    // Caller's slice is untouched.
    assert!(messages[1].content.contains("sk-proj-aaaaa"));
}

#[test]
fn outbound_clean_history_returns_none() {
    let pol = build_redact_policy(RedactionMode::Redact);
    let messages = sample_history("hello there, nothing sensitive here");
    let rewritten = redact_outbound(&messages, &pol, &Provider::Mock).unwrap();
    assert!(rewritten.is_none(), "no matches = no clone");
}

#[test]
fn outbound_off_mode_is_zero_cost_noop() {
    let pol = build_redact_policy(RedactionMode::Off);
    let messages = sample_history("sk-proj-aaaaaaaaaaaaaaaaaaaaaaaabbbbbbbbbbbbbbbbbbbb");
    let rewritten = redact_outbound(&messages, &pol, &Provider::Mock).unwrap();
    assert!(rewritten.is_none(), "off mode never clones");
}

#[test]
fn outbound_block_returns_err_with_kind_label() {
    let pol = build_redact_policy(RedactionMode::Block);
    let messages =
        sample_history("pls use sk-proj-aaaaaaaaaaaaaaaaaaaaaaaabbbbbbbbbbbbbbbbbbbb for the job");
    let err =
        redact_outbound(&messages, &pol, &Provider::Mock).expect_err("block should abort the turn");
    assert!(err.contains("API_KEY"));
    assert!(!err.contains("sk-proj-aaaaa"));
}

#[test]
fn outbound_skips_local_provider_by_default() {
    let pol = build_redact_policy(RedactionMode::Redact);
    let messages = sample_history("sk-proj-aaaaaaaaaaaaaaaaaaaaaaaabbbbbbbbbbbbbbbbbbbb");
    // Ollama is local; skip_local is true by default => no-op.
    for provider in [Provider::Ollama, Provider::Gguf, Provider::Mlx] {
        let rewritten = redact_outbound(&messages, &pol, &provider).unwrap();
        assert!(
            rewritten.is_none(),
            "local provider {provider:?} must bypass redaction by default"
        );
    }
}

#[test]
fn outbound_local_bypass_can_be_disabled() {
    let mut pol = build_redact_policy(RedactionMode::Redact);
    pol.skip_local = false;
    let messages = sample_history("sk-proj-aaaaaaaaaaaaaaaaaaaaaaaabbbbbbbbbbbbbbbbbbbb");
    let rewritten = redact_outbound(&messages, &pol, &Provider::Ollama)
        .unwrap()
        .expect("skip_local=false must redact for Ollama");
    assert!(rewritten[1].content.contains("[REDACTED:API_KEY]"));
}

#[test]
fn outbound_custom_pattern_produces_named_placeholder() {
    let mut pol = build_redact_policy(RedactionMode::Redact);
    pol.extra_patterns.push((
        "CUSTOMER_ID".to_string(),
        Regex::new(r"CUST-\d{8}").unwrap(),
    ));
    let messages = sample_history("please look up CUST-12345678 for me");
    let rewritten = redact_outbound(&messages, &pol, &Provider::Mock)
        .unwrap()
        .expect("custom pattern must trigger rewrite");
    assert!(rewritten[1].content.contains("[REDACTED:CUSTOMER_ID]"));
}

#[test]
fn outbound_allowlist_drops_known_good_match() {
    let mut pol = build_redact_policy(RedactionMode::Redact);
    pol.allowlist
        .push(Regex::new(r"AKIAIOSFODNN7EXAMPLE").unwrap());
    let messages = sample_history("docs example: AKIAIOSFODNN7EXAMPLE (test key)");
    let rewritten = redact_outbound(&messages, &pol, &Provider::Mock).unwrap();
    assert!(
        rewritten.is_none(),
        "allowlisted span must not trigger a rewrite"
    );
}

#[test]
fn outbound_preserves_unaffected_messages_identity() {
    let pol = build_redact_policy(RedactionMode::Redact);
    let mut messages = sample_history("plain system-level content");
    // Add a second user turn with a key; first user turn stays clean.
    messages.push(Message {
        role: Role::User,
        content: "also here is sk-ant-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
        images: vec![],
    });
    let rewritten = redact_outbound(&messages, &pol, &Provider::Mock)
        .unwrap()
        .expect("expected rewrite");
    // Clean turns retain their original content verbatim.
    assert_eq!(rewritten[0].content, messages[0].content);
    assert_eq!(rewritten[1].content, messages[1].content);
    assert!(rewritten[2].content.contains("[REDACTED:API_KEY]"));
}

// --- Redaction: end-to-end via Mock provider ---

#[tokio::test]
async fn mock_sees_original_in_off_mode() {
    // With the global redaction policy at its default (off), the Mock
    // provider sees the user's original text — proves off-mode is a
    // pure pass-through even through the full agent loop.
    let guard = MockGuard::new();
    guard.push_response("ack");

    let mut messages = make_system_messages();
    let mut auto = true;
    let ui = quiet_ui();

    let user_msg = "please remember this url postgres://admin:pw@db.ex.com/p";
    run_agent_turn(
        &Provider::Mock,
        "",
        "mock-model",
        &mut messages,
        user_msg,
        &mut auto,
        &ui,
        MemoryMode::LongTerm,
        false,
    )
    .await
    .unwrap();

    let first_call = &guard.calls()[0];
    assert!(
        first_call
            .iter()
            .any(|m| m.content.contains("postgres://admin:pw@db.ex.com/p")),
        "with default (off) policy, Mock must see the original credential"
    );
}

// --- Describe / Match integrity ---

#[test]
fn redact_result_block_variant_matches_expected_kind() {
    let pol = build_redact_policy(RedactionMode::Block);
    let messages = sample_history("bearer AKIAIOSFODNN7EXAMPLE plus prose");
    let err = redact_outbound(&messages, &pol, &Provider::Mock).unwrap_err();
    assert!(err.contains("AWS_KEY"));
}

#[test]
fn low_level_redact_match_kinds_match_plan_spec() {
    // Direct unit-level check mirroring plan §2 placeholder scheme.
    let pol = build_redact_policy(RedactionMode::Redact);
    let RedactionResult::Redacted { matches, .. } = crate::security::redaction::redact(
        "sk-proj-aaaaaaaaaaaaaaaaaaaaaaaabbbbbbbbbbbbbbbbbbbb and foo@bar.com",
        &pol,
    ) else {
        panic!("expected redacted");
    };
    let kinds: std::collections::HashSet<_> = matches.iter().map(|m| m.kind.clone()).collect();
    assert!(kinds.contains(&DetectorKind::ApiKey));
    assert!(kinds.contains(&DetectorKind::Email));
}
