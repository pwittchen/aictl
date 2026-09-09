# Spec: Loop budget and mid-turn steering

> Report item **7** (Tier 2, "two smaller loop issues"). Shares the `TurnBudget` seam with the
> subagent spec and the accounting from the context-management spec. Sequenced last because
> both halves are more useful once context pressure is handled.

## Context

Two independent problems in the agent loop, both in
[`run.rs`](../../crates/aictl-core/src/run.rs).

**1. The iteration cap is the wrong unit.** `config::DEFAULT_MAX_ITERATIONS = 20`
([`config.rs:53`](../../crates/aictl-core/src/config.rs)) bounds LLM calls per turn.
`run_agent_turn` loops `for llm_calls in 1..=iter_bound` and returns
`AictlError::MaxIterations` when it runs out; `AICTL_MAX_ITERATIONS=0` disables the cap
entirely.

Twenty is low for real coding work — Phase 4's parallel reads help, but a task touching six
files with a test loop routinely needs more. And the two available settings are both wrong:
20 truncates real tasks mid-edit, and `0` removes the runaway guard completely. The cap
exists to stop a loop burning money, but it counts the wrong thing: a runaway loop of twenty
cheap calls is fine, and twenty calls each carrying a 300KB transcript is not.

The eval harness makes this visible — a `20t` entry in the result table is a task that hit the
cap, and the report predicts several.

**2. No way to steer a running turn.** The only mid-turn control is
`run::with_esc_cancel` ([`run.rs:406`](../../crates/aictl-core/src/run.rs)), which throws the
entire turn away. A user who sees the agent heading the wrong way at call 6 of 15 has two
options: watch it finish, or destroy fourteen calls of correct work. What they want is "no,
use the async version" — a redirect, not an abort.

## Goals & Non-goals

**Goals**

- Replace the raw iteration count with a composite `TurnBudget`: iterations, tokens, cost, and
  wall-clock, whichever binds first.
- Budget exhaustion becomes a **graceful stop** — the model gets one final call to summarize
  where it is — rather than a bare error.
- Sensible defaults that let a real coding task finish while still bounding a runaway.
- A steering queue: typing during a turn enqueues a message that is injected at the next
  iteration boundary instead of aborting.
- Esc keeps its current meaning (abort). Steering is additive.
- Clear UI: the user must know the message was queued and when it landed.

**Non-goals**

- No mid-call interruption. A steering message lands between LLM calls, not inside one.
- No steering of tool execution in progress.
- No per-tool budgets.
- No budget enforcement in `aictl-server` (no agent loop there).
- No desktop steering UI at v1 — the desktop composer would need its own affordance; the
  budget half applies to it automatically.

## Design — part 1: `TurnBudget`

### 1. The type

```rust
// crates/aictl-core/src/run.rs
#[derive(Debug, Clone)]
pub struct TurnBudget {
    /// Max LLM calls. `None` = unbounded.
    pub max_iterations: Option<usize>,
    /// Max cumulative input+output tokens across the turn. `None` = unbounded.
    pub max_tokens: Option<u64>,
    /// Max estimated USD spend for the turn. `None` = unbounded.
    pub max_cost_usd: Option<f64>,
    /// Max wall-clock. `None` = unbounded.
    pub max_wall_clock: Option<Duration>,
}

impl TurnBudget {
    /// From config, honoring AICTL_MAX_ITERATIONS and the new keys.
    pub fn from_config() -> Self;
    /// Which limit (if any) this turn has crossed.
    pub fn exceeded(&self, s: &TurnState) -> Option<BudgetLimit>;
}
```

`run_agent_turn` takes `Option<&TurnBudget>`, defaulting to `TurnBudget::from_config()`. That
is the same parameter the subagent spec needs to hand children different limits — **build it
once**, in whichever spec lands first.

Cost uses the existing `TokenUsage::estimate_cost(model)`
([`llm.rs:792`](../../crates/aictl-core/src/llm.rs)), which already handles cache-read
multipliers. For models with no pricing data `estimate_cost` returns `None` and the cost limit
simply does not bind — stated in the docs rather than silently treated as zero.

