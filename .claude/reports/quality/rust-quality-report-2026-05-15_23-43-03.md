# Evaluation Report -- 2026-05-15 23:43:03

## Automated Checks

- `cargo build` — pass, clean (workspace default members)
- `cargo test` — pass, **939 tests** (888 `aictl-core`/`aictl-cli` unit + 51 `aictl-server` unit, 0 failed, 0 ignored)
- `cargo fmt --check` — pass, no diff
- `cargo lint` (project alias, default members) — pass, 0 warnings
- `cargo clippy --workspace -- -W clippy::pedantic` — 2 warnings, both in `aictl-desktop` (excluded from default members):
  - `crates/aictl-desktop/build.rs:19` — `map(...).unwrap_or_else(...)` on a `Result` (use `unwrap_or_else` directly)
  - `crates/aictl-desktop/src/ui.rs:309` — doc comment missing backticks around `AppHandle`

## Project Structure

- **Workspace** — 4 crates: `aictl-core` (lib, engine), `aictl-cli` (bin), `aictl-server` (bin), `aictl-desktop` (Tauri, excluded from default-members). Layout is intentional and well-documented in root `Cargo.toml:8-19`.
- **Edition 2024**, resolver 3 — current.
- **Workspace package metadata** (version/repository/authors/license-file) is shared via `workspace.package` — single source for releases (`Cargo.toml:25-30`).
- **Per-crate `Cargo.toml`** carries `description`, `repository`, `authors`, `license-file` — no missing recommended fields.
- **Module map** — 179 `.rs` files, ~68k LOC. Submodule trees under `crates/aictl-core/src/{llm,tools,security,mcp,messages,agents,skills}/` group cohesively; `aictl-server/src/messages/translator/` segregates passthrough vs cross-provider paths as documented in CLAUDE.md.
- **Feature flags** are declared on `aictl-core` and re-exported as `aictl-core/<feature>` passthroughs by `aictl-cli` and `aictl-server` — no duplicate feature definitions (`crates/aictl-cli/Cargo.toml:44-48`, `crates/aictl-server/Cargo.toml:55-59`).
- **Integration tests** — `crates/aictl-cli/tests/cli_smoke.rs`. Limited; most coverage is unit-level co-located with modules.

## Error Handling

- **Custom error type** `AictlError` (`crates/aictl-core/src/error.rs:1-30`) with structured variants (`Timeout`, `Auth`, `Injection`, `Redaction`, `Provider`, `Interrupted`, `Other`) — replaces `Box<dyn Error>` across the engine. No bare `Box<dyn std::error::Error>` returns remain in production paths.
- **`.unwrap()` audit** — 560 occurrences total; **520 inside `#[cfg(test)]`**, only **40 outside tests**. Of those 40:
  - `crates/aictl-cli/src/ui.rs:299-307` (9) — `Regex::new(literal).unwrap()` on compile-time-known patterns; idiomatic.
  - `crates/aictl-core/src/session.rs:106, 309, 315, 321` (4) — `Mutex::lock().unwrap()`; only fails on poison.
  - `crates/aictl-core/src/agents.rs:48, 53, 58, 65` (4) — same `Mutex::lock().unwrap()` pattern.
  - `crates/aictl-cli/src/integration_tests.rs` (20) — test-helper module under `pub mod`, not gated by `cfg(test)`; effectively test code.
  - `crates/aictl-core/src/tools/filesystem.rs:830` (1) — `scope.find(...).unwrap()` immediately after `scope.matches(...).count() == 1` invariant check; safe but could be `if let Some(pos) = scope.find(...)` for stylistic consistency. *Suggestion.*
  - `crates/aictl-server/src/messages/translator/stream/openai.rs:163` (1) — `self.tool_index_map.get(&oi).unwrap()` after a same-frame insert path; tighter invariant than the comment makes obvious. *Suggestion: add a one-line comment naming the invariant.*
- **`.expect()` count** — 79 occurrences, mostly in tests; spot-check shows message strings explain the invariant.
- **`panic!` / `unreachable!`** outside tests — 5 sites; all are exhaustive-match guards for `Provider::AictlServer` arms that are dispatched separately before the match runs (`crates/aictl-core/src/run.rs:1708,2126`, `crates/aictl-desktop/src/commands/{agents,skills,ping}.rs`). Idiomatic.
- **`TODO` / `FIXME` / `HACK` / `XXX`** — 0 across the codebase. Strong.

