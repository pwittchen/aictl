# Plan: Coding Agent — Phase 4 (parallel tool execution and streaming)

## Context

The agent loop executes tool calls strictly one at a time. `parse_tool_call` ([`tools.rs:239`](../../crates/aictl-core/src/tools.rs)) returns `Option<ToolCall>` — a single call per LLM response — and the loop in `run_agent_turn` ([`run.rs:776`](../../crates/aictl-core/src/run.rs)) dispatches that one call, awaits it, pushes the result, and re-asks the model. Five `read_file`s during an Explore burst take five round-trips. Build + lint on N changed files during the structured Review (Phase 3) is N+1 sequential `exec_shell` spawns. The user pays the wall-clock for every dispatch even when the tools are independent.

The streaming path is similarly coarse: chunks arrive, get word-wrapped into termimad, and render. The `[phase]` prefix in the REPL is updated only between LLM turns — when the model emits `<phase>code</phase>` mid-stream, the prefix doesn't change until the next prompt. The Phase-1 plan called this out as acceptable for v1; in Phase 4 we revisit and make the indicator track the model's self-reported phase as soon as the tag streams in.

Phase 4 is the smallest of the four follow-up phases. It's two adjacent wins:

1. **Parallel tool execution** — when the model emits multiple tool calls in one response (a new grammar this plan introduces) and the calls are independent, run them concurrently via `tokio::JoinSet` and join the results into a single `<tool_results>` injection.
2. **Streaming refinements for coding mode** — real-time `[phase]` updates as the `<phase>` tag arrives mid-stream, plus a tiny "running N tools…" spinner when parallel dispatch is in flight.

Neither change is structural — the security gate, audit log, redaction seams, and Review hook all stay exactly where they are. The change is at the *dispatch boundary* of the agent loop and inside the REPL's streaming UI handler.

## Goals & Non-goals

**Goals**

