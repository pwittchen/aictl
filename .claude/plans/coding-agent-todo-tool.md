# Spec: Todo / plan tool

> Report item **5** (Tier 2). Small, self-contained, and the cheapest fix for the Plan phase's
> biggest weakness.

## Context

The coding prompt's Plan phase produces prose. Prose in the transcript decays: three tool
calls later the plan is 8k tokens back, and after the context-management spec's tier-1
eviction it may be a stub. The model forgets step 4 of its own plan and declares success at
step 3 — the single most common failure mode in the transcripts that motivated the gap report.

Meanwhile the CLI already renders a `[phase]` prefix
([`coding.rs:26`](../../crates/aictl-core/src/coding.rs), `WorkflowPhase`) with nothing behind
it but the model's self-reported tag. There is a display surface with no state to display.

Structured, re-injected todo state is a large part of why dedicated agents stay coherent
across 40-step tasks: the list is short, it is rewritten into the prompt every turn so it
never decays, and crossing an item off is an explicit action the model must take rather than
a claim it can drift on.

## Goals & Non-goals

**Goals**

- A `todo` tool that writes structured task state the host holds outside the transcript.
- Re-injection of the current list into every subsequent turn so it cannot decay.
- Exactly one item `in_progress` at a time, enforced by the host.
- CLI rendering of the list next to the existing phase indicator.
- Session-scoped persistence so `/session` resume restores the list.
- Prompt guidance that makes the model use it for multi-step work and *not* for single-step
  work.

**Non-goals**

- No cross-session or cross-project todo store. This is working memory for one task, not a
  task manager. (Long-term facts already have `memory.rs`.)
- No dependency graph, no priorities, no due dates, no assignees.
- No user-side editing of the list at v1 beyond clearing it.
- No desktop rendering at v1 — same call as the phase indicator, which is CLI-only.
- No enforcement that the model actually completes items. The host tracks; it does not police.

## Design

### 1. State

```rust
// crates/aictl-core/src/todo.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TodoStatus { Pending, InProgress, Completed }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItem {
    pub id: u32,
    pub text: String,          // capped at TODO_MAX_TEXT (200) chars
    pub status: TodoStatus,
}
```

Held in a process-global `Mutex<Vec<TodoItem>>` alongside the other agent-loop state in the
engine (mirroring how `coding.rs` holds the changed-paths tracker). Capped at
`TODO_MAX_ITEMS = 20`; a write exceeding the cap is rejected with a message telling the model
to split the work rather than silently dropping items.

**Not in the transcript.** The list lives in host state and is rendered into the prompt fresh
each turn — that is the entire mechanism. A copy in the transcript would decay exactly like
the prose it replaces.

### 2. The tool

`TOOL_COUNT` 36 → 37 (or 38 if the subagent spec landed first).

Three subcommands, first token of the body:

```
<tool name="todo">
write
1. [in_progress] Add the --lines flag to read_file
2. [pending] Thread the flag through the security gate
3. [pending] Update docs/TOOLS.md
4. [pending] Add tests for the clamp behavior
</tool>
```

```
<tool name="todo">
complete 2
</tool>
```

```
<tool name="todo">
read
</tool>
```

- **`write`** replaces the whole list. Wholesale replacement rather than incremental edits is
  deliberate: it is one grammar to get right, it is idempotent, and it lets the model
  re-plan mid-task without a delete API. Each line is `N. [status] text`; the status bracket
  is optional and defaults to `pending`.
- **`complete <id>`** marks one item completed and promotes the next `pending` item to
  `in_progress`. This is the common case and deserves a one-token call.
- **`read`** returns the current list (rarely needed, since it is in the prompt — included so
  the model has a recovery path).

Result of every subcommand is the rendered list, so the model always sees the post-state.

Host-enforced invariants on `write`:

- At most one `in_progress`. Extras beyond the first are demoted to `pending` and the result
  says so.
- If nothing is `in_progress` and something is `pending`, the first pending item is promoted.
  A list where the model forgot to mark anything in progress is the common case and silently
  fixing it beats an error.
- IDs are renumbered `1..n` on every write. The model's ids are advisory.

### 3. Re-injection

