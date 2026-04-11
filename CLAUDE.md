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

Single-binary async Rust CLI with nineteen modules:

- `src/main.rs` — CLI args (clap), config loading (`~/.aictl/config`), security init, session init, agent loop, single-shot and interactive REPL modes
- `src/agents.rs` — agent prompt management under `~/.aictl/agents/`. Global loaded-agent state (`Mutex<Option<(name, prompt)>>`), CRUD operations (`save_agent`, `read_agent`, `delete_agent`, `list_agents`), name validation (alphanumeric + underscore/dash only)
- `src/commands.rs` — REPL slash command handling (`/agent`, `/behavior`, `/clear`, `/clear-keys`, `/compact`, `/context`, `/copy`, `/exit`, `/help`, `/info`, `/issues`, `/lock-keys`, `/memory`, `/model`, `/security`, `/session`, `/tools`, `/unlock-keys`, `/update`). `MemoryMode` enum (LongTerm/ShortTerm) for conversation history optimization. Returns a `CommandResult` enum consumed by the REPL loop in `main.rs`. Also hosts the `/agent` interactive menu (create manually, create with AI, view all, unload), the `/session` interactive menu (current info, set name, view saved, clear all), the key migration runners (`run_lock_keys`, `run_unlock_keys`, `run_clear_keys`), the non-interactive `print_sessions_cli` and `print_agents_cli` helpers, and the `--config` interactive configuration wizard (`run_config_wizard`).
- `src/config.rs` — config file loading (`~/.aictl/config`) into a `static OnceLock<RwLock<HashMap<String, String>>>` so writes via `config_set` / `config_unset` keep the in-memory cache consistent. Also holds constants (system prompt, spinner phrases, agent loop limits) and project prompt file loading (`load_prompt_file`).
- `src/keys.rs` — secure API key storage. Wraps the system keyring (macOS Keychain, Windows Credential Manager, Linux Secret Service via the `keyring` crate) with transparent plain-text fallback. Provides `get_secret(name)` (used everywhere instead of `config_get` for API keys), `backend_available()` / `backend_name()` for the welcome banner and `/security`, `location(name)` returning a `KeyLocation::{None, Config, Keyring, Both}`, and `lock_key` / `unlock_key` / `clear_key` migration primitives backing the slash commands. `KEY_NAMES` is the canonical list of recognized keys.
- `src/security.rs` — `SecurityPolicy` with shell command validation, path validation (CWD jail, blocked paths, canonicalization), environment scrubbing, shell timeout, output sanitization. Loaded into `static OnceLock` at startup. Configurable via `AICTL_SECURITY_*` keys in `~/.aictl/config`.
- `src/session.rs` — session persistence under `~/.aictl/sessions/`. Generates UUID v4 ids (from `/dev/urandom` with time-based fallback), stores conversation history as JSON per session, maintains a `.names` file mapping uuid → unique human-readable name. Exposes a global current-session handle and an incognito toggle (`set_incognito` / `is_incognito`) that short-circuits all saves.
- `src/tools.rs` — tool-call XML parsing, tool execution dispatch (security gate at entry, output sanitization at exit). `truncate_output` walks back to the nearest UTF-8 char boundary before truncating so multi-byte characters straddling the cut don't panic.
- `src/ui.rs` — `AgentUI` trait with `PlainUI` (single-shot) and `InteractiveUI` (REPL with spinner, colors, markdown rendering) implementations. The welcome banner shows the active key storage backend and lock/plain counts.
- `src/llm.rs` — shared `TokenUsage` type with cost estimation, model list, context limits
- `src/llm_openai.rs`, `src/llm_anthropic.rs`, `src/llm_gemini.rs`, `src/llm_grok.rs`, `src/llm_mistral.rs`, `src/llm_deepseek.rs`, `src/llm_kimi.rs`, `src/llm_zai.rs`, `src/llm_ollama.rs` — provider-specific API call implementations

**Config**: Loaded once at startup from `~/.aictl/config` into a `static OnceLock<RwLock<HashMap<String, String>>>`. CLI args override config values. The `config_get(key)` helper reads config values; `config_set(key, value)` and `config_unset(key)` write through to both the file and the in-memory cache so subsequent reads see the change. No `.env` files or system environment variables are used for program parameters. A project prompt file (default `AICTL.md`, configurable via `AICTL_PROMPT_FILE`) is loaded from the current directory and appended to the system prompt via `build_system_prompt()` in `main.rs`.

**Flow**: CLI args (clap) → `security::init()` → resolve incognito flag (`--incognito` or `AICTL_INCOGNITO`) → load `--agent <name>` if given → `build_system_prompt()` (base system prompt + optional `AICTL.md` content + loaded agent prompt) → `run_agent_turn` loop → provider call → parse response for `<tool>` tags → security validation → execute tool or print final answer. In interactive mode, the REPL also initializes a session (new uuid or loaded from `--session <id|name>`) before printing the welcome banner.

**Agents**: Reusable system prompt extensions stored as plain text files in `~/.aictl/agents/<name>`. Managed via `/agent` menu: create manually (multi-line text input), create with AI (LLM generates prompt from description), view all (browse/load/delete), unload. Can also be loaded via `--agent <name>` / `-A <name>` CLI flag (works in both single-shot and interactive modes). `--list-agents` / `-L` prints all saved agents and exits. When loaded, the agent prompt is appended to the system prompt under `# Agent: <name>`. The REPL prompt shows the agent name in magenta brackets (e.g. `[my-agent] ❯`).

