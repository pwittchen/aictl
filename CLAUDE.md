# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & Run

```bash
cargo build              # debug build
cargo build --release    # release build
cargo run -- <args>      # run with arguments
cargo clippy             # lint
cargo fmt                # format
cargo test               # run tests
```

## Architecture

Single-binary async Rust CLI with seven modules:

- `src/main.rs` — CLI args (clap), config loading (`~/.aictl`), agent loop, single-shot and interactive REPL modes
- `src/commands.rs` — REPL slash command handling (`/clear`, `/compact`, `/context`, `/copy`, `/exit`, `/help`, `/info`, `/mode`, `/model`, `/tools`, `/update`). Returns a `CommandResult` enum consumed by the REPL loop in `main.rs`.
- `src/config.rs` — config file loading (`~/.aictl`), constants (system prompt, spinner phrases, agent loop limits)
- `src/tools.rs` — tool-call XML parsing, tool execution dispatch
- `src/ui.rs` — `AgentUI` trait with `PlainUI` (single-shot) and `InteractiveUI` (REPL with spinner, colors, markdown rendering) implementations
- `src/llm.rs` — shared `TokenUsage` type with cost estimation, model list, context limits
- `src/llm_openai.rs`, `src/llm_anthropic.rs` — provider-specific API call implementations

**Config**: Loaded once at startup from `~/.aictl` into a `static OnceLock<HashMap<String, String>>`. CLI args override config values. The `config_get(key)` helper is used throughout the codebase to read config values. No `.env` files or system environment variables are used for program parameters.

**Flow**: CLI args (clap) → `run_agent_turn` loop → provider call → parse response for `<tool>` tags → execute tool or print final answer.

**Modes**: Single-shot (`-m "message"`) uses `PlainUI`; omitting `-m` starts an interactive REPL with `InteractiveUI` (history, spinner, markdown, colored output). `--quiet`/`-q` (requires `--auto`) suppresses reasoning and tool call output in single-shot mode, printing only the final answer.

**Agent loop** (`run_agent_turn`): Maintains a conversation history (`Vec<Message>`) with system prompt, user message, and assistant/tool-result turns. Loops up to 20 iterations. Tool calls are parsed from custom XML tags in the LLM response text. Supports `--auto` mode (skip confirmation) or interactive y/N confirmation. Always displays token usage, estimated cost, and execution time after each LLM call and as a summary after each turn.

**Tools** (`execute_tool` dispatches by tool name):
- `exec_shell` — runs commands via `tokio::process::Command` (`sh -c`)
- `read_file` — reads file contents via `tokio::fs::read_to_string`
- `write_file` — writes files via `tokio::fs::write` (first line = path, rest = content)
- `list_directory` — lists directory entries with type prefixes
- `search_files` — content search via glob traversal and string matching
- `edit_file` — targeted find-and-replace (requires unique match)
- `find_files` — find files matching a glob pattern with optional base directory
- `search_web` — web search via Firecrawl API (`FIRECRAWL_API_KEY` from `~/.aictl`)
- `fetch_url` — fetch a URL and return readable text content (HTML stripped)
- `extract_website` — fetch a URL and extract main readable content (strips scripts, styles, nav, boilerplate)
- `fetch_datetime` — get current date, time, timezone, and day of week
- `fetch_geolocation` — get geolocation data for an IP address via ip-api.com

**Providers**: OpenAI (`call_openai`) and Anthropic (`call_anthropic`) each convert `&[Message]` to provider-specific formats. Anthropic uses a top-level `system` field; OpenAI includes system messages inline. Both return `TokenUsage` for cost tracking and timing display.

**Key dependencies**: `clap` (CLI parsing), `reqwest` (async HTTP with JSON), `serde`/`serde_json` (serialization), `tokio` (async runtime, process, fs), `crossterm` (terminal styling), `indicatif` (spinner), `rustyline` (REPL input/history), `termimad` (markdown rendering), `scraper` (HTML DOM parsing with CSS selectors), `glob` (file glob pattern matching).

## Conventions

- Rust edition 2024, default rustfmt and clippy settings.
- Commit messages follow the rules in `.claude/skills/commit/SKILL.md` — no AI attribution lines, imperative mood, short messages for small changes, detailed body for larger ones.
- After implementing a new feature or fixing a bug, check `ISSUES.md` — if the change resolves an issue listed there, remove that issue from the list.
