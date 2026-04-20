# Evaluation Report -- 2026-04-20 18:28:48

Quality audit for `aictl` v0.27.0 — single-binary async Rust CLI, ~31k LOC across 80 source files.

## Automated Checks

- **clippy** (`-W clippy::all -W clippy::pedantic`): **PASS** — 0 warnings
- **cargo test**: **PASS** — 635/635 passed, 0 failed, 0 ignored
- **cargo fmt --check**: **PASS** — no diffs
- **cargo build**: **PASS** — clean

## Project Structure

- Rust edition **2024** (`Cargo.toml:4`) — current.
- `Cargo.toml` has `description`, `repository`, `authors`, `license-file` (well-formed). No `readme` or `keywords`/`categories` (minor — publishing metadata).
- 80 `.rs` files, clear tree: top-level modules + `llm/` (providers, incl. `mlx/` arch submodule), `tools/` (31 tools), `commands/` (slash commands), `security/redaction*` — responsibility per file is clear.
- Largest files: `src/security.rs` (1630), `src/security/redaction.rs` (1513), `src/tools/csv_query.rs` (1095), `src/ui.rs` (1021). Each is single-responsibility; still candidates for decomposition when they grow further.
- Three optional features (`gguf`, `mlx`, `redaction-ner`) are documented inline in `Cargo.toml:45-69` — good practice.

## Error Handling

- `.unwrap()`: 260 occurrences across 26 files — but only **9 outside tests**.
  - `src/session.rs:105,291,297,303` — all `CURRENT.lock().unwrap()` on `Mutex<Option<Session>>` (idiomatic; poison only on panic-in-lock).
  - `src/agents.rs:9,14,19,26` — same pattern, `LOADED_AGENT.lock().unwrap()`.
  - `src/ui.rs:624` — `ProgressStyle::with_template("{spinner} {msg}").unwrap()` on a compile-time-valid template.
  - **Suggestion**: none urgent; all are load-bearing-safe. Could wrap the mutex helpers in a single `with_current(|s| ...)` helper to centralize the unwrap.
- `.expect()`: 22 occurrences, all inside `#[cfg(test)]` or on compile-time-constant regexes.
- `panic!` / `unreachable!` / `todo!`: 39 hits total — **all** in test modules, or in `Provider::Mock => unreachable!("Provider::Mock is test-only...")` branches with explicit justification (e.g. `src/commands/agent.rs:264`, `src/run.rs:725`, `src/commands/compact.rs:129`, `src/main.rs:464`). No `todo!` anywhere. Good.
- `Box<dyn std::error::Error>`: 50 occurrences across 17 files, concentrated in `src/llm/*.rs` and `src/tools/*.rs`. **Suggestion**: a unified `AictlError` enum (`thiserror`) would give callers structured variants (timeout / auth / parse / IO) instead of stringly-typed matching.

## Safety & Security

- **`unsafe`**: 0 blocks / functions / impls. Excellent.
- **Shell execution** (`sh -c`): 4 sites — all safe.
  - `src/tools/shell.rs:4-5` — the `exec_shell` tool itself; passes through `security::validate_tool()` before dispatch and env-scrubs via `scrubbed_env()`.
  - `src/tools/clipboard.rs:185-187` — `format!("command -v {bin}")` where `bin` is a `&'static str` from a hardcoded backend list (not user input). Safe.
  - `src/commands/update.rs:65-67, 114-116` — hardcoded `UPDATE_CMD` constant. Safe.
- **Hardcoded secrets**: none. Matches on `AIza…`/`ghp_…`/`sk-…` patterns are only in `src/security/redaction.rs:1060,1067` test fixtures, which is the correct place. Key storage goes through `keys::get_secret` (keyring + plaintext fallback), as documented in `CLAUDE.md`.
- **Path handling**: jailed by `SecurityPolicy::check_path_with` (`src/security.rs:6138`, 92 lines); `validate_tool()` (`src/security.rs:5695`) runs on every tool call. No raw path concatenation observed in tool impls.
- **Prompt-injection guard**: present in `security.rs` with a dedicated 169-line test at line 6894 — good defensive coverage.

## Code Quality

