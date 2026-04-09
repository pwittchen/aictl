# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

See also: [README.md](README.md) for project overview and usage, [ARCH.md](ARCH.md) for detailed architecture documentation.

## Build & Run

```bash
cargo build              # debug build
cargo build --release    # release build
cargo run -- <args>      # run with arguments
cargo lint               # lint (clippy pedantic, alias in .cargo/config.toml)
cargo fmt                # format
cargo test               # run tests
```

## Architecture

Single-binary async Rust CLI with sixteen modules:

- `src/main.rs` — CLI args (clap), config loading (`~/.aictl/config`), security init, session init, agent loop, single-shot and interactive REPL modes
- `src/commands.rs` — REPL slash command handling (`/behavior`, `/clear`, `/compact`, `/context`, `/copy`, `/exit`, `/help`, `/info`, `/issues`, `/model`, `/security`, `/session`, `/thinking`, `/tools`, `/update`). `ThinkingMode` enum (Smart/Fast) for conversation history optimization. Returns a `CommandResult` enum consumed by the REPL loop in `main.rs`. Also hosts the `/session` interactive menu (current info, set name, view saved, clear all), the non-interactive `print_sessions_cli` helper, and the `--config` interactive configuration wizard (`run_config_wizard`).
- `src/config.rs` — config file loading (`~/.aictl/config`), constants (system prompt, spinner phrases, agent loop limits)
- `src/security.rs` — `SecurityPolicy` with shell command validation, path validation (CWD jail, blocked paths, canonicalization), environment scrubbing, shell timeout, output sanitization. Loaded into `static OnceLock` at startup. Configurable via `AICTL_SECURITY_*` keys in `~/.aictl/config`.
- `src/session.rs` — session persistence under `~/.aictl/sessions/`. Generates UUID v4 ids (from `/dev/urandom` with time-based fallback), stores conversation history as JSON per session, maintains a `.names` file mapping uuid → unique human-readable name. Exposes a global current-session handle and an incognito toggle (`set_incognito` / `is_incognito`) that short-circuits all saves.
- `src/tools.rs` — tool-call XML parsing, tool execution dispatch (security gate at entry, output sanitization at exit)
- `src/ui.rs` — `AgentUI` trait with `PlainUI` (single-shot) and `InteractiveUI` (REPL with spinner, colors, markdown rendering) implementations
- `src/llm.rs` — shared `TokenUsage` type with cost estimation, model list, context limits
- `src/llm_openai.rs`, `src/llm_anthropic.rs`, `src/llm_gemini.rs`, `src/llm_grok.rs`, `src/llm_mistral.rs`, `src/llm_deepseek.rs`, `src/llm_zai.rs`, `src/llm_ollama.rs` — provider-specific API call implementations

**Config**: Loaded once at startup from `~/.aictl/config` into a `static OnceLock<HashMap<String, String>>`. CLI args override config values. The `config_get(key)` helper is used throughout the codebase to read config values. No `.env` files or system environment variables are used for program parameters.

**Flow**: CLI args (clap) → `security::init()` → resolve incognito flag (`--incognito` or `AICTL_INCOGNITO`) → `run_agent_turn` loop → provider call → parse response for `<tool>` tags → security validation → execute tool or print final answer. In interactive mode, the REPL also initializes a session (new uuid or loaded from `--session <id|name>`) before printing the welcome banner.

**Modes**: Single-shot (`-m "message"`) uses `PlainUI`; omitting `-m` starts an interactive REPL with `InteractiveUI` (history, spinner, markdown, colored output). `--quiet`/`-q` (requires `--auto`) suppresses reasoning and tool call output in single-shot mode, printing only the final answer. `--config` launches an interactive wizard to set provider, model, and API keys (saves to `~/.aictl/config` and exits). **Thinking modes**: Smart (default, sends all messages to LLM) and Fast (sends system prompt + last 20 messages via a sliding window). Configurable via `/thinking` command or `AICTL_THINKING` in `~/.aictl/config`.

**Sessions**: In interactive mode, the REPL creates a new session (UUID v4) at startup and persists the full conversation to `~/.aictl/sessions/<uuid>` as JSON after every agent turn and after manual/auto compaction. Optional human-readable session names live in `~/.aictl/sessions/.names` (tab-separated, uuid → name; names must be unique). The welcome banner shows the active session uuid (and name, if set), and a notice is printed on exit. `--session <uuid|name>` resumes a saved session, `--list-sessions` and `--clear-sessions` are non-interactive helpers. **Incognito mode**: `--incognito` or `AICTL_INCOGNITO=true` (accepts `true`/`false` only, default `false`) disables all session creation and saves; the welcome banner shows a yellow notice, `/session` prints a disabled message, and no exit save line is printed.

