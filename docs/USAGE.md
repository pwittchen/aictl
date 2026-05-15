# Usage

CLI flag and REPL command reference for the `aictl` binary. For installation see [INSTALL.md](INSTALL.md); for configuration see [CONFIG.md](CONFIG.md).

## Command line

```bash
aictl [--version] [--update] [--uninstall] [--config] [--provider <PROVIDER>] [--model <MODEL>] [--message <MESSAGE>] [--format <FORMAT>] [--auto] [--quiet] [--audit-file <PATH>] [--cwd <PATH>] [--unrestricted] [--incognito] [--agent <NAME>] [--list-agents] [--pull-agent <NAME>] [--skill <NAME>] [--list-skills] [--pull-skill <NAME>] [--list-memories] [--remember <FACT>] [--category <NAME>] [--force] [--session <ID|NAME>] [--list-sessions] [--clear-sessions] [--lock-keys] [--unlock-keys] [--clear-keys] [--pull-gguf-model <SPEC>] [--list-gguf-models] [--remove-gguf-model <NAME>] [--clear-gguf-models] [--pull-mlx-model <SPEC>] [--list-mlx-models] [--remove-mlx-model <NAME>] [--clear-mlx-models] [--pull-ner-model <SPEC>] [--list-ner-models] [--remove-ner-model <NAME>] [--clear-ner-models] [--balance] [--list-balances] [--list-plugins] [--list-hooks] [--list-mcp] [--mcp-server <NAME>] [--client-url <URL>] [--client-master-key <KEY>] [--serve]
```

Omit `--message` to enter interactive REPL mode with persistent conversation history.

## Parameters

Only `--version` (`-v`) and `--help` (`-h`) have short flags. All other options use long form only, by convention.

