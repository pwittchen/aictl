# Evaluation Report -- 2026-04-01 22:54:50

## Automated Checks

| Check | Result |
|-------|--------|
| `cargo clippy -- -W clippy::all -W clippy::pedantic` | **PASS** — no warnings |
| `cargo test` | **PASS** — 56 tests, 0 failures |
| `cargo fmt --check` | **PASS** — no formatting issues |
| `cargo build` | **PASS** — compiles cleanly |

## Project Structure

**Cargo.toml**: Rust edition 2024, version 0.3.0. Has `license-file` but missing `description`, `repository`, `authors`, and `homepage` fields — minor for a private project but would be needed for crates.io publishing.

**Dependencies**: All versions are current and appropriate. No redundant crates detected.

**Module layout** (8 files, 3368 LOC total):

| File | Lines | Responsibility |
|------|-------|----------------|
| `commands.rs` | 830 | REPL slash commands, interactive menus |
| `main.rs` | 693 | CLI args, agent loop, REPL, tab completion |
| `ui.rs` | 578 | PlainUI / InteractiveUI trait implementations |
| `tools.rs` | 573 | Tool parsing and execution |
| `llm.rs` | 274 | Token usage, pricing, context limits |
| `config.rs` | 227 | Config loading, constants, system prompt |
| `llm_anthropic.rs` | 106 | Anthropic API calls |
| `llm_openai.rs` | 87 | OpenAI API calls |

Assessment: Clean separation of concerns. Each file has a clear single responsibility. The module layout is logical and well-organized.

## Error Handling

**`.unwrap()` usage** — 25 occurrences total:
- 14 in test code (`config.rs:179–224`, `tools.rs:514–558`, `llm.rs:147–232`) — **Acceptable**: standard for tests.
- 1 in `ui.rs:280` (`ProgressStyle::with_template(...).unwrap()`) — **Acceptable**: infallible with a static template string.
- 2 in `commands.rs:479,624` (`event::poll(...).unwrap_or(false)`) — **Acceptable**: uses `unwrap_or` fallback.

**`.expect()` usage** — None found. Good.

**`panic!`/`unreachable!`/`todo!`** — 1 occurrence:
- `commands.rs:506` — `unreachable!()` in `select_model` matching provider string from the static `MODELS` array. **Acceptable**: the array only contains "openai" and "anthropic", so this is truly unreachable.

**`Box<dyn std::error::Error>`** — Used as the error type in 5 functions (`main.rs:192,225,288,319,412`, `llm_anthropic.rs:43`, `llm_openai.rs:39`).
- **Suggestion**: For a single-binary CLI this is pragmatic and fine. A custom error enum (via `thiserror`) would improve error discrimination if the project grows, but is not needed now.

## Safety & Security

**`unsafe` blocks** — None. Good.