### 2. Defaults

| Key | Default | Rationale |
|-----|---------|-----------|
| `AICTL_MAX_ITERATIONS` | `40` (was 20) | Raised, but still finite. Real coding tasks need more than 20; a runaway is caught by the other three limits well before 40 expensive calls |
| `AICTL_MAX_TURN_TOKENS` | `500000` | The real runaway guard. ~5 full 100k-token calls |
| `AICTL_MAX_TURN_COST_USD` | `2.00` | The limit users actually care about |
| `AICTL_MAX_TURN_SECONDS` | `900` | Catches a stuck tool loop |

`0` disables any individual limit, preserving today's `AICTL_MAX_ITERATIONS=0` semantics.

Raising the iteration default from 20 to 40 is safe **only because** the token and cost limits
now exist. Landing the raise without them would be a regression in the runaway guard, so the
two must ship together.

### 3. Graceful stop

Today: `Err(AictlError::MaxIterations)`, and the user gets an error with no summary of what
the agent did.

New behavior — when `exceeded()` fires, the loop does **not** immediately return. It makes one
final LLM call with a synthetic user turn appended:

```
<budget_exhausted limit="cost" detail="turn cost $2.03 exceeded the $2.00 budget">
Stop working and answer now. Summarize: what you completed, what you did
not, what the exact next step is, and any file left in a partial state.
Do not call any more tools.
</budget_exhausted>
```

The final call is made with tools **withheld** (empty `tools[]` on the native path; a prompt
instruction on the XML path, plus the host discarding any tool call the model emits anyway),
so it cannot extend the loop. Its answer is returned with a banner:

```
[budget: stopped after 41 calls / $2.03 — see summary below]
```

`AictlError::MaxIterations` is retained for the case where the final call itself fails.

This matters most with the todo tool: the summary is much better when there is a structured
list to report progress against, which is why these two specs are pleasant neighbors.

### 4. Visibility

- `--info` gains `budget: 40 calls / 500k tok / $2.00 / 900s`.
- The turn summary already prints calls and cost; it gains a budget-utilization note when a
  turn used more than half of any limit: `(used 61% of turn budget)`.
- At 80% of any limit the UI emits a one-shot reasoning line: `approaching turn budget (82%
  of cost limit)` — so a long turn is not a surprise.

## Design — part 2: steering

### 5. The queue

```rust
// crates/aictl-core/src/steering.rs
/// Enqueue a message to be injected at the next iteration boundary.
/// Returns false when the queue is full (cap: 3).
pub fn enqueue(msg: String) -> bool;
/// Drain everything queued. Called by the agent loop between iterations.
pub fn drain() -> Vec<String>;
pub fn pending() -> usize;
```

A `Mutex<VecDeque<String>>` in the engine. The engine owns it (not the CLI) because the
injection point is in `run_agent_turn` and the desktop will want the same seam later.

### 6. Injection

At the top of each loop iteration, before the provider call:

```rust
for msg in steering::drain() {
    messages.push(Message {
        role: Role::User,
        content: format!("<user_steering>\n{msg}\n</user_steering>"),
        images: vec![],
    });
    ui.show_reasoning(&format!("steering: {msg}"));
}
```

Injected as a real user turn — it *is* the user talking — wrapped so the model can tell it
from the original prompt. Prompt guidance in both system prompts:

```
A <user_steering> block is the user speaking to you mid-task. It takes
priority over your current plan. Adjust immediately: if it contradicts
what you are doing, stop doing that. Acknowledge it in one clause, do not
restart the task from scratch, and do not re-explain your plan.
```

Steering messages go through the **same guards as any user input**:
`security::detect_prompt_injection`, the `UserPromptSubmit` hook (which can block or rewrite
it), and `redact_outbound` at dispatch. A steering message is not a privileged channel.

### 7. CLI capture

`InteractiveUI` already runs a raw-mode key listener during a turn for Esc
(`with_esc_cancel`). It is extended:

