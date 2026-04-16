# Roadmap

---

## General

### New tools

#### System & Environment

- **list_processes** — List running processes with filtering (by name, port, resource usage). Safer and more structured than `ps aux | grep`.
- **network_request** — A more general HTTP client tool (GET/POST/PUT/DELETE with headers, body, auth) beyond the current fetch-and-extract tools. Useful for API debugging.
- **check_port** — Test if a host:port is reachable. Handy for debugging connectivity issues.
- **system_info** — Return OS, arch, memory, disk, CPU info in a structured way.

#### File & Content

- **diff_files** — Compare two files and return a unified diff. Useful before edits or for understanding changes.
- **archive** — Create or extract tar.gz/zip archives. Common enough to warrant a dedicated tool over shell commands.
- **checksum** — Compute SHA-256/MD5 of a file. Useful for verifying downloads or file integrity.

#### Productivity

- **clipboard** — Read from or write to the system clipboard. The agent could stage results for the user without writing files.
- **notify** — Send a desktop notification (useful for long-running tasks in `--auto` mode).

### UX & Interactivity

- **`/history` command** — View and search the current conversation without scrolling. Support filtering by role or keyword.
- **`/undo` command** — Remove the last user/assistant exchange and retry. Useful when a response goes off track.
- **Resumable model downloads** — Use HTTP range requests so interrupted GGUF/MLX pulls resume instead of restarting from zero.
- **`/model` show current selection** — The model picker should highlight which model is currently active.
- **`/model` update UI** - Right now there's a lot of models - consider making UI horizontal when terminal window is wider
- **`/model` search** - Add model search capability
- **Auto-compaction confirmation** — Currently silent at 80% threshold. A brief notice or opt-in preview would reduce surprise.
- **Streaming output** — Stream LLM responses token-by-token instead of waiting for the full response. Significantly improves perceived latency.
- **AICTL.md entry file** - create fallback, so when there's no AICTL.md/configured file, then fallback to `CLAUDE.md` and then to `AGENTS.md`

### Provider & Model

- **Provider health check** — A `/ping` or `/provider status` command that validates API keys and tests connectivity for all configured providers.
- **Automatic model fallback** — If the primary model returns a rate-limit or outage error, optionally fall back to a configured secondary.

### Agent & Workflow

- **Agent chaining / pipelines** — Run multiple agents in sequence where each agent's output feeds the next (e.g., research agent → summarize agent → write agent).
- **Agent templates** — Ship built-in agents (code reviewer, technical writer, shell expert) as starting points users can customize.

### Developer Experience

- **Integration tests with a mock LLM** — End-to-end tests exercising the full agent loop with a mock provider.
- **Unit tests for `agents.rs`, `session.rs`, `keys.rs`** — These critical modules currently have zero test coverage.
- **Config schema / example file** — Ship a `.aictl/config.example` so users know what keys exist without reading documentation.
- **Plugin / extension system** — Let users add custom tools via external scripts or WASM modules without forking the repo.
- **Per-tool output size limits** — Replace the global 10K char truncation with per-tool configuration.

### Security & Reliability

- **Symlink-aware path validation** — Add regression tests for path traversal via symlinks to harden the CWD jail.
- **Per-tool execution timeouts** — Different tools have different expected runtimes; allow per-tool timeout configuration instead of a single global 30s timeout.
- **Audit log** — Optionally log all tool executions (command, args, result summary) to a file for post-hoc review, separate from session history.

### Platform & Distribution

- **Homebrew formula** — `brew install aictl` to lower the installation barrier on macOS.
- **Shell completions** — Generate bash/zsh/fish completions from the clap definitions and ship them.
- **Man page** — Auto-generate from clap's help text for `man aictl`.

---

## Desktop

Create a desktop app with the same capabilities as the CLI. macOS support is required; other platforms are a stretch goal.

### Modular architecture

Split the codebase into separate modules: `core` (shared logic), `cli`, `desktop` (currently empty) to enable independent development of each target.

### Cargo workspace split

Split into `aictl-core` (library — agents, config, keys, LLM providers, security, sessions, stats, tools), `aictl-cli` (binary — clap, REPL, `PlainUI`, `InteractiveUI`), and `aictl-desktop` (binary — GUI frontend). The `AgentUI` trait stays in core as the abstraction boundary. The hard part is removing scattered `println!`/`eprintln!` calls from core logic and routing them through the trait.

### Core API stabilization

Define clean public types for the desktop to consume: `AgentLoop`, `Conversation`, `ToolCallEvent`, `StreamChunk`, etc. Add a channel-based interface (`tokio::mpsc`) so the desktop can receive events asynchronously — streaming tokens, tool approval requests, progress updates.

### GUI framework: Tauri v2

Use Tauri v2 for the desktop shell. Rust core runs as the Tauri backend; `#[tauri::command]` functions wrap `aictl-core`. The frontend (React/Svelte/Solid) handles the chat UI, session sidebar, agent management, and settings. Chat UIs are trivially good in HTML/CSS — markdown rendering, syntax highlighting, streaming text are solved problems in the web ecosystem. Cross-platform (macOS/Linux/Windows), small binary (~5-10MB), and the same frontend could be reused for a web version later.

### Desktop `AgentUI` implementation

Implement a `DesktopUI` that satisfies the `AgentUI` trait: send messages to the GUI thread instead of stdout, show tool approval via a dialog instead of terminal y/N, render markdown in a rich text widget, show spinners/progress as native UI elements. The agent loop in core calls `ui.show_answer()`, `ui.confirm_tool()`, etc. — the desktop provides a different implementation.

### Tool approval UX

In the CLI it's a blocking y/N prompt. In the desktop, the agent loop should `await` a response from a channel that the UI resolves when the user clicks approve/deny in a dialog.

### Phased rollout

1. Extract `aictl-core` — pure refactoring, CLI stays working.
2. Stabilize the core API — channel-based event interface.
3. Scaffold the desktop app — Tauri + minimal frontend, send a message and see the response.
4. Feature parity incrementally — sessions, agents, tool approval dialogs, settings, stats, one at a time.

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