- `#[allow(dead_code)]` / `#[allow(unused_assignments)]`: **4 total** (`src/run.rs:78,522`, `src/llm/mock.rs:89`, `src/llm/stream.rs:108`). Very low — acceptable.
- `.clone()`: 104 occurrences across 40 files — mostly `String` clones into owned struct fields, which is appropriate for the async/spawn boundaries used here. No obvious hot-path offenders.
- **Long functions (>80 lines, non-test)**:
  - `src/repl.rs:295` `handle_repl_input` — **332 lines** (command dispatch; large match)
  - `src/main.rs:23` `main` — **304 lines**
  - `src/commands/config_wizard.rs` `run_config_wizard` — **268 lines**
  - `src/commands/info.rs` `print_info` — **194 lines**
  - `src/commands/agent.rs:...` `create_agent_with_ai` — **193 lines**
  - `src/llm/gguf.rs` `call_gguf` — **193 lines**
  - `src/commands/compact.rs` `compact` — **171 lines**
  - `src/llm.rs:...` `price_per_million` — **163 lines** (big lookup table; acceptable)
  - `src/llm/anthropic.rs` `call_anthropic` — **153 lines**
  - Others in 80–120 range (`call_kimi`, `call_deepseek`, `call_gemini`, `validate_tool`, `policy_summary`, etc.).
  - **Suggestion**: `handle_repl_input` and `main` are the best candidates for extraction into per-branch helpers. The provider `call_*` fns are long but symmetric across providers — decomposition should be done once and propagated, not piecemeal.
- **Magic numbers**: `max_iter` (iteration cap) is read from config at `src/run.rs:525` (`max_iterations()`), not hardcoded — good.
- **Public API surface**: 197 `pub fn` / `pub struct` / `pub enum`. This is a binary crate, so `pub` is effectively internal; no external `lib.rs` to worry about. Could tighten with `pub(crate)` where re-export isn't needed, but low priority for a binary.

## Testing

- **635 passing tests**, 0 ignored. Split across `#[cfg(test)]` modules in 40 files plus a dedicated `src/integration_tests.rs` (830 lines).
- **No `tests/` directory** — no out-of-crate integration tests. Since `aictl` is a binary (no `lib.rs`), this is expected, but an end-to-end smoke test (spawn `aictl --mock … `) in `tests/` would catch CLI-surface regressions that unit tests miss. **Suggestion**.
- Test quality sampled: redaction tests assert on `RedactionResult::Redacted { … }` variants (`src/security/redaction.rs:1043+`), security tests assert specific deny reasons (`src/security.rs:1226`), tool-parse tests cover ~25 malformed cases. Tests assert behavior, not just absence of panic. Good.
- **Coverage gaps** (files without `#[cfg(test)]`): `src/main.rs`, `src/repl.rs` (the two largest non-logic dispatchers), `src/tools/shell.rs`, `src/tools/web.rs`, `src/tools/image.rs`, `src/tools/geo.rs`, `src/tools/datetime.rs`, `src/tools/filesystem.rs`, most `src/llm/<provider>.rs`, most `src/commands/*.rs`. The LLM providers are hard to unit-test (HTTP), but the REPL dispatcher and filesystem tool are reachable and would benefit.

## Documentation

- **1079 `///` doc comments** across 60 files — strong coverage.
- **Module-level `//!` headers**: 38 files have them — roughly half. Files like `src/main.rs`, `src/tools/shell.rs`, `src/tools/web.rs`, `src/commands/menu.rs` lack a `//!` summary. **Suggestion**: add one-line module docs to the remaining files for `cargo doc` output.
- `README.md` — 769 lines, badges, install, usage, provider table, feature list. **Excellent.**
- `ARCH.md` — 581 lines, accurate to the module map in `CLAUDE.md`. **Excellent.**
- `CLAUDE.md`, `ROADMAP.md` present. No `CHANGELOG.md` at root (releases on GitHub).

## Summary

**Overall score: 9/10.** This codebase is cleaner than the majority of Rust projects its size: zero clippy-pedantic warnings, zero `unsafe`, zero production `panic!`/`todo!`, all production `.unwrap()` justified, a 635-test suite, and clearly-organized modules with a real security boundary rather than hand-waved validation.

**Top 3 strengths**
1. **Security is engineered, not bolted-on** — every tool call routes through `validate_tool()`, env scrubbing is centralized, a network-boundary redactor has its own 1500-line module + dedicated tests, and `unsafe` is absent.
2. **Disciplined error handling** — production unwraps are down to Mutex-lock idioms and a compile-time-valid template; `unreachable!` branches carry rationale strings.
3. **Strong test + docs posture** — 635 unit/integration tests, doc coverage on 60/80 files, README+ARCH both kept in sync with the code.

**Top 3 improvements**
1. Decompose the three largest dispatcher functions — `handle_repl_input` (332 lines, `src/repl.rs:295`), `main` (304 lines, `src/main.rs:23`), `run_config_wizard` (268 lines, `src/commands/config_wizard.rs`) — into per-branch helpers for readability and targeted testing.
2. Introduce a unified `AictlError` enum (via `thiserror`) to replace the 50 `Box<dyn std::error::Error>` sites across LLM/tool code, enabling structured error handling for callers (retry-on-timeout, distinguish auth vs. parse failures, etc.).
3. Add a `tests/` directory with end-to-end CLI smoke tests (using the existing `Provider::Mock`) to cover `main.rs` and `repl.rs`, the two largest untested files.

Report saved to `.claude/reports/quality/rust-quality-report-2026-04-20_18-28-48.md`.