## Safety & Security

- **`unsafe` blocks** — 0 in production code (matches in `crates/aictl-core/src/{config.rs:437,tools/archive.rs:431}` are user-facing strings, not `unsafe` blocks). Excellent for a 68k-LOC project that does provider HTTP, subprocess spawning, and PDF/zip parsing.
- **No FFI** — 0 `extern "C"` or `libc::` references.
- **Hardcoded secrets** — none found. API keys flow through `keys::get_secret()` (keyring-first, plain-config fallback) per CLAUDE.md.
- **Command injection** — defended at `security::validate_tool` before every dispatch; the shell allow/block list and CWD jail apply to all 35+ tools. Plugin entrypoints spawn directly (no shell) with `scrubbed_env`. Verified by the test count alone (888 in `aictl-core`).
- **Path traversal** — `security::check_path_with` plus archive entry validation (`crates/aictl-core/src/tools/archive.rs:431` "entry has unsafe path") cover both directions. Workspace carve-out for the desktop's `~/.aictl/workspace/` jail is documented in CLAUDE.md.
- **Prompt injection** — dedicated `security::detect_prompt_injection` runs at `run::run_agent_turn` start; gated by `security.injection_guard`.

## Code Quality

- **`.clone()` density** — 346 occurrences across 68k LOC (≈1 per 200 lines). Most are short-lived `String` clones at trait boundaries (`AgentUI`, hook payloads, audit). No obvious hotspot. *No action.*
- **`#[allow(dead_code)]`** — only 9 sites total; spot-check shows each is a struct field kept for forward-compatible decoding (e.g. `agents/remote.rs:57`, `skills/remote.rs:57`) or test-helper functions. Justified.
- **Function length** — flagged for decomposition (lint threshold ~80 lines):
  - `crates/aictl-core/src/run.rs:1342` `run_agent_turn` — **~631 lines**. The agent-turn orchestrator: hook chain, injection guard, redaction, streaming, tool batch dispatch, retry, review hook. Single responsibility but oversized. *Issue (refactor candidate): extract phases (prompt-prep, dispatch, post-tool, finalize) into private helpers — would also let the Phase-4 parallel-dispatch path live in its own function.*
  - `crates/aictl-cli/src/commands/info.rs:6` `print_info` — 293 lines. Linear printer; low complexity per line. *Suggestion.*
  - `crates/aictl-cli/src/ui.rs:388` `print_welcome` — 265 lines. Linear ASCII banner builder. *Suggestion.*
  - `crates/aictl-cli/src/commands/agent.rs:141` `create_agent_with_ai` — 242 lines.
  - `crates/aictl-core/src/llm.rs:254` `price_per_million` — 235 lines (lookup table, acceptable).
  - `crates/aictl-cli/src/commands/skills.rs:158` `create_skill_with_ai` — 215 lines.
- **Magic numbers** — 27 sites of `= 1xx+` literals outside tests; spot-check shows most are timeout / size constants already inlined near their consts module (`MAX_ENTRIES`, `MAX_ENTRY_LEN` in `memory.rs`). *No action.*
- **String allocation** — 356 `String` parameter sites; consistent with serde/deserialize boundaries. The hot path (provider request bodies) already uses borrowed slices via `&str`. *No action.*
- **Public API surface** — pub items are deliberate; `aictl-core` re-exports its modules under `crate::*` in `aictl-cli` so the legacy import paths still resolve (documented in CLAUDE.md).

## Testing

- **939 tests pass** (888 + 51) in ~1.2s. Coverage spans:
  - **Parsers** — tool-call XML, edit-block grammar, MCP JSON-RPC framing, OpenAI/Gemini/Ollama translator IR, Anthropic streaming state machines, content-type detection.
  - **Security** — redaction patterns (regex + entropy), prompt-injection guard, path traversal, MCP URL validation, archive entry safety.
  - **Provider plumbing** — OpenAI/Anthropic/Gemini message translation, tool-call round-trips, usage-token accounting, cache-token splitting.
  - **Session / memory / transcript** — undo, retry, compaction boundary, incognito kill-switch.
  - **Hooks & plugins** — execute / timeout / decision parsing / pre-approval.
