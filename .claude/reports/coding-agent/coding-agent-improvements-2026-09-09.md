# Coding-agent improvements — closing the gap to Claude Code / Codex

**Date:** 2026-09-09
**Scope:** what aictl's coding-agent mode is missing relative to dedicated coding agents (Claude Code, OpenAI Codex CLI, opencode), ranked by impact per unit of work.
**Basis:** read of `crates/aictl-core/src/config.rs` (`SYSTEM_PROMPT_CODING`), `run.rs`, `tools.rs`, `coding.rs`, `transcript.rs`, `security.rs`, `ui.rs`, the `llm/` provider modules, and `docs/CODING_AGENT.md`.

---

## Tier 1 — foundations that gate everything else

### 1. Native tool calling instead of XML-in-prose

Every provider gets the same `<tool name="…">` text protocol — `grep tool_calls crates/aictl-core/src/llm/*.rs` returns nothing outside `mock.rs`.

The cost is visible throughout the codebase:

- `tools.rs` carries `TOOL_CLOSE_TAGS` / `STRAY_OPEN_TAGS` / `PARAM_ELEMENTS` normalization purely to survive models leaking their trained XML dialect (observed with `claude-haiku-4-5`).
- `security::check_path_with` rejects paths containing `<` / `>` as a backstop against leaked markup reaching a tool.
- `llm/stream.rs::StreamState` has to hold back any text that could prefix `<tool name="` so partial tool XML never reaches the UI.
- The full ~4KB tool catalogue rides in the system prompt on every single call.

Providers post-trained on native `tools[]` are measurably more reliable at emitting them than at freeform XML, and native calling gives parallel tool calls, tool-call IDs, and structured arguments for free.

**Most of this is already written.** `crates/aictl-server/src/messages/translator/` translates Anthropic-shaped `tools[]` into OpenAI / Gemini / Ollama native shapes and back, including streaming state machines.

**Approach:** introduce an `llm::ToolProtocol` enum — `Native` for OpenAI / Anthropic / Gemini / Mistral / DeepSeek / Grok, `Xml` for Ollama / GGUF / MLX and anything unrecognized. Keep the existing XML parser as the fallback path so local providers and the foreign-dialect tolerance keep working unchanged.

**Estimate:** ~1 day AI-assisted.

### 2. Context management — the sharpest gap

Two concrete holes:

**No tool-result size cap.** `read_file` and `exec_shell` return full output into history unbounded — there is no `max_output_bytes` anywhere in `security.rs` or the tool impls (only `DEFAULT_MAX_WRITE_BYTES` for *writes*). One `cargo build` on a large workspace, one 3MB log, or one `read_file` on a generated file kills the turn mid-loop with a provider error.

> Fix: head/tail truncation per tool result (~8–16KB) with an explicit `[N lines elided — re-read with --lines 400-600]` marker, so the model knows to narrow rather than assuming it saw the whole thing.

**No auto-compaction.** `/compact` is manual only; `grep auto_compact` returns nothing. `llm::context_limit(model)` already exists and is tested.

> Fix: check accumulated token usage in the `run.rs` agent loop and fire compaction at ~75–80% of the model's window. Compact **tool results first** — in a coding session they are 80%+ of the transcript and the oldest ones are almost always dead weight, while user prompts and the model's plans should survive verbatim.

**Estimate:** ~1 day AI-assisted.

### 3. Extended thinking / reasoning effort is not wired

No `thinking` block for Anthropic, no `reasoning_effort` for OpenAI o-series — `grep thinking crates/aictl-core/src/llm/anthropic.rs` is empty. For coding specifically this is where a large share of current model quality lives.

**Approach:** per-model capability flag in the `MODELS` catalog plus a `/behavior` knob to select effort level. Low effort, high return.

---

## Tier 2 — agent loop capabilities

### 4. Subagents / task delegation

`agents.rs` agents are prompt appendices, not spawnable workers. The single highest-leverage thing Claude Code does for long tasks is delegate a search into an isolated context and get back only the conclusion.

`run::run_agent_single` already exists. A `task` tool that calls it with a fresh `Vec<Message>`, its own iteration budget, and returns only a summary would let an Explore phase burn 60k tokens without touching the main thread. Pairs naturally with the existing agent catalogue as subagent personas.

### 5. A todo / plan tool

The Plan phase currently produces prose the model forgets three tool calls later. A `todo` tool holding structured state that is re-injected each turn is a big part of why Claude Code stays on track across 40-step tasks — and it gives the CLI something real to render next to the existing `[phase]` prefix.

### 6. Background processes

`exec_shell` blocks with a timeout. There is no way to start a dev server, tail logs, or kick off a slow test suite and keep working. Needs `spawn` / `poll_output` / `kill` around a process registry.

### 7. Two smaller loop issues

- `DEFAULT_MAX_ITERATIONS` of 20 is low for real coding tasks. Claude Code effectively runs unbounded with a *token* budget rather than a turn count.
- No way to queue a steering message while the agent works — `with_esc_cancel` throws the whole turn away instead of letting the user redirect it.

---

## Tier 3 — code intelligence & safety

### 8. Diagnostics in the edit loop, not just at Review

`lint_file` exists but only fires in the host Review hook after a final answer is emitted. Running a fast compile / typecheck on the touched file immediately after each `edit_file` and appending errors to the tool result closes the feedback loop from "one full turn" to "zero turns".

Full LSP integration (go-to-def, find-refs) is the larger version of this, but the cheap slice captures most of the value.

### 9. File checkpointing

`/undo` (via `transcript.rs`) trims the **transcript** — the files on disk stay edited. An undo therefore silently leaves the workspace inconsistent with the conversation.

**Approach:** per-turn shadow copy or `git stash create` snapshot of touched files, restored on undo. This is what makes the agent safe to let run unattended.

### 10. Approval granularity

`ToolApproval` in `ui.rs` is `Allow` / `Deny` / `AutoAccept` — no persistent per-pattern rule ("always allow `cargo test`"). Users are pushed into either babysitting every call or full auto with nothing in between, which is exactly the axis where Claude Code's allowlist earns its keep.

---

## Tier 4 — the thing that makes the rest verifiable

### 11. An eval harness

738 unit tests and one smoke test (`crates/aictl-cli/tests/cli_smoke.rs`), but nothing that measures **task completion**. Every item above is a prompt or loop change whose effect currently cannot be observed.

**Approach:** twenty handwritten tasks in throwaway repos (fix this failing test, add this flag, rename across three files) with a pass/fail script, runnable against 2–3 models. Turns prompt tuning from taste into measurement.

Worth noting: `SYSTEM_PROMPT_CODING` is ~6KB of accumulated prose with no evidence attached to any paragraph. This is probably where to actually start.

**Estimate:** a few hours AI-assisted.

---

## Suggested order

```
11 → 2 → 1 → 3 → 4/5 → 6 → 8 → 9 → 10 → 7
```

Rationale: get measurement first, then fix the two things that cause hard failures (context blowups, tool-parse drift), then the cheap quality win (thinking / reasoning effort), then the context-economy features. Items 1 and 2 are each roughly a day of AI-assisted work; item 11 is a few hours and pays for itself immediately by making 1–10 measurable.
