# Spec: Context management — result budgeting and in-loop compaction

> Report item **2** (Tier 1). Depends on nothing; gates the subagent and loop-budget specs.

## Context

The gap report calls this "the sharpest gap" and claims there is neither a tool-result size
cap nor auto-compaction. **Both claims are too strong.** Verified against `749ef89`:

- `config::MAX_TOOL_OUTPUT_LEN = 10_000` ([`config.rs:55`](../../crates/aictl-core/src/config.rs))
  and `tools::util::truncate_output` ([`tools/util.rs:6`](../../crates/aictl-core/src/tools/util.rs))
  exist and are called from `shell`, `filesystem`, `git`, `web`, `document`, `lint`,
  `run_code`, `json_query`, `csv_query`, `diff`, `archive`, `clipboard`, `system_info`,
  `list_processes`.
- `config::auto_compact_threshold()` (default `80`) drives `repl::handle_user_turn`
  ([`repl.rs:723`](../../crates/aictl-cli/src/repl.rs)), which compacts before dispatching a
  new user prompt.

The actual gaps are narrower and sharper:

1. **Truncation is a blind tail-drop.** `truncate_output` cuts at 10KB and appends
   `\n... (truncated)`. The model loses the *end* of every long output — which for a build
   log, a test run, or a stack trace is exactly where the useful part is — and gets no hint
   about how to retrieve what it lost.
2. **The cap is a compile-time constant.** No config key, no per-tool differentiation. A
   `read_file` on source and an `exec_shell` running `cargo build` get the same 10KB.
3. **Truncation is applied ad hoc per tool, not centrally.** `tools::execute_tool`
   ([`tools.rs:627`](../../crates/aictl-core/src/tools.rs)) sanitizes centrally
   (`security::sanitize_output` at lines 673 and 728) but never truncates. Any tool whose
   author forgets the call is unbounded — and **MCP results (`mcp::call_tool`) and plugin
   results (`plugins::execute_plugin`) are uncapped today**. An MCP server returning a 3MB
   blob kills the turn.
4. **Compaction never fires inside a turn.** The REPL check runs *between* user prompts,
   keyed off `last_input_tokens` from the previous turn. A single turn that reads twelve
   files and runs three builds can blow the window without the host ever looking. Single-shot
   (`run::run_agent_single`) and the desktop have no compaction at all.
5. **Compaction is indiscriminate.** `run::compact_messages`
   ([`run.rs:2136`](../../crates/aictl-core/src/run.rs)) collapses the whole transcript to
   `[system, user(summary), assistant(ack)]`. In a coding session, tool results are 80%+ of
   the transcript and the oldest are dead weight, while the user's prompt and the model's
   plan are the two things that must survive verbatim — and today they are summarized away
   along with everything else.

## Goals & Non-goals

**Goals**

- Move truncation into `tools::execute_tool` so it covers every dispatch path uniformly,
  including MCP and plugin tools.
- Replace tail-drop with **head+tail retention** and an explicit, actionable elision marker
  naming the line range that was dropped and how to retrieve it.
- Make the cap a policy value (`security::ResourcePolicy`) with a config key and an
  optional per-tool override table.
- Add a mid-turn context check inside `run::run_agent_turn` that fires compaction when
  accumulated input tokens cross the threshold, so long tool loops survive.
- Make compaction **tiered**: evict old tool results first, then summarize, preserving user
  prompts and the most recent assistant plan verbatim.
- Keep the REPL's existing pre-turn check as a cheap fast path; keep `/compact` manual
  behavior unchanged.

**Non-goals**

- No semantic/embedding-based retrieval of elided content. The re-read hint is the
  retrieval mechanism.
- No provider-side context caching changes. Prompt caching is orthogonal (and already
  accounted for in `TokenUsage`).
- No change to `MAX_MESSAGES` (200) as a secondary bound.
- No streaming truncation. Tool results are buffered by construction.
- No server changes.

## Design

### 1. Central truncation in `execute_tool`

