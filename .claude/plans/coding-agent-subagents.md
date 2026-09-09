# Spec: Subagents / task delegation

> Report item **4** (Tier 2). Depends on the context-management spec (shares token accounting).
> Pairs with the todo tool.

## Context

`agents.rs` agents are **prompt appendices**, not workers. `read_agent` loads a markdown body
and `run::build_system_prompt` appends it to the system prompt; the agent runs in the main
conversation, in the main context window, for the rest of the session.

The single highest-leverage thing a dedicated coding agent does on long tasks is delegate:
push a search or an exploration into an isolated context, let it burn 60k tokens, and return
only the conclusion. The parent transcript pays for the answer, not the search.

Today an Explore phase that reads fifteen files leaves fifteen file bodies in the transcript
for the rest of the session. The context-management spec bounds and later elides them, but
elision loses the information; delegation never puts it in the parent transcript at all.

The primitive already exists. `run::run_agent_single`
([`run.rs:2047`](../../crates/aictl-core/src/run.rs)) builds a fresh `Vec<Message>`, runs one
`run_agent_turn`, records stats, and shows the answer. A `task` tool is that function with a
different UI seam and a returned string instead of a printed one.

## Goals & Non-goals

**Goals**

- A `task` tool that runs a nested agent turn in an isolated `Vec<Message>` and returns only
  its final answer to the parent as the tool result.
- Reuse of the existing agent catalogue as subagent personas: `task` accepts an optional
  agent name, resolved through `agents::read_agent` (local-first, same as `--agent`).
- Independent, bounded budgets for the child: its own iteration cap, its own token ceiling,
  its own timeout.
- Hard recursion bound — a subagent cannot spawn subagents at v1.
- A restricted tool surface for children: read-only by default, with write access opt-in.
- Full security-gate, audit, hook, and redaction coverage for every tool the child runs.
- Visible progress in the parent UI so a delegated search does not look like a hang.

**Non-goals**

- No parallel subagents at v1. One child at a time, dispatched like any other side-effect
  tool. (Concurrent children multiply the approval, cancellation, and cost-attribution
  problems; defer until the serial case is proven.)
- No child→parent streaming. The child's answer arrives whole.
- No persistent subagent sessions. Each `task` call is a fresh transcript, discarded after.
- No separate model per child at v1 (a cheap-model-for-search config is an obvious follow-up
  and is called out in Open questions).
- No desktop-specific UI beyond the existing tool-status surface.
- No server changes.

## Design

### 1. The tool

Registered as a built-in, taking `TOOL_COUNT` from 36 to 37.

Body grammar (line-oriented, matching every other tool):

```
<tool name="task">
--agent code-searcher
--tools read
Find every call site of `redact_outbound` outside run.rs and report the file,
line, and what it passes as the provider argument.
</tool>
```

- Optional leading `--agent <name>` — resolved via `agents::read_agent`, whose body becomes
  the child's persona block. Unknown name is an error result, not a silent fallback.
- Optional `--tools read|write` — default `read`. See §4.
- Everything after the flag lines is the child's prompt.

Result returned to the parent:

```
<task_result agent="code-searcher" llm_calls="7" tool_calls="12" tokens="41233">
… the child's final answer verbatim …
</task_result>
```

The counters are in the envelope deliberately: the model should be able to see that a
delegation was expensive and calibrate the next one.

### 2. Execution

New `run::run_subagent`, a sibling of `run_agent_single` rather than a wrapper — the two
differ in enough places (UI seam, return value, budget source, tool restriction, hook
triggers) that wrapping would mean threading five flags through the parent function.

```rust
pub(crate) async fn run_subagent(
    provider: &Provider, api_key: &str, model: &str,
    prompt: &str,
    agent: Option<&AgentEntry>,
    tool_scope: ToolScope,
    ui: &dyn AgentUI,
) -> Result<SubagentResult, AictlError>;

pub(crate) struct SubagentResult {
    pub answer: String,
    pub llm_calls: u32,
    pub tool_calls: u32,
    pub usage: TokenUsage,
}
```