- Extend `parse_tool_call` to a `parse_tool_calls` returning `Vec<ToolCall>` so a single model response can carry multiple `<tool …>` blocks.
- Run independent tool calls in parallel via `tokio::JoinSet`, capped at `AICTL_CODING_PARALLEL_TOOLS_MAX` (default 4). Calls outside the cap queue and run in the next batch.
- Define *independence*: per-tool metadata declares whether a call is parallelizable. Read-only tools (`read_file`, `list_directory`, `search_files`, `find_files`, `git status/log/blame/diff`, `lint_file`, `check_port`, `system_info`, `fetch_url`, `extract_website`, `read_document`, `read_image`, `json_query`, `csv_query`, `calculate`, `fetch_datetime`, `fetch_geolocation`, `clipboard read`) are parallelizable. Side-effectful tools (`write_file`, `edit_file`, `remove_file`, `create_directory`, `exec_shell`, `run_code`, `git commit`, `notify`, `archive`, `save_memory`, `generate_image`, `test`, `clipboard write`) are not — they get serialized one-per-batch even when emitted alongside read-only ones. The model is *told* in the prompt to batch reads; the host enforces by inspection.
- Join parallel results into a single `<tool_results>` user turn whose body lists each result block in the order the model emitted them (deterministic for the model's reading), not the order they completed. Auditing logs per-tool timing independently.
- Update the REPL's `[phase]` indicator in real time: when the streaming sink sees a fully-buffered `<phase>NAME</phase>` tag, it broadcasts a phase-change event the REPL reads on the same tokio receiver that drains deltas. CLI-only — desktop UI doesn't expose phase yet.
- Honor the security gate, redaction, duplicate-call guard, and PreToolUse hook *per call*, not per batch. A blocked call in a parallel batch doesn't block the others.

**Non-goals**

- No DAG scheduler. Parallel dispatch is "everything readable, all at once" — not "wait on these three reads then start this edit". The model still gates the side-effect call into its own LLM turn.
- No speculative execution. We only run what the model emitted; we don't prefetch.
- No multi-model dispatch (running two LLM calls in parallel on different providers). That's a different feature.
- No new tool. The parallelism is a property of the dispatch loop, not a tool API.
- No change to single-call shape. A response with one `<tool>` block runs exactly like today.
- No automatic Review-phase parallelism. The structured Review hook (Phase 3) can opportunistically parallelize its build + lint reads via the same `JoinSet` helper, but that's a follow-up tweak inside `run_structured_review`, not a Phase-4-defining change.
- No streaming changes outside coding mode. The phase-tag-in-stream wiring is gated on `coding_agent_enabled()`.
- No server changes — same as every prior phase.
- No desktop UI for phase changes — desktop is still phase-blind by design (Phase 1 decision).

## Design

### 1. Multi-tool grammar

Today the model is told to emit one `<tool>` per response. We extend the prompt to allow batching of read-only calls:

```
You may emit MORE THAN ONE <tool> block in a single response only when
ALL calls are read-only (read_file, list_directory, search_files,
find_files, git status, git log, git blame, git diff, lint_file,
check_port, system_info, json_query, csv_query, calculate,
fetch_datetime, fetch_url, extract_website, read_document, read_image,
fetch_geolocation, clipboard read). Batched calls run in parallel and
the results return together. Any side-effect call (write_file,
edit_file, remove_file, create_directory, exec_shell, run_code,
git commit, notify, archive, save_memory, generate_image, test,
clipboard write) MUST be emitted alone — one per response. If the host
detects a batched side-effect call, only the first one runs and the
rest are rejected.
```

The text lives in the existing `SYSTEM_PROMPT` and `SYSTEM_PROMPT_CODING` constants. It's the same prose in both — non-coding sessions also benefit from parallel reads, and the feature gate is "the model emitted multiple `<tool>` blocks", not "coding mode is on".

### 2. `parse_tool_calls`

`parse_tool_call` keeps working (returns `Option<ToolCall>` for back-compat with existing tests / callers); a new sibling `parse_tool_calls(response: &str) -> Vec<ToolCall>` walks the response and collects every well-formed `<tool …>…</tool>` block in source order. Malformed tags don't poison the list — they're either picked up by `looks_like_malformed_tool_call` (existing behavior) or silently ignored when a later well-formed tag is present.

```rust
pub fn parse_tool_calls(response: &str) -> Vec<ToolCall> {
    let mut out = Vec::new();
    let mut remaining = response;
    while let Some(call) = parse_tool_call(remaining) {
        // Find where this call ended so we can advance past it.
        let needle = format!("<tool name=\"{}\"", call.name);
        let Some(start) = remaining.find(&needle) else { break };
        let after_start = &remaining[start..];
        let Some(end) = after_start.find("</tool>") else { break };
        let advance = start + end + "</tool>".len();
        out.push(call);
        remaining = &remaining[advance..];
    }
    out
}
```

Single-call shape: `parse_tool_calls(r).len() == 1` for every response that today returns `Some(_)` from `parse_tool_call`. Existing tests pass unchanged.

### 3. Per-tool side-effect classification

A new const adjacent to `BUILTIN_TOOLS`:

```rust
/// Tools whose execution mutates state outside the host's memory: file
/// writes, process spawns, network sends, persisted memory writes,
/// clipboard writes. These are *not* parallelizable — the model must
/// emit them alone, one per LLM response.
const SIDE_EFFECT_TOOLS: &[&str] = &[
    "write_file",
    "edit_file",
    "remove_file",
    "create_directory",
    "exec_shell",
    "run_code",
    "notify",
    "archive",
    "save_memory",
    "generate_image",
    "test",
];

/// `git` is split: status/log/blame/diff are read-only, commit is a
/// side-effect. The dispatch loop inspects the body's first token to
/// classify.
fn is_git_side_effect(input: &str) -> bool {
    let first = input.split_whitespace().next().unwrap_or("");
    matches!(first, "commit")
}

/// `clipboard` is split similarly — `read` is parallelizable, `write`
/// is a side-effect.
fn is_clipboard_side_effect(input: &str) -> bool {
    let first = input.split_whitespace().next().unwrap_or("");
    matches!(first, "write")
}

pub fn is_parallelizable(call: &ToolCall) -> bool {
    if SIDE_EFFECT_TOOLS.contains(&call.name.as_str()) { return false; }
    if call.name == "git" && is_git_side_effect(&call.input) { return false; }
    if call.name == "clipboard" && is_clipboard_side_effect(&call.input) { return false; }
    // MCP and plugin tools are conservatively *not* parallelizable in v1.
    // Their side-effect surface is unknown to us, so the safe default is
    // serial. A future MCP capability bit can lift this.
    if call.name.starts_with("mcp__") { return false; }
    if crate::plugins::find(&call.name).is_some() { return false; }
    true
}
```

### 4. Dispatch loop changes

The existing single-call branch in `run_agent_turn` ([`run.rs:1214`](../../crates/aictl-core/src/run.rs)) is replaced with a per-batch dispatch. After `parse_tool_calls`:

```rust
let calls = tools::parse_tool_calls(&response);
let malformed_tool_call = calls.is_empty() && tools::looks_like_malformed_tool_call(&response);
let is_final_answer = calls.is_empty() && !malformed_tool_call;
```

Three cases:

- `calls.is_empty()` — same as today (final-answer branch or malformed branch).
- `calls.len() == 1` — same dispatch path as today, including the duplicate-call guard. No behavioral change for the model's most common shape.
- `calls.len() > 1` — new batch path (§5).

For the batch path:

1. Validate the batch: at most one side-effect call, all calls have unique normalized keys (a batch can't contain two `read_file foo.rs`).
2. If a side-effect call is present alongside read-only ones, reject the batch *partially*: run only the side-effect call (the first by source order in the batch is the canonical one) and inject a `<tool_result>` per read-only sibling explaining "rejected: batched alongside side-effect call `<name>`; emit reads in a separate turn". This matches the prompt rule and avoids silently dropping work the model expected to land.
3. Otherwise, dispatch all calls in parallel.

### 5. Parallel dispatch via `JoinSet`

A new helper in `run.rs`:

```rust
async fn dispatch_parallel(
    calls: &[tools::ToolCall],
    ui: &dyn AgentUI,
    auto: &mut bool,
    messages: &mut Vec<Message>,
    streamed: bool,
) -> Result<u32, AictlError> {
    let cap = config::coding_parallel_tools_max();   // default 4
    let mut tool_calls_executed = 0u32;
    // Iterate calls in chunks of `cap`. Each chunk runs concurrently;
    // the next chunk starts only after the previous one drains.
    for chunk in calls.chunks(cap) {
        ui.start_spinner(&format!("running {} tools in parallel...", chunk.len()));
        let mut set: tokio::task::JoinSet<(usize, tools::ToolCall, Result<ui::ToolApproval, AictlError>, Option<tools::ToolOutput>)> = tokio::task::JoinSet::new();
        for (idx, call) in chunk.iter().enumerate() {
            let call = call.clone();
            set.spawn(async move {
                // Per-call: hooks + approval + execute. Re-uses the
                // existing handle_tool_call body, refactored to return
                // the output instead of pushing onto `messages` directly.
                let (approval, output) = run_single_call(&call, /*auto*/ true, /* see below */).await?;
                Ok((idx, call, Ok(approval), output))
            });
        }
        // Collect in completion order…
        let mut collected: Vec<(usize, tools::ToolCall, Option<tools::ToolOutput>)> = Vec::with_capacity(chunk.len());
        while let Some(joined) = set.join_next().await {
            let (idx, call, approval, output) = joined.map_err(|e| AictlError::Other(format!("tool task panicked: {e}")))?;
            collected.push((idx, call, output));
        }
        ui.stop_spinner();
        // …then sort by source-order index so the model reads results
        // in the order it emitted the calls.
        collected.sort_by_key(|(idx, _, _)| *idx);
        // Join into a single tool_results block on `messages`.
        let mut body = String::new();
        for (_, call, output) in &collected {
            body.push_str(&format!("\n<tool_result name=\"{}\">\n{}\n</tool_result>\n",
                call.name,
                output.as_ref().map(|o| o.text.as_str()).unwrap_or("(no output)")
            ));
            tool_calls_executed += 1;
        }
        messages.push(Message {
            role: Role::User,
            content: format!("<tool_results>{body}</tool_results>"),
            images: collected.into_iter().flat_map(|(_, _, o)| o.map(|x| x.images).unwrap_or_default()).collect(),
        });
    }
    Ok(tool_calls_executed)
}
```

**Approval / auto-accept under parallel dispatch**: human-in-the-loop confirmation is a UI bottleneck. Three modes:

- `*auto == true` (or AutoAccept already engaged): every call in the batch is auto-approved. Same as today.
- `*auto == false` and the batch is all read-only: the UI shows a single confirmation prompt ("approve N reads?") with the same Allow/AutoAccept/Deny options. Approval applies to the whole batch.
- `*auto == false` and the batch has a side-effect call: the side-effect call goes through the existing per-call confirm; the read-only siblings get their own bundled prompt. This is exclusively in the partial-rejection branch from §4(2); in practice the side-effect runs serially because the host rejected the batch — only the side-effect call ends up dispatched.

**Per-call seams stay intact**:

- `validate_tool` runs per call inside the spawned task. A denied call doesn't poison the batch — its `<tool_result>` contains the denial message; siblings continue.
- PreToolUse / PostToolUse hooks run per call. A pre-hook can block one call without affecting siblings.
- Duplicate-call guard runs per call (each call lands in its own slot read/write; concurrent writes are serialized by `Mutex`).
- Audit logs are per call with the parsed wall-clock duration of *that* call, not the batch.
- Redaction's tool-result seam runs per call before the result is concatenated into the batch body.

### 6. Side-effect serialization within a batch

The model is told not to mix; if it does anyway, the host short-circuits (§4(2)). For completeness, if a batch *somehow* contains multiple side-effect calls (e.g. two `edit_file`s), only the first runs and the rest produce a `<tool_result>` explaining the rejection — this preserves the invariant that side-effect calls are serialized one per LLM response.

### 7. Streaming refinements for coding mode

`StreamState` in `crates/aictl-core/src/llm/stream.rs` already holds a buffer that scans for `<tool name="` to know when to suspend visible output. We add a parallel scan for `<phase>` ↔ `</phase>` and emit a `StreamEvent::PhaseChange(WorkflowPhase)` on the same channel used for `StreamEvent::Delta` / `StreamEvent::Suspend`.

```rust
enum StreamEvent {
    Delta(String),
    Suspend,
    PhaseChange(WorkflowPhase),       // new
}
```

`run_with_streaming` ([`run.rs:463`](../../crates/aictl-core/src/run.rs)) gains a `handle` arm for `PhaseChange` that forwards into the UI via a new `AgentUI::on_phase_change(phase: WorkflowPhase)` method. `PlainUI` and `InteractiveUI` implement it:

- `PlainUI`: no-op. Phase prefixes are CLI-REPL-only.
- `InteractiveUI`: updates a `Mutex<Option<WorkflowPhase>>` on the UI struct; the next time the REPL re-renders the prompt prefix (on `stream_end` or on the next prompt), it reads the updated phase. Visible result: `[explore]` flips to `[plan]` as soon as the model emits the tag, not on the next user prompt.

The `<phase>` tag is *also* still parsed post-stream by the REPL driver (the existing path in `crate::repl::run_and_display_turn` or wherever it lives) so the host's state machine stays consistent. The streaming wire is for UI immediacy, not for state-machine correctness.

### 8. Configuration

New key:

```
AICTL_CODING_PARALLEL_TOOLS_MAX=4      # cap on concurrent dispatches per batch
                                       # 0 disables parallel dispatch entirely
                                       # values >16 are clamped to 16
```

The same cap applies to non-coding sessions (parallel reads aren't coding-specific). Naming under `AICTL_CODING_` is a slight misnomer; we accept it for grouping with the rest of the coding-agent config block — multi-tool batching landed in Phase 4 of the coding agent rollout, and that's where users will look for the knob. A docstring on the constant points out the cross-cutting reach.

Setting `AICTL_CODING_PARALLEL_TOOLS_MAX=0` is the kill switch — the dispatch loop falls back to "first call only; reject the rest with a serialize message" so a problematic provider can be quarantined without re-rolling.

### 9. CLI surface

- No new flag. The `--info` banner gains one line: `parallel-tools: N` (showing the configured cap, or `disabled` when set to 0).
- The `[phase]` indicator now updates mid-stream as described in §7. No new commands.
- A short reasoning line ("running N tools in parallel…") fires once per batch, mirroring the existing single-call "running tool..." spinner.

### 10. Desktop surface

- The toolbar's existing "running tool…" spinner becomes "running N tools…" when N > 1. Wired via a small extension to the IPC event the desktop already listens for (`tool_start` → `{ count: usize }`).
- No new toolbar icon, no Settings field.

### 11. Runtime shape and integration points

| File | Change |
|------|--------|
| `crates/aictl-core/src/tools.rs` | Add `parse_tool_calls`; add `SIDE_EFFECT_TOOLS` const + `is_parallelizable` helper |
| `crates/aictl-core/src/run.rs` | Replace single-call branch with batch-aware dispatch; add `dispatch_parallel`; refactor `handle_tool_call` body into a `run_single_call` callable inside a `JoinSet` task |
| `crates/aictl-core/src/llm/stream.rs` | Extend `StreamState` to recognize `<phase>…</phase>` tags; emit `StreamEvent::PhaseChange` |
| `crates/aictl-core/src/ui.rs` (`AgentUI`) | Add `fn on_phase_change(&self, phase: WorkflowPhase)` default-no-op |
| `crates/aictl-cli/src/ui.rs` | `InteractiveUI::on_phase_change` updates the prompt-prefix state; `PlainUI::on_phase_change` is the default no-op |
| `crates/aictl-core/src/config.rs` | Add `AICTL_CODING_PARALLEL_TOOLS_MAX`; revise `SYSTEM_PROMPT` and `SYSTEM_PROMPT_CODING` with the multi-tool batching paragraph |
| `crates/aictl-core/src/audit.rs` | None — per-call audit already keyed off `ToolCall`, parallel dispatch logs each call independently |
| `crates/aictl-cli/src/commands/info.rs` | One new banner line |
| `crates/aictl-desktop/webview/src/components/ToolStatus.tsx` (or similar) | Switch spinner text from "running tool…" to "running N tools…" when N > 1 |
| `README.md` | Phase 4 bullet under Coding-agent mode (one line) |
| `CLAUDE.md` | Update existing coding-mode paragraph: "Phase 4: parallel read-only tool dispatch via `tokio::JoinSet`, capped at `AICTL_CODING_PARALLEL_TOOLS_MAX`; mid-stream `<phase>` tag updates the REPL prefix in real time." |
| `ROADMAP.md` | Remove the Phase 4 bullet once shipped |

### 12. Testing

**Unit tests**

- `parse_tool_calls`:
  - Empty response → `[]`.
  - Single call → one entry, identical name + input to `parse_tool_call`.
  - Three reads in source order → three entries in source order.
  - Mixed well-formed + malformed → returns the well-formed; `looks_like_malformed_tool_call` still fires on the broken one.
- `is_parallelizable`:
  - Every name in `SIDE_EFFECT_TOOLS` → false.
  - `git` with `status` / `log` / `blame` / `diff` → true; with `commit` → false.
  - `clipboard` with `read` → true; with `write` → false.
  - MCP `mcp__foo__bar` → false.
  - Plugin tool → false.
- `dispatch_parallel` (with mock tools that sleep different durations):
  - Three tools sleeping 100 / 200 / 50 ms complete in ~200 ms wall-clock (not 350).
  - Results in `<tool_results>` body match source order regardless of completion order.
  - Cap of 2 splits 5 tools into chunks 2/2/1; total wall-clock is max(chunk1) + max(chunk2) + max(chunk3).
  - One denied tool (security gate fails) doesn't block siblings; the denial message appears in its `<tool_result>` block.
- `StreamState` `<phase>` recognition:
  - Tag arriving in one chunk emits `PhaseChange(Plan)`.
  - Tag split across two chunks (`<pha` then `se>plan</phase>`) still emits once.
  - Tag with an unknown label (`<phase>refactor</phase>`) emits nothing (matches `WorkflowPhase::parse_tag` rejection).

**Integration tests** (CLI mock-LLM harness)

- Parallel batch fixture: mock provider emits three `<tool name="read_file">` blocks in one response; verify the loop produces one `<tool_results>` user turn with three blocks, and the next LLM call sees that turn.
- Partial-rejection fixture: mock provider emits one `edit_file` plus two `read_file`s; verify only `edit_file` runs and the two `read_file`s have rejection messages in their result blocks.
- Mid-stream phase update fixture: mock provider streams `<phase>code</phase>` followed by prose, then the response ends; verify the REPL's prompt-prefix state updates before the next prompt is rendered.

**Manual smoke**

1. Ask the agent to "read these five files and summarize" with five specific paths. Inspect the trace: one LLM call, one batch dispatch, five concurrent reads, one `<tool_results>` injection. Compare wall-clock against `AICTL_CODING_PARALLEL_TOOLS_MAX=1` (serial baseline).
2. Ask the agent to "edit foo.rs and read bar.rs in the same response". Verify the host rejected the read, ran the edit, and the model's next turn re-emits the read alone.
3. Set `AICTL_CODING_PARALLEL_TOOLS_MAX=0` and repeat (1). Verify the model still gets results but only the first read runs, with rejections for the others. (Confirm this is the intended kill-switch behavior — see Open Questions; an alternative is "serialize all into one chunk of size 1".)
4. In the REPL, ask for a phased workflow and watch `[explore]` → `[plan]` → `[code]` transitions land mid-stream rather than on prompt boundaries.

**CI gates**

```bash
# Phase 4 symbols stay out of the server crate.
grep -rE 'parse_tool_calls|dispatch_parallel|PhaseChange' crates/aictl-server/src/  # must be empty

# Parallel cap clamp is enforced in code, not just docs.
grep -E 'PARALLEL_TOOLS_MAX' crates/aictl-core/src/config.rs
```

## Rollout phases

Three independent PRs, in this order (each depends on the prior):

1. **`parse_tool_calls` + side-effect classifier** — pure parsing/classification, zero behavioral change because the dispatch loop still picks `calls[0]` until PR 2.
2. **Batch dispatch via `JoinSet`** — flips the loop from single-call to batch; tests above gate the merge. Touches the agent loop's largest function but the new code is concentrated in `dispatch_parallel` + a small refactor of `handle_tool_call`.
3. **Mid-stream phase updates** — the smallest change, isolated to the streaming pipe and one new `AgentUI` method. Lands last to avoid coupling its UX with the dispatch-loop test surface.

PR 1 is mergeable without PRs 2 / 3 (no behavioral change). PRs 2 / 3 are independent of each other after PR 1 lands.

## Verification

Phase 4 sign-off requires:

1. `cargo build --workspace` clean on default features and `--all-features`.
2. `cargo lint` clean.
3. `cargo test` clean including new unit + integration tests.
4. Wall-clock improvement demonstrated on the multi-read smoke: N parallel reads finish in ~max-read time, not sum-of-read times.
5. `AICTL_CODING_PARALLEL_TOOLS_MAX=1` produces today's serial behavior bit-for-bit (regression).
6. Manual smoke checklist above.
7. CI grep gates pass.

## Risks

- **MCP tool side-effect surface unknown**: classifying all `mcp__*` as not-parallelizable is safe but conservative. Mitigation: leave it conservative in v1; a Phase 4.5 plan can add an MCP capability bit (`safe_to_parallelize: true` in the tool's input schema metadata) to opt in. The current shape never silently parallelizes something dangerous.
- **Approval UX confusion**: a single "approve N reads?" prompt could mislead a user who expects per-call confirmation. Mitigation: the prompt shows all N tool names; an explicit "review individually" option drops back to serial per-call confirmation for the batch. Default to bundled — most users want speed.
- **Memory pressure under wide batches**: spawning 16 concurrent `read_file`s on a low-memory machine could spike. Mitigation: the cap defaults to 4 (small enough to be safe everywhere we ship), is configurable up to 16, and the file-size cap on each read (existing `MAX_TOOL_OUTPUT_LEN`) bounds per-call memory anyway.
- **Reordering of `<tool_results>` confuses the model**: we sort by source order before joining, but a model that *expects* completion-order signals could be confused. Mitigation: the prompt makes the contract explicit — "results return in the order you emitted the calls"; the source-order sort enforces that contract.
- **Mid-stream phase update flicker**: the `[phase]` prefix flipping mid-token-stream could be visually jarring. Mitigation: the prefix only updates between *lines* (the InteractiveUI prompt is on its own line and only repaints when the prompt is drawn — the indicator changes before the next prompt, not in the middle of a streamed paragraph). The `<phase>` tag is at the *start* of a turn by convention, so this is the natural seam.
- **PreToolUse hook semantics under parallel**: a hook that depends on observing the previous tool's PostToolUse before deciding on the next call wouldn't see that ordering in a batch. Mitigation: hooks remain per-call but they no longer enforce "I saw the previous call first" — document this in `CLAUDE.md` and the hooks README. Side-effect calls remain serialized; the constrained case (hook needs cross-call observation of mutations) still works.
- **Audit log interleaving**: per-call audit entries from a batch land in the JSONL file in completion order, not source order. The session ID + timestamp let readers reconstruct ordering; mitigation is documentation, not code.

## Scope boundaries with other plans

- **Phase 1 (`coding-agent.md`)**: prerequisite. `WorkflowPhase` is reused; the `<phase>` tag parser ([`coding.rs:67`](../../crates/aictl-core/src/coding.rs)) is exactly the regex the streaming sink needs.
- **Phase 2 (`coding-agent-phase-2.md`)**: orthogonal. The smarter `edit_file` and ripgrep-backed search benefit Phase 4 (faster reads in parallel = even faster Explore bursts).
- **Phase 3 (`coding-agent-phase-3.md`)**: complementary. The structured Review hook (`run_structured_review`) can opportunistically `JoinSet` its build + lint reads via the same helper this plan introduces. A small refactor in Phase 3's PR after Phase 4 lands turns its sequential awaits into a parallel block. Not a Phase 4 deliverable but a low-cost follow-up.
- **Java/Kotlin/C detection (`coding-agent-detect-jvm-c.md`)**: orthogonal. Augments `detect_linter` / `detect_test_cmd` — those run inside the host, not the agent loop.

## Open questions

- **Approval bundling default**: bundled-by-default (one prompt for N reads) or per-call-by-default (N prompts)? Lean bundled — the speed win is the entire point, and per-call confirmation re-introduces the bottleneck we're trying to remove. Provide an opt-out config (`AICTL_CODING_CONFIRM_PARALLEL=per-call`).
- **Kill-switch shape**: `AICTL_CODING_PARALLEL_TOOLS_MAX=0` could mean either (a) reject all batches and force the model back to serial, or (b) silently serialize each call. Lean (b) — strictly more compatible with what models actually emit. Document this.
- **Streaming `<phase>` updates for non-coding sessions**: skipped in v1 (gated on `coding_agent_enabled()`). Worth revisiting once we know whether any non-coding workflow uses phase tags for anything. Default no.
- **`tool_results` envelope shape**: the proposed shape is `<tool_results><tool_result name="..."></tool_result>...</tool_results>`. Alternative: a list of standalone `<tool_result>` user turns in a single message. Lean the wrapped shape — keeps the response shape symmetric with the model's batched emission and simplifies parsing for downstream tooling.
- **Cap default**: 4 is a guess. Worth measuring the throughput vs. memory curve on a real-world Explore burst (e.g. "read every file in src/") before locking in. Bump to 8 if benchmarks justify it.
- **Should `test` and `exec_shell` ever parallelize with each other in a side-effect-pair batch**: emphatically no — these are the most likely to step on each other (shared `target/`, shared `node_modules/`). The classifier rules out the case explicitly.
