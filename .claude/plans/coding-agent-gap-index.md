# Coding-agent gap closure — spec index

Source: [`.claude/reports/coding-agent/coding-agent-improvements-2026-09-09.md`](../reports/coding-agent/coding-agent-improvements-2026-09-09.md).

Each numbered item in that report becomes one implementable spec below. Specs follow the
house plan format (`Context / Goals & Non-goals / Design / Integration points / Testing /
Rollout / Verification / Risks / Open questions`) used by the shipped plans in
[`done/`](done/), and are written to be picked up independently — read the spec, implement,
move the file into `done/`.

## Specs

| # | Report item | Spec | Size |
|---|-------------|------|------|
| 11 | Eval harness | [`coding-agent-eval-harness.md`](coding-agent-eval-harness.md) | S |
| 2 | Context management | [`coding-agent-context-management.md`](coding-agent-context-management.md) | M |
| 1 | Native tool calling | [`coding-agent-native-tool-calling.md`](coding-agent-native-tool-calling.md) | L |
| 3 | Reasoning effort / extended thinking | [`coding-agent-reasoning-effort.md`](coding-agent-reasoning-effort.md) | S |
| 4 | Subagents / task delegation | [`coding-agent-subagents.md`](coding-agent-subagents.md) | M |
| 5 | Todo / plan tool | [`coding-agent-todo-tool.md`](coding-agent-todo-tool.md) | S |
| 6 | Background processes | [`coding-agent-background-processes.md`](coding-agent-background-processes.md) | M |
| 8 | In-loop diagnostics | [`coding-agent-inline-diagnostics.md`](coding-agent-inline-diagnostics.md) | S |
| 9 | File checkpointing | [`coding-agent-file-checkpointing.md`](coding-agent-file-checkpointing.md) | M |
| 10 | Approval granularity | [`coding-agent-approval-rules.md`](coding-agent-approval-rules.md) | M |
| 7 | Loop budget + steering | [`coding-agent-loop-budget-and-steering.md`](coding-agent-loop-budget-and-steering.md) | M |

## Two corrections to the report

The report's Tier-1 item 2 overstates the gap. Verified against the tree at `749ef89`:

1. **Tool-result truncation already exists.** `config::MAX_TOOL_OUTPUT_LEN` is `10_000`
   ([`config.rs:55`](../../crates/aictl-core/src/config.rs)) and
   `tools::util::truncate_output` ([`tools/util.rs:6`](../../crates/aictl-core/src/tools/util.rs))
   is called by `shell`, `filesystem`, `git`, `web`, `document`, `lint`, `run_code`,
   `json_query`, `csv_query`, `diff`, `archive`, `clipboard`, `system_info`, and
   `list_processes`. The real gaps are narrower and are what the spec targets: the cap is a
   **tail-drop with no head/tail split and no re-read hint**, it is a **compile-time
   constant** rather than a policy knob, it is applied **ad hoc per tool** instead of
   centrally in `tools::execute_tool`, and **MCP and plugin results are entirely
   uncapped** (`grep truncate crates/aictl-core/src/{mcp,plugins}.rs` is empty).

2. **Auto-compaction already exists — but only in the REPL, between turns.**
   `config::auto_compact_threshold()` (default 80) drives `repl::handle_user_turn`
   ([`repl.rs:723`](../../crates/aictl-cli/src/repl.rs)), which compacts *before* dispatching
   a new user prompt using `last_input_tokens` from the previous turn. Nothing checks the
   window **inside** `run::run_agent_turn`, so a tool loop that inflates the transcript
   mid-turn still dies on a provider error; single-shot (`run_agent_single`) and the desktop
   never compact at all. The spec moves the check into the engine loop and keeps the REPL
   pre-turn check as a cheap fast path.

Everything else in the report matched the tree.

## Ordering and dependencies

The report's suggested order (`11 → 2 → 1 → 3 → 4/5 → 6 → 8 → 9 → 10 → 7`) holds. Hard
dependencies between specs:

```
eval-harness ──────────────► (measures every spec below; nothing depends on it to compile)

context-management ─┬──────► subagents        (child-context budget reuses the accounting)
                    └──────► loop-budget      (token budget replaces the iteration cap)

native-tool-calling ───────► reasoning-effort (both extend the MODELS catalog into a
                                               capability table; do the table once)

todo-tool ─────────────────► subagents        (optional: a subagent result can close a todo)

inline-diagnostics ────────► (independent)
file-checkpointing ────────► approval-rules   (checkpoints are what make broad allowlists
                                               safe to grant)
background-processes ──────► (independent)
```

Only `native-tool-calling` is structural. Everything else is additive and can be reverted
by flipping a config key.

## Cross-cutting invariants

Every spec here must preserve, without exception:

- **The security gate.** `security::validate_tool` runs before any tool executes, and
  `security::sanitize_output` runs on every result. New tools get a `validate_tool` arm.
- **The two redaction seams.** `run::redact_outbound` at the network boundary,
  `redaction::redact_for_persistence` at the write boundary. New persistence (checkpoints,
  todo state, eval transcripts) that can contain user data goes through the latter.
- **The audit log.** Every tool dispatch logs through `audit::log_tool`.
- **Hooks are harness behavior.** `PreToolUse` / `PostToolUse` fire per call and are not
  bypassed by `--unrestricted`.
- **The server stays out of it.** `aictl-server` has no agent loop. Every spec below adds a
  CI grep gate asserting its symbols never appear under `crates/aictl-server/src/`.
- **Additive grammars.** Existing tool bodies, config keys, and CLI flags keep working
  unchanged; new behavior is opt-in or defaults to today's semantics.
- **Coding-mode gating.** Anything that changes the *prompt* is gated on
  `config::coding_agent_enabled()`; anything that changes the *tool surface* is universal
  (matching the Phase 2/3/4 precedent).