**Agent loop** (`run_agent_turn`): Maintains a conversation history (`Vec<Message>`) with system prompt, user message, and assistant/tool-result turns. Loops up to 20 iterations. Tool calls are parsed from custom XML tags in the LLM response text. Supports `--auto` mode (skip confirmation) or interactive y/N confirmation. Always displays token usage, estimated cost, and execution time after each LLM call and as a summary after each turn.

**Security** (`src/security.rs`): All tool calls pass through `security::validate_tool()` before execution. Shell commands are validated against blocked/allowed lists with command substitution blocking. File tools are restricted to the working directory (CWD jail) with path canonicalization to defeat traversal attacks. Individual tools can be disabled via `AICTL_SECURITY_DISABLED_TOOLS`. Shell subprocesses get a scrubbed environment (strips `*_KEY`, `*_SECRET`, `*_TOKEN`, `*_PASSWORD`) and a configurable timeout (default 30s). Tool output is sanitized to prevent `<tool>` tag injection. Configurable via `AICTL_SECURITY_*` keys in `~/.aictl/config`. Bypassed entirely with `--unrestricted`.

**Tools** (`execute_tool` dispatches by tool name; all tools can be globally disabled via `AICTL_TOOLS_ENABLED=false` in config, default `true`):
- `exec_shell` — runs commands via `tokio::process::Command` (`sh -c`) with env scrubbing and timeout
- `read_file` — reads file contents via `tokio::fs::read_to_string`
- `write_file` — writes files via `tokio::fs::write` (first line = path, rest = content)
- `remove_file` — removes a file via `tokio::fs::remove_file` (regular files only)
- `create_directory` — creates a directory (and parents) via `tokio::fs::create_dir_all`
- `list_directory` — lists directory entries with type prefixes
- `search_files` — content search via glob traversal and string matching
- `edit_file` — targeted find-and-replace (requires unique match)
- `find_files` — find files matching a glob pattern with optional base directory
- `search_web` — web search via Firecrawl API (`FIRECRAWL_API_KEY` from `~/.aictl/config`)
- `fetch_url` — fetch a URL and return readable text content (HTML stripped)
- `extract_website` — fetch a URL and extract main readable content (strips scripts, styles, nav, boilerplate)
- `fetch_datetime` — get current date, time, timezone, and day of week
- `fetch_geolocation` — get geolocation data for an IP address via ip-api.com

**Providers**: OpenAI (`call_openai`), Anthropic (`call_anthropic`), Gemini (`call_gemini`), Grok (`call_grok`), Mistral (`call_mistral`), DeepSeek (`call_deepseek`), Z.ai (`call_zai`), and Ollama (`call_ollama`) each convert `&[Message]` to provider-specific formats. Anthropic uses a top-level `system` field; OpenAI, Grok, Mistral, DeepSeek, and Z.ai include system messages inline; Gemini uses a `systemInstruction` field and maps assistant role to `model`; Ollama uses its native `/api/chat` endpoint with `stream: false`. All return `TokenUsage` for cost tracking and timing display. Ollama requires no API key; its models are discovered dynamically via `/api/tags`.

**Key dependencies**: `clap` (CLI parsing), `reqwest` (async HTTP with JSON), `serde`/`serde_json` (serialization), `tokio` (async runtime, process, fs), `crossterm` (terminal styling), `indicatif` (spinner), `rustyline` (REPL input/history), `termimad` (markdown rendering), `scraper` (HTML DOM parsing with CSS selectors), `glob` (file glob pattern matching).

## Conventions

- Rust edition 2024, default rustfmt and clippy settings.
- Commit messages follow the rules in `.claude/skills/commit/SKILL.md` — no AI attribution lines, imperative mood, short messages for small changes, detailed body for larger ones.
- After implementing a new feature or fixing a bug, check `ISSUES.md` — if the change resolves an issue listed there, remove that issue from the list.
- Claude Code skills are in `.claude/skills/` — `/commit`, `/update-docs`, `/evaluate-rust-quality`, `/evaluate-rust-security`, `/evaluate-rust-performance`. Evaluation reports are saved to `.claude/reports/`.
