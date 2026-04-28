# Roadmap

---

## Server

Run `aictl-server` as a long-lived HTTP **LLM proxy** that exposes every supported LLM provider through a single OpenAI-compatible gateway API. Reuses the same `~/.aictl/config` as the CLI; an alternative path can be supplied via `--config <path>` (and a matching `AICTL_CONFIG` env var) so multiple server instances can run side-by-side with different keys and policies. See [.claude/plans/server.md](.claude/plans/server.md) for the development plan.

### Scope: proxy only, no agent capabilities

The server is **not** an agent host. It does not run the agent loop, dispatch tools, load agents/skills, manage sessions, or expose any of the slash-command surface — those remain CLI-only and REPL-only features. The server's single job is to forward requests to the configured LLM providers (cloud and local) with redaction, audit, and key management handled centrally. Any client that needs agent loops, tool dispatch, agents, skills, sessions, or coding-mode workflows uses the CLI.

### Launch surface

- `aictl-server` is a separate binary in the workspace. Flags: `--config <path>`, `--bind <addr:port>` (default `127.0.0.1:7878`), `--quiet`, `--log-level <level>`, `--log-file <path>`. Long-form only, matching existing CLI conventions.
- Config file resolution: explicit `--config` wins, otherwise fall back to the default `~/.aictl/config`. The loader in `config.rs` already uses `OnceLock<RwLock<HashMap>>` — extend it to accept an override path at init time instead of hard-coding the home-directory location.
- Server runs the same redaction and audit subsystems as the CLI on the proxy path. The CLI security gate (`security::validate_tool`) does not apply because no tools are dispatched server-side.

### HTTP API

- `POST /v1/chat/completions` — OpenAI-compatible chat endpoint. Routes to whichever provider matches the requested `model` id. Streaming via `stream: true` returns SSE frames in OpenAI's `data: {"choices":[{"delta":...}]}` shape.
- `POST /v1/completions` — OpenAI-compatible legacy text-completion endpoint, with the same provider routing rules.
- `GET /v1/models` — list every model from `llm::MODELS` plus locally available Ollama/GGUF/MLX models, with provider, context window, and availability flags.
- `GET /healthz` — liveness probe (no auth).
- `GET /v1/stats` — usage stats from `stats.rs` (authenticated).

This endpoint set is intentionally narrow. There are no agent endpoints, no `/v1/chat`, no agent/skill/session listings, no `/v1/tools/*`. Clients using the OpenAI SDK can point their base URL at `aictl-server` and transparently consume Anthropic, Gemini, Grok, Mistral, DeepSeek, Kimi, Z.ai, Ollama, GGUF, or MLX models — that is the entire feature set.

### Master API key

Every request requires `Authorization: Bearer <master-key>` regardless of bind address. The server reads the master key from `~/.aictl/config` (`server_master_key`), or `--master-key <value>` overrides it for a single launch.

If neither is set on first startup, the server **generates a cryptographically random master key**, writes it to `~/.aictl/config` (`server_master_key`), and prints it once to stderr (and to the configured server log) so the operator can copy it. Subsequent launches reuse the persisted key. Rotate by deleting the config entry (server regenerates on next launch) or by setting `server_master_key` manually.

The master key gates the entire server. There is no second tier of credentials and no per-route auth bypass except `GET /healthz`. Comparison is constant-time. This replaces the previous `server_token` knob — the auto-generation behavior makes "you must set a token" the default rather than a manual setup step.

### Auth and isolation

- Local-only bind by default (`127.0.0.1`). Remote bind requires `--bind 0.0.0.0:...`; the master key is required regardless of bind address.
- CORS off by default; opt-in via config for browser clients.
- TLS not terminated by the server in v1 — operators run nginx/Caddy in front for HTTPS.

### Server log

The server writes a structured request log to `~/.aictl/server.log` by default; `--log-file <path>` and `server_log_file` override the destination. Each entry includes a timestamp, request id, method, path, status, elapsed time, model, provider, and (where present) the upstream provider's request id, with redaction applied to any included payload preview. The audit log (`~/.aictl/audit/<session-id>`) continues to record provider dispatches as it does for the CLI; the new server log is a higher-level operational record specifically for the proxy. Log level is set by `server_log_level` (`trace`/`debug`/`info`/`warn`/`error`) or `--log-level`.

### Implementation notes

- Likely framework: `axum` (already in the Tokio ecosystem) for low-overhead async routing and SSE support.
- Reuse the existing `llm::call_<provider>` functions directly — the server is a thin translation layer between OpenAI's request/response schema and each provider's native format. There is no `HttpUI`, no `AgentUI` consumer, no agent loop wiring on the server.
- Coding agent mode and every other CLI/REPL feature (slash commands, agents, skills, plugins, hooks, sessions, tool dispatch) stay CLI-only. The server has no surface for them.
- The provider implementations already live in the workspace's `engine` crate, so `aictl-server` would consume them the same way `cli` does today — a new workspace member with a path dependency on `engine`.

---

## Desktop

Create a desktop app with the same capabilities as the CLI. macOS support is required; other platforms are a stretch goal. The workspace already exposes a frontend-agnostic `engine` crate; `aictl-desktop` would be a new workspace member that depends on it the same way `cli` does today.

### Core API stabilization

Define clean public types for the desktop to consume: `AgentLoop`, `Conversation`, `ToolCallEvent`, `StreamChunk`, etc. Add a channel-based interface (`tokio::mpsc`) so the desktop can receive events asynchronously — streaming tokens, tool approval requests, progress updates.

### GUI framework: Tauri v2

Use Tauri v2 for the desktop shell. Rust core runs as the Tauri backend; `#[tauri::command]` functions wrap the `engine` crate. The frontend (React/Svelte/Solid) handles the chat UI, session sidebar, agent management, and settings. Chat UIs are trivially good in HTML/CSS — markdown rendering, syntax highlighting, streaming text are solved problems in the web ecosystem. Cross-platform (macOS/Linux/Windows), small binary (~5-10MB), and the same frontend could be reused for a web version later.

### Desktop `AgentUI` implementation

Implement a `DesktopUI` that satisfies the `AgentUI` trait: send messages to the GUI thread instead of stdout, show tool approval via a dialog instead of terminal y/N, render markdown in a rich text widget, show spinners/progress as native UI elements. The agent loop in core calls `ui.show_answer()`, `ui.confirm_tool()`, etc. — the desktop provides a different implementation.

### Tool approval UX

In the CLI it's a blocking y/N prompt. In the desktop, the agent loop should `await` a response from a channel that the UI resolves when the user clicks approve/deny in a dialog.

### Phased rollout

1. Stabilize the core API — channel-based event interface on top of the existing `engine` crate.
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
