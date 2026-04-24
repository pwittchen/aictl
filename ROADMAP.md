# Roadmap

---

## General

### Developer Experience

- **Plugin / extension system** — Let users add custom tools via external scripts or WASM modules without forking the repo. See [.claude/plans/plugin-system.md](.claude/plans/plugin-system.md) for the development plan.
- **MCP server support** — Allow users to connect [Model Context Protocol](https://modelcontextprotocol.io) servers as a source of additional tools, resources, and prompts. Config entries under `~/.aictl/config` (or a dedicated `~/.aictl/mcp.json`) declare each server's transport (stdio command or HTTP/SSE URL), environment, and enablement. At startup, spawn/connect each configured server, list its tools/resources, namespace them (e.g. `mcp__<server>__<tool>`) and merge them into the built-in tool registry so the agent loop can dispatch them transparently. Tool calls route through the MCP client, results flow back like any other tool output, and the security policy (`security::validate_tool`) still applies — per-server allow/deny lists, CWD jailing for stdio servers, and redaction on outbound payloads. Add `/mcp` slash command to list, enable/disable, or restart servers, and `--list-mcp` / `--mcp-server <name>` CLI flags for scripted use. Remote catalogue (similar to `agents/remote.rs` and `skills/remote.rs`) can expose a curated set of official servers pullable on demand. See [.claude/plans/mcp-support.md](.claude/plans/mcp-support.md) for the development plan.
- **Inter-instance messaging** — When two or more `aictl` instances run on the same machine, let them exchange messages so one instance can send a prompt, note, or context snippet to another and vice-versa. Likely implementation: a local transport (Unix domain socket or named pipe under `~/.aictl/ipc/`) where each instance registers with its session id, a `/send <session-id> <message>` slash command to dispatch, and an inbox surfaced in the receiving REPL (either interrupting the prompt or queued until the next turn). Needs to decide whether incoming messages feed into the conversation automatically or require the user to accept them, and how this interacts with `--unrestricted` and the security policy. See [.claude/plans/inter-instance-messaging.md](.claude/plans/inter-instance-messaging.md) for the development plan.

---

## Modular Architecture

Prerequisite for both Server and Desktop. Today the codebase is a single binary crate where the agent loop, providers, security, and CLI presentation are entangled. To support multiple frontends (CLI, Server, Desktop) without forking logic, separate the code that is *frontend-agnostic* from the code that is *CLI-specific*. See [.claude/plans/modular-architecture.md](.claude/plans/modular-architecture.md) for the development plan.

### Cargo workspace split

Split the repo into a workspace with three crates:

- **`aictl-core`** (library) — shared engine. Owns: `agents`, `audit`, `config`, `keys`, `llm/` (all providers), `run`, `security/`, `session`, `skills`, `stats`, `tools/`. Defines the `AgentUI` trait as the boundary between engine and frontend. Exposes typed events (`AgentEvent`, `ToolCallEvent`, `StreamChunk`, `TokenUsage`) over `tokio::mpsc` so any frontend can consume the agent loop asynchronously.
- **`aictl-cli`** (binary) — pure CLI surface. Owns: `main.rs` (clap parsing), `commands/` slash-command handlers, `ui.rs` (`PlainUI` + `InteractiveUI`), REPL line editor, terminal markdown rendering (`termimad`), spinners, and the y/N tool-approval prompt. Depends on `aictl-core`.
- **`aictl-server`** (binary, future) — HTTP gateway. Implements `HttpUI` against the `AgentUI` trait, translates engine events into JSON / SSE responses. Depends on `aictl-core` only.
- **`aictl-desktop`** (binary, future) — Tauri shell. Implements `DesktopUI` against the `AgentUI` trait. Depends on `aictl-core` only.

### What stays in core, what moves to CLI

- **Stays in core**: anything that produces structured output (events, errors, typed results) or operates on persistent state (config, sessions, keys, audit, stats, agents/skills directories). All provider HTTP calls. The agent loop iteration limit, timeout, and tool dispatch. The security policy.
- **Moves to CLI**: every `println!`/`eprintln!`, `print!`, `eprint!`, `dialoguer` prompt, terminal color code, spinner widget, and `termimad` render currently scattered through the engine. These get routed through `AgentUI` methods (`show_answer`, `show_tool_call`, `confirm_tool`, `show_error`, `progress_start/end`) that the CLI implements with terminal output and the server/desktop implement with channel sends.
- **Shared but parameterized**: the prompt-file lookup (`AICTL.md` → `CLAUDE.md` → `AGENTS.md`) lives in core but takes the working directory as an argument rather than reading `std::env::current_dir()` directly, so the server can scope it per-request.

### Refactoring steps

1. **Inventory side-effects** — grep core modules for `println!`, `eprintln!`, `print!`, `eprint!`, `dialoguer::`, `indicatif::`, `termimad::` and list every call site that needs to become an `AgentUI` method invocation.
2. **Extend `AgentUI`** — add the missing methods so `PlainUI` and `InteractiveUI` can absorb every stripped-out call without losing functionality. Keep the trait small and event-shaped, not procedural.
3. **Convert `Cargo.toml` to a workspace** — move existing `src/` into `crates/aictl-cli/src/`, then progressively pull frontend-agnostic modules into `crates/aictl-core/src/`. CI stays green at every step.
4. **Lock the public API** — once the split compiles, mark `aictl-core` re-exports explicitly (`pub use`) and treat the surface as semver-stable so server and desktop can depend on it confidently.
5. **Feature flags follow the split** — `gguf`, `mlx`, `redaction-ner` move to `aictl-core` (they gate provider/redaction code). `aictl-cli` only carries CLI-presentation features.

### Why this comes first

Without this split, the Server section duplicates `run::run_agent_turn` wrapper logic and the Desktop section can't avoid re-implementing tool dispatch. Doing the extraction once means Server, Desktop, and any future frontend (web, MCP server, automation harness) all consume the same engine with the same security guarantees.

### Separate binaries vs. one binary

Ship the CLI and Server as **two separate binaries** (`aictl` and `aictl-server`) from the same workspace, not as one binary with a `serve` subcommand. Reasons:

- **Dependency footprint** — the server pulls in `axum`, HTTP/SSE machinery, JSON schemas, and auth middleware. None of that belongs in a CLI binary that most users install via `cargo install` or Homebrew. Keeping them separate keeps the CLI lean and its cold-start fast.
- **Different runtime profiles** — CLI is short-lived, single-user, terminal-bound. Server is long-lived, multi-user, network-bound. They want different defaults for logging, panic behavior, signal handling, and telemetry.
- **Different security postures** — the CLI assumes the local user owns the machine; the server must assume an untrusted caller. Mixing them in one binary makes it easy for a CLI-only assumption to leak into a server code path.
- **Distribution** — most CLI users will never run the server. Server operators want a narrow artifact (Docker image, systemd unit) without REPL deps like `rustyline`/`termimad`.
- **Natural fit with the workspace split** — `aictl-core` does the real work; `aictl-cli` and `aictl-server` are thin frontends. One-binary-with-subcommand would re-entangle what the workspace split just separated.

**Optional ergonomic shim**: if user feedback later asks for `aictl serve`, add a thin subcommand in the CLI binary that `exec`s `aictl-server` when present on `$PATH` — keeps the convenience without bundling the server deps.

---

## Server

Run `aictl` as a long-lived HTTP server that exposes the agent loop and every supported LLM provider through a single gateway API. Reuses the same `~/.aictl/config` as the CLI; an alternative path can be supplied via `--config <path>` (and a matching `AICTL_CONFIG` env var) so multiple server instances can run side-by-side with different keys, models, and security policies.

### Launch surface

- `aictl serve` subcommand (or `--serve` flag) starts the HTTP server. Flags: `--config <path>`, `--bind <addr:port>` (default `127.0.0.1:7878`), `--unrestricted`, `--quiet`. Long-form only, matching existing CLI conventions.
- Config file resolution: explicit `--config` wins, otherwise fall back to the default `~/.aictl/config`. The loader in `config.rs` already uses `OnceLock<RwLock<HashMap>>` — extend it to accept an override path at init time instead of hard-coding the home-directory location.
- Server runs the same security gate, redaction, audit, and stats subsystems as the CLI. `--unrestricted` disables `security::validate_tool` exactly as in the CLI; redaction and audit always run.

### HTTP API

- `POST /v1/chat` — single-shot agent turn. Body: `{ "prompt": "...", "model": "...", "agent": "...", "skill": "...", "session_id": "..." }`. Returns the final assistant message plus tool-call trace and token usage. Server-Sent Events variant at `POST /v1/chat/stream` for token streaming, mirroring the CLI's `StreamState` behavior (tool XML never reaches the client).
- `GET /v1/models` — list every model from `llm::MODELS` plus locally available Ollama/GGUF/MLX models, with provider, context window, and availability flags.
- `GET /v1/agents`, `GET /v1/skills` — list installed agents/skills with frontmatter (`name`, `description`, `source`, `category`).
- `GET /v1/sessions`, `GET /v1/sessions/{id}`, `DELETE /v1/sessions/{id}` — session CRUD backed by the existing `session.rs` store.
- `POST /v1/tools/{name}` — direct tool invocation (gated by the security policy) for clients that want the tool registry without going through the agent loop.
- `GET /healthz`, `GET /v1/stats` — liveness + usage stats from `stats.rs`.

### LLM gateway mode

Beyond the agent endpoint, expose an OpenAI-compatible passthrough (`POST /v1/completions`, `POST /v1/chat/completions`) that routes to whichever provider matches the requested model id. This lets existing OpenAI-SDK clients point at `aictl` and transparently consume Anthropic, Gemini, Grok, Mistral, DeepSeek, Kimi, Z.ai, Ollama, GGUF, or MLX models — with redaction, audit, and key management handled centrally by the server.

### Auth and isolation

- Local-only bind by default (`127.0.0.1`). Remote bind requires explicit `--bind 0.0.0.0:...` plus a bearer token configured in `~/.aictl/config` (`server_token = "..."`).
- Per-request `X-AICTL-Session` header for session affinity; otherwise a fresh ephemeral session is created.
- CORS off by default; opt-in via config for browser clients.

### Implementation notes

- Likely framework: `axum` (already in the Tokio ecosystem) for low-overhead async routing and SSE support.
- Reuse `run::run_agent_turn` directly — wrap it with a per-request `AgentUI` impl (`HttpUI`) that pipes events into an `mpsc` channel feeding the response stream.
- Coding agent mode (see below) stays CLI-only; the server rejects coding-mode requests.
- Same module split as the desktop plan benefits the server: an `aictl-core` crate keeps the agent loop reusable across `aictl-cli`, `aictl-desktop`, and a future `aictl-server` binary.

---

## Desktop

Create a desktop app with the same capabilities as the CLI. macOS support is required; other platforms are a stretch goal. Builds on the **Modular Architecture** section above — `aictl-desktop` is a new workspace crate that depends on `aictl-core`.

### Core API stabilization

Define clean public types for the desktop to consume: `AgentLoop`, `Conversation`, `ToolCallEvent`, `StreamChunk`, etc. Add a channel-based interface (`tokio::mpsc`) so the desktop can receive events asynchronously — streaming tokens, tool approval requests, progress updates.

### GUI framework: Tauri v2

Use Tauri v2 for the desktop shell. Rust core runs as the Tauri backend; `#[tauri::command]` functions wrap `aictl-core`. The frontend (React/Svelte/Solid) handles the chat UI, session sidebar, agent management, and settings. Chat UIs are trivially good in HTML/CSS — markdown rendering, syntax highlighting, streaming text are solved problems in the web ecosystem. Cross-platform (macOS/Linux/Windows), small binary (~5-10MB), and the same frontend could be reused for a web version later.

### Desktop `AgentUI` implementation

Implement a `DesktopUI` that satisfies the `AgentUI` trait: send messages to the GUI thread instead of stdout, show tool approval via a dialog instead of terminal y/N, render markdown in a rich text widget, show spinners/progress as native UI elements. The agent loop in core calls `ui.show_answer()`, `ui.confirm_tool()`, etc. — the desktop provides a different implementation.

### Tool approval UX

In the CLI it's a blocking y/N prompt. In the desktop, the agent loop should `await` a response from a channel that the UI resolves when the user clicks approve/deny in a dialog.

### Phased rollout

1. Stabilize the core API — channel-based event interface (assumes the Modular Architecture workspace split is already done).
2. Scaffold the desktop app — Tauri + minimal frontend, send a message and see the response.
3. Feature parity incrementally — sessions, agents, tool approval dialogs, settings, stats, one at a time.

---

## Coding Agent

Provide configurable mode, which will transform the general purpose agent into the coding agent. There should be additional skills/tools and prompts available for such mode, which won't be available in the "default" general purpose mode. Coding agent should work only in CLI app and be unavailable for server and desktop.

### Agent workflow

The coding agent should follow a structured five-phase workflow for every task:

1. **Explore** — Read relevant files, search the codebase, understand the existing code and project structure before making any changes.
2. **Plan** — Formulate an explicit plan of action: what to change, where, and why. Present the plan to the user for approval before proceeding.
3. **Code** — Implement the changes according to the plan. Make minimal, focused edits. Follow existing code style and conventions.
4. **Review** — Self-review the changes: check the diff for unintended modifications, run the linter, verify the original request was addressed. Fix any issues found.
5. **Test** — Run relevant tests to verify correctness. If tests fail, fix the failures and re-run until green. Report the final test results to the user.

This workflow should be enforced via the coding-specific system prompt and reflected in the agent loop behavior. Each phase naturally feeds into the next — skipping phases (e.g., coding without exploring first) leads to lower quality results.

**Implementation approach**:

- **Phase tracking via an enum**: Introduce a `WorkflowPhase` enum (`Explore`, `Plan`, `Code`, `Review`, `Test`) in the agent loop. Track the current phase and inject phase-specific guidance into the system prompt at each transition. The LLM signals phase transitions via a structured tag (e.g., `<phase>plan</phase>`) parsed alongside tool calls — or implicitly when certain tools are used (e.g., first `write_file`/`edit_file` call transitions from Plan to Code).
- **Phase-specific system prompt injection**: Before each LLM turn, append a short directive based on the current phase. For example, during Explore: "You are in the exploration phase. Read files, search the codebase, and understand the problem. Do not make any edits yet." During Plan: "You are in the planning phase. Describe your plan step by step. Do not edit files until the user approves." This steers the LLM without hard-blocking tool calls.
- **Explore phase**: The agent should use `read_file`, `search_files`, `find_files`, `list_directory`, and the future `git` tool (status, log, blame) to build context. The phase ends when the LLM has enough understanding to propose a plan — signaled by the LLM producing a plan output rather than more read/search calls.
- **Plan phase**: The agent outputs a numbered plan. In interactive mode, the user can approve, modify, or reject it. In `--auto` mode, the plan is logged and execution proceeds immediately. Store the plan in the conversation history so the agent can reference it during later phases.
- **Code phase**: The agent executes edits according to the plan. Track which plan steps have been addressed. The phase ends when the LLM stops making edits and signals completion.
- **Review phase**: Automatically inject a review prompt after the Code phase completes. The agent runs `git diff`, the project linter (auto-detected or configured via `AICTL_CODING_LINTER`), and checks the diff against the original plan. If issues are found, loop back to Code to fix them. This overlaps with the "Self-review before completion" section below — share the implementation.
- **Test phase**: After review passes, the agent runs tests. Auto-detect the test command (`cargo test`, `npm test`, `pytest`, etc.) or use a configured `AICTL_CODING_TEST_CMD`. Parse output for pass/fail. On failure, loop back to Code → Review → Test until green or a retry limit is hit (configurable, default 3 attempts).
- **UI integration**: In interactive mode, show the current phase in the prompt or status line (e.g., `[explore] ❯`, `[plan] ❯`). In `--quiet` mode, suppress phase indicators. The `/skip` command could allow jumping to the next phase when the user decides the current one is done.
- **Configuration**: `AICTL_CODING_WORKFLOW=true` (default when coding agent mode is active). Individual phases can be skipped via config (e.g., `AICTL_CODING_SKIP_REVIEW=true`) for quick iterations. The retry limit for the test loop is configurable via `AICTL_CODING_TEST_RETRIES`.

### Streaming output

Stream LLM responses token-by-token instead of waiting for the full response. Parse tool calls from the stream incrementally (detect `<tool` opening tag, buffer until `</tool>`). Display reasoning text in real-time, then execute tools after the stream completes.

### Parallel tool execution

Allow the LLM to return multiple `<tool>` tags per response and execute independent calls concurrently via `tokio::JoinSet`. Inject all results back as a batch before the next LLM turn. Remove the "use at most one tool call per response" constraint from the system prompt.

### Coding-specific system prompt

Add coding-specific guidance to the system prompt when in coding agent mode: read files before editing, run tests after changes, don't introduce security vulnerabilities, prefer minimal changes, diagnose errors before retrying, check git status before and after changes. This dramatically improves coding behavior without any tool changes.

### Smarter edit tool

Improve `edit_file` beyond single-occurrence exact-match find-and-replace:
- Support multiple edits per call (list of old/new blocks applied top-to-bottom).
- Add line-number addressing as a fallback when exact match is ambiguous.
- Add fuzzy matching with a similarity threshold for near-misses (trailing whitespace, indentation changes).
- Consider a write-with-diff approach: the LLM writes the full new file content, and the user sees a diff for approval.

### Read file with line numbers and selective reading

Add line numbers to `read_file` output so the LLM can reference them in edits. Support selective reading (e.g., read lines 50-100 of a file). Increase the truncation limit beyond 10KB for large files — consider 50-100KB with smart truncation (keep first/last N lines, summarize middle).

### Native git tool

Dedicated `git` tool with subcommands: `status`, `diff`, `log`, `blame`, `commit`, `branch`, `checkout`, `add`, `stash`. Safety tiers: read-only ops (status, diff, log, blame) auto-approved; write ops (commit, branch) need confirmation; destructive ops (push --force, reset --hard) always require explicit approval. Feed `git diff` and `git status` into the system prompt automatically so the LLM always knows the repo state.

### Code-aware search

Replace hand-rolled glob+match search with ripgrep (`rg`) for 10-100x speed improvement on large repos. Respect `.gitignore` in `find_files` and `search_files`. Add symbol search via `ctags`, `tree-sitter`, or LSP to find definitions/references by symbol name rather than raw text.

### Automatic context injection

On startup, inject a project summary into the system prompt: git branch, recent commits, directory tree (depth 2), language/framework detection. After each edit, auto-run the relevant linter and feed errors back. After test failures, auto-include the failure output in the next turn.

### Test loop integration

Close the edit-test-fix cycle: after edits, auto-detect the test framework and run relevant tests. Parse test output to identify failures. Feed failures back to the LLM for automatic fixing. Support a test-driven mode: run tests, fix failures, repeat until green.

### Multi-turn planning and task decomposition

For complex tasks, let the LLM create an explicit plan (list of steps) and track completion. Show the user the plan before execution for approval. Allow the user to modify the plan mid-execution.

### Self-review before completion

Before reporting a task as done, the agent should automatically review its own changes. This catches regressions, forgotten files, and style violations without the user having to ask.

**Desired behavior**: After the agent finishes editing, it runs a review pass — check `git diff` for unintended changes, run the project linter, run relevant tests, and verify the original request was actually addressed. If any check fails, the agent fixes the issue and re-reviews before presenting the result to the user.

**Implementation approach**:
- Add a `review` phase to the agent loop that triggers when the LLM produces a final answer after tool calls. Before surfacing the answer, inject a "review your changes" prompt that asks the LLM to: (1) run `git diff` and verify only intended files changed, (2) run the linter (`cargo clippy`, `eslint`, etc. — auto-detected or configured), (3) run tests related to the changed files, (4) confirm the original task is complete.
- The review phase reuses existing tools (`exec_shell` / future `git` and `test` tools) — no new tool is needed, just an additional agent loop iteration with a review-specific prompt.
- Make the review configurable: `AICTL_CODING_REVIEW=true` in config or a `--review` flag. Allow skipping with `/skip-review` during interactive mode.
- In `--auto` mode, the review runs silently and only surfaces if it finds issues. In interactive mode, show a brief summary ("Review: diff OK, linter clean, 3/3 tests pass").
- For the system prompt, add guidance like: "After completing edits, always review your changes before reporting done. Check the diff is minimal, the linter is clean, and tests pass."

### Tool output improvements

- `list_directory` should show file sizes and support recursive tree view (depth-limited).
- Tool output truncation at 10KB is too aggressive for large files — consider 50-100KB with smart truncation (keep first/last N lines, summarize middle).
- Per-tool output size limits instead of a single global cap.