It builds `messages = vec![system]` where the system prompt is
`build_system_prompt_with(None)` plus the agent body plus a **delegation preamble**:

```
You are a subagent. You were given one specific task by a parent agent and
you cannot ask follow-up questions. Do the task with the tools available,
then answer with the findings only — no preamble, no offer to continue.
Your answer is the entire product; everything else you do is discarded.
Be specific: file paths, line numbers, exact strings.
```

That preamble is load-bearing. Without it models end subagent runs with "Would you like me
to fix these?", which the parent then has to interpret.

The child then runs `run_agent_turn` with its own budget (§3) and the result is
`turn.answer`.

### 3. Budgets

The child gets its own limits, independent of the parent's:

| Key | Default | Meaning |
|-----|---------|---------|
| `AICTL_SUBAGENT_MAX_ITERATIONS` | `15` | Iteration cap for a child turn |
| `AICTL_SUBAGENT_MAX_TOKENS` | `100000` | Abort the child when accumulated input tokens exceed this |
| `AICTL_SUBAGENT_TIMEOUT_SECS` | `300` | Wall-clock cap on the whole child run |
| `AICTL_SUBAGENT_MAX_PER_TURN` | `5` | Max `task` calls the parent may make in one turn |
| `AICTL_SUBAGENT_ENABLED` | `true` in coding mode, `false` otherwise | Master switch |