| Flag | Description |
|------|-------------|
| `--version`, `-v` | Print version information |
| `--help`, `-h` | Print help |
| `--update` | Update to the latest version |
| `--uninstall` | Remove the aictl binary from `~/.cargo/bin/aictl`, `~/.local/bin/aictl`, and `$AICTL_INSTALL_DIR/aictl` (if set) and exit. Leaves `~/.aictl/` untouched |
| `--config` | Interactive configuration wizard — set provider, model, and API keys step by step |
| `--provider` | LLM provider (`openai`, `anthropic`, `gemini`, `grok`, `mistral`, `deepseek`, `kimi`, `zai`, `ollama`, `gguf`, `mlx`, or `aictl-server`). Falls back to `AICTL_PROVIDER` in `~/.aictl/config` |
| `--model` | Model name (e.g. `gpt-4o`). Falls back to `AICTL_MODEL` in `~/.aictl/config` |
| `--message` | Message to send (omit for interactive mode) |
| `--agent` | Load a saved agent by name (works in both single-shot and interactive modes) |
| `--list-agents` | Print saved agents from `~/.aictl/agents/` and exit. Combine with `--category <name>` to filter |
| `--pull-agent` | Download an official agent from the aictl repo into `~/.aictl/agents/`. Combine with `--force` to skip the overwrite prompt |
| `--skill` | Invoke a saved skill by name for a single turn. In single-shot mode the skill body is injected as a transient system prompt for the `--message` call only; in REPL mode it applies to the first user turn, then the REPL reverts to normal |
| `--list-skills` | Print saved skills from `~/.aictl/skills/` and exit. Combine with `--category <name>` to filter |
| `--pull-skill` | Download an official skill from the aictl repo into `~/.aictl/skills/<name>/SKILL.md`. Combine with `--force` to skip the overwrite prompt |
| `--list-memories` | Print saved long-term memories from `~/.aictl/memory.json` and exit |
| `--remember` | Append a fact to long-term memory (`~/.aictl/memory.json`) and exit. No-op when memory is disabled or running in incognito mode |
| `--auto` | Run in autonomous mode (skip tool confirmation prompts) |
| `--quiet` | Suppress tool calls and reasoning, only print the final answer (requires `--auto`) |
| `--format` | Output format for single-shot (`--message`) mode: `md` (default — raw markdown source from the LLM, with streaming when stdout is a TTY), `text` (markdown stripped to plain prose), or `json` (one-line `{"answer", "model", "provider"}` envelope on stdout; reasoning/tool chatter and streaming suppressed). Ignored in interactive REPL |
| `--audit-file` | Write the per-line JSON audit log to an explicit path. Intended for single-shot (`--message`) runs, which otherwise have no session id to key the default `~/.aictl/audit/<session-id>` file by. Force-enables audit logging even when `AICTL_SECURITY_AUDIT_LOG=false`. Parent directories are created on demand |
| `--cwd` | Working directory for this run. The CLI changes into this path before any tool dispatch and uses it as the CWD jail root, so file/shell tools resolve relative paths here and are restricted to this subtree. Accepts absolute, relative, and `~`-prefixed paths. Falls back to `AICTL_WORKING_DIR_CLI` (canonical) or `AICTL_WORKING_DIR` (legacy) in `~/.aictl/config`; when none are set, the launch directory is used |
| `--unrestricted` | Disable all security restrictions (use with caution) |
| `--incognito` | Start interactive REPL without saving any session (disables `/session`). Falls back to `AICTL_INCOGNITO` in `~/.aictl/config` |
| `--session` | Load a saved session by uuid or name on startup (interactive mode only) |
| `--list-sessions` | Print saved sessions from `~/.aictl/sessions/` and exit |
| `--clear-sessions` | Remove all saved sessions and exit |
| `--lock-keys` | Migrate plain-text API keys from `~/.aictl/config` into the system keyring and exit |
| `--unlock-keys` | Migrate API keys from the system keyring back into `~/.aictl/config` and exit |
| `--clear-keys` | Remove API keys from both `~/.aictl/config` and the system keyring and exit |
| `--pull-gguf-model` | Download a native GGUF model (spec: `hf:owner/repo/file.gguf`, `owner/repo:file.gguf`, or `https://…/file.gguf`). Saved under `~/.aictl/models/gguf/` and exits |
| `--list-gguf-models` | Print all downloaded native GGUF models and exit |
| `--remove-gguf-model` | Remove a downloaded native GGUF model by name and exit |
| `--clear-gguf-models` | Remove every downloaded native GGUF model and exit |
| `--pull-mlx-model` | Download a native MLX model (spec: `mlx:owner/repo` or `owner/repo`). Saved under `~/.aictl/models/mlx/<name>/` and exits |
| `--list-mlx-models` | Print all downloaded native MLX models and exit |
| `--remove-mlx-model` | Remove a downloaded native MLX model by name and exit |
| `--clear-mlx-models` | Remove every downloaded native MLX model and exit |
| `--pull-ner-model` | Download a redaction NER model (spec: `owner/repo` or `hf:owner/repo`; default shape: `onnx-community/gliner_small-v2.1`). Saved under `~/.aictl/models/ner/<name>/` and exits. Inference requires the `redaction-ner` cargo feature; management works on every build |
| `--list-ner-models` | Print all downloaded NER models and exit |
| `--remove-ner-model` | Remove a downloaded NER model by name and exit |
| `--clear-ner-models` | Remove every downloaded NER model and exit |
| `--balance` / `--list-balances` | Show remaining credit / quota for each configured cloud provider and exit. Real numbers from DeepSeek and Kimi (via their official `/user/balance` and `/v1/users/me/balance` endpoints); other providers report "unknown" with a hint pointing at their billing dashboard. Local providers (Ollama / GGUF / MLX) are out of scope |
| `--list-plugins` | Print installed plugins (name, description, location) and exit. Reads from `~/.aictl/plugins/` (override via `AICTL_PLUGINS_DIR`). When `AICTL_PLUGINS_ENABLED=false` the listing is empty with a hint about the master switch |
| `--list-hooks` | Print configured hooks (event, matcher, command, status) and exit. Reads from `~/.aictl/hooks.json` (override via `AICTL_HOOKS_FILE`) |
| `--list-mcp` | Print configured MCP servers (name, state, tool count) and exit. Reads from `~/.aictl/mcp.json` (override via `AICTL_MCP_CONFIG`). When `AICTL_MCP_ENABLED=false` the listing is empty with a hint about the master switch |
| `--mcp-server` | Restrict this session to only the named MCP server (every other configured server is force-disabled for the process). Effective only when `AICTL_MCP_ENABLED=true` |
| `--client-url` | Route every non-local LLM call through this `aictl-server` URL for this invocation. Overrides `AICTL_CLIENT_HOST`. Empty string (`""`) disables routing for this run even if `AICTL_CLIENT_HOST` is set. Not persisted |
| `--client-master-key` | Master key the CLI presents to the configured `aictl-server` for this invocation. Overrides `AICTL_CLIENT_MASTER_KEY` from config or the keyring. Not persisted (visible in shell history and `ps` — the persistent path is `/keys` or `--config`) |
| `--serve` | Launch the bundled `aictl-server` HTTP LLM proxy if it's installed. Convenience shortcut from the CLI; trailing args after `--` are forwarded verbatim, e.g. `aictl --serve -- --bind 0.0.0.0:7878 --quiet`. See [SERVER.md](SERVER.md) |

CLI flags take priority over config file values.

## REPL commands

The interactive REPL supports slash commands:

