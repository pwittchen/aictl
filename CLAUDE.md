# CLAUDE.md

Guidance for Claude Code when working in this repository.

See [README.md](README.md) for user-facing docs and [ARCH.md](ARCH.md) for architecture detail. This file is the compact reference for code changes.

## Build & Run

```bash
cargo build              # debug build
cargo build --release    # release build
cargo run -- <args>      # run with arguments
cargo lint               # clippy pedantic (alias in .cargo/config.toml)
cargo fmt                # format
cargo test               # run tests
```

## Module map

Single-binary async Rust CLI. Top-level modules under `src/`, plus three submodule trees: `llm/` (providers), `tools/` (tool impls), `commands/` (slash-command handlers).

- `main.rs` — CLI (clap), config + security + session init, agent loop driver, single-shot vs REPL
- `run.rs` — `run_agent_turn` loop, tool-call dispatch, outbound redaction, stream suspend wiring
- `agents.rs` (+ `agents/remote.rs`) — agent prompts in `~/.aictl/agents/` (per-user catalogue) plus project-local overrides at `<cwd>/.aictl/agents/` or `<cwd>/.claude/agents/` as a legacy fallback (the presence of `.aictl/` skips `.claude/` entirely — see `config::local_config_root`). Both bare `<name>` and `<name>.md` filenames are accepted (the `.md` convention matches the remote catalogue and the project-local directories). `read_agent` resolves local-first; `list_agents` merges both with local entries winning on name collision. Each `AgentEntry` carries an `Origin` (`Global` / `Local` / `LocalClaude`) that drives the badge in `/agent`, `--list-agents`, and the entry-aware `delete_agent_entry` / `save_agent_entry` so menu actions land at the correct file. Loaded agent is appended to the system prompt. Optional YAML frontmatter (`name`, `description`, `source`, `category`) — `source: aictl-official` renders an `[official]` badge alongside the origin tag. `agents/remote.rs` fetches the live catalogue from `.aictl/agents/` in the project repo via GitHub's trees API and pulls a single `.md` on demand (REPL browse entry or `--pull-agent <name>` + `--force`) — pulled files always land in the per-user `~/.aictl/agents/`. The frontmatter is stripped before the body is injected into the system prompt.
- `audit.rs` — per-session JSONL tool-call log under `~/.aictl/audit/<session-id>`. `set_file_override(path)` (wired from `--audit-file <PATH>`) redirects logging to an explicit file and force-enables the subsystem so single-shot runs (no session id) can capture an audit trail.
- `commands.rs` + `commands/` — slash commands (`/agent`, `/balance`, `/behavior`, `/clear`, `/compact`, `/config`, `/context`, `/copy`, `/exit`, `/gguf`, `/help`, `/history`, `/info`, `/keys`, `/memory`, `/mlx`, `/model`, `/ping`, `/plugins`, `/retry`, `/roadmap`, `/security`, `/session`, `/skills`, `/stats`, `/tools`, `/undo`, `/uninstall`, `/update`, `/version`); any other `/<name>` falls through to a user-defined skill lookup.
- `config.rs` — `~/.aictl/config` loader (`OnceLock<RwLock<HashMap>>`), constants, `load_prompt_file`, `local_config_root` (resolves `<cwd>/.aictl/` or the legacy `<cwd>/.claude/` root used by agents and skills for project-local overrides)
- `keys.rs` — keyring-backed API key storage with plain-text fallback; use `get_secret(name)` not `config_get` for keys
- `security.rs` — `SecurityPolicy`: shell/path validation, CWD jail, env scrub, output sanitization, prompt-injection guard
- `security/redaction.rs` (+ `redaction/ner.rs`) — network-boundary redactor (A: regex, B: entropy, C: optional NER)
- `session.rs` — session persistence + incognito toggle
- `skills.rs` (+ `skills/remote.rs`) — `~/.aictl/skills/<name>/SKILL.md` CRUD + frontmatter parse, with project-local overrides at `<cwd>/.aictl/skills/<name>/SKILL.md` or `<cwd>/.claude/skills/<name>/SKILL.md` as a legacy fallback (same `.aictl/` > `.claude/` precedence as agents). `find` resolves local-first; `list` merges both with local entries winning on name collision. Each `SkillEntry` carries an `Origin` (`Global` / `Local` / `LocalClaude`) that drives the badge in `/skills` and `--list-skills` and is consumed by the entry-aware `delete_entry` so menu actions target the correct directory. Skills are one-turn-scoped markdown playbooks: for one `run::run_agent_turn` call the skill body is concatenated onto `messages[0].content` (not inserted as a separate System message — Anthropic/Gemini only keep the last System they see) and never written into session history. Invoked via `/<skill-name>`, `--skill <name>`, or the `/skills` menu. `AICTL_SKILLS_DIR` overrides the per-user default directory (local overrides are unaffected). Optional YAML frontmatter (`name`, `description`, `source`, `category`) — `source: aictl-official` renders an `[official]` badge alongside the origin tag. `skills/remote.rs` fetches the live catalogue from `.aictl/skills/<name>/SKILL.md` in the project repo via GitHub's trees API and pulls a single SKILL.md on demand (REPL browse entry or `--pull-skill <name>` + `--force`) — pulled directories always land in the per-user catalogue.
- `stats.rs` — usage stats under `~/.aictl/stats`
- `tools.rs` + `tools/` — XML parsing, dispatch, duplicate guard, per-tool impls (31 tools). Unknown tool names fall through to `plugins::find()` so user-installed plugin tools dispatch through the same gate.
- `plugins.rs` — discovery + execution of user-installed plugin tools under `~/.aictl/plugins/<name>/` (override via `AICTL_PLUGINS_DIR`). Each plugin pairs a `plugin.toml` manifest (name/description/entrypoint/optional `requires_confirmation`/`timeout_secs`/`schema_hint`) with an executable. `init()` walks the directory, parses the manifest with a hand-rolled mini-TOML parser (subset: strings, bools, ints, triple-quoted strings), validates the entrypoint stays inside the plugin dir (symlink-aware, rejects collisions with built-in tool names), and stores survivors. `execute_plugin` spawns the entrypoint directly (no shell), pipes the tool body in on stdin, returns stdout — or `[exit N] <stderr>` on non-zero exit. Pinned to `security::working_dir()` with `scrubbed_env()` and the plugin's manifest timeout (falling back to `security::shell_timeout`). The whole subsystem is gated behind `AICTL_PLUGINS_ENABLED=true` (default off) — third-party code must not auto-load. `--list-plugins` prints the catalogue; `/plugins` is the REPL menu.
- `ui.rs` — `AgentUI` trait: `PlainUI` (single-shot) + `InteractiveUI` (REPL)
- `llm.rs` + `llm/` — `TokenUsage`, `MODELS` catalog, provider calls (OpenAI, Anthropic, Gemini, Grok, Mistral, DeepSeek, Kimi, Z.ai, Ollama, GGUF, MLX). `llm/balance.rs` exposes per-provider credit/quota probes used by `/balance` and `--balance` / `--list-balances`: real fetchers for DeepSeek (`GET /user/balance`) and Kimi (`GET /v1/users/me/balance` — base URL via `LLM_KIMI_BASE_URL` for the `.cn` endpoint); every other cloud provider returns `Unknown` with a billing-dashboard hint. Local providers are not probed.

