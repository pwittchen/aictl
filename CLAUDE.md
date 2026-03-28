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

Single-binary async Rust CLI (`src/main.rs`) that runs an agent loop: the LLM can invoke tools, observe results, and continue reasoning until it produces a final answer.

**Flow**: CLI args (clap) → `run_agent` loop → provider call → parse response for `<tool>` tags → execute tool or print final answer.

**Agent loop** (`run_agent`): Maintains a conversation history (`Vec<Message>`) with system prompt, user message, and assistant/tool-result turns. Loops up to 20 iterations. Tool calls are parsed from custom XML tags in the LLM response text. Supports `--auto` mode (skip confirmation) or interactive y/N confirmation.

**Tools**: `execute_tool` dispatches by tool name:
- `shell` — runs commands via `tokio::process::Command` (`sh -c`)
- `read_file` — reads file contents via `tokio::fs::read_to_string`
- `write_file` — writes files via `tokio::fs::write` (first line = path, rest = content)

**Providers**: OpenAI (`call_openai`) and Anthropic (`call_anthropic`) each convert `&[Message]` to provider-specific formats. Anthropic uses a top-level `system` field; OpenAI includes system messages inline.

**Key dependencies**: `clap` (CLI parsing, derive macros), `reqwest` (async HTTP with JSON), `serde`/`serde_json` (serialization), `tokio` (async runtime, process, fs).

## Conventions

- Rust edition 2024, default rustfmt and clippy settings.
- Commit messages follow the rules in `.claude/skills/commit/SKILL.md` — no AI attribution lines, imperative mood, short messages for small changes, detailed body for larger ones.