`tools::util::truncate_output` is replaced by a richer `tools::util::budget_output`:

```rust
pub(crate) struct OutputBudget {
    /// Total bytes retained. Head+tail split, not a raw cut.
    pub max_bytes: usize,
    /// Fraction of the budget given to the head. Remainder goes to the tail.
    pub head_ratio: f32,      // default 0.4 — the tail matters more for logs
}

/// Truncate `s` to `budget`, retaining the head and the tail and replacing
/// the middle with an actionable marker. Walks to UTF-8 char boundaries on
/// both cuts. No-op when `s` fits.
pub(crate) fn budget_output(s: &mut String, budget: OutputBudget, hint: &ElisionHint);
```

`ElisionHint` carries what the model needs to get the rest back:

```rust
pub(crate) enum ElisionHint {
    /// A file read — the model can narrow with `read_file --lines`.
    File { path: String, total_lines: usize },
    /// Command output — the model can re-run piped through a filter.
    Command { label: String },
    /// Anything else.
    Generic,
}
```

Rendered markers:

```
… 1,847 lines elided (bytes 4001–192204 of 196204) …
re-read a narrower slice with: read_file
src/big.rs
--lines 400-600
```

```
… 812 lines elided from the middle of `cargo build` output …
head and tail retained; re-run with a filter (e.g. `2>&1 | grep -E '^error'`) for the rest
```

The `File` hint's line numbers are exact because `read_file` already counts lines for its
`--lines` support (Phase 2). For `Command` the marker states head/tail retention explicitly —
the current `... (truncated)` reads as "the output ended here", which is a lie the model acts
on.

Call site — in `tools::execute_tool`, immediately before the existing `sanitize_output` at
[`tools.rs:728`](../../crates/aictl-core/src/tools.rs):

```rust
let mut result = /* … existing match … */;
util::budget_output(&mut result, budget_for(&tool_call.name), &hint);
let sanitized = crate::security::sanitize_output(&result);
```

Order matters: budget **before** sanitize, so redaction placeholders are never split across
the elision boundary, and sanitization cost is bounded.

The per-tool `truncate_output` calls are removed. They become dead weight once the central
call lands, and leaving them in means two different budgets fighting. The one exception is
`tools/test.rs`, whose `MAX_FAILURES` / `MAX_FAILURE_MESSAGE` caps are *structural*
(bounding a parsed `TestSummary`, not raw text) and stay.

### 2. Budget as policy

`security::ResourcePolicy` ([`security.rs:89`](../../crates/aictl-core/src/security.rs)) gains
a field alongside `max_file_write_bytes`:

```rust
pub struct ResourcePolicy {
    pub shell_timeout_secs: u64,
    pub max_file_write_bytes: usize,
    pub max_tool_output_bytes: usize,   // new
}
```

Loaded from `AICTL_SECURITY_MAX_TOOL_OUTPUT_BYTES`, defaulting to the current
`MAX_TOOL_OUTPUT_LEN` value so behavior is unchanged until a user opts in. `MAX_TOOL_OUTPUT_LEN`
stays as the default constant.

Per-tool overrides live in a small table in `tools.rs` — the defaults encode "a build log is
worth more context than a directory listing":

```rust
fn budget_for(tool: &str) -> OutputBudget {
    let max = crate::security::policy().resources.max_tool_output_bytes;
    match tool {
        // Logs and test output: tail-heavy, and worth more room.
        "exec_shell" | "test" | "run_code" => OutputBudget { max_bytes: max * 2, head_ratio: 0.25 },
        // Source reads: head-heavy (imports, signatures) and line-addressable.
        "read_file" | "read_document"      => OutputBudget { max_bytes: max * 2, head_ratio: 0.6 },
        // Listings and search hits: uniform, cheap to re-narrow.
        "list_directory" | "find_files" | "search_files" => OutputBudget { max_bytes: max, head_ratio: 1.0 },
        _ => OutputBudget { max_bytes: max, head_ratio: 0.5 },
    }
}
```