## Key behaviors (non-obvious)

- **Config**: `~/.aictl/config` only — no `.env`, no system env vars for program parameters. CLI args override. `config_set` / `config_unset` write through to disk and cache.
- **Prompt file**: `AICTL.md` in CWD is appended to system prompt (override via `AICTL_PROMPT_FILE`). Falls back to `CLAUDE.md` then `AGENTS.md` unless `AICTL_PROMPT_FALLBACK=false`.
- **Security gate**: every tool call passes through `security::validate_tool()` before exec and output sanitization on return. `--unrestricted` bypasses validation; audit + redaction keep running.
- **Redaction**: runs in `run::redact_outbound` right before provider dispatch. Local providers (Ollama/GGUF/MLX) skipped unless `AICTL_SECURITY_REDACTION_LOCAL=true`. Session file keeps the original user text — redaction is a network-boundary control.
- **Streaming**: `call_X(..., on_token)` — `Some` → streaming path, `None` → buffered. `StreamState` in `src/llm/stream.rs` holds back anything that could prefix `<tool name="` so tool XML never hits the UI. Auto-disables under `--quiet`, compaction, agent-prompt generation, non-TTY stdout. Skips termimad markdown re-render.
- **Agent loop**: up to 20 iterations. Every provider call wrapped in `tokio::time::timeout` (`AICTL_LLM_TIMEOUT`, default 30s; `0` disables).
- **Key storage**: `keys::get_secret(name)` checks keyring first, falls back to plain config. `keyring` v3 needs `apple-native` + `sync-secret-service` features or it silently uses a mock store.
- **CLI flags**: long-form only. Only short flags are `-v` / `-h`.
- **Cargo features** (default off): `gguf` (llama-cpp-2), `mlx` (macOS+aarch64), `redaction-ner` (gline-rs). Model management CLI paths compile on every build; only the inference call is feature-gated and returns a rebuild hint when missing.

## Conventions

- Rust edition 2024, default rustfmt and clippy settings.
- Commit messages follow `.claude/skills/commit/SKILL.md` — no AI attribution, imperative mood, short for small changes.
- After implementing a feature or fixing a bug, check `ROADMAP.md` — remove the item if resolved.
- Claude Code skills live in `.claude/skills/` — `/commit`, `/update-docs`, `/evaluate-rust-quality`, `/evaluate-rust-security`, `/evaluate-rust-performance`. Evaluation reports land in `.claude/reports/`.
