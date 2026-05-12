# Plan: Coding Agent Mode

## Context

Today aictl is a single general-purpose agent — every session uses the same `SYSTEM_PROMPT` from `crates/aictl-core/src/config.rs`, the same tool catalogue, and the same loose flow ("LLM decides, user approves, repeat"). That's fine for chat-shaped work (explain this code, refactor this function, draft a commit message), but it leaves real coding sessions under-instructed: the model jumps to edits without reading enough, doesn't run tests by default, and rarely reviews its own diff before declaring victory.

The roadmap's "Coding Agent" section proposes a dedicated mode that flips the general-purpose agent into a coding-specialist agent: a stricter five-phase workflow (Explore → Plan → Code → Review → Test), an opinionated system prompt baked for code-editing sessions, and a richer set of code-aware tooling. The mode is **CLI-only**. The server has no agent loop ([`server.md`](done/server.md) is a pure proxy), and the desktop frontend is a chat-shaped UX that doesn't fit a multi-phase workflow; both stay on the general-purpose prompt.

This plan covers Phase 1 of the coding agent: the on/off setting, the prompt-override mechanism, the workflow loop, and the minimum CLI surface to turn the feature on. Smarter `edit_file`, ripgrep-backed search, automatic test-loop integration, and dedicated `test` / extended `git` tooling are sequenced into Phase 2+ so v1 can ship behind a feature gate without dragging a 20-file refactor along.

## Goals & Non-goals

**Goals**

- Add a single config knob `AICTL_CODING_AGENT=true` (default **`false`**) that activates coding-agent mode for the CLI. When off, behavior is identical to today.
- When on, replace the general-purpose `SYSTEM_PROMPT` with a coding-specialist prompt at the same seam (`run::build_system_prompt`) — the override applies for the whole session.
- Reflect the active mode in the REPL prompt and the `--info` banner so the user always knows which agent they're talking to.
- Enforce the five-phase Explore → Plan → Code → Review → Test workflow via prompt steering (not tool gating) and a lightweight `WorkflowPhase` enum tracked in the agent loop.
- Expose the on/off toggle through three surfaces: the config file (persistent), a CLI flag (`--coding-agent` / `--no-coding-agent`, one-launch override), and a `/coding-agent` slash command (live toggle in the REPL).
- Hard-block the mode in `aictl-server` and `aictl-desktop`. The config key is read by `aictl-core`, but the activation seam is gated on `Role::Cli`.

**Non-goals**

- **Not a redesign of the tool surface.** No new tools land in v1 beyond what the existing catalogue already provides (`read_file`, `search_files`, `find_files`, `list_directory`, `edit_file`, `write_file`, `git`, `exec_shell`, `lint_file`, …). Phase 2+ may add a dedicated `test` tool, ripgrep-backed search, and extended `git` subcommands.
- **Not a parallel agent system.** Coding-agent mode reuses `run::run_agent_turn` exactly — only the system prompt and phase tracker change. No second loop, no new dispatcher.
- **Not server- or desktop-visible.** The desktop's Tauri commands and the server's HTTP routes never branch on `AICTL_CODING_AGENT`. Setting the key in `~/.aictl/config` with no CLI in play is a no-op.
- **Not a replacement for `--agent <name>`.** The named-agent system (persistent persona prompts under `~/.aictl/agents/`) keeps working. Coding-agent mode is orthogonal: it overrides the *base* system prompt, then a loaded agent / project prompt / behavior override append on top in the existing order.
- **No auto-detection.** The mode is explicit. We do not heuristically activate coding mode when the user runs aictl inside a git repo — too easy to be wrong, and the "I just want to ask a quick question" path would be ruined.
- **No multi-tenant coding sessions.** One mode setting per process. No per-prompt or per-message overrides.
- **No required user approval gate on the Plan phase in v1.** The roadmap mentions presenting the plan for approval before proceeding. We log the plan and proceed, with an `AICTL_CODING_PLAN_APPROVE=true` opt-in for users who want the interactive checkpoint. Defaults favor flow over ceremony; users who want the checkpoint flip the flag.

