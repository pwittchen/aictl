---
name: add-mcp-server
description: Connect a Model Context Protocol server to aictl by adding an entry to ~/.aictl/mcp.json. Walks the user through command, args, env, and timeout, then merges the new server into the existing config without disturbing other entries.
allowed-tools: Bash, Read, Edit, Write
---

## Purpose

Help the user wire a new MCP (Model Context Protocol) server into their personal aictl config at `~/.aictl/mcp.json`. The aictl harness spawns each enabled stdio server at startup, completes the JSON-RPC `initialize` handshake, calls `tools/list`, and exposes every discovered tool to the agent under the name `mcp__<server>__<tool>`. This skill captures intent, drafts a safe entry, and edits the file in place — preserving every existing server.

Source of truth for MCP semantics: `crates/aictl-core/src/mcp/config.rs` (parser) and `crates/aictl-core/src/mcp.rs` (lifecycle). Reference example: `examples/mcp.json`. When in doubt, read those before answering.

## Prerequisite: opt in to MCP

The whole subsystem is gated behind `AICTL_MCP_ENABLED=true` (default off) — third-party processes do not auto-spawn. Before doing anything else, check that the user has opted in:

```sh
grep -E '^[[:space:]]*AICTL_MCP_ENABLED' ~/.aictl/config 2>/dev/null
```

If the value isn't `true`, tell the user they need to set it (`AICTL_MCP_ENABLED=true` in `~/.aictl/config`) for the server to actually load. Don't flip the flag for them — opt-in is theirs to give.

## Inputs to gather from the user

Before editing the file, get clear answers:

1. **Server name** — short, alphanumeric/underscore/dash only (e.g. `filesystem`, `github`, `tiny_add`). Becomes the prefix in `mcp__<name>__<tool>`.
2. **Command** — the executable to spawn. Resolved via `PATH` (no shell). Examples: `python3`, `npx`, `docker`, an absolute path.
3. **Args** — the argv list for the command. Use absolute paths for any local script. Keep filesystem scopes tight (don't grant `/`).
4. **Env** — optional environment variables. For secrets, use `${keyring:NAME}` so the literal token never lands in the file.
5. **Enabled** — usually `true`. Set `false` if the user wants the server staged but inert.
6. **Timeout** — per-call RPC timeout in seconds. Omit to inherit the default (`AICTL_MCP_TIMEOUT` or 30s). Raise it for slow servers.

If the user is wiring a known server (filesystem, github, etc.), prefer the canonical shape from `examples/mcp.json` over inventing a new one.

## Schema reference

Each entry under `mcpServers` is:

```json
{
  "command": "<exe-on-PATH-or-abs-path>",
  "args": ["..."],
  "env": { "KEY": "value-or-${keyring:NAME}" },
  "enabled": true,
  "timeout_secs": 30
}
```

Rules enforced by the parser:

- The server name must match `[A-Za-z0-9_-]+`. Bad names abort startup.
- `command` is required and non-empty. Spawned directly with `tokio::process` — no shell expansion, no globs.
- `args`, `env`, `enabled`, `timeout_secs` are all optional. Defaults: `[]`, `{}`, `true`, default RPC timeout.
- `${keyring:NAME}` inside any `env` value pulls a secret from `keys::get_secret(NAME)` (system keyring, falling back to plain config). Missing secrets are left as literal `${keyring:NAME}` text so the failure is loud at handshake time.
- Top-level `_comment` keys (and any unknown per-entry keys) are silently ignored — safe to use for inline notes.

## Workflow

### 1. Confirm the file path

The default is `~/.aictl/mcp.json`. If `AICTL_MCP_CONFIG` is set in `~/.aictl/config`, use that instead:

```sh
grep -E '^[[:space:]]*AICTL_MCP_CONFIG' ~/.aictl/config 2>/dev/null
```

### 2. Read the current file

```sh
cat ~/.aictl/mcp.json 2>/dev/null
```

If the file is missing or empty, treat the starting state as `{ "mcpServers": {} }`. If JSON parsing fails, stop and ask the user — do not overwrite a malformed file blindly. If a server with the chosen name already exists, confirm with the user before replacing.

### 3. Draft the new entry

Build a single JSON object under `mcpServers.<name>`:

```json
{
  "command": "<exe>",
  "args": ["..."],
  "enabled": true,
  "timeout_secs": 30
}
```

Drop fields that aren't needed. Add `env` only when the server requires it. Use `${keyring:NAME}` for any secret — and remind the user to store the secret first (`/keys` in the REPL, or have them run `aictl` and use the menu).

### 4. Merge into the existing file

The config maps `mcpServers.<name>` to a single object per server. Add the new entry; create the `mcpServers` object if missing; keep every other server untouched.

Use `Read` to load the current file, then `Edit` or `Write` to produce the merged result. Pretty-print with 2-space indent so diffs stay readable.

If `jq` is available and the file is large, this is the safest one-liner:

```sh
jq --argjson entry '<NEW_ENTRY>' '.mcpServers["<NAME>"] = $entry' ~/.aictl/mcp.json > /tmp/mcp.json && mv /tmp/mcp.json ~/.aictl/mcp.json
```

Do **not** clobber the file with `Write` unless you've first read its full content and reconstructed every existing entry.

### 5. Validate

After saving, run:

```sh
aictl --list-mcp
```

Confirm the new server appears in the catalogue. If MCP is not opted in (`AICTL_MCP_ENABLED` not set to `true`), the catalogue will be empty — say so and remind the user. If the file failed to parse, aictl prints a parse error to stderr — re-read and fix.

For a deeper check, the user can boot the REPL and run `/mcp` to see live server state (Connected / Failed / Disabled) and the discovered tool list.

### 6. Suggest a smoke test

For first-time users, point them at the bundled smoke server:

- `examples/mcp/tiny_add/server.py` — Python stdio server exposing one tool (`mcp__tiny_add__add`).
- `examples/mcp.json` — copy-paste-ready entry. Update the absolute path.

Once wired, ask the agent to "use the tiny_add tool to add 2 and 3". A working MCP path returns `5`.

## Rules

- Always read `~/.aictl/mcp.json` before writing. Preserve every existing entry.
- Never put a literal API token, password, or session cookie in `env`. Use `${keyring:NAME}` and store the secret separately.
- Use absolute paths in `args` for local scripts. Relative paths break the moment the user runs aictl from a different directory.
- Keep filesystem-server scopes tight (`/tmp`, a project root, etc.) — don't grant `/` or `$HOME` unless the user explicitly asks.
- Default timeout (omit `timeout_secs`) is fine for most servers. Raise it for ones that do heavy IO; lower it for ones the user wants to fail fast.
- Set `enabled: false` if the user wants the server staged but not spawned at startup.
- Do not commit or push `~/.aictl/mcp.json` — it is personal config, often laced with paths/secrets, not project state.
- Do not invent fields. The parser only looks at `command`, `args`, `env`, `enabled`, `timeout_secs`. Other keys are silently ignored, but adding them is misleading.
- Phase 1 supports the **stdio** transport only. HTTP/SSE servers are not yet wired — if the user asks for one, say so explicitly rather than guessing a config shape.
- Don't disable an existing server unless asked. To temporarily skip one without editing the file, suggest `AICTL_MCP_DISABLED=<name>` in `~/.aictl/config` or `--mcp-server <other>` on the CLI.

## Canonical examples

Pick whichever shape matches the user's intent and adapt the name / paths.

### Local Python smoke server (tiny_add)

```json
{
  "mcpServers": {
    "tiny_add": {
      "command": "python3",
      "args": ["/Users/you/code/aictl/examples/mcp/tiny_add/server.py"],
      "enabled": true,
      "timeout_secs": 10
    }
  }
}
```

### Filesystem server via npx (scoped to /tmp)

```json
{
  "mcpServers": {
    "filesystem": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"],
      "enabled": true
    }
  }
}
```

### GitHub server in Docker, token from keyring

Prerequisite: store `GITHUB_TOKEN` via `/keys` in the REPL first.

```json
{
  "mcpServers": {
    "github": {
      "command": "docker",
      "args": ["run", "--rm", "-i", "ghcr.io/github/github-mcp-server"],
      "env": {
        "GITHUB_PERSONAL_ACCESS_TOKEN": "${keyring:GITHUB_TOKEN}"
      },
      "enabled": true
    }
  }
}
```

### Staged but disabled

```json
{
  "mcpServers": {
    "experimental": {
      "command": "/usr/local/bin/my-mcp-server",
      "args": ["--port", "stdio"],
      "enabled": false
    }
  }
}
```

## Report back

After saving, tell the user:

- The exact path of the file you edited.
- The server name, command, and args you added (one line each).
- Whether `AICTL_MCP_ENABLED=true` is already set, or whether they still need to flip it.
- Whether `aictl --list-mcp` showed the new server.
- One concrete way to verify it (`/mcp` in the REPL, or a sample prompt that exercises one of the server's tools).

Do not run `aictl` itself beyond `--list-mcp`. Anything that boots the agent loop spawns the server and may invoke a real provider — that is the user's call.