`run::build_system_prompt_with` appends, when the list is non-empty **and** coding-agent mode
is on:

```
<todo>
1. [x] Add the --lines flag to read_file
2. [>] Thread the flag through the security gate
3. [ ] Update docs/TOOLS.md
4. [ ] Add tests for the clamp behavior
</todo>
```

Compact markers (`[x]` / `[>]` / `[ ]`) rather than words — this block is paid for on every
call and the list can be 20 items.

Placement: after `<repo_context>`, immediately before any agent persona block. It is the most
volatile part of the prompt and belongs last among the host-injected blocks, which also keeps
the cacheable prefix stable for prompt caching.

### 4. Prompt guidance

Added to `SYSTEM_PROMPT_CODING` (coding mode only):

```
TASK LIST. For any task that takes more than two or three tool calls, call
`todo write` at the end of your Plan phase with the concrete steps. Call
`todo complete <id>` the moment a step is actually done — not when you
intend to do it. Your current list is injected above on every turn; it is
the only record that survives, so keep it accurate.

Do not use the task list for single-step work, and do not add steps like
"understand the code" or "verify the change" — steps are things that
produce a diff or a decision.
```

The last sentence is doing real work: without it, models produce five-item lists where three
items are process narration.

### 5. Lifecycle

- **Cleared** at the start of every new user turn in the REPL — a new user prompt is a new
  task. `run_agent_turn` already calls `tools::clear_call_history()` at that seam;
  `todo::clear()` goes next to it, gated on coding mode.

  Exception: if the previous turn ended with incomplete items **and** the new user message is
  short and continuation-shaped (`"continue"`, `"go on"`, `"keep going"`, `"yes"`), the list
  is preserved. Heuristic, deliberately narrow, and the `/todo` command can restore it if the
  heuristic is wrong.
- **Persisted** with the session. `session::save_messages` gains a sibling call so
  `/session` resume restores the list. The file is `~/.aictl/sessions/<id>.todo.json`, kept
  separate so the message-file format is untouched. It passes through
  `redaction::redact_for_persistence` like every other write of model-authored text.
- **Not** carried into subagents. A child gets a clean slate; its findings come back as an
  answer, and the parent updates its own list.

### 6. CLI surface

- The REPL prompt prefix becomes `[code 2/4]` — phase plus todo progress — when a list is
  active. Falls back to `[code]` when empty. This is why the todo tool pairs with the phase
  indicator: the prefix finally has something quantitative in it.
- After each turn, if the list changed, the REPL prints a compact render:
  ```
    ✓ 1. Add the --lines flag to read_file
    ▸ 2. Thread the flag through the security gate
      3. Update docs/TOOLS.md
      4. Add tests for the clamp behavior
  ```
  Suppressed under `--quiet` and in `text`/`json` formats.
- `/todo` slash command: bare shows the list; `/todo clear` empties it. No editing at v1.
- `--info` gains `todo: 2/4` when non-empty.

### 7. Integration points

| File | Change |
|------|--------|
| `crates/aictl-core/src/todo.rs` | **New** — state, invariants, rendering, persistence |
| `crates/aictl-core/src/tools/todo.rs` | **New** — body parsing and dispatch |
| `crates/aictl-core/src/tools.rs` | Register `todo`; bump `TOOL_COUNT`; add to `SIDE_EFFECT_TOOLS` (it mutates host state) |
| `crates/aictl-core/src/run.rs` | Inject `<todo>` in `build_system_prompt_with`; clear on new user turn |
| `crates/aictl-core/src/config.rs` | Prompt guidance; `AICTL_CODING_TODO` kill switch; catalogue entry |
| `crates/aictl-core/src/session.rs` | Save/restore the sidecar file |
| `crates/aictl-cli/src/repl.rs` | Prompt prefix with progress; post-turn render |
| `crates/aictl-cli/src/commands/todo.rs` | **New** — `/todo` |
| `crates/aictl-cli/src/commands.rs`, `help.rs` | Register the command |
| `crates/aictl-cli/src/commands/info.rs` | One line |
| `docs/TOOLS.md`, `docs/CODING_AGENT.md`, `docs/USAGE.md`, `CLAUDE.md` | Document |