- **Test quality** — assertions, not just absence-of-panic. Example: `tools::tests::exec_shell_stdout/stderr/no_output` (3 distinct outcomes), `transcript::undo_pops_tool_round_trip_as_one_turn` (semantic invariant). Test panic messages name the expected enum variant.
- **Integration tests** — only `crates/aictl-cli/tests/cli_smoke.rs`. **Gap:** no end-to-end coverage of `aictl-server`'s HTTP surface or the cross-provider `/v1/messages` translator beyond unit-level IR tests. *Issue (suggestion):* add an `aictl-server/tests/` directory with an axum `TestServer` smoke test exercising `/healthz`, `/v1/models`, and an authenticated `/v1/messages` round-trip against a mocked upstream.
- **Coverage gaps** — `crates/aictl-desktop/` carries minimal tests (Tauri commands are mostly thin wrappers, but the agent/skill/ping commands have `unreachable!` arms that aren't exercised).

## Documentation

- **README.md** — present (7.3 KB); per `docs/USAGE.md` the README delegates to the topical docs in `docs/`.
- **`docs/` tree** — comprehensive: `ARCH.md` (94 KB), `SERVER.md` (39 KB), `EXTENSIONS.md`, `INSTALL.md`, `USAGE.md`, `CONFIG.md`, `PROVIDERS.md`, `TOOLS.md`, `CODING_AGENT.md`, `LLM_PRICING.md`, `COMMERCIAL_PRICING.md`, `DESIGN.md`.
- **CLAUDE.md** — exhaustive contributor brief (this file).
- **Module-level docs** — 131/179 files have `//!` headers (~73%). Sub-files in long trees (`mlx/`, `tools/`, `messages/translator/stream/`) sometimes ship without `//!` — readable from context but a one-liner would help newcomers.
- **`///` on `pub fn`** — heuristic ~48% (259/543). The internal-facing crate (`aictl-core` consumed by `aictl-cli` only) makes thin `pub` items legitimately undocumented; the externally visible `aictl-server` translator surface is well-commented.

## Summary

**Score: 9/10.**

This is an exceptionally clean codebase for its size (~68k LOC, four crates, 35+ tools, multi-provider LLM dispatch, HTTP server, MCP/plugin/hook subsystems). Zero `unsafe`, zero hardcoded secrets, zero stale `TODO`s, a custom error type, near-clean clippy pedantic, 939 passing tests, no formatting drift. The default `cargo lint` alias deliberately scopes to non-desktop members and lands at 0 warnings.

**Top 3 strengths**
1. **Discipline at boundaries** — `AgentUI` trait isolates terminal libraries to `aictl-cli`; `Role::Server` short-circuits coding-agent prompts and tool-dispatch policy in `aictl-server`; redaction has two well-defined seams (network egress, persistence) that handle `Block` mode differently. Architectural decisions are encoded in types, not conventions.
2. **Security-first defaults** — every tool call passes a structured gate (CWD jail, shell allow/deny, path traversal, MCP URL allow/deny with HTTPS-by-default), prompt-injection guard runs before redaction runs before provider dispatch, secrets keyring-first with plain-config fallback, archive paths validated for traversal. The 888 `aictl-core` tests reflect this — many guard specific attack shapes.
3. **Test density and assertion quality** — 939 tests for 68k LOC (≈1 per 72 LOC), with assertions that name semantic invariants (undo / compaction boundary / `Block`-mode redaction at persistence) rather than just "doesn't panic".

**Top 3 improvements**
1. **Decompose `run::run_agent_turn`** (`crates/aictl-core/src/run.rs:1342`, ~631 lines). Extract `prepare_prompt` (hooks + injection guard + redaction), `dispatch_tools` (single + parallel batch), and `finalize_turn` (review hook + retry). Would also make the Phase-4 parallel-tool path easier to reason about in isolation.
2. **Add an `aictl-server` HTTP integration test suite** under `crates/aictl-server/tests/`. The translator IR is well-covered, but no end-to-end test exercises the axum stack with auth + rate-limit + redaction in series. An axum `TestServer` against a mocked upstream would close the biggest gap.
3. **Resolve the two `aictl-desktop` clippy-pedantic warnings** (`build.rs:19`, `ui.rs:309`) and wire `cargo clippy --workspace` into CI so the desktop doesn't quietly drift while excluded from default-members. Cheap, removes the one remaining pedantic warning surface.