| Command | Description |
|---------|-------------|
| `/agent` | Manage agents (create manually, create with AI, view/load/delete, unload) |
| `/clear` | Clear conversation context |
| `/compact` | Summarize conversation into a compact context |
| `/retry` | Remove the last user/assistant exchange and retry with the same prompt (useful when a response goes off track) |
| `/undo` | Drop the last N turns from the conversation without re-running (`/undo` = 1, `/undo 3` = 3); refuses to cross a `/compact` boundary |
| `/context` | Show context usage (token and message counts vs limits) |
| `/copy` | Copy last response to clipboard |
| `/help` | Show available commands |
| `/history` | View the in-memory conversation; optional role or keyword filter (e.g. `/history user rust`) |
| `/info` | Show setup info (provider, model, behavior, agent, version, OS, binary size) |
| `/roadmap` | Fetch and render the project roadmap; optional section filter (e.g. `/roadmap desktop`) |
| `/gguf` | Manage native GGUF models (view downloaded, pull, remove, clear all) |
| `/mlx` | Manage native MLX models (Apple Silicon; view downloaded, pull, remove, clear all) |
| `/security` | Show current security policy (blocked commands, CWD jail, timeouts, etc.) |
| `/session` | Manage sessions (show current info, set name, view/load/delete saved, clear all) |
| `/skills` | Manage skills (create manually, create with AI, view/invoke/delete) — one-turn markdown playbooks |
| `/stats` | Manage usage statistics — view today/month/overall (sessions, calls, tokens, estimated cost) or clear all |
| `/behavior` | Switch between auto and human-in-the-loop mode during the session |
| `/model` | Switch model and provider during the session (persists to `~/.aictl/config`) |
| `/ping` | Validate every configured API key and probe provider connectivity (cloud providers + Ollama daemon) |
| `/plugins` | Manage external plugin tools — list installed plugins, view a manifest, toggle the master switch (`AICTL_PLUGINS_ENABLED`) |
| `/hooks` | Manage lifecycle hooks — view all configured hooks per event, toggle individual entries on/off, test-fire a hook with a synthetic payload, or reload `~/.aictl/hooks.json` |
| `/mcp` | Manage external MCP (Model Context Protocol) servers — list configured servers, view tool catalogues with input schemas, toggle the master switch (`AICTL_MCP_ENABLED`) |
| `/memory` | Manage long-term memory — toggle on/off, browse saved facts, delete one, or clear all. Disabled in incognito mode |
| `/remember` | Save a fact to long-term memory: `/remember <fact>`. The fact is loaded into the system prompt of every future conversation. No-op in incognito mode |
| `/balance` | Show remaining credit / quota for each configured cloud provider (real numbers from DeepSeek and Kimi; "unknown" with a billing-dashboard hint elsewhere) |
| `/tools` | Show available tools |
| `/keys` | Manage API key storage — lock (config → keyring), unlock (keyring → config), or clear (both stores) |
| `/config` | Re-run the interactive configuration wizard |
| `/update` | Update to the latest version |
| `/uninstall` | Remove the aictl binary from `~/.cargo/bin/` and `~/.local/bin/` (asks for confirmation) |
| `/version` | Check current version against the latest available |
| `/exit` | Exit the REPL |

Any unrecognized `/<name>` that matches a saved skill (see [EXTENSIONS.md → Skills](EXTENSIONS.md#skills)) runs that skill for the next turn: `/<skill-name>` runs it with a default trigger, `/<skill-name> <task>` routes `<task>` as the user message.

Press **Esc** during any LLM call or tool execution to interrupt the operation and return to the prompt. Conversation history is rolled back so the interrupted turn has no effect.

## Sessions

In interactive mode, each REPL run is a session. A new uuid is generated at startup and the conversation is persisted to `~/.aictl/sessions/<uuid>` as JSON after every agent turn and compaction. Session names (optional, unique) are stored in `~/.aictl/sessions/.names`. On exit, the session uuid (and name, if set) is printed.

Use `/session` to show current session info, assign a readable name, browse saved sessions (load or delete with confirmation), or clear all sessions. Pass `--session <uuid|name>` to resume an existing session on startup. Incognito mode (`--incognito` or `AICTL_INCOGNITO=true`) runs the REPL without creating or saving any session file; `/session` is disabled and displays a notice.

## Examples

```bash
# With defaults configured in ~/.aictl/config, just run:
aictl

# Or send a single message:
aictl --message "What is Rust?"

# Override provider/model from the command line:
aictl --provider openai --model gpt-4o --message "What is Rust?"

# Agent with tool calls (interactive confirmation)
aictl --message "List files in the current directory"

# Autonomous mode (no confirmation prompts)
aictl --auto --message "What OS am I running?"

# Quiet mode (only final answer, no tool calls or reasoning)
aictl --auto --quiet --message "What OS am I running?"

# JSON envelope on stdout (for scripting; tool/reasoning chatter dropped)
aictl --format json --message "What is Rust?"

# Plain prose with markdown stripped
aictl --format text --message "Explain Rust ownership"
```
