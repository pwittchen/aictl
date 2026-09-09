# Spec: Coding-agent eval harness

> Report item **11** (Tier 4). Ships first — it is what makes items 1–10 measurable.

## Context

The workspace has ~738 unit tests plus one CLI smoke test
([`crates/aictl-cli/tests/cli_smoke.rs`](../../crates/aictl-cli/tests/cli_smoke.rs)) and a
mock-LLM integration harness ([`crates/aictl-cli/src/integration_tests.rs`](../../crates/aictl-cli/src/integration_tests.rs),
864 lines). All of it asserts *mechanism*: that `parse_tool_calls` returns three calls, that
a rejection envelope has the right shape, that `detect_test_cmd` picks `./gradlew`.

Nothing asserts **task completion**. `SYSTEM_PROMPT_CODING` is ~6KB of accumulated prose
([`config.rs`](../../crates/aictl-core/src/config.rs)) with no evidence attached to any
paragraph, and the five-phase loop, the `<test_failure>` re-injection, the Review hook retry
budget, and the `<repo_context>` block were each tuned by reading transcripts. Every
remaining item in the gap report is a prompt or loop change whose effect currently cannot be
observed — which means there is no way to know if a change helped, and no way to notice a
regression when a provider ships a new model.

This spec builds the smallest thing that turns that into a number.

## Goals & Non-goals

**Goals**

- A `evals/` tree of self-contained task fixtures: a seed repo, a prompt, and a
  deterministic pass/fail check script.
- A runner binary that, for each (task × model), materializes the seed repo into a temp dir,
  runs `aictl` single-shot against it with coding-agent mode on, runs the check, and records
  the outcome plus cost/latency/turn counts.
- A JSON result file per run and a markdown summary table, both committed under
  `.claude/reports/evals/` so runs are diffable across prompt changes.
- 20 hand-written tasks at v1, spanning the phases the coding prompt claims to drive:
  explore-only, single-file edit, multi-file rename, fix-a-failing-test, add-a-CLI-flag,
  and two "should refuse / should ask" negative tasks.
- Runnable against ≥3 models across ≥2 providers, and against local providers for
  regression-only use.
- Fully offline-capable for the *harness itself* — only the LLM call touches the network.

**Non-goals**

- No SWE-bench / public benchmark integration. Those measure a different thing (patch
  generation against a frozen issue set) and pull in a Python toolchain.
- No LLM-as-judge scoring at v1. Every check is a script exiting `0`/non-zero. Fuzzy
  grading is a follow-up once the deterministic set is stable.
- No CI gating on pass rate. Evals cost money and are non-deterministic; they run on demand
  and their result is a report, not a build failure.
- No new engine capability. The harness drives the *existing* CLI as a subprocess. It must
  not link `aictl-core` in a way that lets a refactor silently change what is measured.
- No parallel model dispatch at v1 beyond a task-level worker pool.

## Design

### 1. Layout

```
evals/
  tasks/
    fix-failing-test-rust/
      task.toml            # metadata + prompt
      seed/                # files copied into the temp workspace
        Cargo.toml
        src/lib.rs
      check.sh             # exit 0 == pass; runs inside the workspace
    add-cli-flag-node/
    rename-across-files/
    explore-summarize/
    ...
  README.md
```

`task.toml`:

```toml
name = "fix-failing-test-rust"
description = "One unit test fails on an off-by-one; the fix is a single character."
prompt = "The test suite is failing. Find the bug and fix it."
category = "fix"            # explore | edit | refactor | fix | flag | negative
timeout_secs = 300
max_cost_usd = 0.50         # runner aborts the task if the turn exceeds this
git_init = true             # seed the workspace as a git repo (so <repo_context> is real)
expect = "pass"             # pass | refuse  — `refuse` inverts the check
```

`check.sh` runs with CWD set to the materialized workspace and gets no arguments. It is the
whole specification of "done":

```sh
#!/bin/sh
set -e
cargo test --quiet 2>&1 | grep -q "test result: ok"
```

Seeds stay tiny (single-digit files) so materialization is instant and the model's context
is dominated by the task, not by repo noise.

### 2. Runner

New binary in a new workspace member, `crates/aictl-evals` (`[[bin]] name = "aictl-evals"`),
**excluded from `default-members`** exactly like `aictl-desktop` — a bare `cargo build` /
`cargo test` must not pull it in.

```
aictl-evals run   [--task <name>]... [--model <m>]... [--repeat N] [--jobs N] [--out <dir>]
aictl-evals list
aictl-evals report [--json <file>]     # re-render markdown from a JSON result file
```

Per (task, model, repetition) the runner:

1. Creates a temp dir under the scratch root, copies `seed/` into it, and runs `git init &&
   git add -A && git commit` when `git_init = true`.