### 8. Testing

**Unit**

- Parsing: numbered lines with and without status brackets; blank lines ignored; text over
  200 chars truncated; > 20 items rejected with the split message.
- Invariants: two `in_progress` → first kept, second demoted, result notes it; zero
  `in_progress` with pendings → first promoted; all-completed list stays as-is.
- `complete <id>`: valid id completes and promotes the next pending; unknown id errors
  without mutating; completing the last item leaves nothing in progress.
- Rendering: markers correct; empty list renders nothing (no empty `<todo>` block in the
  prompt).
- Clearing: new user turn clears; a continuation-shaped message preserves; the heuristic is
  case-insensitive and does not fire on a long message that merely starts with "continue".
- Persistence round-trip, including redaction of a todo item containing a secret-shaped
  string.

**Integration**

- Mock provider writes a 4-item list, then completes items 1 and 2 across turns; assert the
  system prompt on turn 3 contains `[x]` for 1–2 and `[>]` for 3.
- `AICTL_CODING_TODO=false` removes the tool from the catalogue and injects nothing.
- Non-coding mode: the tool is registered but nothing is injected (the model can call it; the
  host just does not steer with it).

**Eval**

The multi-file tasks (9–12, 16–17) are where this should move the number. Watch specifically
for the "declared done at step 3 of 4" failure — check `check.sh` failures where the answer
claims success.

### 9. Rollout

Two PRs:

1. **`todo.rs` state + tool + injection + prompt guidance.** Complete and useful on its own.
2. **CLI surface** — prompt prefix, post-turn render, `/todo`, `--info`, session persistence.

### 10. Verification

1. `cargo build --workspace`, `cargo lint`, `cargo test` clean.
2. A manual four-step task shows the prefix advancing `[code 1/4]` → `[code 4/4]`.
3. `/session` resume restores an in-progress list.
4. `AICTL_CODING_TODO=false` leaves the prompt byte-identical to today.
5. Eval comparison on the multi-file tasks.
6. CI gate: `grep -rE 'todo::' crates/aictl-server/src/` empty.

### 11. Risks

- **Prompt bloat.** 20 items at ~15 tokens each is ~300 tokens per call. Real but small
  against a 4KB tool catalogue. Mitigation: the cap; compact markers; the block is omitted
  entirely when empty.
- **Ceremony without benefit.** Models can produce a list and then ignore it, spending tokens
  for nothing. Mitigation: the "steps produce a diff or a decision" prompt line, and the eval
  is the check — if list-writing does not move pass rate on multi-step tasks, the feature is
  wrong and should be cut rather than tuned.
- **The clear heuristic mis-firing.** Preserving a stale list across an unrelated new task is
  worse than clearing one the user wanted kept. Mitigation: the heuristic requires a *short*
  continuation-shaped message; `/todo clear` is one command; and the post-turn render makes a
  stale list immediately visible.
- **Interaction with compaction.** The list is not in the transcript, so tier-1/2 eviction
  cannot touch it — which is the point, but it means a compacted session keeps its plan while
  losing its reasoning. That is the intended trade.
- **Prefix noise.** `[code 2/4]` in the REPL prompt on every line could feel busy. Mitigation:
  it only appears in coding mode with a non-empty list, which is exactly when it is
  informative.

### 12. Open questions

- **Should `todo write` be replace-only?** An `append` subcommand is tempting for
  incremental discovery mid-task. Lean no at v1 — replace is idempotent and re-planning is a
  healthy behavior to make cheap.
- **Should the host block a final answer with pending items** the way the Review hook blocks
  on lint failures? Tempting and probably too aggressive: sometimes the remaining items are
  genuinely the user's call. Lean: append a note to the answer banner
  (`[todo: 2 of 4 items remain]`) rather than looping. Revisit with eval data.
- **Desktop rendering.** A checklist in the composer would be genuinely nice and is a natural
  follow-up once the CLI shape is proven. Explicitly out of scope here, matching the Phase 4
  precedent for the phase indicator.
- **Should completion be inferred** from tool activity rather than requiring an explicit call?
  Inference is unreliable and the explicit call is a useful forcing function. No.
