# Evaluation Report -- 2026-05-15 23:46:22

## Automated Checks

- **`cargo build --workspace`**: clean.
- **`cargo fmt --check`**: clean.
- **`cargo clippy --workspace -- -W clippy::all -W clippy::pedantic`**: clean across `default-members`; 2 pedantic warnings only in `aictl-desktop` (excluded from `cargo lint`):
  - `crates/aictl-desktop/build.rs:19` — `map(..).unwrap_or_else(..)` → suggest `map_or_else`.
  - `crates/aictl-desktop/src/ui.rs:309` — doc comment missing backticks on `AppHandle`.
- **`cargo test --workspace --lib --bins --tests`**: **1 091 tests pass, 0 fail** (133 + 19 + 888 + 51).
- **`cargo test` (doctests)**: rustdoc fails for `aictl-core` with `extern location for md5/zip/pdf_extract does not exist` — a stale-rlib issue in the cargo+whisper-rs-sys build cache. The lib has no doctests; bins/integration tests are unaffected.

## Project Structure

- Four-crate workspace, `resolver = "3"`, edition **2024**, version **0.46.0**. Workspace metadata centralised on `[workspace.package]` (repo, license-file, authors). `aictl-desktop` correctly excluded from `default-members`.
- 179 `.rs` files, **68 198 LOC**. Largest modules:
  - `security/redaction.rs` 2 401, `security.rs` 2 227, `run.rs` 2 156, `coding.rs` 1 426, `repl.rs` 1 364, `tools.rs` 1 346, `config.rs` 1 327, `tools/filesystem.rs` 1 254, `cli/ui.rs` 1 159.
- Layout matches `CLAUDE.md`: engine in `aictl-core`, REPL in `aictl-cli`, HTTP proxy in `aictl-server`, Tauri desktop in `aictl-desktop`. Sub-modules grouped sensibly (`llm/`, `tools/`, `mcp/`, `messages/`, `routes/`, `commands/`). Features (`gguf`, `mlx`, `redaction-ner`) declared on the core crate and passthrough'd by the front-ends — clean.
- All workspace members declare `description`, `repository`, `license-file`, `authors`. No old/redundant deps spotted (reqwest 0.13, tokio 1, serde 1, regex 1, clap 4, thiserror 2 — current).

## Error Handling

- Custom error type `AictlError` (`crates/aictl-core/src/error.rs:18`) with structured variants (`Timeout`, `Auth`, `Provider`, `EmptyResponse`, `Stream`, `Io`, `Http`, `Json`, `Injection`, `Redaction`, `Interrupted`, `MaxIterations`, `Other`) and `From` impls for `reqwest::Error` / `serde_json::Error` / `std::io::Error` / `String` / `&str` / `Interrupted`. No bare `Box<dyn Error>` remains anywhere in `src/`.
- `.unwrap()`: 560 occurrences across the workspace, but **only ~40 are outside `#[cfg(test)]` / `mod tests` blocks** — almost all of those are `Mutex::lock().unwrap()` (poison panic — standard idiom) or `OnceLock` regex compilation on infallible patterns (`crates/aictl-cli/src/ui.rs:299-307`).
  - Suggest: `crates/aictl-server/src/messages/translator/stream/openai.rs:163` — `.get(&oi).unwrap()` is correct (the previous block inserts into the map first) but would read better as `.expect("tool_index_map populated by the start-of-block branch above")`.
  - Suggest: `crates/aictl-core/src/tools/filesystem.rs:830` — same shape, guarded by `count == 1`; an `.expect("count==1 above guarantees a match")` documents the invariant.
- `panic!` / `unreachable!` / `todo!`: all production occurrences are exhaustive-match guards (`Provider::AictlServer` arms in `run.rs:1708`, `run.rs:2126`, desktop `commands/{skills,agents,ping}.rs`) or `unreachable!()` after pattern dispatch in `tools/checksum.rs:141`. Each is followed by a static-string reason. No `todo!` / `unimplemented!` in production code.

## Safety & Security

- **No `unsafe` blocks anywhere in the workspace** (0 matches for `unsafe fn` / `unsafe {`).
- **No hardcoded credentials** — the only `sk-` literals live in `security/redaction.rs` regex tables, `session.rs` redaction tests, and `config.rs` parser fixtures. Secret retrieval routes through `keys::get_secret` (keyring-first).
- **Shell invocations**: 45 `Command::new` sites; the 5 `Command::new("sh") -c` paths (`tools/shell.rs:4`, `hooks.rs:444`, `coding.rs:665`, `tools/clipboard.rs:185`, `cli/commands/update.rs`, `server/update.rs`) either run author-controlled scripts (update.rs is the curl-installer reinvocation) or pass through the `security::validate_tool` / `validate_shell` gate (`security.rs:366`, `security.rs:582+`). Env scrub is uniform via `security::scrubbed_env()`.
- **Path handling**: `security::check_path_with` (`security.rs:942`) canonicalizes for existing paths, canonicalizes `parent + filename` for writes, and manually strips `..` for non-existing trees — defence-in-depth against traversal. Workspace carve-out keeps the desktop's `~/.aictl/workspace/` usable while siblings stay blocked. Symlink-aware checks in plugins (rejects entrypoints escaping the plugin dir).
- **Network egress**: MCP remote transports go through `security::validate_mcp_url` (hostname allow/deny, HTTPS by default) at parse time *and* every dispatch. Outbound LLM redaction in `run::redact_outbound`; persistence-time redaction in `session::save_messages` and the REPL history seam. Both seams documented in `CLAUDE.md`.