2. Spawns the release `aictl` binary as a subprocess with:
   - `--cwd <workspace>` (pins the CWD jail and `<repo_context>` to the fixture),
   - `--coding-agent`, `--quiet`, `--format json`, `--behavior auto`,
   - `--audit-file <out>/audit-<task>-<model>-<rep>.jsonl`,
   - `AICTL_*` env-equivalents written to a **throwaway config dir** (see §3),
   - the task prompt as the single-shot argument.
3. Applies `timeout_secs` as a hard kill.
4. Runs `check.sh` and records the exit code, inverted when `expect = "refuse"`.
5. Parses the audit JSONL for turn count, tool-call count and tool mix, and the
   `--format json` envelope for the final answer.

The runner never imports `aictl-core`. It shells out. That is deliberate: the thing being
measured is the shipped binary's behavior, and an in-process harness would let a refactor
change the measured path without changing the measurement.

### 3. Config isolation

Evals must not read or write the developer's `~/.aictl/`. The runner sets `HOME` (and on
macOS `XDG_CONFIG_HOME` where honored) to a per-run temp dir and writes a minimal config
containing only the provider API key it was given, plus:

```
AICTL_CODING_AGENT=true
AICTL_MEMORY_ENABLED=false
AICTL_INCOGNITO=true
AICTL_MCP_ENABLED=false
AICTL_PLUGINS_ENABLED=false
AICTL_HOOKS_FILE=/dev/null
AICTL_SKILLS_DIR=<empty temp dir>
```

Rationale: memory, MCP servers, plugins, hooks and user skills all mutate the system prompt
or the tool surface. An eval that silently picks up the developer's `save_memory` history is
not comparable across machines. API keys come from the developer's real config via an
explicit `--key-from-config` read at startup, or from `AICTL_EVAL_<PROVIDER>_KEY`.

### 4. Result schema

`.claude/reports/evals/<ISO8601>-<label>.json`:

```json
{
  "started_at": "2026-09-10T09:00:00Z",
  "aictl_version": "0.47.18",
  "git_sha": "749ef89",
  "prompt_hash": "sha256:…",
  "runs": [
    {
      "task": "fix-failing-test-rust",
      "model": "claude-sonnet-5",
      "provider": "anthropic",
      "repetition": 0,
      "outcome": "pass",
      "check_exit": 0,
      "llm_calls": 6,
      "tool_calls": 11,
      "tool_mix": { "read_file": 4, "edit_file": 1, "test": 2, "search_files": 4 },
      "input_tokens": 41233, "output_tokens": 2210,
      "cost_usd": 0.0731,
      "wall_clock_ms": 38102,
      "failure_reason": null
    }
  ]
}
```

`prompt_hash` is `sha256(SYSTEM_PROMPT_CODING)`, surfaced by a tiny
`aictl --print-prompt-hash` flag (hidden from `--help`). It is the join key that makes
"did this prompt edit help?" answerable: two result files with different prompt hashes and
the same git-tracked task set are directly comparable.

### 5. Report rendering

`aictl-evals report` writes a sibling `.md`:

```
## Eval run 2026-09-10 — prompt sha 4f2a…

| task | claude-sonnet-5 | gpt-5.4 | gemini-3.1-pro |
|------|-----------------|---------|----------------|
| fix-failing-test-rust    | ✅ 6t $0.07 | ✅ 9t $0.04 | ❌ 20t $0.05 |
| rename-across-files      | ✅ 11t $0.14 | ❌ 20t $0.09 | ❌ 20t $0.08 |
…
| **pass rate**            | **17/20**   | **12/20**   | **9/20**     |
```

`20t` hitting the iteration cap is itself a signal — it is exactly the failure mode report
item 7 (loop budget) predicts, and this table is where it becomes visible.

### 6. The task set (v1)

Twenty tasks, biased toward things the coding prompt explicitly claims to do:

| # | Task | Category | What it probes |
|---|------|----------|----------------|
| 1–3 | Fix a failing test (Rust / Python / Node) | fix | `test` tool, `<test_failure>` re-injection, retry budget |
| 4–5 | Fix a compile error (Rust / TypeScript) | fix | build detection, Review hook |
| 6–8 | Add a CLI flag end-to-end (parse + use + doc) | edit | multi-file coherence, doc gate |
| 9–10 | Rename a symbol across 3+ files | refactor | `search_files`, multi-block `edit_file` |
| 11–12 | Add a function with tests to an existing module | edit | test authoring |
| 13–14 | Summarize what a repo does; make no edits | explore | Explore discipline, parallel reads, no spurious writes |
| 15 | Locate the source of a described bug; do not fix it | explore | instruction adherence |
| 16–17 | Update README to match changed behavior | edit | the documentation gate in the prompt |
| 18 | Task whose prerequisite file does not exist | negative | asks instead of fabricating |
| 19 | Ambiguous request with two valid readings | negative | asks instead of guessing |
| 20 | Large file (5k lines), edit one function | edit | `read_file --lines`, `edit_file @N-M`, truncation behavior |

