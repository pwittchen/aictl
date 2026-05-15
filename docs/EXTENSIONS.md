# Extensions

aictl ships with five extension points for tailoring the agent loop without forking the repo:

- **[Agents](#agents)** — reusable system-prompt extensions loaded for the whole session.
- **[Skills](#skills)** — one-turn markdown playbooks invoked on demand.
- **[Plugins](#plugins)** — user-installed external tool binaries (any language) merged into the tool catalogue.
- **[Hooks](#hooks)** — user-defined shell commands the harness runs at lifecycle events.
- **[MCP servers](#mcp-servers)** — connect [Model Context Protocol](https://modelcontextprotocol.io) servers and merge their tools into the agent loop.

## Agents

Agents are reusable system prompt extensions that specialize the LLM for dedicated tasks or behaviors. Agent prompts are stored as plain text files in `~/.aictl/agents/`.

Use `/agent` to open the agent menu:

- **Create agent manually** — enter a name and type or paste the agent prompt text directly
- **Create agent with AI** — provide a name and brief description; the LLM generates the full agent prompt
- **Browse official agents** — browse the live catalogue of curated agents shipped in the aictl repo (see "Official catalogue" below), preview them, and pull the ones you want to `~/.aictl/agents/`
- **View all agents** — browse saved agents, view their prompt, load an agent, or delete it
- **Unload agent** — remove the currently loaded agent (only shown when one is loaded)

Agents can also be loaded from the command line with `--agent <name>`, which works in both single-shot and interactive modes.

Agent names may contain only letters, numbers, underscores, and dashes. When an agent is loaded, its prompt is appended to the system prompt and the agent name appears in magenta brackets before the input prompt (e.g. `[my-agent] ❯`).

### Official catalogue

aictl ships with a curated set of first-party agents (e.g. `researcher`, `software-architect`, `critic`, `security-auditor`, `psychologist`) that live in the project's GitHub repo under [`.aictl/agents/`](../.aictl/agents/) — **not** bundled into the binary. New catalogue agents are available the moment they land on `master`, no release needed.

Pull agents from the catalogue in two ways:

- From the REPL, `/agent` → **Browse official agents**. Agents are grouped by category; each row shows `[ ]` (not pulled), `[✓]` (matches upstream), or `[↑]` (upstream differs). Press `v` to preview an agent's prompt before pulling, `p` / Enter to pull.
- From the shell, `aictl --pull-agent <name>` downloads a single agent. Add `--force` to overwrite an existing local file without prompting.

Catalogue agents carry `source: aictl-official` in their frontmatter; both `/agent` and `--list-agents` render an `[official]` badge so you can tell at a glance which agents came from the catalogue and which you wrote yourself. Users can edit or delete pulled agents freely — there is nothing special about them on disk. Public-repo reads are unauthenticated (≈60 requests/hour), which is plenty for browse-then-pull; errors are reported in the REPL without crashing the session.

## Skills

Skills are markdown playbooks invoked on demand for a **single turn** — unlike agents, which persist for the whole session. A skill encodes a repeatable procedure ("run the commit workflow", "review the pending diff") that the LLM should follow this one time; after the turn completes, the skill is gone. Skills live under `~/.aictl/skills/<name>/SKILL.md` (overridable via `AICTL_SKILLS_DIR`).

Each `SKILL.md` starts with YAML frontmatter (`name`, `description`) followed by the markdown body:

```markdown
---
name: commit
description: Commit staged changes with a clear, project-style message.
---

When the user asks you to commit:
1. Run `git status` and `git diff --cached` to see what's staged.
2. ...
```

Use `/skills` to open the skill menu:

- **Create skill manually** — enter a name and description, then type or paste the body
- **Create skill with AI** — provide a name and one-line description; the LLM drafts the body
- **Browse official skills** — browse the live catalogue of curated skills shipped in the aictl repo (see "Official catalogue" below), preview them, and pull the ones you want to `~/.aictl/skills/<name>/SKILL.md`
- **View all skills** — browse saved skills with view / invoke / delete actions

Invoke a skill directly by typing `/<skill-name>` at the REPL prompt. `/commit` runs the skill with a default trigger so the body alone drives the turn; `/commit review the staged diff` routes the trailing text as the user message. `--skill <name>` works the same way in single-shot and REPL modes. `--list-skills` prints saved skills and exits.

Skill names may contain only letters, numbers, underscores, and dashes and must not collide with a built-in slash command (e.g. `help`, `exit`, `agent`) — such names are rejected at save time. The skill body is merged into the base system prompt for the turn (rather than sent as a separate system message) so every provider, including those that accept only a single top-level `system` field, sees the skill alongside the tool catalog.

### Official catalogue

aictl ships with a curated set of first-party skills that live in the project's GitHub repo under [`.aictl/skills/`](../.aictl/skills/) — **not** bundled into the binary. New catalogue skills are available the moment they land on `master`, no release needed.

Pull skills from the catalogue in two ways:

- From the REPL, `/skills` → **Browse official skills**. Skills are grouped by category; each row shows `[ ]` (not pulled), `[✓]` (matches upstream), or `[↑]` (upstream differs). Press `v` to preview a skill's body before pulling, `p` / Enter to pull.
- From the shell, `aictl --pull-skill <name>` downloads a single skill. Add `--force` to overwrite an existing local file without prompting.

Catalogue skills carry `source: aictl-official` in their frontmatter; both `/skills` and `--list-skills` render an `[official]` badge so you can tell at a glance which skills came from the catalogue and which you wrote yourself. Users can edit or delete pulled skills freely — there is nothing special about them on disk.

## Plugins

Plugins are user-installed external tools that extend the agent without forking the repo. A plugin is a directory under `~/.aictl/plugins/<name>/` containing a `plugin.toml` manifest and an executable entrypoint (any language — shell script, Python, compiled binary, anything that reads stdin and writes stdout).

```
~/.aictl/plugins/
└── kubectl_query/
    ├── plugin.toml
    └── run            # executable; chmod +x
```

`plugin.toml`:

```toml
name = "kubectl_query"
version = "0.1.0"
description = "Query a Kubernetes cluster. Input: 'get|describe|logs <resource> [name]'."
entrypoint = "run"           # relative path inside the plugin dir; default "run"
requires_confirmation = true # keep true unless the plugin is purely read-only
timeout_secs = 30            # optional; falls back to AICTL_SECURITY_SHELL_TIMEOUT
schema_hint = """
First line: subcommand (get|describe|logs)
Second line: resource type
Third line (optional): resource name
"""
```

Wire protocol:

- **stdin** — the raw `<tool>…</tool>` body the LLM emitted, exactly as it would be passed to a built-in tool. No JSON framing.
- **stdout** — the result string returned to the LLM verbatim (after `<tool>` tag escaping).
- **exit code** — `0` for success; non-zero is reported back to the LLM as `[exit N] <stderr>`. Chatty stderr on success is suppressed.
- **environment** — same scrubbed env that `exec_shell` uses (secrets / `_KEY` / `_TOKEN` / `_PASSWORD` stripped).
- **working directory** — pinned to the security CWD jail.

Plugins are gated behind `AICTL_PLUGINS_ENABLED=true` (default `false`) — third-party code does not auto-load. Discovery happens once at startup; restart aictl to pick up new plugins. A malformed manifest, missing entrypoint, or symlink that escapes the plugin directory causes that single plugin to be skipped with a stderr warning, never a startup failure.

CLI surface:

- `aictl --list-plugins` — non-interactive listing (name, description, location).
- `/plugins` (REPL) — list manifests, view a plugin's `plugin.toml`, toggle the master switch, show the plugins directory.

The standard security gate (`security::validate_tool`) runs before dispatch, so `AICTL_SECURITY_DISABLED_TOOLS` can disable a plugin name exactly like a built-in tool, the confirmation prompt fires unchanged, and `--unrestricted` bypasses validation just as it does for built-ins. To silence one plugin without touching its manifest, add it to `AICTL_PLUGINS_DISABLED=foo,bar`.

A reference `echo_back` plugin lives at [`examples/plugins/echo_back/`](../examples/plugins/echo_back/) — copy the directory to `~/.aictl/plugins/echo_back/` and set `AICTL_PLUGINS_ENABLED=true` to try it.

## Hooks

Hooks are user-defined shell commands the harness runs at lifecycle events. Use them for harness-level automation that does not belong in an agent prompt — running `cargo fmt` after every edit, blocking specific shell commands, snapshotting the transcript before compaction, or mirroring desktop notifications to a webhook.

Hooks live in `~/.aictl/hooks.json` (override the path with `AICTL_HOOKS_FILE`):

```json
{
  "PreToolUse": [
    { "matcher": "exec_shell", "command": "echo seen", "timeout": 30 }
  ],
  "PostToolUse": [
    { "matcher": "edit_file|write_file", "command": "cargo fmt --message-format short 2>&1 | head -c 2000" }
  ],
  "Stop": [
    { "matcher": "*", "command": "date '+turn ended at %H:%M:%S' >> /tmp/aictl-hook.log" }
  ]
}
```

Each hook is `{ matcher, command, timeout, enabled }`. `matcher` is a glob over the tool name (`exec_shell`, `read_*`, `edit_file|write_file`, `mcp__*__*`) for tool events, or `*` for non-tool events. `command` runs via `sh -c` in the security working directory with a scrubbed env. `timeout` defaults to 60 seconds; `enabled` defaults to `true`.

Supported events:

| Event | Fires |
|-------|-------|
| `SessionStart` | REPL boots; single-shot run starts |
| `SessionEnd` | REPL exits; single-shot run finishes |
| `UserPromptSubmit` | After Enter, before the injection guard. Can rewrite or block the prompt |
| `PreToolUse` | Before a tool runs (and before user y/N confirm). Can deny or pre-approve |
| `PostToolUse` | After the tool result joins history. Can append `additionalContext` for the next turn |
| `Stop` | After the agent's final answer (no tool call) |
| `PreCompact` | Before `/compact` summarizes the conversation |
| `Notification` | Inside the `notify` tool, before the OS pop. Can suppress noisy alerts |

Each hook receives a JSON payload on stdin (`event`, `session_id`, `cwd`, plus `tool` / `prompt` / `notification` / `trigger` depending on the event) and may return JSON on stdout to influence the harness:

| Stdout | Effect |
|--------|--------|
| empty | Continue silently |
| `{"decision":"block","reason":"..."}` | Abort the action; reason is surfaced to the LLM |
| `{"decision":"approve","reason":"..."}` | Pre-approve a tool call — skip the user's y/N prompt |
| `{"additionalContext":"..."}` | Inject a `<hook_context>` user turn into history before the next LLM call |
| `{"rewrittenPrompt":"..."}` | `UserPromptSubmit` only — replace the user's text before the agent sees it |
| plain text | Treated as `additionalContext` |

Exit code `2` is shorthand for `{"decision":"block","reason":"<stderr>"}`. Failures (spawn error, timeout, non-2 nonzero exit) are logged to stderr and treated as "continue" so a broken hook can't wedge the agent loop.

Hooks are *harness* behavior, not LLM behavior — `--unrestricted` does not bypass them. Automated rules like "always run `cargo fmt` after `edit_file`" belong here, not in agent prompts or memory.

CLI surface:

- `aictl --list-hooks` — non-interactive listing (event, matcher, command, status).
- `/hooks` (REPL) — view all hooks grouped by event, toggle individual entries, test-fire a hook with a synthetic payload, or reload the file from disk.

A reference `hooks.json` with one example per event (all `enabled: false` so they don't fire until you flip them on) lives at [`examples/hooks.json`](../examples/hooks.json).

## MCP servers

aictl can connect to [Model Context Protocol](https://modelcontextprotocol.io) servers and merge their tools into the agent loop alongside built-ins and plugins. This unlocks the existing MCP ecosystem — filesystem, git, GitHub, Postgres, Slack, and dozens of others — without aictl having to integrate each one individually. Three transports are supported — **stdio** (spawn a local process), **http** (modern Streamable HTTP), and **sse** (legacy HTTP+SSE) — and the **tools** capability is wired up; resources and prompts are still on the roadmap.

Servers are declared in `~/.aictl/mcp.json` (override the path with `AICTL_MCP_CONFIG`) in a shape compatible with Claude Desktop:

```json
{
  "mcpServers": {
    "filesystem": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-filesystem", "/Users/me/Documents"],
      "enabled": true,
      "timeout_secs": 30
    },
    "github": {
      "command": "docker",
      "args": ["run", "--rm", "-i", "ghcr.io/github/github-mcp-server"],
      "env": { "GITHUB_PERSONAL_ACCESS_TOKEN": "${keyring:GITHUB_TOKEN}" }
    },
    "remote": {
      "transport": "http",
      "url": "https://mcp.example.com/v1",
      "headers": { "Authorization": "${keyring:MCP_REMOTE_TOKEN}" },
      "enabled": true,
      "timeout_secs": 30
    }
  }
}
```

Per-entry fields:

- **stdio (default)** — `command` + `args` (resolved via `PATH`, no shell), optional `env`, `enabled`, `timeout_secs`.
- **http / sse** — set `transport: "http"` (Streamable HTTP) or `"sse"` (HTTP+SSE) and supply `url`, optional `headers`, `enabled`, `timeout_secs`.

Both `env` (stdio) and `headers` (remote) values may use `${keyring:NAME}` to pull a secret from the system keyring instead of checking it into the file. The whole subsystem is gated behind `AICTL_MCP_ENABLED=true` (default `false`) — third-party server processes do not auto-spawn. Remote URLs are validated by a hostname allow/deny gate (`AICTL_MCP_ALLOW_HOSTS`, `AICTL_MCP_DENY_HOSTS`) and require HTTPS unless `AICTL_MCP_ALLOW_HTTP=true`.

At startup, every enabled server is spawned in parallel, the JSON-RPC `initialize` handshake completes, and the server's `tools/list` response is merged into the agent loop's catalogue. Each tool is reachable as `mcp__<server>__<tool>` and the model invokes it like any built-in:

```xml
<tool name="mcp__filesystem__read_file">
{"path": "/Users/me/Documents/notes.md"}
</tool>
```

The body is a JSON object that matches the tool's input schema (the schema is appended to the system prompt so the model formats calls correctly). Failed servers are recorded in `ServerState::Failed` and never abort startup — a single broken entry can't take down the rest of the catalogue.

Security model:

- Every MCP call passes through the same `security::validate_tool` gate as built-ins. `AICTL_SECURITY_DISABLED_TOOLS` accepts qualified MCP names (`mcp__github__create_issue`).
- `AICTL_MCP_DENY_SERVERS=github,slack` blocks every tool from listed servers, even when the master switch is on.
- Outbound redaction runs on the entire message stream regardless of transport, so detected secrets never reach the server.
- The CWD jail does **not** apply — MCP servers run in their own process with their own privileges. Users who want strict isolation should keep `AICTL_MCP_ENABLED=false` or curate the server list aggressively.

CLI / REPL surface:

- `aictl --list-mcp` — non-interactive listing (server name, state, tool count, command).
- `aictl --mcp-server <name>` — restrict this session to only the named server (every other configured server is force-disabled for the process; not persisted).
- `/mcp` (REPL) — list servers, browse per-server tool catalogue with input schemas, toggle the master switch, show the config path.
- `/info` and the welcome banner show MCP server / tool counts when enabled.

A bundled `tiny_add` smoke-test server (Python, ~70 lines, exposes one `add` tool) lives at [`examples/mcp/tiny_add/server.py`](../examples/mcp/tiny_add/server.py) and a fully-annotated example config at [`examples/mcp.json`](../examples/mcp.json).
