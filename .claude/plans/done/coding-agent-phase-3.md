# Plan: Coding Agent — Phase 3 (test loop and self-review polish)

## Context

Phase 1 ([`coding-agent.md`](coding-agent.md)) shipped the prompt-driven five-phase loop: the model is *told* to run tests in the Test phase and review its diff in the Review phase, but neither phase has any host-side enforcement. The current shape:

- **Test phase**: the model emits an `<tool name="exec_shell">cargo test</tool>` (or whatever `coding::detect_test_cmd` returns), reads the textual output, decides "passed" or "failed" by eye, and reports. There is no parsing of failure counts, no structured retry, no "the test failed, here's the exact diff between expected and got" surfaced back to the model. `AICTL_CODING_TEST_RETRIES` is documented but the loop is purely a model-side discipline; if the model declares success too early, nothing notices.
- **Review phase**: identical pattern — the model is asked to run `git diff`, `lint_file` on changed paths, and maybe `cargo build`. The host doesn't verify any of those happened. The "review" is whatever the model chose to say.
- **Session start**: the agent begins with zero context about the repo state. Branch name, recent commits, working-tree dirtiness, top-level directory tree — all things the model has to discover with three or four early Explore tool calls before it can plan. In coding-agent mode those calls happen *every session*, which is wasteful tokens for a strictly deterministic snapshot.

Phase 3 closes those gaps with three pieces:

1. A dedicated `test` tool that wraps the project's test runner and emits a *structured* pass/fail summary the host can act on.
2. Session-start context injection: a one-shot "here's the repo state" block prepended to the conversation when coding-agent mode is on, so the model can skip the boilerplate Explore round.
3. A structured Review hook that runs deterministically before the model is allowed to emit a "final answer" in coding mode — the model is no longer trusted to self-certify; the host runs `git diff`, the build, and the linter, parses the results, and either short-circuits to "Review complete, here's the answer" or feeds the failures back into the loop as a synthetic user turn.

Phase 3 is where the coding agent stops being a *suggestion* and starts being an *enforced* workflow.

## Goals & Non-goals

**Goals**