**Command injection** — `tool_exec_shell` (`tools.rs:56`) passes user/LLM-provided input directly to `sh -c`. This is by design (the tool's purpose is shell execution), and the agent loop has a confirmation prompt before execution in non-auto mode. The `--auto` flag bypasses confirmation, which is documented. **Acceptable given the design**.

**Hardcoded secrets** — None found. API keys are loaded from `~/.aictl` at runtime.

**Path traversal** — `read_file`, `write_file`, `list_directory`, `edit_file` accept arbitrary paths from the LLM. Same risk profile as shell execution — mitigated by the confirmation prompt. **Acceptable given the design**.

## Code Quality

**Cloning** — 10 `.clone()` calls in non-test code:
- `main.rs:130` (`stop.clone()` for Arc) — **Necessary**: Arc clone for async task.
- `main.rs:238` (`response.clone()`) — **Suggestion**: The response is used once for `messages.push` and once for `parse_tool_call` + display. Could avoid the clone by restructuring, but the impact is low.
- `commands.rs:116,157` (cloning messages for compact summarization) — **Necessary**: need a separate copy to send to LLM.
- `llm_openai.rs:50,77`, `llm_anthropic.rs:52,57,63,96` — Content clones when building provider-specific request structs. **Necessary** given the current API design (provider functions take `&[Message]` and build owned request bodies).

**String handling** — Generally good. `&str` used appropriately for function parameters. No excessive allocation patterns.

**Dead code** — No `#[allow(dead_code)]` attributes. `#[allow(unused_assignments)]` at `main.rs:201` for `last_input_tokens` — **Acceptable**: the variable is assigned in a loop and read after.

**Magic numbers**:
- `commands.rs:699-702`: `1_048_576` and `1_024` for byte-to-MB/KB conversion — **Suggestion**: Could use named constants, but these are well-known values in a localized formatting function.
- `commands.rs:188,191`: `100` for percentage calculation — standard, not a magic number.

**Function length** (>80 lines):
- `run_interactive` (`main.rs:407–627`, ~220 lines) — **Issue**: Suppressed via `#[allow(clippy::too_many_lines)]`. This function handles the entire REPL loop including command dispatch, auto-compact, and agent turn orchestration. Would benefit from extracting the command-dispatch match arm into a helper.
- `run_agent_turn` (`main.rs:184–309`, ~125 lines) — **Issue**: Also suppressed via `#[allow(clippy::too_many_lines)]`. Core agent loop; less straightforward to decompose but the tool-call handling block (lines 264–301) could be a helper.
- `select_model` (`commands.rs:441–546`, ~105 lines) — Acceptable for a self-contained TUI menu function.
- `select_mode` (`commands.rs:594–684`, ~90 lines) — Acceptable, but nearly duplicates `select_model` structure.

**Code duplication**:
- **Issue**: `select_model` and `select_mode` (`commands.rs:441–684`) share the same TUI menu loop pattern (enable raw mode, draw lines, poll events, handle up/down/enter/esc, redraw, restore terminal). A generic `select_from_menu` helper could eliminate ~60 lines of duplication.

**`#[allow(...)]` annotations** — 13 total. Most are justified `clippy::cast_possible_truncation` or `clippy::cast_precision_loss` with known-safe ranges. The two `clippy::too_many_lines` suppressions should be addressed.

**Public API surface** — Appropriate. Only items needed across modules are `pub`. Internal helpers are private.

## Testing

**Test count**: 56 unit tests across 5 modules.

| Module | Tests | Coverage |
|--------|-------|----------|
| `commands.rs` | 12 | Command dispatch, all slash commands |
| `config.rs` | 8 | Config file parsing (comments, quotes, export prefix, mixed) |
| `tools.rs` | 9 | Tool-call XML parsing (valid, edge cases, missing tags) |
| `ui.rs` | 9 | Truncation, first-input formatting |
| `llm.rs` | 18 | Pricing for all models, cost estimation, context limits |

**Coverage gaps**:
- **Issue**: No tests for tool *execution* functions (`tool_exec_shell`, `tool_read_file`, `tool_write_file`, etc.) — only parsing is tested.
- **Issue**: No tests for LLM provider API call functions (`call_openai`, `call_anthropic`) — would need HTTP mocking.
- **Issue**: No tests for `run_agent_turn`, `run_agent_single`, or `run_interactive` — integration-level testing is absent.
- No integration tests in `tests/` directory.

**Test quality**: Tests assert specific values and behavior, not just absence of panic. Good use of edge cases (empty input, missing tags, unicode). `commands_list_matches_handler` verifies that the `COMMANDS` constant stays in sync with the handler — good practice.

## Documentation

**Module-level doc comments (`//!`)**: None in any source file.
- **Suggestion**: Add `//!` comments to each module describing its purpose.

**Function/type doc comments (`///`)**: Good coverage on public API:
- `main.rs`: `TurnResult`, `Cli` fields, `Interrupted`, `with_esc_cancel`, `run_agent_turn`, `run_agent_single`, `run_interactive` all documented.
- `commands.rs`: `COMMANDS`, `CommandResult` variants, `handle`, `select_model`, `select_mode`, `build_menu_lines`, `print_info`, `run_update`, `run_update_cli` documented.
- `config.rs`: `http_client`, `config_set` documented.
- `tools.rs`: `truncate_output` documented.
- `ui.rs`: `ToolApproval` variants, `AgentUI` trait, helper functions documented.
- `llm.rs`: `MODELS`, `price_per_million`, `context_limit`, `estimate_cost`, `pct`, `pct_usize` documented.

**README.md**: Comprehensive — covers installation (script + source), usage, all CLI flags, config file format, providers with pricing tables, agent loop, tool list, examples, testing, and architecture reference. Well-structured.

## Summary

**Score: 7.5 / 10**

### Top 3 Strengths

1. **Clean automated checks**: Zero clippy warnings (even with pedantic), zero test failures, clean formatting — the CI pipeline is solid.
2. **Well-organized architecture**: Clear module boundaries, logical separation of concerns, appropriate use of traits (`AgentUI`) for the UI abstraction layer. Provider implementations are cleanly isolated.
3. **Comprehensive documentation**: Thorough README with pricing tables, examples, and architecture reference. Good `///` doc coverage on public APIs. CLAUDE.md and ARCH.md provide excellent onboarding context.

### Top 3 Improvements

1. **Decompose long functions**: `run_interactive` (220 lines) and `run_agent_turn` (125 lines) in `main.rs` are suppressing `clippy::too_many_lines`. Extract command-dispatch and tool-call handling into named helpers to improve readability.
2. **Reduce TUI menu duplication**: `select_model` and `select_mode` in `commands.rs` share ~80% of their structure. Extract a generic arrow-key menu selection function to eliminate code duplication.
3. **Expand test coverage**: 56 tests cover parsing and pure logic well, but tool execution, LLM API calls, and the agent loop have no test coverage. Adding integration tests with mocked HTTP responses would significantly improve confidence in the core flow.
