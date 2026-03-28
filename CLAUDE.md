# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & Run

```bash
cargo build              # debug build
cargo build --release    # release build
cargo run -- <args>      # run with arguments
cargo clippy             # lint
cargo fmt                # format
cargo test               # run tests (none yet)
```

## Architecture

Single-binary async Rust CLI with four modules:

- `src/main.rs` — CLI args (clap), agent loop, single-shot and interactive REPL modes
- `src/tools.rs` — system prompt, tool-call XML parsing, tool execution dispatch
- `src/ui.rs` — `AgentUI` trait with `PlainUI` (single-shot) and `InteractiveUI` (REPL with spinner, colors, markdown rendering) implementations
- `src/llm/` — provider modules (`openai.rs`, `anthropic.rs`) and shared `TokenUsage` type with cost estimation (`mod.rs`)

**Flow**: CLI args (clap) → `run_agent_turn` loop → provider call → parse response for `<tool>` tags → execute tool or print final answer.

**Modes**: Single-shot (`-M "message"`) uses `PlainUI`; omitting `-M` starts an interactive REPL with `InteractiveUI` (history, spinner, markdown, colored output).

**Agent loop** (`run_agent_turn`): Maintains a conversation history (`Vec<Message>`) with system prompt, user message, and assistant/tool-result turns. Loops up to 20 iterations. Tool calls are parsed from custom XML tags in the LLM response text. Supports `--auto` mode (skip confirmation) or interactive y/N confirmation. Tracks cumulative token usage across iterations.

**Tools** (`execute_tool` dispatches by tool name):
- `shell` — runs commands via `tokio::process::Command` (`sh -c`)
- `read_file` — reads file contents via `tokio::fs::read_to_string`
- `write_file` — writes files via `tokio::fs::write` (first line = path, rest = content)
- `list_directory` — lists directory entries with type prefixes
- `search_files` — grep-based content search
- `edit_file` — targeted find-and-replace (requires unique match)
- `web_search` — web search via Firecrawl API (`FIRECRAWL_API_KEY`)

**Providers**: OpenAI (`call_openai`) and Anthropic (`call_anthropic`) each convert `&[Message]` to provider-specific formats. Anthropic uses a top-level `system` field; OpenAI includes system messages inline. Both return `TokenUsage` for optional cost tracking (`--usage`).

**Key dependencies**: `clap` (CLI parsing), `reqwest` (async HTTP with JSON), `serde`/`serde_json` (serialization), `tokio` (async runtime, process, fs), `crossterm` (terminal styling), `indicatif` (spinner), `rustyline` (REPL input/history), `termimad` (markdown rendering).

## Conventions

- Rust edition 2024, default rustfmt and clippy settings.
- Commit messages follow the rules in `.claude/skills/commit/SKILL.md` — no AI attribution lines, imperative mood, short messages for small changes, detailed body for larger ones.