## Code Quality

- `.clone()`: 346 calls workspace-wide — proportionate for an LLM-heavy codebase that copies chat history into provider-specific payloads. Spot-checked the hot loop in `run::run_agent_turn` — clones are on small `String`s (provider names, tool names) or unavoidable for `Vec<Message>` retained for retry/undo.
- **Long functions** to consider decomposing:
  - `run::run_agent_turn` — 639 lines (`crates/aictl-core/src/run.rs:1342`). The agent loop is intrinsically branchy (compaction, redaction, stream-suspend, multi-call dispatch, server-proxy fork), but the file already has helpers (`handle_tool_batch`, `handle_tool_call`, `run_parallel_call`). Worth extracting the "post-tool-result accumulation + injection-guard" block into a dedicated helper.
  - `repl::run_and_display_turn` — 416 lines (`crates/aictl-cli/src/repl.rs:862`).
  - `run::build_system_prompt_with` — 138 lines, mostly conditional concatenation; readable but could use a `PromptBuilder`.
  - `run::handle_tool_batch` 216, `handle_tool_call` 164 — already split out; size justified by the per-call lifecycle.
  - `llm::price_per_million` 238 — match-arm data table, fine as-is.
- `#[allow(dead_code)]`: 9 sites, all legitimate (translator response shapes used only in tests, mock LLM helpers, optional ANSI/version fields gated by feature/platform).
- Magic numbers: only mild — agent loop cap `20` (`run.rs`) is implicit; `MAX_ENTRIES = 200` / `MAX_ENTRY_LEN = 1000` in `memory.rs` are named constants. Reasonable.
- Wildcard imports: 76 occurrences but all under `mod tests` (`use super::*;`) — idiomatic.

## Testing

- **1 091 unit + integration tests, 0 failures.** Per-crate: aictl-core 888, aictl-cli (lib) 133, aictl-cli (bin) 19, aictl-server 51.
- 77 of 179 source files carry tests (43 %). Modules in `aictl-server/src/messages/translator/` and `aictl-core/src/security/` are particularly well covered; `aictl-cli/src/commands/agent.rs` (1 100 LOC) and `commands/skills.rs` (951 LOC) carry no unit tests — coverage gap.
- `cli_smoke.rs` integration test exercises end-to-end binary behaviour. `integration_tests.rs` (under `#[cfg(test)]` in `aictl-cli/src/`) drives `run_agent_turn` against a scripted mock LLM — good behavioural coverage.
- Test bodies assert behaviour (return values / state checks / panic-shaped `assert!(matches!(..))`), not just "doesn't panic". Shared mock infrastructure in `llm/mock.rs` with a `MockGuard` for serialization.
- **Doctests are broken** — fix would unblock contributors who run a bare `cargo test`. Either add `#[cfg(any())]` guards on the affected `use` lines or commit empty `#[doc(hidden)] mod doctest_shim;` to settle the rustdoc dep walk.

## Documentation

- `README.md` present (117 lines) plus a 12-file `docs/` tree covering `ARCH`, `CONFIG`, `INSTALL`, `USAGE`, `PROVIDERS`, `TOOLS`, `EXTENSIONS`, `CODING_AGENT`, `SERVER`, `LLM_PRICING`, `DESIGN`, `COMMERCIAL_PRICING` (3 379 LOC of prose). `CLAUDE.md` doubles as a dense engineering reference.
- Module-level `//!` headers: 32 of 39 top-level `.rs` files in `aictl-core`/`aictl-cli`/`aictl-server` (82 %).
- Doc comments on public items in `aictl-core`: ~238 / 462 items (≈ 51 % rough estimate). Strong on subsystems with side-effects (`error.rs`, `security.rs`, `run.rs`, `hooks.rs`, `mcp.rs`); thinner on the `tools/` submodules where the function name is the contract.

## Summary

**Overall: 8.5 / 10** — production-grade Rust hygiene for a CLI + library + server + desktop workspace. Edition 2024, structured error type, no `unsafe`, no hardcoded secrets, 1 091 green tests, clean clippy pedantic on the shipped crates.

**Top strengths**
1. Strong security posture — bespoke `SecurityPolicy` with CWD jail, path canonicalisation, regex+entropy+NER redaction, two-seam outbound/persistence rewrite, explicit MCP URL allow/deny gate.
2. Disciplined error design — `AictlError` variants the agent loop actually branches on (Timeout / Auth / Interrupted), `From` impls for ergonomic `?`, no `Box<dyn Error>` residue.
3. Excellent test density (1 091 tests, mock-LLM-driven end-to-end coverage of `run_agent_turn`).

**Top improvements**
1. Fix the doctest build (stale extern rlibs for `md5`/`zip`/`pdf_extract`) so `cargo test` works out-of-the-box.
2. Decompose `run::run_agent_turn` (639 lines) and `repl::run_and_display_turn` (416 lines) into named helpers — the rest of the file already shows the pattern.
3. Add unit tests to `aictl-cli/src/commands/agent.rs` (1 100 LOC) and `commands/skills.rs` (951 LOC) — both are pure logic that lends itself to in-process testing.
