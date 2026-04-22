# Roadmap

---

## General

### Developer Experience

- **Plugin / extension system** — Let users add custom tools via external scripts or WASM modules without forking the repo. See [.claude/plans/plugin-system.md](.claude/plans/plugin-system.md) for the development plan.
- **MCP server support** — Allow users to connect [Model Context Protocol](https://modelcontextprotocol.io) servers as a source of additional tools, resources, and prompts. Config entries under `~/.aictl/config` (or a dedicated `~/.aictl/mcp.json`) declare each server's transport (stdio command or HTTP/SSE URL), environment, and enablement. At startup, spawn/connect each configured server, list its tools/resources, namespace them (e.g. `mcp__<server>__<tool>`) and merge them into the built-in tool registry so the agent loop can dispatch them transparently. Tool calls route through the MCP client, results flow back like any other tool output, and the security policy (`security::validate_tool`) still applies — per-server allow/deny lists, CWD jailing for stdio servers, and redaction on outbound payloads. Add `/mcp` slash command to list, enable/disable, or restart servers, and `--list-mcp` / `--mcp-server <name>` CLI flags for scripted use. Remote catalogue (similar to `agents/remote.rs` and `skills/remote.rs`) can expose a curated set of official servers pullable on demand.
- **Inter-instance messaging** — When two or more `aictl` instances run on the same machine, let them exchange messages so one instance can send a prompt, note, or context snippet to another and vice-versa. Likely implementation: a local transport (Unix domain socket or named pipe under `~/.aictl/ipc/`) where each instance registers with its session id, a `/send <session-id> <message>` slash command to dispatch, and an inbox surfaced in the receiving REPL (either interrupting the prompt or queued until the next turn). Needs to decide whether incoming messages feed into the conversation automatically or require the user to accept them, and how this interacts with `--unrestricted` and the security policy.
- **`/undo` command** — Remove the last user message and all following assistant/tool responses from the in-memory conversation and the persisted session file, so the next turn runs as if the last exchange never happened. Useful for recovering from a bad prompt, an off-track tool loop, or a response that polluted context. Implementation: walk the messages vector backward to the most recent `role == "user"` entry, truncate from there, and rewrite the session JSONL. Consider supporting `/undo N` to pop multiple turns, and decide how it interacts with `/compact` (probably refuse to undo past a compaction boundary).
- **Model search in `/model`** — After invoking `/model`, present two entry points: **Browse** (the current paged provider → model picker) and **Search** (a type-ahead query over the full `MODELS` catalog). The search view takes a substring/fuzzy query against model id, provider, and aliases, renders the filtered list with the same formatting as browse, and lets the user pick one to activate. Implementation: extend `commands/model.rs` (or the matching handler) with a top-level menu step, reuse the existing `MODELS` iteration for both paths, and share the selection/confirmation code so switching models behaves identically regardless of entry point. Consider accepting an inline argument (`/model search <query>`) to skip straight to filtered results for scripted use.

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