MCP and plugin results fall into the `_` arm — the first time they are bounded at all.

Server-scoped mirror: **none**. The server does not run tools, matching the existing
carve-out documented in `CLAUDE.md`.

### 3. In-loop context check

`run::run_agent_turn` already tracks `last_input_tokens` per iteration (used for the summary
line and by the REPL after the turn). The loop gains a check at the top of each iteration,
after the spinner and before the provider call:

```rust
let limit = llm::context_limit(model);
let pct = llm::pct(last_input_tokens, limit);
if pct >= config::auto_compact_threshold() && !compaction_exhausted {
    ui.show_reasoning(&format!("context at {pct}% — compacting tool results"));
    let freed = context::relieve(messages, provider, api_key, model, ui).await?;
    if freed == 0 { compaction_exhausted = true; }   // nothing left to shed; let the
                                                     // provider error surface honestly
}
```

`compaction_exhausted` prevents a compact-loop when the transcript is already minimal (a
single enormous user message, say) — one attempt per turn that frees nothing disables further
attempts for that turn.

This runs for **every** frontend, because it lives in the engine loop: REPL, single-shot, and
desktop all inherit it. The REPL's existing `handle_user_turn` pre-check stays — it is cheaper
(no mid-turn state to reason about) and it keeps the user-visible "auto-compacting…" message
at the natural seam.

### 4. Tiered compaction

New module `crates/aictl-core/src/context.rs`:

```rust
/// Shed context in increasing order of destructiveness until the transcript
/// fits under `target_pct` of the model's window. Returns bytes freed.
pub async fn relieve(
    messages: &mut Vec<Message>,
    provider: &Provider, api_key: &str, model: &str,
    ui: &dyn AgentUI,
) -> Result<usize, AictlError>;
```

Three tiers, applied in order, stopping as soon as the estimate fits:

**Tier 1 — evict stale tool results.** Walk `messages` oldest-first. Any user message whose
content is a `<tool_result>` / `<tool_results>` envelope, that is not among the **most recent
`AICTL_CONTEXT_KEEP_RESULTS` (default 6)** such envelopes, is replaced in place by a stub:

```
<tool_result name="read_file" elided="true">
(result for `src/foo.rs` elided to reclaim context; re-run the tool if you still need it)
</tool_result>
```

No LLM call, instant, and reversible in the sense that the model can just re-read. In a
coding session this alone typically reclaims the majority of the transcript.

**Tier 2 — evict elided-stub runs.** If tier 1 was not enough, remove the stub messages
entirely (and their paired assistant tool-call turns), preserving conversational alternation.
Still no LLM call.

**Tier 3 — summarize, preserving anchors.** Fall back to an LLM summary, but unlike today's
`compact_messages` it preserves verbatim:

- the system message (index 0),
- every user message that `transcript::is_user_prompt` identifies as a real user prompt
  (not a tool result, not a hook context block),
- the most recent assistant message containing a plan (heuristic: the last assistant message
  before the first `edit_file`/`write_file` tool call, or the last one carrying a
  `<phase>plan</phase>` tag).

Everything between anchors is replaced by one summary user turn carrying the existing
`transcript::COMPACTION_HEADER` prefix, so `transcript::is_post_compaction` and `/undo` keep
working unchanged.

`run::compact_messages` is retained as-is for the manual `/compact` path (users asking for a
full collapse should get one) and is refactored to share tier 3's provider dispatch.

### 5. Token accounting for the check