- **Esc** — abort the turn (unchanged).
- **Any printable key** — enter steering-input mode: the spinner line is replaced by a
  `steer> ` prompt, the keystroke is its first character, and the user types a line. Enter
  enqueues; Esc cancels back to the spinner.
- The queued message is confirmed inline: `↳ queued (will apply at the next step)`, and when
  it lands: `↳ steering applied`.

The mechanics deserve care: the streaming renderer is writing to the same terminal. The
steering prompt claims the bottom line, streamed output continues above it, and the line is
redrawn after each flush — the same pattern `indicatif` already uses for a progress bar under
streaming output. If that proves visually unstable, the fallback is to buffer streamed output
while the steering prompt is open (a second or two of held output is acceptable; a garbled
terminal is not).

`AICTL_STEERING=false` disables capture entirely for users who find the keystroke hijack
surprising. Non-TTY and `--quiet` disable it automatically.

### 8. Interaction with the rest of the loop

- **Parallel batches**: steering drains between iterations, so an in-flight batch completes
  first. Correct — cancelling half a batch would leave the transcript with missing results.
- **Test-failure / review-result re-injection**: those already push synthetic user turns at
  the same boundary. Steering drains first, so a user redirect lands ahead of a host retry
  turn — the user's intent should win over the host's automatic retry.
- **Subagents**: steering does not reach a child. The child is a bounded delegation; the
  message applies when control returns to the parent. Keystrokes during a child run queue for
  the parent's next iteration.
- **Budget**: a steering message does not extend the budget. If the user steers into a bigger
  task they will hit the budget and get the graceful summary — at which point they can
  continue with a new turn.

### 9. Configuration

| Key | Default | Meaning |
|-----|---------|---------|
| `AICTL_MAX_ITERATIONS` | `40` | **Existing key, new default** |
| `AICTL_MAX_TURN_TOKENS` | `500000` | `0` disables |
| `AICTL_MAX_TURN_COST_USD` | `2.00` | `0` disables |
| `AICTL_MAX_TURN_SECONDS` | `900` | `0` disables |
| `AICTL_STEERING` | `true` | Mid-turn steering capture |
| `AICTL_STEERING_QUEUE_MAX` | `3` | Queued messages before rejection |

### 10. Integration points

| File | Change |
|------|--------|
| `crates/aictl-core/src/run.rs` | `TurnBudget`, `TurnState`, `BudgetLimit`; budget checks; graceful-stop final call; steering drain |
| `crates/aictl-core/src/steering.rs` | **New** — the queue |
| `crates/aictl-core/src/config.rs` | Four budget keys, two steering keys; both prompt paragraphs |
| `crates/aictl-cli/src/ui.rs` | Key listener extension; `steer>` prompt; queued/applied indicators |
| `crates/aictl-cli/src/commands/info.rs` | Budget line |
| `crates/aictl-desktop/src/` | Budget applies automatically; no steering UI at v1 |
| `docs/CONFIG.md`, `docs/USAGE.md`, `docs/CODING_AGENT.md`, `CLAUDE.md` | Document both halves |

### 11. Testing

**Unit**

- `TurnBudget::exceeded`: each limit binds independently; `0` disables; all-zero is unbounded;
  the first limit crossed is the one reported.
- `from_config`: defaults; `AICTL_MAX_ITERATIONS=0` still means unlimited iterations.
- Cost limit with a model lacking pricing data does not bind (and does not panic).
- `steering::enqueue`: cap enforced; `drain` empties; `pending` accurate; thread-safe under
  concurrent enqueue/drain.

**Integration** (mock-LLM harness)

- Budget exhaustion: a mock looping forever hits the iteration limit, the host makes exactly
  one more call with tools withheld, and the answer carries the banner. Assert the final call's
  request contains no tools.
- A tool call emitted during the final call is discarded, not dispatched.
- Token limit binds before the iteration limit when the mock reports large usage.
- Steering: enqueue between iterations 2 and 3; assert iteration 3's request contains the
  `<user_steering>` block, and that it is positioned before any host-injected `<test_failure>`
  turn queued the same iteration.
- A steering message that trips the injection guard is rejected without aborting the turn.
- `AICTL_STEERING=false`: keystrokes are ignored and Esc still aborts.