Task 20 is the direct regression test for the context-management spec; tasks 13–14 for the
parallel-read path; task 15 and 18–19 catch the "eager agent" failure mode that prompt edits
most often introduce.

### 7. Integration points

| File | Change |
|------|--------|
| `Cargo.toml` (workspace) | Add `crates/aictl-evals` as a member; **exclude from `default-members`** |
| `crates/aictl-evals/` | New crate: runner, task loader, result schema, report renderer |
| `evals/tasks/**` | 20 fixture directories |
| `crates/aictl-cli/src/main.rs` | Add hidden `--print-prompt-hash` flag |
| `.gitignore` | Ignore `evals/.work/` (materialized workspaces) |
| `docs/CODING_AGENT.md` | New "Measuring changes" section pointing at `evals/README.md` |
| `CLAUDE.md` | Workspace layout: note the fifth crate and why it is out of `default-members` |
| `.claude/reports/evals/` | Committed result JSON + markdown |

### 8. Testing

The harness itself needs tests that do not call an LLM:

- Task loader: valid `task.toml` parses; missing `check.sh` errors loudly; unknown
  `category` rejected.
- Workspace materialization: seed copied byte-exact; `git_init` produces a repo with one
  commit; the temp dir is removed on both success and panic.
- Runner against `--provider mock` (the existing `llm/mock.rs` path): a scripted mock that
  emits one `write_file` then a final answer produces `outcome: "pass"` on a fixture whose
  `check.sh` greps for that file. This is the end-to-end test of the harness with zero
  network and zero cost, and it runs in `cargo test`.
- Report renderer: golden-file test on a fixed result JSON.
- Config isolation: assert the spawned process's `HOME` is not the developer's, by having a
  mock-provider fixture whose `check.sh` asserts `~/.aictl/memory.json` was never created.

### 9. Rollout

Three PRs:

1. **Crate skeleton + task format + mock-provider end-to-end test.** No real tasks beyond
   two trivial ones. Fully testable in CI, zero cost.
2. **The 20 fixtures.** Pure data plus `check.sh` scripts; each verified by hand once.
3. **Report rendering + first committed baseline run** across three models, checked into
   `.claude/reports/evals/`. This baseline is the number every subsequent spec is graded
   against.

### 10. Verification

1. `cargo build` (default members) does **not** build `aictl-evals`.
2. `cargo build -p aictl-evals` clean; `cargo lint` clean on the new crate.
3. `cargo test` includes the mock-provider end-to-end run and passes offline.
4. `aictl-evals run --model mock` completes all 20 tasks without network access (most will
   fail the check — that is fine; the assertion is that the harness completes).
5. A real baseline run across three models produces a committed JSON + markdown pair.
6. Deleting a paragraph from `SYSTEM_PROMPT_CODING` and re-running produces a different
   `prompt_hash` and a comparable table.

### 11. Risks

- **Cost.** 20 tasks × 3 models × 2 repetitions is ~120 agent runs. Mitigation: `max_cost_usd`
  per task aborts runaway turns; `--task` / `--model` filters make partial runs cheap; the
  default `--repeat` is 1.
- **Flakiness read as regression.** Non-determinism means a 17/20 → 16/20 delta is noise.
  Mitigation: report the pass rate with the repetition count visible; require `--repeat 3`
  before claiming a prompt change helped; treat only ≥3-task swings as signal.
- **Fixture rot.** A seed repo pinned to a toolchain version stops building. Mitigation:
  keep seeds toolchain-free where possible (no lockfiles, no pinned deps); the harness
  reports "check script failed to run" distinctly from "task failed".
- **Measuring the harness instead of the agent.** If the runner's config isolation drifts,
  results become machine-specific. Mitigation: the isolation test in §8.
- **Tasks that are too easy.** A set every model passes measures nothing. Mitigation: the
  baseline run in PR 3 is the calibration — any task passed by all three models at 3/3 gets
  hardened or replaced before the set is frozen.

### 12. Open questions

- **Where do fixtures live** — `evals/` at the repo root (visible, invites contribution) or
  `crates/aictl-evals/tasks/` (crate-local, tidier)? Lean root: they are data, not Rust, and
  a contributor adding a task should not need to know the crate exists.
- **Should the baseline run be committed?** Lean yes — the diff of a result file across a
  prompt change is the single most useful artifact this produces, and the files are small.
  Risk is repo churn if runs are frequent; mitigate with a `--out` default of `/tmp` and an
  explicit `--commit` flag for baselines.
- **Negative tasks (18–19) need a judgment call on the answer text**, which is the one place
  a deterministic script is weak. v1 checks only "no files were modified"; a follow-up can
  add LLM-as-judge for the answer prose.
- **Local providers in the matrix?** Useful as a free regression signal, but GGUF/MLX pass
  rates will be near zero and could drown the table. Lean: excluded from the default matrix,
  available via `--model`.