`last_input_tokens` is only known *after* a provider call returns usage. On the first
iteration of a turn it is `0`, so the check is a no-op — which is correct (a fresh turn
inherits the previous turn's transcript, and the REPL pre-check covered that). For subsequent
iterations it is the real number from the provider.

For providers that do not report usage, fall back to a byte-based estimate:
`chars / 4` across all message contents. Add `context::estimate_tokens(&[Message]) -> u64`
with that heuristic, used only when `last_input_tokens == 0` and `messages.len() > 1`.

### 6. Configuration

| Key | Default | Meaning |
|-----|---------|---------|
| `AICTL_SECURITY_MAX_TOOL_OUTPUT_BYTES` | `10000` | Base per-result byte budget; per-tool multipliers apply |
| `AICTL_AUTO_COMPACT_THRESHOLD` | `80` | **Existing** — now also drives the in-loop check |
| `AICTL_CONTEXT_KEEP_RESULTS` | `6` | Tool-result envelopes kept verbatim by tier 1 |
| `AICTL_CONTEXT_IN_LOOP_COMPACT` | `true` | Kill switch for the mid-turn check |

### 7. CLI / desktop surface

- `/context` gains two lines: `tool-result budget: 10000 B (×2 for logs/reads)` and
  `in-loop compaction: on (threshold 80%)`.
- `--info` gains `context: 80% / keep 6 results`.
- The mid-turn compaction emits a reasoning line (`context at 84% — compacting tool
  results`), so the user sees why a turn paused. Suppressed under `--quiet` and in
  `text`/`json` output formats like all other reasoning output.
- Desktop: the existing context meter (`commands/context.rs`) gains the same two fields in
  its status struct. No new UI beyond rendering them.

### 8. Integration points

| File | Change |
|------|--------|
| `crates/aictl-core/src/tools/util.rs` | Replace `truncate_output` with `budget_output` + `ElisionHint` |
| `crates/aictl-core/src/tools.rs` | Central `budget_output` call in `execute_tool`; `budget_for` table; hint construction |
| `crates/aictl-core/src/tools/*.rs` | Remove the 15 per-tool `truncate_output` calls (keep `test.rs`'s structural caps) |
| `crates/aictl-core/src/security.rs` | `ResourcePolicy::max_tool_output_bytes` + loader |
| `crates/aictl-core/src/context.rs` | **New** — `relieve`, the three tiers, `estimate_tokens` |
| `crates/aictl-core/src/run.rs` | In-loop check in `run_agent_turn`; `compact_messages` shares tier 3 dispatch |
| `crates/aictl-core/src/config.rs` | `AICTL_CONTEXT_KEEP_RESULTS`, `AICTL_CONTEXT_IN_LOOP_COMPACT`, `AICTL_SECURITY_MAX_TOOL_OUTPUT_BYTES` |
| `crates/aictl-cli/src/commands/context.rs` | Two new lines |
| `crates/aictl-cli/src/commands/info.rs` | One new line |
| `crates/aictl-desktop/src/commands/context.rs` | Two new status fields |
| `docs/CONFIG.md`, `docs/TOOLS.md`, `docs/CODING_AGENT.md`, `CLAUDE.md` | Document the budget and the tiers |

### 9. Testing

**Unit**

- `budget_output`: under-budget input untouched; over-budget input retains exactly the head
  and tail byte counts implied by `head_ratio`; cuts land on UTF-8 boundaries (multi-byte
  char straddling both cut points — extends the existing test in `util.rs`);
  `head_ratio: 1.0` degrades to today's tail-drop.
- `ElisionHint` rendering: `File` marker contains the path and a syntactically valid
  `read_file --lines` body; `Command` marker states head+tail retention.
- `budget_for`: log tools get the doubled budget; unknown tool names (`mcp__x__y`,
  plugin names) get the base budget — the regression test for gap (3).
- `context::relieve` tier 1: a transcript with 20 tool-result envelopes keeps the last 6
  verbatim and stubs 14; returns bytes freed > 0.
- Tier 1 idempotence: a second `relieve` on an already-stubbed transcript frees 0 and does
  not re-stub.
- Tier 3 anchors: user prompts identified by `transcript::is_user_prompt` survive verbatim;
  the compaction header is present; `transcript::is_post_compaction` returns true afterwards.
- `estimate_tokens`: monotonic in transcript size; within 2× of a known tokenizer count on a
  fixture (loose bound — it only needs to be the right order of magnitude).

**Integration** (mock-LLM harness)

- Mock provider returns escalating `input_tokens` across iterations; assert compaction fires
  exactly once when the threshold is crossed and the next provider call sees a shorter
  transcript.
- `AICTL_CONTEXT_IN_LOOP_COMPACT=false` disables it.
- A transcript that cannot be shrunk (one giant user message) attempts once, sets
  `compaction_exhausted`, and lets the provider error through rather than looping.

**Eval**

Task 20 from the eval harness (5k-line file, edit one function) is the end-to-end regression:
it must pass with the budget in place, and the audit log must show `read_file --lines` calls
rather than repeated full reads.

### 10. Rollout

1. **Central budgeting** — `budget_output`, `budget_for`, the `execute_tool` call site, and
   removal of the per-tool calls. Behavior-visible but self-contained; MCP/plugin capping
   lands here.
2. **`context.rs` tiers 1–2** — no LLM calls, pure transcript surgery, wired into
   `relieve` but not yet called from the loop.
3. **In-loop check + tier 3** — flips `run_agent_turn`. Gated by
   `AICTL_CONTEXT_IN_LOOP_COMPACT` so it can be disabled without a re-roll.

PR 1 is independently valuable and independently revertable.

### 11. Verification

1. `cargo build --workspace` (default + `--all-features`) and `cargo lint` clean.
2. `cargo test` clean including the new unit and integration tests.
3. `grep -rn "truncate_output" crates/aictl-core/src/tools/` returns only `util.rs` and
   `test.rs`'s structural caps.
4. An MCP tool returning a 3MB payload no longer reaches the transcript uncapped
   (reproducible with the `examples/mcp/tiny_add/server.py` smoke server modified to echo a
   large blob).
5. A manual session reading twelve large files in one turn completes instead of erroring.
6. CI gate: `grep -rE 'context::relieve|budget_output' crates/aictl-server/src/` empty.

### 12. Risks

- **Head+tail split hides the middle of a diff.** For `diff_files` and `git diff` the middle
  is not obviously less valuable. Mitigation: `head_ratio: 0.5` for those, and the marker
  states the byte range so the model can re-run scoped to a path.
- **Removing per-tool truncation changes memory profile.** Tools currently truncate before
  returning; centralizing means the full string exists in memory once. Bounded already by
  each tool's own read limits (file size caps, shell timeout), but a tool that streams
  unbounded output could spike. Mitigation: keep an early bail in `exec_shell`'s reader at
  4× the budget, which is where the unbounded case actually lives.
- **Tier 1 stubbing confuses the duplicate-call guard.** A model that re-reads a stubbed file
  hits the guard if it was the immediately preceding call. Mitigation: `context::relieve`
  calls `tools::clear_call_history()` after stubbing — the transcript changed underneath the
  model, so the guard's premise no longer holds.
- **Tier 3 anchor heuristic picks the wrong plan message.** Mitigation: preserving one extra
  assistant message is cheap; when the heuristic finds nothing, fall back to preserving the
  most recent assistant message unconditionally.
- **In-loop compaction surprises single-shot users** whose output now includes a compaction
  pause. Mitigation: the reasoning line is suppressed under `--quiet` / `--format json`, and
  the answer envelope is unchanged.

### 13. Open questions

- **Should tier 1 stub or delete?** Stubbing preserves alternation and gives the model a
  breadcrumb; deleting frees more. Lean stub for tier 1, delete in tier 2 — as specified.
- **`head_ratio` defaults are guesses.** The eval harness's tool-mix data on task 20 and the
  `fix` tasks is the calibration input. Revisit after the first baseline.
- **Should the threshold differ in coding mode?** Coding transcripts grow faster; 80% may fire
  too late when a single `cargo build` result can add 20KB. Lean: keep one knob, and revisit
  with a `AICTL_CODING_AUTO_COMPACT_THRESHOLD` override only if evals show late firing.
- **Does the desktop need a user-visible compaction event?** It has a context meter but no
  event stream for this. Lean: reuse the existing reasoning-event channel; no new IPC.