## How it differs from existing extension points

|                  | Coding-agent mode                          | Loaded agent                       | Skill                              |
|------------------|--------------------------------------------|------------------------------------|------------------------------------|
| Scope            | Whole session                              | Whole session                      | Single turn                        |
| Seam             | Replaces the base system prompt            | Appends after the base prompt      | Concatenated onto `messages[0]`    |
| Activation       | Config / `--coding-agent` / `/coding-agent`| `--agent <name>` / `/agent`        | `/<skill>` / `--skill <name>`      |
| What it changes  | Base prompt + workflow loop + UI hints     | Just the appended persona block    | One-turn procedure                 |
| Frontends        | CLI only                                   | CLI, desktop                       | CLI, desktop                       |

The three compose cleanly: coding-agent mode sets the base, a loaded agent narrows the persona ("Rust expert in coding mode"), and a skill drops a one-turn procedure on top ("apply the standard commit recipe in this Rust-expert coding session").

---

## Design

### 1. Configuration

One new key in `~/.aictl/config`:

```
AICTL_CODING_AGENT=false                    # default; flip to true to activate
AICTL_CODING_PLAN_APPROVE=false             # default; opt-in to interactive Plan-phase checkpoint
AICTL_CODING_SKIP_REVIEW=false              # default; allows skipping the Review phase
AICTL_CODING_SKIP_TEST=false                # default; allows skipping the Test phase
AICTL_CODING_TEST_RETRIES=3                 # default; max Code → Review → Test re-loops on failure
AICTL_CODING_LINTER=                        # empty = auto-detect (cargo clippy / eslint / ruff / …)
AICTL_CODING_TEST_CMD=                      # empty = auto-detect (cargo test / npm test / pytest / …)
```

`AICTL_CODING_AGENT` is the master switch; everything else is workflow tuning that has no effect when the master switch is off. CLI flags override config for a single launch.

**Config helpers**: a small accessor in `aictl-core::config` so call-sites don't sprinkle `config_get("AICTL_CODING_AGENT")` lookups everywhere:

```rust
pub fn coding_agent_enabled() -> bool;
```

The accessor returns `false` whenever `config::role()` is `Role::Server` (the server has no agent loop) so the answer is honest no matter who loads the engine. The desktop frontend never reads the key — the Tauri commands hand the engine a normal `run_agent_turn` call, and the coding-agent gate (§3) returns the default prompt regardless of config when not invoked from the CLI's REPL path.

### 2. Coding-agent system prompt

A new `SYSTEM_PROMPT_CODING` constant sits next to `SYSTEM_PROMPT` and `SYSTEM_PROMPT_CHAT_ONLY` in `crates/aictl-core/src/config.rs`. Same shape (XML tool spec at the top, tool catalogue, rules block at the bottom), different *prose*:

- **Role framing**: "You are aictl in coding-agent mode. You are a careful, disciplined coding collaborator who reads code before changing it, plans before editing, edits minimally, and verifies before declaring done."
- **Five-phase workflow**: explicit description of Explore → Plan → Code → Review → Test, with a paragraph per phase and the cue that the LLM should signal phase transitions via a `<phase>NAME</phase>` tag at the start of a turn (`explore`, `plan`, `code`, `review`, `test`). The tag is **optional** — when omitted, the loop infers the phase from tool usage (first `write_file` / `edit_file` transitions to Code; final answer without tool calls transitions to Review/Test depending on flags).
- **Phase-specific rules**:
  - *Explore*: prefer `read_file`, `search_files`, `find_files`, `list_directory`, `git status` / `git log` / `git blame` / `git diff`. Do not edit yet.
  - *Plan*: produce a numbered plan (what to change, where, why). Wait for approval if `AICTL_CODING_PLAN_APPROVE=true`. Otherwise proceed.
  - *Code*: apply minimal focused edits via `edit_file` / `write_file`. Follow existing code style and conventions.
  - *Review*: run `git diff`, run `lint_file` on changed files (or the project linter via `exec_shell` when `AICTL_CODING_LINTER` is set), confirm only intended files changed.
  - *Test*: run the detected test command. On failure, loop back to Code → Review → Test up to `AICTL_CODING_TEST_RETRIES` times.
