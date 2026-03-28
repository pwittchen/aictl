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

Single-binary async Rust CLI (`src/main.rs`) that sends a user message to an LLM provider and prints the response.

**Flow**: CLI args (clap) → provider dispatch → HTTP request (reqwest) → parse JSON response → print to stdout.

**Providers**: OpenAI (`call_openai`) and Anthropic (`call_anthropic`) each have their own request/response types and endpoint logic. Provider selection is handled via a `clap::ValueEnum` enum routed in `main()`.

**Key dependencies**: `clap` (CLI parsing, derive macros), `reqwest` (async HTTP with JSON), `serde`/`serde_json` (serialization), `tokio` (async runtime).

## Conventions

- Rust edition 2024, default rustfmt and clippy settings.
- Commit messages follow the rules in `.claude/skills/commit/SKILL.md` — no AI attribution lines, imperative mood, short messages for small changes, detailed body for larger ones.
