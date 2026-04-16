## Roadmap

### Modular architecture

Split the codebase into separate modules: `core` (shared logic), `cli`, `desktop` (currently empty) to enable independent development of each target.

### Desktop

Create a desktop app with the same capabilities as the CLI. macOS support is required; other platforms are a stretch goal.

### Coding Agent

Provide configurable mode, which will transofrm the general purpose agent into the coding agent. There should be additional skills/tools and prompts available for such mode, which won't be available in the "default" general purpose mode. Coding agent should work only in CLI app and be unavailable for server and desktop.

### New tools

#### Code & Development

- **run_code** — Execute a code snippet in a sandboxed interpreter (Python, Node, etc.) and return stdout/stderr. Useful for quick calculations, data transforms, or testing logic without writing files.
- **git** — A dedicated git tool (status, diff, log, blame, commit) so the LLM can inspect and manipulate repos without raw shell access, with tighter security controls than `exec_shell`.
- **lint_file** — Run a language-appropriate linter/formatter on a file and return diagnostics. Avoids the agent needing to know which linter to invoke.

#### Data & Transformation

- **json_query** — Query/transform JSON data with jq-like expressions. Saves the agent from writing ad-hoc scripts for common data wrangling.
- **csv_query** — Read and filter CSV/TSV files with SQL-like expressions, return results as a table.
- **calculate** — Evaluate mathematical expressions safely (no eval). Avoids the agent shelling out for arithmetic.

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
- **open_url** — Open a URL in the user's default browser.

### UX & Interactivity

- **`/history` command** — View and search the current conversation without scrolling. Support filtering by role or keyword.
- **`/undo` command** — Remove the last user/assistant exchange and retry. Useful when a response goes off track.
- **Resumable model downloads** — Use HTTP range requests so interrupted GGUF/MLX pulls resume instead of restarting from zero.
- **`/model` show current selection** — The model picker should highlight which model is currently active.
- **Auto-compaction confirmation** — Currently silent at 80% threshold. A brief notice or opt-in preview would reduce surprise.
- **Streaming output** — Stream LLM responses token-by-token instead of waiting for the full response. Significantly improves perceived latency.

### Provider & Model

- **Multi-modal audio/voice input** — Accept audio files or microphone input, transcribe via Whisper/Gemini, and feed into the conversation.
- **Provider health check** — A `/ping` or `/provider status` command that validates API keys and tests connectivity for all configured providers.
- **Automatic model fallback** — If the primary model returns a rate-limit or outage error, optionally fall back to a configured secondary.

### Agent & Workflow

- **Agent chaining / pipelines** — Run multiple agents in sequence where each agent's output feeds the next (e.g., research agent → summarize agent → write agent).
- **Agent templates** — Ship built-in agents (code reviewer, technical writer, shell expert) as starting points users can customize.
- **Scheduled / cron tasks** — Run a prompt on a schedule (e.g., "summarize my git log every morning"). Could use OS-level cron under the hood.
- **Multi-turn tool approval batching** — In non-auto mode, let the user approve multiple pending tool calls at once instead of one-by-one.

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