- **Tooling discipline** (drawn from the roadmap's "Coding-specific system prompt" section):
  - Read files before editing.
  - Run tests after changes.
  - Don't introduce security vulnerabilities (OWASP top 10).
  - Prefer minimal changes — three similar lines beats a premature abstraction.
  - Diagnose errors before retrying.
  - Check git status before and after changes.
- **Same XML tool spec and the same tool catalogue** as `SYSTEM_PROMPT`. We do not hide tools in coding mode; we just steer their use.
- **Rules footer**: identical to the base prompt's "one tool call per response, no tool tags when answering normally, show reasoning before tool calls."

The full prompt body lives in `config.rs` as a `pub const` so it's part of the binary, not loaded from disk. That keeps the activation seam fast and avoids a "did the prompt file ship?" failure mode. A future plan can lift it to a bundled markdown asset if maintenance pressure justifies it.

### 3. Prompt-override seam

`run::build_system_prompt` is the single place every frontend's system prompt is assembled. Today the function picks between `SYSTEM_PROMPT` and `SYSTEM_PROMPT_CHAT_ONLY` based on `tools::tools_enabled()`. We extend it to also check `config::coding_agent_enabled()`:

```rust
pub fn build_system_prompt() -> String {
    let base = match (tools::tools_enabled(), config::coding_agent_enabled()) {
        (false, _)     => SYSTEM_PROMPT_CHAT_ONLY,    // tools off wins — coding agent without tools is meaningless
        (true, true)   => SYSTEM_PROMPT_CODING,
        (true, false)  => SYSTEM_PROMPT,
    };
    // … rest unchanged: plugins, MCP, project prompt file, loaded agent,
    // behavior override, memory block append after the base.
}
```

Three properties of this seam:

1. **One file, one function.** Every frontend (CLI, server, desktop) calls the same `build_system_prompt`. The server and desktop are kept out by the `Role::Server` short-circuit inside `coding_agent_enabled()` — they read `false` no matter what `~/.aictl/config` says. The CLI sets `Role::Cli` by default, so it sees the real value.
2. **Coding mode replaces the base; everything else still appends.** A loaded agent (`# Agent: rust-expert`), the project prompt file (`AICTL.md`), the behavior override (`~/.aictl/AICTL.md`), and the memory block all still concatenate on top of the coding-agent base — they don't get clobbered. Persona refinements compose with the coding-agent baseline.
3. **`tools::tools_enabled() = false` always wins.** Coding-agent mode is meaningless without tools. If a user turned tools off for some reason, coding mode silently degrades to the chat-only prompt and the workflow loop is bypassed.

### 4. Workflow phase tracker

`run_agent_turn` gains a lightweight phase tracker. The CLI passes a `WorkflowPhase` into the loop when coding mode is on; the server and desktop never construct one.

```rust
pub enum WorkflowPhase {
    Explore,
    Plan,
    Code,
    Review,
    Test,
}
```

State machine:

- Session starts in `Explore`.
- Phase transitions happen in three ways, checked in order each turn:
  1. **Explicit signal**: the model emits `<phase>NAME</phase>` at the start of its response. Parsed alongside the tool-call XML (same `tools.rs` parser path, new opcode). Strips the tag from the visible output so the user doesn't see the marker.
  2. **Implicit signal**: first `write_file` or `edit_file` tool call transitions to `Code`; final answer with no tool calls and no prior plan transitions to `Review` (or `Test` if review is disabled).
  3. **User command**: `/skip` in the REPL forces the next phase (`Explore` → `Plan` → `Code` → `Review` → `Test`). `/skip-review` and `/skip-test` jump straight to the next-next phase.
- On `Review` entry, the loop appends a turn-scoped instruction to the next system message: "You are entering the Review phase. Run `git diff` to confirm the intended changes, run the linter on changed files, verify the original request is addressed. If issues, return to Code." This mirrors the skill mechanism — the appended block lives for exactly one turn.
- On `Test` entry, similarly: "Run the project's test command. Parse output for failures. On failure, fix and re-test up to `AICTL_CODING_TEST_RETRIES` times."
- The phase is **non-persistent**. A reloaded session restarts in `Explore`. We don't try to recover the exact phase from session history — the LLM re-derives it from the conversation context.

The phase tracker is a CLI-side concern. The engine exposes a small hook so the CLI can inject phase-specific guidance per turn:

```rust
pub fn build_system_prompt_with(phase_hint: Option<&str>) -> String;
```

`phase_hint` defaults to `None` (no change from today). When `Some(s)`, the string is appended after the coding-agent base as a "Phase guidance" block. Server and desktop never pass anything in.

### 5. CLI surface

**Long-form flags** on `aictl`:

- `--coding-agent` — force coding-agent mode on for this launch. Wins over config.
- `--no-coding-agent` — force coding-agent mode off for this launch. Wins over config.

These follow the existing long-flag-only convention.

**Slash command**: `/coding-agent` in the REPL. Behavior:

- No argument: prints the current state (on/off) and a one-liner about how to flip it.
- `on` / `off` / `toggle`: flips the in-memory `config_overlay` *and* writes through to `~/.aictl/config` via `config_set`. The user sees an immediate confirmation, and the new state persists across launches.
- The base system prompt is re-assembled on the next turn, so the new mode takes effect immediately without needing to reload the session.

**`--info` banner**: a new line shows the current coding-agent state (`coding-agent: on` / `coding-agent: off`). Drops in next to the existing role/provider/model/agent lines so the user has a single glance to confirm.

**REPL prompt indicator**: when coding mode is on, the prompt prefix shows the current phase in dim brackets:

```
[explore] ❯ read the auth middleware and tell me how sessions are stored
[plan]    ❯ here's the plan…
[code]    ❯ applying edits…
[review]  ❯ running git diff and the linter…
[test]    ❯ cargo test — 42/42 pass
```

When coding mode is off, the prompt looks exactly like today. The `[phase]` prefix is dimmed (low-contrast) so it doesn't dominate the line.

**Single-shot mode**: `aictl --message "fix the bug" --coding-agent` works end-to-end. The five phases execute back-to-back without user prompts (the Plan phase logs to stderr but does not pause unless `AICTL_CODING_PLAN_APPROVE=true`). `--quiet` suppresses the `[phase]` markers in stdout.

### 6. Server and desktop gating

- **Server (`crates/aictl-server`)**: the server already does not call `run::run_agent_turn` — it dispatches directly to `llm::call_<provider>` via `server_proxy`. Coding-agent mode is structurally absent. The `coding_agent_enabled()` accessor short-circuits to `false` when `config::role()` is `Role::Server` so even if an operator sets `AICTL_CODING_AGENT=true` in the shared `~/.aictl/config`, the server reads `false` and ignores it. **Verification**: a CI grep ensures `SYSTEM_PROMPT_CODING` is never referenced in `crates/aictl-server/`.
- **Desktop (`crates/aictl-desktop`)**: the desktop calls `run::run_agent_turn` (it's a real agent loop), so the `coding_agent_enabled()` accessor would normally return `true` if the user flipped the config. To keep the mode CLI-only, the accessor *also* gates on a process-level flag: `coding_agent_active()` returns `true` only when the active role is `Role::Cli` **and** the config key is set. The desktop sets `Role::Desktop` immediately after `load_config` (a new variant in `config::Role`), so it reads `false` and uses the regular prompt. Slash commands and CLI flags that toggle the mode never run on the desktop side — the Tauri command surface doesn't expose them — so there's no UI path that would let a desktop user accidentally enable a CLI-shaped feature. **Verification**: a CI grep ensures `SYSTEM_PROMPT_CODING` is never referenced in `crates/aictl-desktop/`.
- **`config::Role`**: today the enum has `Cli` and `Server`. Add a `Desktop` variant. The desktop's `main.rs` (or its Tauri setup) calls `set_role(Role::Desktop)` after `load_config`. `coding_agent_enabled()` reads `Role::Cli` only.

### 7. Phase-specific tool guidance vs. tool gating

We **steer** via prompt, we do **not block** via gate. Two reasons:

1. The roadmap explicitly chose prompt-steering ("This steers the LLM without hard-blocking tool calls"). The model occasionally needs to read a file during the Code phase to remind itself of context; gating would break that.
2. The security model already lives in `security::validate_tool`. Adding a second axis (phase × tool) doubles the surface that has to stay correct under refactor. Keep coding-agent mode as a *guidance* layer on top of the existing security gate.

Result: every tool stays callable from every phase. The system prompt biases the model toward the right tools per phase ("during Explore, prefer read_file / search_files / find_files / list_directory / git status / git log / git blame / git diff"). If the model deviates, the human reviewer (or the auto-Review phase) catches it.

### 8. Interaction with existing extension points

- **Agents (`--agent <name>`)**: still load on top of the coding-agent base. Concrete example: `aictl --coding-agent --agent rust-expert` produces "coding-agent base prompt + Rust-expert persona block." Both apply.
- **Skills (`/<skill-name>`)**: still one-turn-scoped. A `/commit` skill invoked mid-coding-session injects on top of the coding-agent base for that turn only.
- **Project prompt file (`AICTL.md`)**: still appended in the existing position.
- **Behavior override (`~/.aictl/AICTL.md`)**: still appended in the existing position.
- **Long-term memory**: still appended via `memory::prompt_block()`.
- **MCP / plugins**: still discovered and surfaced in the prompt tool catalogue — the catalogue extension already happens inside `build_system_prompt`, regardless of which base prompt was chosen.

### 9. Auto-detection of linter and test command

When `AICTL_CODING_LINTER` is empty:

- `Cargo.toml` present → `cargo clippy --all-targets -- -D warnings` (or `cargo lint` if a `.cargo/config.toml` alias exists).
- `package.json` present → check for `eslint` in `devDependencies` → `npx eslint .`; else `tsc --noEmit` if `tsconfig.json` exists.
- `pyproject.toml` or `requirements.txt` present → `ruff check` (preferred) → `flake8` → `pyflakes`, first one that resolves.
- `go.mod` present → `go vet ./...` and `gofmt -d`.
- Otherwise: no auto-linter; the Review phase still runs `git diff` and `lint_file` on changed files individually.

When `AICTL_CODING_TEST_CMD` is empty:

- `Cargo.toml` → `cargo test`.
- `package.json` with a `test` script → `npm test`.
- `pyproject.toml` or `pytest.ini` → `pytest`.
- `go.mod` → `go test ./...`.
- Otherwise: the Test phase logs "no test command detected" and skips. The user can set `AICTL_CODING_TEST_CMD` to wire one up.

Auto-detection runs lazily on Review / Test entry, not at session start, so it doesn't slow down the warm-up path. Detection results are cached for the session.

### 10. Runtime shape and integration points

| File | Change |
|------|--------|
| `crates/aictl-core/src/config.rs` | Add `SYSTEM_PROMPT_CODING` const; add `AICTL_CODING_AGENT` and related key names; add `coding_agent_enabled()` accessor; add `Role::Desktop` variant |
| `crates/aictl-core/src/run.rs` | Branch in `build_system_prompt` on `coding_agent_enabled()`; add `build_system_prompt_with(phase_hint)`; thread phase hints through `run_agent_turn` |
| `crates/aictl-cli/src/main.rs` | Add `--coding-agent` / `--no-coding-agent` flags; overlay them onto config at startup via `config_overlay` |
| `crates/aictl-cli/src/commands.rs` | Register `/coding-agent` slash command |
| `crates/aictl-cli/src/commands/coding_agent.rs` | **New** — handler: print status, on / off / toggle, persist via `config_set` |
| `crates/aictl-cli/src/ui.rs` | `InteractiveUI` shows `[phase]` prefix when coding mode is on; threads a `Option<WorkflowPhase>` through the prompt rendering path |
| `crates/aictl-cli/src/repl.rs` (or equivalent agent-loop driver) | Owns the `WorkflowPhase` for the session; updates it on tool dispatch (`Code` on first edit), on `<phase>` tag observation, and on `/skip*` |
| `crates/aictl-cli/src/commands/info.rs` | Add `coding-agent: on/off` line to the banner |
| `crates/aictl-server/` | No code change. CI grep enforces `SYSTEM_PROMPT_CODING` is unreferenced |
| `crates/aictl-desktop/src/main.rs` (or equivalent setup) | Call `set_role(Role::Desktop)` after `load_config` |
| `CLAUDE.md` | Add "Coding-agent mode" paragraph under "Key behaviors (non-obvious)" |
| `README.md` | Short "Coding-agent mode" subsection next to "Agents" / "Skills" |
| `ROADMAP.md` | Remove the "Coding Agent" section when Phase 1 ships; Phase 2+ items move to a follow-up section |

### 11. Testing

**Unit tests**

- `config::coding_agent_enabled()`:
  - Returns `false` when key unset.
  - Returns `false` when key set to `false`.
  - Returns `true` when key set to `true` and `Role::Cli`.
  - Returns `false` when key set to `true` and `Role::Server`.
  - Returns `false` when key set to `true` and `Role::Desktop`.
- `run::build_system_prompt`:
  - Coding mode on + tools on → starts with `SYSTEM_PROMPT_CODING` body.
  - Coding mode on + tools off → starts with `SYSTEM_PROMPT_CHAT_ONLY` body (tools-off wins).
  - Coding mode off + tools on → starts with `SYSTEM_PROMPT` body (today's behavior unchanged).
  - Loaded agent / project prompt / behavior / memory all still append after the coding-agent base.
- `WorkflowPhase` transitions:
  - Fresh session starts in `Explore`.
  - `<phase>plan</phase>` tag transitions to `Plan`.
  - First `write_file` / `edit_file` transitions to `Code`.
  - `/skip` advances exactly one phase.
- Slash command:
  - `/coding-agent` with no arg prints state.
  - `/coding-agent on` writes `AICTL_CODING_AGENT=true` to config and updates the in-memory overlay.
  - `/coding-agent off` writes `false`.
  - `/coding-agent toggle` flips.

**Integration tests** (CLI, with the mock-LLM harness)

- `--coding-agent --message "..."` produces a system prompt containing the coding-agent header, sends it to the mock provider, and the mock asserts the prompt shape.
- `--no-coding-agent` overrides a `true` config key for one launch; config file is unchanged on exit.
- A multi-turn fixture exercises the phase tracker: Explore (3 reads) → Plan (numbered list) → Code (one `edit_file`) → Review (`git diff` + `lint_file`) → Test (mock `cargo test` success), and the final answer surfaces with "Review: clean, 42/42 tests pass."
- A failure-loop fixture has the mock provider return a test failure once, then a fix, then success on retry; verify `AICTL_CODING_TEST_RETRIES` controls the loop limit.

**Manual smoke**

1. `aictl --coding-agent` in a Rust project — issue a small bug-fix request; verify the agent reads first, plans, edits, runs `cargo clippy` and `cargo test`, and reports a clean review.
2. Same, but with `--no-coding-agent` set globally and `--coding-agent` flag on one launch — confirm the flag wins for that launch and the config is unchanged after exit.
3. `/coding-agent toggle` mid-session — confirm the next turn uses the new base prompt and the `[phase]` indicator appears/disappears.
4. Launch `aictl-server` with `AICTL_CODING_AGENT=true` in `~/.aictl/config` — server starts, `/v1/chat/completions` calls do not include the coding-agent prompt.
5. Launch the desktop app with the same config — desktop chat uses the regular `SYSTEM_PROMPT`.

**CI gates**

```bash
# Coding-agent prompt is CLI-only.
grep -rE 'SYSTEM_PROMPT_CODING' crates/aictl-server/src/        # must be empty
grep -rE 'SYSTEM_PROMPT_CODING' crates/aictl-desktop/src/       # must be empty
# Coding-agent UI is CLI-only.
grep -rE 'WorkflowPhase|coding[-_]agent' crates/aictl-server/src/   # must be empty
grep -rE 'WorkflowPhase|coding[-_]agent' crates/aictl-desktop/src/  # must be empty
```

### 12. Documentation

- **`README.md`**: a short "Coding-agent mode" subsection near the existing "Agents" / "Skills" sections — one paragraph on what it does, the on/off knob, and a pointer to the longer doc.
- **`CLAUDE.md`**: a paragraph under "Key behaviors (non-obvious)" — "Coding-agent mode: CLI-only base-prompt override gated by `AICTL_CODING_AGENT` and `Role::Cli`; assembled in `run::build_system_prompt`; phase tracker lives in the CLI agent-loop driver."
- **No new top-level doc file** in v1. If the feature grows into a multi-page reference (extended `git` tool, ripgrep integration, test-loop tuning), Phase 2+ creates `CODING-AGENT.md` next to `SERVER.md` and `ARCH.md`.

---

## Rollout phases

**Phase 1 — this plan**:
1. `SYSTEM_PROMPT_CODING` constant + the `AICTL_CODING_AGENT` master switch.
2. Prompt override in `build_system_prompt`.
3. `--coding-agent` / `--no-coding-agent` flags + `/coding-agent` slash command + `--info` line.
4. `WorkflowPhase` enum + phase-prefix REPL indicator + phase-specific prompt guidance injection.
5. Server / desktop gating via `Role::Server` / `Role::Desktop`.
6. Auto-detection of linter and test command.
7. `--list-skip-*` config knobs for the Review / Test phases.

**Phase 2 — better edit and search** (separate plans):
- Smarter `edit_file` (multi-edit, line-number addressing, fuzzy match) — from ROADMAP "Smarter edit tool" section.
- Ripgrep-backed `search_files` / `find_files` — from ROADMAP "Code-aware search" section.
- Read-file-with-line-numbers and selective reading — from ROADMAP "Read file with line numbers" section.

**Phase 3 — test loop and self-review polish** (separate plan):
- Dedicated `test` tool with structured output parsing.
- Automatic context injection (git branch, recent commits, dir tree) at session start.
- Self-review automation tightening — the existing v1 Review phase is prompt-driven; v3 makes it a structured pre-final-answer hook.

**Phase 4 — parallel tool execution and streaming** (separate plan):
- Parallel tool execution (`tokio::JoinSet`) — from ROADMAP "Parallel tool execution" section.
- Streaming refinements specific to coding mode (real-time `[phase]` updates, etc.).

---

## Verification

Phase 1 sign-off requires:

1. `cargo build --workspace` clean on default features and `--all-features`.
2. `cargo lint --workspace` clean.
3. `cargo test --workspace` clean including the new unit + integration tests in §11.
4. CI greps in §11 pass (coding-agent symbols absent from server and desktop crates).
5. Manual smoke checklist in §11 passes.
6. `AICTL_CODING_AGENT` defaults to `false` when the key is absent from a fresh `~/.aictl/config`. A first-run user sees no behavior change until they opt in.
7. With the key set to `true` in shared config, both `aictl-server --help` and the desktop app launch and operate identically to a config with the key absent.

## Risks

- **Phase tracker drift between LLM and loop**: the model's `<phase>` tag and the loop's implicit inference could disagree, leaving the user looking at `[explore]` while the model has clearly entered Code. Mitigation: keep the indicator best-effort and dim; never gate behavior on it. The Review phase's structured prompts are the ground truth, not the indicator label.
- **Prompt bloat**: `SYSTEM_PROMPT_CODING` adds tokens to every coding-mode turn. Mitigation: keep it tight and prose-heavy; do not duplicate the tool catalogue (the existing catalogue extension already appends below). Measure the token cost in a smoke run before merging.
- **Auto-detection false positives**: detecting `package.json` and running `npm test` in a monorepo with no test script could fail noisily. Mitigation: detect *and verify* the script exists before running; on missing-script, log a one-line note and skip the Test phase. Users with monorepo layouts set `AICTL_CODING_TEST_CMD` explicitly.
- **CLI-only enforcement leaks**: a contributor adds a desktop Tauri command that calls `set_role(Role::Cli)` for some unrelated reason, accidentally re-enabling coding mode. Mitigation: the CI grep on `SYSTEM_PROMPT_CODING` plus a `#[cfg(test)]` invariant test in `aictl-core` that pins `coding_agent_enabled()` to `false` under each non-Cli role.
- **User confusion between coding-agent mode and a loaded coding agent**: `aictl --agent code-reviewer` is *not* coding-agent mode. We already have an "agent" concept (the loaded-prompt extension). Mitigation: the `/coding-agent` command name and the README phrasing both lean on the word "mode" rather than "agent" in user-facing copy. The internal name `WorkflowPhase` (not `CodingPhase`) also helps.

## Scope boundaries with other plans

- **Server (`server.md`)**: no overlap. Server has no agent loop; coding mode is structurally absent.
- **Modular architecture (`modular-architecture.md`)**: prerequisite. The seam we extend (`run::build_system_prompt`) lives in `aictl-core` because of the modular split. No new modular work needed here.
- **Skills (`skills.md`)** and **Agents (`agent-templates.md`)**: orthogonal. Coding-agent mode overrides the base prompt; agents and skills append on top.
- **MCP (`mcp-support.md`)**, **Plugins (`plugin-system.md`)**: orthogonal. Their tool catalogues continue to be appended to `build_system_prompt` regardless of which base is chosen.
- **Desktop app (`desktop-app.md`)**: explicit non-target. Coding-agent mode does not run in the desktop. A future "desktop coding workspace" would be a separate plan with its own UX shape.

## Open questions

- **Plan-phase approval default**: ship with `AICTL_CODING_PLAN_APPROVE=false` (proceed silently) or `=true` (require Enter to confirm)? Lean false for flow; users who want the ceremony flip the bit. Revisit after dogfooding.
- **`<phase>` tag visibility**: strip from visible output (current plan) or render as a dim `[Phase: code]` line so the user sees the model's self-reported phase? Stripping is cleaner; rendering helps debugging. Possibly a `AICTL_CODING_SHOW_PHASE_TAG=true` debug flag.
- **Where does the phase tracker live?**: today this plan puts it in the CLI agent-loop driver (`repl.rs` or the equivalent). An argument for putting it in `aictl-core::run` is that future frontends might want phase awareness without re-implementing the state machine. Defer the move until a second frontend actually asks for it.
- **`/coding-agent` config-set side effect**: persisting the toggle to disk on every flip is convenient but surprising — the user runs `/coding-agent on` to try the mode for one session and finds it still on tomorrow. Alternative: in-memory only by default, with `/coding-agent on persist` to write through. Lean toward "persist by default with a clear confirmation message" so the behavior matches the config-file mental model, but flag for feedback.
- **Auto-Review enable-by-default**: should `AICTL_CODING_SKIP_REVIEW` default to `false` (Review always runs) or should we let the user opt into the Review phase? Lean "always runs" — the whole point of coding mode is the discipline.
- **Test-failure loop UX**: when the Test phase fails and we re-enter Code, do we surface the test output to the user immediately, or only after the retry budget is exhausted? Lean "immediately" — surprise failures merit surprise feedback, even if the loop ultimately recovers.