**Modes**: Single-shot (`-m "message"`) uses `PlainUI`; omitting `-m` starts an interactive REPL with `InteractiveUI` (history, spinner, markdown, colored output). `--quiet`/`-q` (requires `--auto`) suppresses reasoning and tool call output in single-shot mode, printing only the final answer. `--config` launches an interactive wizard to set provider, model, and API keys (saves to `~/.aictl/config` and exits). **Memory modes**: Long-term (default, sends all messages to LLM) and Short-term (sends system prompt + last 20 messages via a sliding window). Configurable via `/memory` command or `AICTL_MEMORY` in `~/.aictl/config`.

**Sessions**: In interactive mode, the REPL creates a new session (UUID v4) at startup and persists the full conversation to `~/.aictl/sessions/<uuid>` as JSON after every agent turn and after manual/auto compaction. Optional human-readable session names live in `~/.aictl/sessions/.names` (tab-separated, uuid → name; names must be unique). The welcome banner shows the active session uuid (and name, if set), and a notice is printed on exit. `--session <uuid|name>` resumes a saved session, `--list-sessions` and `--clear-sessions` are non-interactive helpers. **Incognito mode**: `--incognito` or `AICTL_INCOGNITO=true` (accepts `true`/`false` only, default `false`) disables all session creation and saves; the welcome banner shows a yellow notice, `/session` prints a disabled message, and no exit save line is printed.

**API key storage** (`src/keys.rs`): API keys are read via `keys::get_secret(name)` which checks the system keyring first and falls back to plain-text `~/.aictl/config`. Backend selection is automatic per OS (macOS Keychain via `apple-native`, Windows Credential Manager via `windows-native`, Linux Secret Service via `sync-secret-service`). The welcome banner shows `keys: <backend> (N locked · N plain · N both)` and `/security` lists the per-key location after the security policy summary. Migration is performed via `/lock-keys` (config → keyring), `/unlock-keys` (keyring → config), and `/clear-keys` (remove from both, with confirmation). When the keyring backend is unavailable the helper functions return an error and aictl transparently keeps using the plain-text store. Important: the `keyring` crate v3 must be built with explicit platform features enabled (`apple-native`, `windows-native`, `sync-secret-service`) — without them it silently falls back to a mock in-memory store that pretends writes succeeded.

**Agent loop** (`run_agent_turn`): Maintains a conversation history (`Vec<Message>`) with system prompt, user message, and assistant/tool-result turns. Loops up to 20 iterations. Tool calls are parsed from custom XML tags in the LLM response text. Supports `--auto` mode (skip confirmation) or interactive y/N confirmation. Always displays token usage, estimated cost, and execution time after each LLM call and as a summary after each turn.

**Security** (`src/security.rs`): All tool calls pass through `security::validate_tool()` before execution. Shell commands are validated against blocked/allowed lists with command substitution blocking. File tools are restricted to the working directory (CWD jail) with path canonicalization to defeat traversal attacks. Individual tools can be disabled via `AICTL_SECURITY_DISABLED_TOOLS`. Shell subprocesses get a scrubbed environment (strips `*_KEY`, `*_SECRET`, `*_TOKEN`, `*_PASSWORD`) and a configurable timeout (default 30s). Tool output is sanitized to prevent `<tool>` tag injection. User prompts pass through `security::detect_prompt_injection()` at the top of `run_agent_turn` — inputs containing instruction-override phrases or forged role/tool tags are blocked before ever reaching the LLM; toggled via `AICTL_SECURITY_INJECTION_GUARD` (default `true`). Configurable via `AICTL_SECURITY_*` keys in `~/.aictl/config`. Bypassed entirely with `--unrestricted`.

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

**Providers**: OpenAI (`call_openai`), Anthropic (`call_anthropic`), Gemini (`call_gemini`), Grok (`call_grok`), Mistral (`call_mistral`), DeepSeek (`call_deepseek`), Kimi (`call_kimi`), Z.ai (`call_zai`), and Ollama (`call_ollama`) each convert `&[Message]` to provider-specific formats. Anthropic uses a top-level `system` field; OpenAI, Grok, Mistral, DeepSeek, Kimi, and Z.ai include system messages inline; Gemini uses a `systemInstruction` field and maps assistant role to `model`; Ollama uses its native `/api/chat` endpoint with `stream: false`. All return `TokenUsage` for cost tracking and timing display. Ollama requires no API key; its models are discovered dynamically via `/api/tags`.

**Key dependencies**: `clap` (CLI parsing), `reqwest` (async HTTP with JSON), `serde`/`serde_json` (serialization), `tokio` (async runtime, process, fs), `crossterm` (terminal styling), `indicatif` (spinner), `rustyline` (REPL input/history), `termimad` (markdown rendering), `scraper` (HTML DOM parsing with CSS selectors), `glob` (file glob pattern matching), `keyring` (system keyring access — built with `apple-native`, `windows-native`, `sync-secret-service` features).

## Conventions

- Rust edition 2024, default rustfmt and clippy settings.
- Commit messages follow the rules in `.claude/skills/commit/SKILL.md` — no AI attribution lines, imperative mood, short messages for small changes, detailed body for larger ones.
- After implementing a new feature or fixing a bug, check `ISSUES.md` — if the change resolves an issue listed there, remove that issue from the list.
- Claude Code skills are in `.claude/skills/` — `/commit`, `/update-docs`, `/evaluate-rust-quality`, `/evaluate-rust-security`, `/evaluate-rust-performance`. Evaluation reports are saved to `.claude/reports/`.
