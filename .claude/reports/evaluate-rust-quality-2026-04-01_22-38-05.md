# Evaluation Report — 2026-04-01 22:38:05

## Automated Checks

| Check | Result |
|-------|--------|
| `cargo build` | **PASS** — compiles without errors |
| `cargo test` | **PASS** — 56 tests, 0 failures |
| `cargo clippy -- -W clippy::all -W clippy::pedantic` | **PASS** — 0 warnings |
| `cargo fmt --check` | **FAIL** — 3 formatting diffs in `src/tools.rs` |

**Issue**: `cargo fmt` reports 3 style diffs in `src/tools.rs` (lines 105, 272, 392). Run `cargo fmt` to fix.

## Project Structure

**Module layout** (8 files, 3311 lines total): Clean, logical single-responsibility layout. Each module has a clear purpose. File sizes are reasonable — `commands.rs` (830) and `main.rs` (665) are the largest but within acceptable range.

**Cargo.toml**:
- Edition 2024 — current.
- Dependencies are modern and appropriate. No redundant crates.
- `license-file = "LICENSE"` present.
- **Suggestion**: Missing `description`, `repository`, `authors`, and `keywords` fields. These matter for `crates.io` publishability and project discoverability.

## Error Handling

- All `.unwrap()` calls (25 total) are in test code — acceptable.
- No `.expect()` calls anywhere — clean.
- One `unreachable!()` at `src/commands.rs:506` — inside an exhaustive TUI input loop, justified by the preceding match guard.
- `Box<dyn std::error::Error>` used as the error type in 9 locations across `main.rs`, `llm_openai.rs`, `llm_anthropic.rs`. **Suggestion**: A custom error enum (e.g., via `thiserror`) would improve error categorization and make error handling more precise, but is not strictly necessary for a CLI tool of this size.
- `#[allow(unused_assignments)]` at `src/main.rs:187` for `last_input_tokens` — the initial assignment is indeed overwritten; this is a minor code smell but harmless.
- `unwrap_or_default()` used 6 times in non-test code — all are on `Option`/`Result` where the default is a reasonable fallback (empty string, zero tokens). Acceptable.

## Safety & Security