`max_iterations()` is process-global today; the child cannot simply read it. `run_agent_turn`
gains an optional `TurnBudget` parameter (defaulting to today's values from config) so the
child can be handed different limits without a global. This is the same seam the loop-budget
spec extends, so coordinate: **if the loop-budget spec lands first, `TurnBudget` already
exists and this spec just constructs a different one.**

Exceeding a budget is not an error — the child returns whatever it has with a note appended:

```
<task_result … truncated="iterations">
(subagent hit its 15-iteration budget; the findings below may be incomplete)
…
```

Recursion: `run_subagent` sets a task-local depth flag; the `task` tool checks it and returns
an error result at depth ≥ 1. Depth is tracked with a `tokio::task_local!` rather than a
global, so a future parallel-children change does not break it.

### 4. Tool scope

```rust
pub(crate) enum ToolScope {
    /// Read-only: every tool for which `tools::is_parallelizable` is true,
    /// plus `test`. No writes, no shell, no MCP, no plugins, no nested task.
    Read,
    /// Everything the parent can call except `task` itself.
    Write,
}
```

`Read` reuses `tools::is_parallelizable` deliberately — it is already the codebase's audited
answer to "does this tool have side effects?", and having two divergent definitions of
read-only would be a security bug waiting to happen. `test` is added explicitly because it is
classified side-effecting (it spawns a process) but is exactly what a "check whether this
theory holds" subagent needs.

Scope is enforced in `tools::execute_tool` via a task-local scope value, checked immediately
after the existing security gate. A scope violation returns a normal denial result, is
audited as `DeniedByPolicy`, and does not abort the child.

`--tools write` requires an explicit parent-side confirmation (§6) even when `*auto` is on,
unless `AICTL_SUBAGENT_ALLOW_WRITE=true`. An auto-approved agent spawning a write-capable
child that then auto-approves its own writes is a two-step escalation the user never saw.

### 5. Prompt guidance

Added to `SYSTEM_PROMPT_CODING` only (delegation is a coding-workflow discipline; the general
prompt keeps its current shape):

```
DELEGATION. Use the `task` tool to push a bounded, read-heavy investigation
into a subagent when the answer is small but finding it is expensive —
"which files reference X", "does this pattern appear anywhere else",
"summarize what this module does". The subagent's tool output never enters
your context; only its answer does.

Do NOT delegate: anything requiring your accumulated context, anything you
can answer with one or two reads, or the actual editing. Delegate the
search, not the decision.
```

The "delegate the search, not the decision" line is the one that matters. Models over-delegate
when told delegation is cheap.

### 6. Approval and UI

`task` is a side-effect tool for dispatch purposes (never parallelized, never batched), so it
flows through `run::handle_tool_call` and the normal confirmation path. The confirm prompt
shows the agent name, the scope, and the prompt's first line.

During the child run the parent UI shows a spinner with the child's tool activity summarized:
`subagent(code-searcher): read_file × 4, search_files × 2…`. Implemented by passing the parent
`ui` into `run_subagent` wrapped in a `SubagentUI` adapter that:

- swallows `show_answer` (the answer is the tool result, not UI output),
- swallows `stream_chunk` (no child streaming at v1),
- forwards `show_reasoning` at reduced prominence,
- counts tool calls and updates the parent spinner,
- **auto-approves** child tool calls within the granted scope (the scope *is* the approval;
  a second prompt per child tool call would defeat the purpose).

Esc cancellation: `with_esc_cancel` wraps the child run, so Esc aborts the subagent and
returns a partial result to the parent rather than killing the parent turn.

### 7. Accounting

Child usage is **added to the parent's `TurnResult.usage`** so `/stats`, the cost meter, and
the turn summary reflect the true spend. `stats::record` is called once, by the parent, with
the merged totals — calling it in both places would double-count.

The audit log records the `task` dispatch itself, and every child tool call, tagged with a
`subagent: <name>` field so a reader can attribute them.

### 8. Integration points

| File | Change |
|------|--------|
| `crates/aictl-core/src/tools/task.rs` | **New** — body parsing, agent resolution, scope parsing, envelope construction |
| `crates/aictl-core/src/tools.rs` | Register `task`; `TOOL_COUNT` 36 → 37; add to `SIDE_EFFECT_TOOLS`; scope check after the security gate |
| `crates/aictl-core/src/run.rs` | `run_subagent`; `SubagentResult`; `TurnBudget` parameter on `run_agent_turn`; usage merge |
| `crates/aictl-core/src/ui.rs` | `SubagentUI` adapter (engine-side, wraps `&dyn AgentUI`) |
| `crates/aictl-core/src/agents.rs` | No change — `read_agent` is reused as-is |
| `crates/aictl-core/src/config.rs` | Five new keys; delegation paragraph in `SYSTEM_PROMPT_CODING`; `task` entry in the tool catalogue |
| `crates/aictl-core/src/security.rs` | `validate_tool` arm for `task` (prompt-injection scan on the child prompt) |
| `crates/aictl-core/src/audit.rs` | `subagent` attribution field |
| `docs/TOOLS.md`, `docs/CODING_AGENT.md`, `docs/CONFIG.md`, `CLAUDE.md` | Document the tool, scopes, and budgets |

### 9. Testing

**Unit**

- Body parsing: flags in either order; missing flags default correctly; prompt-only body
  works; unknown `--agent` errors; unknown `--tools` value errors.
- `ToolScope::Read` rejects `write_file`, `exec_shell`, `mcp__*`, plugin tools, and `task`;
  admits `read_file`, `search_files`, `test`.
- Recursion guard: a `task` call inside a subagent returns the depth error.
- Budget truncation: a child hitting its iteration cap returns `truncated="iterations"` with
  the partial answer, not an `Err`.
- Envelope: counters reflect the child's actual calls.

**Integration** (mock-LLM harness)

- Parent emits `task`; the mock child emits two reads then an answer; assert the parent
  transcript contains **only** the `<task_result>` envelope — no child tool results, no child
  reads.
- Usage merge: parent `TurnResult.usage` equals parent-only usage plus child usage.
- Scope enforcement end-to-end: a `--tools read` child attempting `write_file` gets a denial
  result and continues.
- Write scope requires confirmation when `*auto` is on and
  `AICTL_SUBAGENT_ALLOW_WRITE` is unset.
- Esc during a child run yields a partial `<task_result>` and a live parent turn.

**Eval**

Add two tasks to the harness: a repo large enough that answering requires ≥8 file reads, run
with and without `AICTL_SUBAGENT_ENABLED`. The metric is parent-transcript token count at the
end of the run, not just pass/fail.

### 10. Rollout

1. **`TurnBudget` parameter on `run_agent_turn`** — pure refactor, defaults reproduce today.
   (Skip if the loop-budget spec already landed it.)
2. **`run_subagent` + `SubagentUI` + scope enforcement**, no tool registered yet. Testable
   through the integration harness by calling it directly.
3. **The `task` tool + prompt guidance + docs.** Flips the feature on.

### 11. Verification

1. `cargo build --workspace`, `cargo lint`, `cargo test` clean.
2. `AICTL_SUBAGENT_ENABLED=false` removes `task` from the catalogue and from
   `native_catalogue()` — the model never sees it.
3. Manual: a delegated "find every call site" task on this repo returns file:line results and
   leaves nothing but the envelope in the parent transcript (verify with `/context`).
4. Audit log shows child tool calls attributed to the subagent.
5. Cost in `/stats` after a delegated turn matches the sum of parent + child.
6. CI gate: `grep -rE 'run_subagent|ToolScope' crates/aictl-server/src/` empty.

### 12. Risks

- **Over-delegation.** Models delegate trivially-answerable questions, doubling latency and
  cost for no gain. Mitigation: the "delegate the search, not the decision" prompt line;
  `AICTL_SUBAGENT_MAX_PER_TURN` caps it; the envelope's visible counters give the model
  feedback. Watch the eval tool-mix data for `task` calls with < 3 child tool calls.
- **Cost opacity.** A child can spend more than the whole parent turn. Mitigation: merged
  accounting means `/stats` tells the truth; the spinner shows live child activity; the
  token budget is a hard stop.
- **Scope escalation.** A write-capable child under an auto-approving parent is a real
  privilege escalation. Mitigation: `--tools write` needs explicit confirmation regardless of
  `*auto`, gated by its own config key.
- **Prompt injection via delegated content.** A child reading a hostile file could emit an
  answer engineered to steer the parent. The child's answer enters the parent transcript as a
  tool result — the same trust level as any other tool output, and
  `security::detect_prompt_injection` already covers that seam. Worth stating explicitly in
  docs rather than treating as new.
- **Cancellation leaving orphan work.** Esc must not leave a child's `tokio` tasks running.
  Mitigation: the child runs inside the parent's task tree; `with_esc_cancel` dropping the
  future cancels it. Verify with a long-running child.
- **Recursion guard via task-local.** If a future change spawns the child on a detached
  runtime task, the task-local does not propagate and the guard silently fails. Mitigation:
  a unit test that asserts depth propagation, and a comment at the spawn site.

### 13. Open questions

- **Cheap model for children.** A `AICTL_SUBAGENT_MODEL` key pointing search subagents at a
  fast, cheap model is the obvious next step and probably where most of the economic win is.
  Deferred to keep v1's failure surface small; revisit immediately after the eval baseline.
- **Parallel children.** Two independent searches at once is a natural fit for the existing
  `JoinSet` batching. Deferred: cost attribution, cancellation, and approval all get harder.
- **Should `task` results be persisted to the session?** Yes as written (they are a normal
  tool result), but a very long child answer competes with the context it was meant to save.
  The context-management budget applies to it like any result — confirm the budget for `task`
  is generous (it is a summary, not a log) rather than the base value.
- **Reusing the agent catalogue as personas** assumes catalogue agents read well as subagent
  instructions. Many are written as "you are a persona for a conversation". May need a
  `subagent: true` frontmatter marker and a separate section in `/agent`.
