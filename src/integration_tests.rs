//! End-to-end tests for [`crate::run::run_agent_turn`] driven by a scripted
//! mock LLM. Covers the full loop: message accumulation, tool-call parsing,
//! dispatch, duplicate detection, malformed-tag retry, denial handling, auto
//! mode, memory-window windowing, and iteration cap.
//!
//! The mock lives at [`crate::llm::mock`]; its [`MockGuard`] serializes these
//! tests across parallel `cargo test` threads and resets shared state
//! (response queue, call log, `tools::CALL_HISTORY`) on each construction.

use std::cell::Cell;

use crate::commands::MemoryMode;
use crate::llm::TokenUsage;
use crate::llm::mock::MockGuard;
use crate::message::{Message, Role};
use crate::run::{Provider, run_agent_turn};
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