**Manual smoke**

1. Long task; type "actually use the sync API" mid-turn; verify the queue indicator, the
   applied indicator, and that the model adjusts without restarting.
2. Same, with streaming active — verify the terminal is not garbled (this is the risky one).
3. Esc still aborts cleanly with a steering message queued (queue is cleared on abort).
4. Set `AICTL_MAX_TURN_COST_USD=0.05` and run a real task; verify the graceful summary.

**Eval**

Re-run the full suite after the budget change and compare `20t`-style cap failures. This is
the direct measurement of whether the raise helps; watch median cost per task in the same
table to confirm the runaway guard still binds.

### 12. Rollout

Three PRs:

1. **`TurnBudget` + limits + graceful stop.** Self-contained, testable, and the piece the
   subagent spec depends on.
2. **`steering.rs` + engine injection + prompt guidance.** Testable through the integration
   harness by enqueuing programmatically — no terminal work yet.
3. **CLI key capture + indicators.** The terminal-interaction risk is isolated here and can be
   reverted without losing the engine seam.

### 13. Verification

1. `cargo build --workspace`, `cargo lint`, `cargo test` clean.
2. Setting all four budget keys to today's values (`AICTL_MAX_ITERATIONS=20` plus zeros)
   reproduces today's behavior, except that exhaustion produces a summary rather than an
   error — an intentional, documented difference.
3. `AICTL_STEERING=false` reproduces today's key handling exactly.
4. Manual smoke checklist, especially item 2.
5. Eval comparison on cap-failure counts and median cost.
6. CI gate: `grep -rE 'TurnBudget|steering::' crates/aictl-server/src/` empty.

### 14. Risks

- **Raising the iteration cap increases runaway cost** if the token/cost limits are buggy.
  Mitigation: they ship in the same PR, with tests asserting each binds independently; the
  cost limit is the one users feel, so it gets the tightest test coverage.
- **Terminal corruption from steering capture.** Raw-mode key handling concurrent with
  streamed output is the most fragile thing in this spec. Mitigation: PR 3 is separable and
  revertable; the buffer-output fallback is specified; non-TTY and `--quiet` disable it.
- **Accidental keystroke hijack.** A user typing during a turn out of habit suddenly gets a
  steering prompt. Mitigation: the prompt is obvious and Esc-cancellable; `AICTL_STEERING=false`
  exists. (Note the collision: Esc-in-steering-mode cancels the steering input, while Esc on
  the spinner aborts the turn. Two meanings for one key, disambiguated only by mode — call it
  out in docs and make the mode visually unmistakable.)
- **Steering as an injection vector.** It is user input and goes through the same guards, so
  the risk is not new — but a model told steering "takes priority over your current plan"
  might over-weight it. Mitigation: the wording says adjust, not obey-unconditionally, and the
  block is clearly attributed.
- **Graceful stop costing an extra call.** Budget exhaustion now costs one more LLM call than
  before, slightly overshooting the limit. Accepted: a summary is worth one call, and the
  overshoot is bounded and stated in the banner.
- **Model ignoring the no-tools instruction** on the XML path. Mitigation: the host discards
  any tool call in the final response rather than trusting the instruction.

### 15. Open questions

- **Are the default limits right?** $2.00 and 500k tokens are guesses calibrated to "a real
  coding task should finish". The eval cost distribution is the calibration data — set them
  from the 95th percentile of successful runs.
- **Should the budget be per-turn or per-session?** Per-turn as specified, which means a user
  can bypass it by continuing. A session budget is the thing that actually bounds a day's
  spend and is a natural follow-up, probably as a `/stats`-adjacent warning rather than a hard
  stop.
- **Should steering be able to abort a specific tool** rather than only redirect at the
  boundary? Useful for a long `exec_shell`, and it needs a distinct kill path per tool. Out
  of scope.
- **Desktop steering.** The composer is already a text input, so queueing while a turn runs is
  natural there — arguably more natural than in the terminal. Worth doing right after v1;
  the engine seam is deliberately frontend-agnostic to make that cheap.