- Add a dedicated `test` tool that runs the detected test command, parses the output for at least Rust / Node / Python / Go (the four detectors that already exist in `coding.rs`), and returns a structured envelope: `{ command, exit_code, duration, passed, failed, skipped, failures: [...], raw_tail: "..." }`.
- Replace the prompt-driven retry loop with a host-driven one: when the `test` tool reports `failed > 0`, the host injects a synthetic `<test_failure>` turn carrying the structured failures (not the raw tail) and re-runs the agent loop, up to `AICTL_CODING_TEST_RETRIES`.
- Replace the Review-phase prompt with a structured hook that fires when the model is about to emit a no-tool-call response from the Review phase. The hook runs `git diff --stat`, the project build, and `lint_file` on changed paths; on any failure it injects the result as a synthetic user turn and continues; on success it lets the final answer through.
- Inject a session-start context block (`<repo_context>`) into the system prompt only in coding-agent mode (not the chat-only base, not the general-purpose base). The block contains: current branch, last 5 commits one-line, working-tree dirty flag with `git status --short`, top-level dir tree (depth 2, capped at 60 entries), detected linter command, detected test command, detected build command (new).
- Add a build detector (`coding::detect_build_cmd`) parallel to the existing linter/test detectors.
- Stay strictly CLI + desktop scoped. Server gets no new code, same as every prior coding-agent phase.
- Preserve a fast-path: when `AICTL_CODING_AGENT=false` (the default), Phase 3 code paths are dead — no `<repo_context>` injection, no Review hook, no structured `test` tool dispatched. The `test` tool is also callable in non-coding mode (we don't make it coding-only) but with no host-side retry/Review-hook integration.

**Non-goals**

- No language-server integration. Diagnostics still come from `lint_file` and the project's build.
- No auto-fix loop ("the test said X, write code to fix X"). The host injects the failure context; the model is responsible for the fix. We are not building a self-coding system here.
- No coverage reporting, no benchmark integration, no flake detection. The `test` tool reports the runner's exit code and parsed counts and stops.
- No per-test-file granularity. The unit of action is "the whole test command". Selecting a subset is the user's job.
- No new sandboxing. The `test` tool runs through the same security gate as `exec_shell` — `allowed_shells`, `working_dir`, `shell_timeout` all apply.
- No persistent test history across sessions. Each session starts fresh; the `--info` banner can show "last run: N pass / M fail" but nothing is written to disk.
- No structured Review hook for non-coding-agent sessions. The hook is gated on `coding_agent_enabled()`.
- No Plan-phase approval changes — that's still governed by `AICTL_CODING_PLAN_APPROVE` from Phase 1.

## Design

### 1. Dedicated `test` tool

A new built-in tool with a small, opinionated surface. The model has used `exec_shell cargo test` so far; we want the model to call `test` instead, and the host can then parse the result instead of asking the model to.

**Tool name**: `test`.

**Body grammar** (all optional, simplest case is an empty body):

```
                       — run the auto-detected test command for the project
<filter>               — pass <filter> as the test-name filter (cargo test <filter>, pytest -k <filter>, etc.)
--cmd <command>        — override entirely (escape hatch; falls back to exec_shell semantics)
--watch                — reserved, not implemented in v1 (returns "not implemented")
```

**Output shape** (text body, since every other tool returns text; structured fields are in a JSON object the model can parse if it wants but the prose summary is what the model actually reads):

```
Command: cargo test --color=never
Exit:    0
Time:    12.4s

Passed:  42
Failed:  0
Skipped: 0

(no failures)
```

When `failed > 0`:

```
Command: cargo test --color=never
Exit:    101
Time:    8.2s

Passed:  40
Failed:  2
Skipped: 0

Failures:
  tests::auth::login_rejects_empty_password
    assertion failed: response.is_err()
    at tests/auth.rs:42:5

  tests::auth::session_token_rotates_on_refresh
    expected: "rotated"
    actual:   "stale"
    at tests/auth.rs:68:5

(showing 2 of 2 failures; full output truncated)
```

The structured form is in a sibling JSON object pushed onto the conversation as a `<test_failure>` block by the agent loop (see §3), so the model never has to parse the prose.

**Implementation**:

- New file `crates/aictl-core/src/tools/test.rs`. Implements `tool_test(input: &str) -> String` (signature matches other tools).
- Resolves the command via:
  1. `--cmd <override>` if present.
  2. Else `coding::detect_test_cmd(working_dir)`. If that returns `None`, the tool body is the explicit "no test command detected for this project. Set `AICTL_CODING_TEST_CMD` in `~/.aictl/config` or pass `--cmd` to override. Detected language markers checked: Cargo.toml, package.json, pyproject.toml/pytest.ini, go.mod." error.
- Appends the filter if provided. For each runner:
  - Cargo: `cargo test <filter>`.
  - npm: `npm test -- <filter>` (npm convention; works for jest, mocha, vitest with `--`).
  - pytest: `pytest -k <filter>`.
  - go: `go test ./... -run <filter>`.
  - Others (Phase 2.5 — Java/Kotlin/C from `coding-agent-detect-jvm-c.md`): pass via the runner's documented filter flag.
- Spawns through the same path as `exec_shell` — direct subprocess, scrubbed env, working-dir-pinned, `shell_timeout` applied. Same security gate, same redaction, same audit log.
- Parses output through a per-runner parser in `tools/test/parsers.rs`. Each parser is a `fn parse(stdout: &str, stderr: &str) -> TestSummary`. Unknown runners fall back to "best-effort" — try a regex over `\d+ passed`, `\d+ failed`, `\d+ skipped`; if nothing matches, leave the counts at zero and set `parse_warning: "unknown runner; counts unavailable"`.

```rust
pub struct TestSummary {
    pub command: String,
    pub exit_code: i32,
    pub duration_ms: u128,
    pub passed: u32,
    pub failed: u32,
    pub skipped: u32,
    pub failures: Vec<TestFailure>,    // capped at 25 entries
    pub raw_tail: String,              // last ~4 KB of combined stdout+stderr
    pub parse_warning: Option<String>,
}

pub struct TestFailure {
    pub name: String,
    pub message: String,               // first ~400 chars
    pub location: Option<String>,      // "file:line" when available
}
```

The tool body returns the prose rendering; the structured form sits on a thread-local slot the agent loop reads after the tool dispatch completes (see §3).

**Catalogue entry**: `BUILTIN_TOOLS` ([`tools.rs:147`](../../crates/aictl-core/src/tools.rs)) grows by one. `TOOL_COUNT` bumps from 35 → 36. The dispatch arm in `execute_tool` adds `"test" => test::tool_test(input).await`.

### 2. Session-start `<repo_context>` block

A new helper in `coding.rs`:

```rust
pub struct RepoContext {
    pub branch: Option<String>,
    pub last_commits: Vec<String>,        // up to 5, oneline
    pub dirty: bool,
    pub dirty_files: Vec<String>,         // git status --short, capped at 40
    pub top_level_tree: Vec<String>,      // depth 2, capped at 60
    pub linter: Option<String>,
    pub test_cmd: Option<String>,
    pub build_cmd: Option<String>,
}

pub fn collect_repo_context(working_dir: &Path) -> RepoContext;
```

`collect_repo_context` is synchronous, uses `std::process::Command` directly (not `tools/git.rs`, which is async and slower for a one-shot read), and runs the four reads in sequence (they're cheap; parallelizing them would save ~30ms at most and the code becomes a JoinSet for no real gain). Errors anywhere — not a git repo, no rev-list, etc. — produce `None` fields, not failures. The whole helper is best-effort.

The block lands at the *bottom* of the system prompt in coding-agent mode, so it appears before any phase guidance:

```
# Repo context

Branch: feature/parallel-tools
Working tree: dirty (3 modified, 1 untracked)

Recent commits:
  8bf8a14 Update ROADMAP.md
  fbf6547 Note Java/Kotlin and C auto-detection gaps in roadmap
  a830bcc Sketch coding-agent phases 2-4 in roadmap
  a9fdf57 Prune retired Claude and Grok models from catalog
  4bfd914 Bump version to 0.46.0

Modified files:
  M crates/aictl-core/src/coding.rs
  M crates/aictl-core/src/tools.rs
  M ROADMAP.md
  ?? .claude/plans/coding-agent-phase-2.md

Top-level layout:
  crates/
    aictl-core/
    aictl-cli/
    aictl-server/
    aictl-desktop/
  examples/
  Cargo.toml
  CLAUDE.md
  README.md
  ROADMAP.md

Project commands:
  build: cargo build
  lint:  cargo lint
  test:  cargo test
```

`build_system_prompt_with` ([`run.rs:313`](../../crates/aictl-core/src/run.rs)) calls a new `coding::format_repo_context()` and appends the result *only* when `coding_agent_enabled()` is true. The block is collected once per session and cached in a `OnceLock<String>` keyed off the working directory — re-collecting on every turn is cheap but pointless. A `coding::invalidate_repo_context()` helper lets the CLI's `/coding refresh` (new sub-subcommand) bust the cache when the user wants a fresh snapshot mid-session.

**Build command detection**: a new sibling to `detect_linter` / `detect_test_cmd`:

```rust
pub fn detect_build_cmd(working_dir: &Path) -> Option<String> {
    if let Some(cmd) = config::coding_build_cmd_override() { return Some(cmd); }
    if working_dir.join("Cargo.toml").is_file() { return Some("cargo build".to_string()); }
    if working_dir.join("package.json").is_file() {
        if package_json_has_script(&working_dir.join("package.json"), "build") {
            return Some("npm run build".to_string());
        }
        if working_dir.join("tsconfig.json").is_file() {
            return Some("npx tsc --noEmit".to_string());
        }
    }
    if working_dir.join("go.mod").is_file() { return Some("go build ./...".to_string()); }
    if working_dir.join("pyproject.toml").is_file() { return Some("python -m build".to_string()); }
    None
}
```

A new config key `AICTL_CODING_BUILD_CMD` mirrors the existing `AICTL_CODING_LINTER` / `AICTL_CODING_TEST_CMD` pair. (Java/Kotlin/C detection lands in [`coding-agent-detect-jvm-c.md`](coding-agent-detect-jvm-c.md) — that plan's detectors plug in here too.)

### 3. Host-driven test retry loop

The agent loop ([`run.rs:776`](../../crates/aictl-core/src/run.rs)) is extended in coding-agent mode only. After `handle_tool_call` returns for a `test` tool dispatch *and* the parsed `TestSummary` has `failed > 0`:

1. Append a synthetic `<test_failure>` user message to `messages` (in addition to the existing `<tool_result>` that already landed). The synthetic message carries a *short* structured rendering — failure name, location, message — not the prose. The model uses it to plan the fix on the next iteration.
2. Increment a `test_retry_count` counter in the loop scope.
3. If `test_retry_count >= AICTL_CODING_TEST_RETRIES` (default 3), append a final `<test_failure_terminal>` block telling the model "the test loop has exhausted its retry budget; surface the remaining failures to the user without further edits" and let the loop continue normally — the model produces its final answer with the failures acknowledged rather than buried.

The host doesn't *force* the model to re-edit. The `<test_failure>` block is informational + steering; the model decides whether to call `edit_file` again, ask the user, or give up. The retry budget is a backstop against infinite loops, not a hard contract.

**Where the `TestSummary` lives**: a `OnceCell<TestSummary>` on a per-tool-call slot, written by `tool_test` and read by the agent loop right after the tool dispatch returns. Single-producer, single-consumer, per-turn — no concurrency story needed.

### 4. Structured Review hook

Today the Review phase is prose-only. Phase 3 makes it deterministic: when the agent loop sees a no-tool-call response (the existing "final answer" branch) *and* coding-agent mode is on *and* the current phase is Review (or Code, since the model may try to skip Review), the loop *suspends* the final answer, runs the structured review, and decides whether to release the answer or feed the review failure back into the loop.

```rust
async fn run_structured_review(
    messages: &mut Vec<Message>,
    ui: &dyn AgentUI,
) -> ReviewOutcome {
    let diff = run_git("diff --stat").await;
    let changed = parse_changed_paths(&diff);
    if changed.is_empty() {
        return ReviewOutcome::Pass { reason: "no changes to review".into() };
    }
    let build_cmd = coding::detect_build_cmd(working_dir());
    let lint_cmd = coding::detect_linter(working_dir());
    let build = run_exec_shell(build_cmd).await;
    let lints = run_lint_each(&changed).await;
    if build.exit_code != 0 || lints.iter().any(|l| l.exit_code != 0) {
        return ReviewOutcome::Fail { build, lints };
    }
    ReviewOutcome::Pass { reason: "build + lint clean".into() }
}
```

The result is converted into a `<review_result>` user turn (success or failure detail) and pushed onto `messages`. On failure, the loop continues (`continue` in the existing `for llm_calls in …` loop), so the next iteration sees the failure and produces a new turn — typically another `edit_file`. On success, the final answer the model produced is released to the user with a `[review: clean — build + lint passed]` banner prepended by the CLI (desktop appends it as a status chip in the chat header — out of scope for v3 in the desktop; for now just the prepended line).

**Retry budget**: same as the test loop — `AICTL_CODING_REVIEW_RETRIES` (default 2) caps the Review-failure → Code loop. After exhaustion, the loop releases the model's final answer with a `[review: N failures remain]` banner so the user sees the unresolved state.

**Skip path**: when `AICTL_CODING_SKIP_REVIEW=true` (already in Phase 1's config), the structured Review hook short-circuits to `Pass { reason: "skipped per config" }` and emits no banner. Same for `--skip review` mid-session.

**When the hook does *not* fire**:

- Coding-agent mode is off.
- No file changes since the session started (the host tracks this via a `HashSet<PathBuf>` of paths touched by `write_file` / `edit_file` / `remove_file` / `create_directory` — populated in `handle_tool_call`).
- The current phase is `Explore` or `Plan`. Review only gates Code → Test transitions and the terminal "I'm done" answer.
- The user has explicitly skipped Review for this turn (`/skip review`).

### 5. Coding-mode prompt updates

`SYSTEM_PROMPT_CODING` ([`config.rs`](../../crates/aictl-core/src/config.rs)) gets three revisions:

- **Test phase**: rewritten to direct the model toward the `test` tool ("call `<tool name='test'></tool>` with no arguments to run the project's test command. Pass a filter as the body to narrow to specific tests."). The "exec_shell cargo test" fallback line stays as a backup for projects where auto-detection fails.
- **Review phase**: simplified — the host now runs build + lint deterministically, so the prompt no longer asks the model to. Instead it tells the model: "your final answer in Review will be checked against `git diff`, the project build, and `lint_file`. If any check fails you will be sent a `<review_result>` block — fix and re-emit." Less prompt prose; more host enforcement.
- **Header**: a short paragraph about the `<repo_context>` block at the top: "the system prompt includes a `<repo_context>` snapshot. Use it to skip basic discovery — branch, recent commits, dirty files, and detected build/lint/test commands are already known."

### 6. CLI surface

- `/coding refresh` — invalidates the cached `<repo_context>` and reruns `collect_repo_context`. Useful after the user has been editing outside the agent and wants the agent to re-orient.
- `--info` banner gains three lines: `coding-build`, `coding-test`, `coding-lint`, each showing the auto-detected (or overridden) command. Reads from `coding::detect_*` directly.
- `/coding status` extends to print the resolved build/lint/test commands and the current test_retry / review_retry budgets.

### 7. Desktop surface

- Settings → Coding Agent gains read-only "Resolved commands" fields: build, lint, test. Same source of truth (`detect_*`) via three new Tauri commands: `coding_agent_build_cmd`, `coding_agent_lint_cmd`, `coding_agent_test_cmd` (each returns `String | null`). Three commands instead of one bundled one keeps the IPC surface mirrored with the existing per-feature shape.
- The Review-result banner ("review: clean" / "review: 2 failures remain") appears as a status pill in the chat header for the duration of the message (auto-fades after 4 s).
- No new composer-toolbar icon. The `test` tool dispatches like any other tool — confirmation flow, auto-accept, etc., all reuse the existing path.

### 8. Configuration

New keys (all default-off / default-zero so existing configs are unaffected):

```
AICTL_CODING_BUILD_CMD=                  # empty = auto-detect
AICTL_CODING_REVIEW_RETRIES=2            # cap on Code → Review → Code loops
AICTL_CODING_REPO_CONTEXT=true           # opt-out for users who want a leaner prompt
AICTL_CODING_REPO_CONTEXT_TREE_DEPTH=2   # tunable; cap at 4
AICTL_CODING_REPO_CONTEXT_TREE_MAX=60    # cap on entries listed
AICTL_CODING_TEST_FILTER_DEFAULT=        # default --filter arg passed to `test` when none provided
```

Existing keys (`AICTL_CODING_TEST_RETRIES`, `AICTL_CODING_LINTER`, `AICTL_CODING_TEST_CMD`, `AICTL_CODING_SKIP_REVIEW`, `AICTL_CODING_SKIP_TEST`) keep working.

### 9. Runtime shape and integration points

| File | Change |
|------|--------|
| `crates/aictl-core/src/tools/test.rs` | **New** — `tool_test` + per-runner parsers + `TestSummary` struct |
| `crates/aictl-core/src/tools/test/parsers.rs` | **New** — cargo / npm / pytest / go parsers |
| `crates/aictl-core/src/tools.rs` | Add `"test"` arm in `execute_tool`; add `("test", "...")` to `BUILTIN_TOOLS`; bump `TOOL_COUNT` to 36 |
| `crates/aictl-core/src/coding.rs` | Add `RepoContext` + `collect_repo_context` + `format_repo_context` + `detect_build_cmd` + `package_json_has_script` (reused from existing test-detection helper, refactored to take the script name) |
| `crates/aictl-core/src/config.rs` | Add `AICTL_CODING_BUILD_CMD` / `AICTL_CODING_REVIEW_RETRIES` / repo-context keys; new accessors; revise `SYSTEM_PROMPT_CODING` (Test + Review + Header paragraphs) |
| `crates/aictl-core/src/run.rs` | `build_system_prompt_with` calls `coding::format_repo_context()` when coding mode is on; agent loop tracks `test_retry_count` / `review_retry_count` / changed-paths set; pre-final-answer Review hook gated on `coding_agent_enabled()` and phase ∈ {Code, Review}; synthesize `<test_failure>` / `<review_result>` turns |
| `crates/aictl-cli/src/commands/coding.rs` | Add `/coding refresh` subcommand; extend `/coding status` |
| `crates/aictl-cli/src/commands/info.rs` | Three new lines (build/lint/test) in the banner when coding mode is on |
| `crates/aictl-desktop/src/lib.rs` | Three new Tauri commands: `coding_agent_build_cmd` / `coding_agent_lint_cmd` / `coding_agent_test_cmd` |
| `crates/aictl-desktop/webview/src/lib/ipc.ts` | Thin wrappers for the three new commands |
| `crates/aictl-desktop/webview/src/components/Settings.tsx` | "Resolved commands" read-only section under Coding Agent |
| `crates/aictl-desktop/webview/src/components/ChatHeader.tsx` (or similar) | Review-result status pill |
| `README.md` | Phase 3 bullet under Coding-agent mode; mention the `test` tool, the `<repo_context>` injection, and the host-side Review |
| `CLAUDE.md` | Update the existing coding-mode paragraph with: "Phase 3: dedicated `test` tool with structured failure injection; `<repo_context>` block; host-driven Review (build + lint) before final-answer release in Code/Review phases." |
| `ROADMAP.md` | Remove the Phase 3 bullet once shipped |

### 10. Testing

**Unit tests**

- `tools/test/parsers.rs` table tests for cargo / npm / pytest / go: feed a real captured output blob (committed to `tests/fixtures/`), assert the parsed `passed` / `failed` / `failures[]` shape.
- `coding::collect_repo_context`: in a `tempfile::tempdir` with a synthetic git repo, verify branch / dirty / last_commits / dirty_files come back as expected; verify `linter` / `test_cmd` / `build_cmd` populate from the fixture project markers.
- `coding::format_repo_context`: golden-string test that the rendered block matches the documented layout.
- Repo-context cache: `OnceLock` populates once; `invalidate_repo_context()` flushes; second call repopulates.
- `coding::detect_build_cmd`: same fixture suite as `detect_test_cmd`.

**Integration tests** (CLI mock-LLM harness)

- Test-loop fixture: mock provider emits `<tool name='test'></tool>`, the test runner returns a structured failure, the agent loop injects `<test_failure>`, the next mock response edits a file and re-runs the test which now passes; verify the final answer references the fix.
- Test-retry exhaustion: mock test keeps failing; verify the loop stops at `AICTL_CODING_TEST_RETRIES` and the final answer surfaces the remaining failures.
- Review-pass fixture: mock provider edits a file, emits a final answer; host runs `git diff --stat` + `cargo build` + `lint_file` (all clean); final answer is released with the `[review: clean]` banner.
- Review-fail fixture: same but build is broken; verify the loop runs another iteration, the model re-edits, the second review passes.
- Repo-context fixture: launch in a fresh git repo with two commits; verify the model's first message sees a `<repo_context>` block with those two commits in the `Recent commits` section.

**Manual smoke**

1. In a Rust project with passing tests, ask "what happens if I delete `Cargo.toml`'s `[dev-dependencies]` block?" — agent edits, the test runs, structured failure is fed back, agent reverts the edit. Verify `<test_failure>` shape in the audit log.
2. In a Rust project with one broken test, ask "make tests pass". Agent reads, edits, runs `test`, sees the structured failure, plans, edits, runs again, succeeds. Verify the prose `Passed: N / Failed: 0` line shows up.
3. Run `/coding refresh` after editing a file manually outside the agent. Verify the next message's system prompt has an updated `Modified files` list.
4. In the desktop, launch coding mode in a repo, see "Resolved commands: build cargo build / lint cargo lint / test cargo test" in Settings → Coding Agent.
5. Trigger a Review failure: ask the model to insert a syntax error, verify the host catches it via `cargo build` failure, the `<review_result>` block carries the compiler error, and the model's next turn fixes it.

**CI gates**

```bash
# Test tool present, count bumped.
grep -E 'TOOL_COUNT: usize = 36' crates/aictl-core/src/tools.rs

# Phase 3 symbols stay out of the server crate.
grep -rE 'collect_repo_context|tool_test|run_structured_review' crates/aictl-server/src/   # must be empty
```

## Rollout phases

Ship Phase 3 in three independent PRs — each is useful on its own:

1. **`test` tool + structured retry loop** — the largest single piece. Lands first because the Review hook (PR 3) leans on the test loop's pattern of injecting structured user turns.
2. **`<repo_context>` injection + `detect_build_cmd`** — pure prompt-content addition, low risk.
3. **Structured Review hook** — depends on (1) for the injection pattern and (2) for the build-command detection.

Each PR includes its own tests, prompt revisions, and doc updates. The CI gates above must pass after every PR (not just the last).

## Verification

Phase 3 sign-off requires:

1. `cargo build --workspace` clean on default features and `--all-features`.
2. `cargo lint` clean.
3. `cargo test` clean including new unit + integration tests.
4. `TOOL_COUNT` matches `BUILTIN_TOOLS.len()` (existing CI test catches this).
5. Manual smoke checklist above.
6. `AICTL_CODING_AGENT=false` regression: a chat-only session shows zero behavior change — no `<repo_context>`, no Review hook fires, no `test` tool retry loop. The `test` tool itself remains callable (it's a normal tool) but the surrounding orchestration is dormant.
7. The audit log captures `test` tool dispatches with the parsed pass/fail counts so `~/.aictl/audit/<session>` is post-hoc reviewable.

## Risks

- **Test parser drift**: cargo / npm / pytest / go output formats change across versions. A parser miss leaves `passed=0, failed=0, parse_warning=...` which short-circuits the retry loop (no failures detected → release the final answer). Mitigation: on `parse_warning` *and* a non-zero `exit_code`, treat as a failure with no parsed detail; inject a `<test_failure>` block carrying the raw tail. The model then sees something to act on instead of a silent pass.
- **Review hook bricks the loop**: a structured Review failure → re-edit → Review failure → re-edit cycle could exhaust the retry budget on a real-but-unfixable issue (e.g. the failing test is upstream). Mitigation: the retry cap is a hard backstop; on exhaustion the final answer is released with `[review: N failures remain]` and the user gets the unresolved state.
- **Repo-context prompt bloat**: a 60-entry dir tree + 40 dirty files + 5 commits adds ~1 KB of tokens to every coding-mode turn. Mitigation: the caps are configurable; for very large repos the user can drop them to zero (`AICTL_CODING_REPO_CONTEXT=false`) and the block is suppressed entirely.
- **Cached `<repo_context>` stale**: the agent makes an edit, but the next turn still sees the pre-edit snapshot. Mitigation: the host *updates* the changed-files set in-place on every `write_file` / `edit_file` / `remove_file` / `create_directory` call, and the rendered context's "modified files" line reflects the in-session changes; the deeper fields (commits, top-level tree) are still cached because they change rarely.
- **`test` tool wraps `exec_shell` privileges**: anything the model can do with `test` it could already do with `exec_shell`. Mitigation: the new dispatch arm runs through the same `validate_tool` security gate, same shell timeout, same allowed_shells. No new security surface.
- **Review hook drift in non-test projects**: a project with no `Cargo.toml` / `package.json` / etc. has no build command, so the Review hook only runs `git diff --stat` + per-file `lint_file`. Mitigation: when `detect_build_cmd` returns `None`, log "no build command detected — skipping build check" into the `<review_result>` block; the model is informed rather than the hook silently degrading.

## Scope boundaries with other plans

- **Phase 1 (`coding-agent.md`)**: prerequisite. The `WorkflowPhase` machinery and the `coding_agent_enabled()` gate are reused unchanged.
- **Phase 2 (`coding-agent-phase-2.md`)**: orthogonal. The smarter `edit_file` and ripgrep-backed search benefit Phase 3 (Review can grep faster, edits land more reliably), but Phase 3 doesn't depend on Phase 2 having shipped. If Phase 2 ships after Phase 3, the Review hook's "lint each changed file" path just runs against today's lint shape.
- **Phase 4 (`coding-agent-phase-4.md`)**: the structured Review hook benefits from parallel tool execution (build + lint runs could parallelize). Phase 4 will revisit `run_structured_review` to switch the sequential `.await` chain to a `JoinSet`.
- **Java/Kotlin/C detection (`coding-agent-detect-jvm-c.md`)**: Phase 3 ships `detect_build_cmd` for the four languages the codebase already detects (Rust/Node/Python/Go). The JVM/C plan plugs additional detectors into both `detect_build_cmd` and the existing `detect_linter` / `detect_test_cmd` — no Phase 3 changes needed beyond reading the augmented detector results.

## Open questions

- **Should the `test` tool be coding-mode-only?** Lean no — it's useful in any session ("run the project's tests for me"). The *retry loop* is coding-mode-only; the tool itself is universal. Keeps the surface predictable.
- **`<repo_context>` for non-git directories**: today the helper returns `None` for branch / commits / dirty when not in a git repo. Should it still render the dir tree + detected commands, or omit the whole block? Lean "still render" — the dir tree and detected commands are useful regardless of VCS.
- **Review hook on Plan-phase output**: when the model produces a final-answer-shaped response from the Plan phase (e.g. "here's the plan" with no tool calls), should Review fire? Lean no — the Review hook is gated on "phase ∈ {Code, Review}". Plan-phase final answers go straight through, matching the v1 behavior.
- **Test-failure prompt budget**: a `<test_failure>` with 25 failures in 400-char messages is ~10 KB of tokens. Reasonable, but should we cap harder for very large test suites? Lean keep the 25/400 caps but make them configurable (`AICTL_CODING_TEST_FAILURES_SHOWN` / `AICTL_CODING_TEST_FAILURE_MESSAGE_LEN`).
- **Multi-language repos**: a repo with both `Cargo.toml` and `package.json` — should the `test` tool run both? Lean no, run the first detected (Cargo wins by precedence in `detect_test_cmd`). The user can `--cmd` override or set `AICTL_CODING_TEST_CMD`. Multi-runner orchestration is out of scope.
- **Desktop Review pill auto-fade duration**: 4 s feels right but undertested. Revisit after dogfooding.