- **No `unsafe` blocks** — good.
- **No hardcoded secrets** — API keys are loaded from `~/.aictl` config. Clean.
- **Command injection**: `tool_exec_shell` (`src/tools.rs:53-56`) passes LLM-generated input directly to `sh -c`. This is by design (the tool's purpose is to run shell commands), and is mitigated by the interactive confirmation prompt. However:
  - **Issue**: `tool_search_files` (`src/tools.rs:147-148`) passes the search pattern directly to `grep` as a command argument. While `grep` arguments are safer than `sh -c`, malicious patterns could still cause excessive CPU usage (ReDoS-style). Low risk since the LLM generates these.
- **Path traversal**: `tool_read_file` and `tool_write_file` accept arbitrary paths from the LLM with no sandboxing. This is by design (agent needs filesystem access), mitigated by tool confirmation. Acceptable for the tool's purpose.
- **Suggestion**: The `search_files` tool uses system `grep`. Consider using a Rust-native approach (e.g., `grep` crate or manual line search) to avoid subprocess overhead and potential argument injection edge cases.

## Code Quality

- **Clones**: 10 `.clone()` calls in non-test code. Most are necessary (building API request bodies from borrowed `&[Message]`, `response.clone()` needed because it's used after push). `messages.clone()` at `src/commands.rs:116` for compact — necessary since the original must be preserved. No unnecessary clones found.
- **`reqwest::Client::new()`**: Created fresh in 6 locations (`tools.rs:213,321,371,457`, both LLM providers). **Suggestion**: Reuse a single `Client` instance across calls. Creating a new client each time wastes connection pool setup. Pass a shared client or store one globally.
- **Magic numbers**:
  - `10_000` used as truncation limit in `tools.rs:47-48` and `tools.rs:303-304`. The `truncate_output` helper centralizes one case, but `tool_find_files` has its own inline truncation. **Suggestion**: Extract `10_000` to a named constant (e.g., `MAX_TOOL_OUTPUT_LEN`).
  - `4096` hard-coded as `max_tokens` in `src/llm_anthropic.rs:70`. **Suggestion**: Make this a named constant or configurable.
  - `1_048_576` and `1_024` in `commands.rs:699-702` — standard byte size literals, acceptable.
- **Dead code**: No `#[allow(dead_code)]` annotations. No obviously unused public items.
- **`pub` surface**: `tools::confirm_tool_call` (`src/tools.rs:474`) appears unused — it exists alongside the `InteractiveUI` confirmation. **Issue**: Verify if this is dead code; if so, remove it.
- **Function length**: Two functions have `#[allow(clippy::too_many_lines)]`: `run_agent_turn` (105 lines) and `run_interactive` (200 lines). The `run_interactive` function at 200 lines is a large match-based REPL dispatcher. **Suggestion**: Extract the slash-command dispatch block (lines ~445-530) into a helper function to bring `run_interactive` under 100 lines.
- **Return type complexity**: `run_agent_turn` returns a 6-element tuple `(String, TokenUsage, u32, u32, Duration, u64)`. **Suggestion**: Introduce a `TurnResult` struct for clarity.

## Testing

- **56 tests** across 5 modules:
  - `commands` — 12 tests (command routing)
  - `config` — 8 tests (config parsing)
  - `llm` — 18 tests (pricing, context limits, cost estimation)
  - `tools` — 9 tests (XML tool call parsing only)
  - `ui` — 9 tests (input extraction, truncation)
- **No integration tests** (`tests/` directory absent).
- **Coverage gaps**:
  - **Issue**: `llm_openai.rs` and `llm_anthropic.rs` — zero tests. HTTP interactions could be tested with mock responses.
  - **Issue**: `tools.rs` — only `parse_tool_call` is tested. No tests for `execute_tool` or any individual `tool_*` function. `tool_edit_file`, `tool_write_file`, `tool_find_files` are good candidates for unit tests.
  - **Issue**: `main.rs` — no tests for `version_info_string`, `fetch_remote_version`, or the agent loop logic.
  - `pct()` and `pct_usize()` in `llm.rs` lack tests.
- **Test quality**: Tests use proper assertions with specific expected values. The `commands_list_matches_handler` test verifies the command list stays in sync with the handler — a good practice.

## Documentation

- **No module-level doc comments** (`//!`) in any source file. **Issue**: Each module should have a brief `//!` explaining its purpose.
- **Doc comments on public functions**: Mixed coverage. `handle`, `config_set`, `select_model`, `select_mode`, `run_update`, `run_update_cli`, `context_limit`, `pct`, `pct_usize` have doc comments. Missing from: `compact`, `print_context`, `print_info`, `load_config`, `config_get`, `parse_tool_call`, `execute_tool`, `confirm_tool_call`, `call_openai`, `call_anthropic`.
- **README.md**: Present with install instructions (curl and source), usage, configuration, and CLI flags. Adequate for a personal project.

## Summary

**Score: 7.5 / 10**

**Top 3 Strengths**:
1. **Zero clippy pedantic warnings** — the codebase is exceptionally clean by Rust linting standards, with deliberate `#[allow]` annotations where suppression is justified.
2. **Solid test suite** — 56 tests with good assertion quality and a meta-test ensuring command list consistency.
3. **Clean architecture** — well-decomposed modules with single responsibilities, per-tool handler functions, and centralized helpers (e.g., `pct()`, `truncate_output`).

**Top 3 Improvements**:
1. **Fix `cargo fmt`** — 3 formatting diffs in `tools.rs` should be resolved immediately. This is the only automated check failing.
2. **Expand test coverage** — LLM provider modules, tool execution functions, and `version_info_string` have zero tests. Adding unit tests for `tool_edit_file`, `tool_find_files`, and the provider functions (with HTTP mocking) would significantly improve confidence.
3. **Reuse `reqwest::Client`** — creating a new HTTP client for every API call and tool invocation wastes connection pool resources. Pass a shared client through the call chain or use a lazy static.
