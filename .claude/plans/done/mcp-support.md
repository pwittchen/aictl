# Plan: MCP (Model Context Protocol) Server Support for aictl

## Context

[Model Context Protocol](https://modelcontextprotocol.io) is a JSON-RPC protocol for exposing **tools**, **resources**, and **prompts** from an external server to an LLM agent. A thriving catalogue of first- and third-party servers exists already (filesystem, git, GitHub, Slack, Postgres, Kubernetes, Puppeteer, etc.), so adding MCP support unlocks a large ecosystem without aictl having to implement each integration. Users declare servers in their config, aictl spawns or connects to them at startup, and their tools are merged into the built-in registry so the agent loop dispatches them transparently.

This plan is adjacent to, but deliberately distinct from, the [plugin system](plugin-system.md):

| Aspect | Plugins (Tier 1) | MCP servers |
|--------|------------------|-------------|
| Protocol | stdin/stdout raw bytes, one call per process spawn | JSON-RPC 2.0, long-lived process or HTTP connection |
| Lifecycle | Spawn-on-call, short-lived | Spawn-at-startup, long-lived |
| Schema | `description` + free-form `schema_hint` | JSON Schema for each tool |
| Capabilities | One custom tool per plugin dir | Many tools + resources + prompts per server |
| Discovery | Local filesystem (`~/.aictl/plugins/`) | Config declares servers; server itself enumerates what it offers |
| Ecosystem | aictl-specific | Cross-tool (Claude Desktop, Cursor, Continue, etc.) |

The two systems should coexist without colliding: plugins for one-off, language-agnostic scripts authored by the user; MCP for consuming the broader ecosystem.

## Goals & Non-goals

**Goals**
- Connect to MCP servers declared in config and expose their tools via the existing `<tool name="mcp__<server>__<tool>">...</tool>` XML contract.
- Support both transports in the MCP spec: stdio (spawn a child process) and HTTP/SSE (connect to a URL).
- Route every MCP tool call through `security::validate_tool()` and outbound redaction — MCP servers must not bypass the security gate.
- Surface MCP **resources** (read-only URI-addressed content) via a dedicated meta-tool the LLM can call.
- Surface MCP **prompts** as invocable slash-commands alongside user skills.
- Provide a `/mcp` REPL command and `--list-mcp` / `--mcp-server <name>` CLI flags for inspection and scripted use.
- Offer a curated remote catalogue (similar to `agents/remote.rs` and `skills/remote.rs`) for quickly adding well-known servers.
- Fail open per-server: a single broken server must not crash aictl startup or block other servers.

**Non-goals**
- No acting as an MCP server ourselves (exposing aictl tools to other MCP clients). That's a future phase — this plan is client-only.
- No hot-reload of server config. Restart aictl to pick up config edits, matching how plugins and agents work.
- No proxying aictl's own LLM providers back through MCP. MCP is an input channel, not an output channel in this plan.
- No per-tool allow/deny lists in v1 beyond the existing `AICTL_SECURITY_DISABLED_TOOLS` (which happens to cover MCP tools by name once they're namespaced).
- No support for the protocol's experimental `completion` or `sampling` features — both are rare in deployed servers and out of scope for v1.

## Approach: Phased rollout

### Phase 1 — stdio transport + tools only (MVP)

Get the core wiring in place with the simplest transport and capability. A minimal `mcp.rs` module that spawns a stdio server, completes the JSON-RPC handshake, calls `tools/list`, and merges the returned tools into the dispatch table in `tools.rs::execute_tool`. Master switch `AICTL_MCP_ENABLED=false` by default.

### Phase 2 — HTTP/SSE transport

Add the streamable-HTTP transport path. Shares the same `McpClient` trait surface; only the underlying framing changes. Adds bearer-token auth config per server.

### Phase 3 — resources + prompts

Introduce `mcp_read_resource` meta-tool and surface prompts as slash commands. Wire `/mcp` REPL menu to browse both.

### Phase 4 — remote catalogue

`mcp/remote.rs` mirroring `agents/remote.rs` — browse and pull curated server config entries from `.aictl/mcp/` in the project repo.

This plan specifies Phase 1 in detail, Phases 2–4 in outline, and flags open questions for each.

---

## Phase 1 — stdio + tools design

### 1. Configuration layout

Two supported locations, checked in order:

1. `~/.aictl/mcp.json` — preferred when the user has more than one or two servers. Structured JSON keeps the top-level `~/.aictl/config` clean.
2. Inline keys under `~/.aictl/config` — convenient for a single server; matches how the rest of aictl's config works.

`mcp.json` shape (compatible with Claude Desktop's config so users can copy-paste):

```json
{
  "mcpServers": {
    "filesystem": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-filesystem", "/Users/pw/Documents"],
      "env": { "FOO": "bar" },
      "enabled": true,
      "timeout_secs": 30
    },
    "github": {
      "command": "docker",
      "args": ["run", "--rm", "-i", "ghcr.io/github/github-mcp-server"],
      "env": { "GITHUB_PERSONAL_ACCESS_TOKEN": "${keyring:GITHUB_TOKEN}" },
      "enabled": true
    }
  }
}
```

Fields:
- `command` + `args` — the executable and argv for the stdio child. `command` is resolved via `PATH` (no shell). No shell metacharacters parsed.
- `env` — extra environment applied on top of the scrubbed base env from `util::scrubbed_env()`. Values support a `${keyring:NAME}` substitution pattern so API tokens live in the keyring, not the config file.
- `enabled` — per-server opt-in. Absent ⇒ `true` only if the top-level `AICTL_MCP_ENABLED=true` is also set.
- `timeout_secs` — per-call timeout for `tools/call` RPCs. Falls back to `AICTL_MCP_TIMEOUT` (default 30s).

### 2. Top-level config keys

```
AICTL_MCP_ENABLED=false              # master switch; default off (third-party code)
AICTL_MCP_CONFIG=~/.aictl/mcp.json   # override config path (for testing)
AICTL_MCP_TIMEOUT=30                 # per-RPC timeout default
AICTL_MCP_STARTUP_TIMEOUT=10         # initialize handshake timeout per server
AICTL_MCP_DISABLED=                  # comma-separated server names to skip at init()
```

Master-switch pattern deliberately mirrors `AICTL_PLUGINS_ENABLED`: MCP servers are third-party code, they must not load silently on first run after an update.

### 3. Module layout

New modules:

```
src/mcp.rs                 # public API: init, list_servers, find, call_tool, read_resource
src/mcp/
├── client.rs              # McpClient trait, shared JSON-RPC framing
├── stdio.rs               # StdioClient: spawn child, pipe JSON-RPC over stdin/stdout
├── http.rs                # HttpClient (Phase 2)
├── protocol.rs            # types: InitializeResult, Tool, Resource, Prompt, CallToolResult
├── config.rs              # parse mcp.json, resolve ${keyring:…}, validate names
└── remote.rs              # curated catalogue (Phase 4)
```

Key public API (minimum surface for Phase 1):

```rust
pub struct McpServer {
    pub name: String,              // alphanumeric + `_`/`-`, validated
    pub transport: Transport,      // Stdio { command, args, env } | Http { url, auth }
    pub enabled: bool,
    pub timeout: std::time::Duration,
    pub tools: Vec<McpTool>,       // populated by handshake
    pub resources: Vec<McpResource>,
    pub prompts: Vec<McpPrompt>,
    pub state: ServerState,        // Ready | Failed(reason) | Disabled
}

pub struct McpTool {
    pub name: String,              // bare, as reported by server
    pub description: String,
    pub input_schema: serde_json::Value,  // JSON Schema from tools/list
}

static SERVERS: OnceLock<Vec<RwLock<McpServer>>> = OnceLock::new();

pub async fn init() -> Result<()>;
pub fn list() -> impl Iterator<Item = /* snapshot of each server */>;
pub fn find_tool(qualified: &str) -> Option<(&McpServer, &McpTool)>;
pub async fn call_tool(qualified: &str, body: &str) -> Result<String, McpError>;
pub async fn shutdown();            // send shutdown, wait, SIGKILL on timeout
```

Namespacing: every MCP tool is reachable from the agent loop as `mcp__<server>__<tool>` — the same convention the current `mcp__github-aif__*` tools already use in Claude Code. The separator is a double underscore because single underscores appear inside server and tool names.

### 4. JSON-RPC client

Use `rmcp` (the official Rust MCP SDK) for the protocol implementation rather than hand-rolling JSON-RPC framing. Two considerations before adding:

- **Binary size / cold start** — `rmcp` pulls in `jsonrpc-core` and `tower`. Put MCP behind an `mcp` cargo feature like `gguf`/`mlx`/`redaction-ner` so users who don't enable MCP don't pay for it. The `init()` call and `/mcp` command must compile without the feature and print a rebuild hint ("rebuild aictl with `--features mcp` to enable MCP support"), matching how `mlx`/`gguf` work.
- **Version pinning** — MCP is young, wire format still iterating. Pin `rmcp` to an exact minor version and revisit when the spec hits 1.0.

Alternative: implement the subset we need (initialize, tools/list, tools/call, resources/list, resources/read, prompts/list, prompts/get, shutdown) directly over `tokio::process::ChildStdin/Stdout` with newline-delimited JSON. ~400 lines, no new dep. **Decision deferred to implementation**: start with `rmcp` for speed-to-MVP; drop it later if the dep cost is too high.

### 5. Server lifecycle

At startup (after `security::init()` and `config::init()`):

1. `mcp::init()` reads the config, parses each server entry, validates names (`is_valid_name`, same rule as agents/skills: alphanumeric + `_`/`-`).
2. For each enabled server, spawn the transport and issue `initialize` with aictl's client capabilities.
3. Wrap `initialize` in `tokio::time::timeout(AICTL_MCP_STARTUP_TIMEOUT)` so a hung server doesn't block startup.
4. On success, call `tools/list`, `resources/list`, `prompts/list` in parallel and populate the `McpServer` struct.
5. On any failure (spawn error, handshake error, timeout, schema rejection), transition the server to `ServerState::Failed(reason)`, log the reason, and continue with the remaining servers. Startup never fails because of MCP.
6. All servers connect in parallel (`tokio::join!` over a vec of futures) so slow handshakes don't serialize.

At shutdown (`Drop` on the `SERVERS` container, or explicit `mcp::shutdown()` from `main.rs` on SIGTERM/Esc-quit):

- Send `shutdown` over JSON-RPC, await ack with a 2s timeout.
- Close stdin to signal EOF.
- Wait up to 3s on the child exit, then `kill_on_drop`.

### 6. System-prompt catalog injection

`build_system_prompt()` currently swaps between `SYSTEM_PROMPT` and `SYSTEM_PROMPT_CHAT_ONLY` based on `tools_enabled()`. Extend it to append MCP tools in the same catalog format as built-ins, grouped per server so the LLM can tell them apart:

```
### mcp__filesystem__read_file (mcp)
Read the contents of a file.

Arguments (JSON):
{
  "path": { "type": "string", "description": "Absolute path to the file" }
}
```

The `(mcp)` suffix mirrors the `(plugin)` convention from `plugin-system.md` — inspectable via `/tools` and makes the provenance visible to the LLM.

### 7. Tool-call body format

MCP tools take a JSON object. aictl's built-in tools take free-form text. Bridge: the `<tool>` body for an MCP call is a JSON object literal:

```xml
<tool name="mcp__filesystem__read_file">
{"path": "/tmp/notes.md"}
</tool>
```

`execute_tool` inspects the tool name: if it starts with `mcp__`, route to `mcp::call_tool(name, body)` instead of the built-in match arm. `call_tool`:

1. Locates the server + tool (already enumerated at startup — no extra RPC).
2. Validates the body is JSON that matches the stored `input_schema` (via `jsonschema` crate; already a transitive dep of `rmcp`, otherwise add it).
3. Issues `tools/call` with the JSON args. Wraps the RPC in the per-server timeout.
4. Unpacks the `CallToolResult` — concatenates `content[]` text blocks; for image content, return the bytes as `ToolOutput::images` (existing `ToolOutput` already supports images).
5. Runs the concatenated text through `security::sanitize_output` before returning to the agent loop.

Errors surface as `[mcp error] <message>`, matching the `[exit N]` convention from plugins. The LLM can then self-correct (e.g., missing required arg) on the next turn.

### 8. Security gate

`security::validate_tool()` runs **before** MCP dispatch in the existing gate. The current implementation switches on `tool_call.name.as_str()`; add a catch-all at the bottom:

```rust
name if name.starts_with("mcp__") => validate_mcp_tool(name, &tool_call.input),
```

`validate_mcp_tool` enforces:
- **Disabled-tools list** — the fully qualified `mcp__foo__bar` name can appear in `AICTL_SECURITY_DISABLED_TOOLS` to block it.
- **Per-server deny** — `AICTL_MCP_DENY_SERVERS=github,slack` blocks every tool from listed servers even when the master switch is on.
- **Body size** — reuse `max_file_write_bytes` as an upper bound on the JSON body sent to the server to prevent runaway payloads.
- **No filesystem pre-validation** — unlike `read_file` / `write_file`, we can't inspect a generic MCP tool's intent statically. The CWD jail doesn't apply; the server runs in its own process with its own sandboxing decisions. Surface this in `--security` output ("MCP tools run with server's own privileges; CWD jail does not apply"). Users who want a strict policy should keep `AICTL_MCP_ENABLED=false`.

Outbound redaction still runs on the entire message stream, so the server never sees secrets detected by the redactor, regardless of transport.

### 9. Confirmation UX

MCP tools route through the same y/N confirmation gate as built-ins. The prompt copy names the server for provenance:

```
[mcp: filesystem] requires confirmation
  mcp__filesystem__read_file
  {"path": "/tmp/notes.md"}
Execute? [y/N]
```

`--auto` bypasses confirmation for MCP tools exactly as it does for built-ins. No per-server "trusted" flag in v1 — if a user wants a server that never prompts, they can run with `--auto` or use `AICTL_SECURITY_DISABLED_PROMPT` on specific tool names once that's available.

### 10. Duplicate-call guard

The existing `is_duplicate_call` guard in `tools.rs` normalizes on `(tool_name, normalized_input)`. Since MCP tool bodies are JSON, two calls with the same semantic args but different whitespace would currently be treated as distinct. Fix: for `mcp__*` calls, normalize the JSON body via `serde_json::from_str` + canonical serialization before feeding to `normalize_input`.

### 11. `/mcp` slash command

New `src/commands/mcp.rs`. Interactive menu with these actions:

- **List** — table of `name`, `transport`, `state`, `tool count`, `resource count`, `prompt count`.
- **Show** — picks a server, dumps its full tool/resource/prompt list with schemas.
- **Restart** — re-runs handshake for a single server (kills and respawns). Useful when a server crashes mid-session.
- **Enable/Disable** — flips the per-server `enabled` flag in `mcp.json` and re-runs handshake.
- **Reload config** — re-parse `mcp.json` without restart; diff current vs. desired, spawn new servers, shut down removed ones.

Every action is gated on `AICTL_MCP_ENABLED=true` (otherwise it prints the rebuild/feature-enable hint).

### 12. CLI flags (long-form only, matching existing convention)

```
--list-mcp                # print all configured servers + state, exit
--mcp-server <name>       # restrict this session to only the named server (others disabled)
--pull-mcp <name>         # Phase 4: pull a catalogue entry into ~/.aictl/mcp.json
```

### 13. Welcome banner

When `AICTL_MCP_ENABLED=true`, the banner gains a `mcp: N servers, M tools` line. When off, print nothing (quiet for users who don't use MCP). Failed servers surface as `mcp: N servers (2 failed — run /mcp for details)` so the user isn't left wondering why a tool is missing.

### 14. Audit logging

Every MCP call gets an audit entry with the qualified tool name, body (sanitized by redaction), outcome (Executed / DeniedByPolicy / DuplicateCall), and elapsed time. The existing `audit::log_tool` is oblivious to tool origin — it only needs the `ToolCall` — so no code change is required; it just starts seeing `mcp__*` names naturally.

### 15. Integration points

| File | Change |
|------|--------|
| `src/mcp.rs` | **New** — public API, `init`, `list`, `find_tool`, `call_tool`, `shutdown` |
| `src/mcp/client.rs` | **New** — `McpClient` trait, shared JSON-RPC framing |
| `src/mcp/stdio.rs` | **New** — stdio transport impl |
| `src/mcp/protocol.rs` | **New** — wire types |
| `src/mcp/config.rs` | **New** — parse `mcp.json`, `${keyring:…}` resolution |
| `src/tools.rs` | `mcp__*` fall-through in `execute_tool`; body normalization in duplicate guard |
| `src/run.rs` | `build_system_prompt()` appends MCP catalog |
| `src/security.rs` | `mcp__*` arm in `validate_tool`; `AICTL_MCP_DENY_SERVERS` support |
| `src/main.rs` | `mod mcp`, `mcp::init()` after `security::init()`, `mcp::shutdown()` on exit, `--list-mcp` / `--mcp-server` flags |
| `src/commands.rs` + `src/commands/mcp.rs` | **New** — `/mcp` REPL command + menu |
| `src/commands/tools.rs` | Include MCP tools in the `/tools` list, annotated `(mcp: <server>)` |
| `src/config.rs` | Readers for `AICTL_MCP_ENABLED`, `AICTL_MCP_CONFIG`, `AICTL_MCP_TIMEOUT`, `AICTL_MCP_STARTUP_TIMEOUT`, `AICTL_MCP_DISABLED`, `AICTL_MCP_DENY_SERVERS` |
| `src/ui.rs` | Banner gains `mcp: …` line when enabled |
| `Cargo.toml` | Add `rmcp` (or hand-rolled alt); `jsonschema` if not already transitive; gate behind `mcp` feature |
| `src/audit.rs` | No change — generic over tool name |

### 16. Testing

- **Unit tests** (`src/mcp/`):
  - Config parse: happy path, missing fields, invalid name, `${keyring:…}` substitution, env merge vs. scrub.
  - Tool-name qualification / de-qualification (`mcp__foo__bar` ⇔ (`foo`, `bar`), including handling of double-underscore in tool names which spec allows).
  - Duplicate guard: same JSON with whitespace differences collapses to one entry.
  - System-prompt injection: server tools appear with `(mcp)` suffix.
- **Integration tests** (`tests/mcp.rs`, running against a tiny built-in mock MCP server):
  - Stdio handshake + `tools/list` + `tools/call` round-trip.
  - Handshake timeout ⇒ server marked `Failed`, aictl startup still succeeds.
  - Server crash mid-session ⇒ next tool call surfaces `[mcp error] server not ready`, no panic.
  - `security::validate_tool()` with `mcp__foo__bar` in `AICTL_SECURITY_DISABLED_TOOLS` ⇒ blocked.
  - `AICTL_MCP_ENABLED=false` ⇒ no MCP tools in catalog, no dispatch attempted.
  - JSON schema violation in `<tool>` body ⇒ clear error back to LLM, no server RPC.
- **Manual smoke test**: point `mcp.json` at `npx @modelcontextprotocol/server-filesystem /tmp` and verify `read_file`/`write_file` calls round-trip through the agent loop. Add this as a documented walkthrough in `docs/mcp.md`.

### 17. Documentation

- New `docs/mcp.md`: config schema, transports, security model, catalogue walkthrough, example `mcp.json` with 3–4 common servers.
- `ARCH.md` gains an "MCP" section describing the client architecture (transport abstraction, lifecycle, dispatch path).
- `CLAUDE.md` gets a `src/mcp.rs` paragraph next to `src/plugins.rs` once that lands, or in its place if MCP ships first.
- `ROADMAP.md`: remove the "MCP server support" entry once Phase 1 ships.
- `README.md`: add "MCP server support" to the feature list; link to `docs/mcp.md`.
- Website (`website/index.html`, `website/guides.html`): short section under "Extensibility".

---

## Phase 2 — HTTP/SSE transport (outline)

- Add `src/mcp/http.rs` implementing `McpClient` over `reqwest` (already a dep).
- Config shape for HTTP entries:
  ```json
  "linear": {
    "url": "https://mcp.linear.app/sse",
    "auth": { "bearer": "${keyring:LINEAR_MCP_TOKEN}" },
    "enabled": true
  }
  ```
- Transport abstraction lives behind the `McpClient` trait so `mcp.rs` dispatch is identical to stdio.
- CORS / origin concerns don't apply — we're the client, not a browser.
- Reconnect policy: exponential backoff up to 60s on transport error; surface state as `Failed(reconnecting in Ns)` in `/mcp`.

## Phase 3 — resources and prompts (outline)

**Resources** — MCP servers expose read-only URI-addressed content (e.g., `file:///path/to/x`, `postgres://table/schema`). Surface via a new meta-tool:

```xml
<tool name="mcp_read_resource">
{"server": "postgres", "uri": "postgres://tables/users"}
</tool>
```

Single tool rather than per-resource names — MCP resources are often enumerated in the thousands, so splitting them into catalog entries would blow up the system prompt. Resources still appear in `/mcp show <server>` for user browsing.

**Prompts** — MCP prompts are parameterized templates ("summarize_pr", "triage_issue", …). They map onto aictl's existing skill system: treat each MCP prompt as a synthetic skill `mcp__<server>__<prompt>` that, when invoked, calls `prompts/get` with the arguments and injects the returned messages as the one-turn skill body. Surfaces in `/skills` alongside user skills, marked `(mcp)`.

This phase may be contentious — the mapping of prompts to skills assumes prompts are one-turn injections. MCP prompts can also return multiple messages forming a conversation starter. If that's common in real catalogues, we'll need a second surface. Decide during Phase 3 based on observed usage.

## Phase 4 — remote catalogue (outline)

Mirror `src/agents/remote.rs` and `src/skills/remote.rs`:

- Pinned repo coords: `pwittchen/aictl`, branch `master`, path `.aictl/mcp/`.
- Each `.aictl/mcp/<name>.json` is a single-server config entry with a description comment.
- `--pull-mcp <name>` appends the entry to `~/.aictl/mcp.json` (or creates the file).
- `--list-mcp --remote` browses the catalogue.
- Curated entries call out the auth requirements in the description, so a pull-then-edit-token flow is expected.

## Rollout phases

1. **Phase 1a** — `rmcp` behind `mcp` feature, `src/mcp.rs` skeleton, stdio transport, handshake + `tools/list` + `tools/call`, `execute_tool` fall-through, `build_system_prompt` catalog injection, security gate arm. CLI `--list-mcp` only.
2. **Phase 1b** — `/mcp` REPL command, confirmation UX, welcome banner line, integration tests against a mock server, `docs/mcp.md`.
3. **Phase 2** — HTTP/SSE transport.
4. **Phase 3** — resources (`mcp_read_resource`) and prompts-as-skills.
5. **Phase 4** — remote catalogue + `--pull-mcp`.
6. **Phase 5** (optional, later) — aictl as an MCP server, exposing built-in tools to other MCP clients. Dovetails with the `aictl-server` roadmap entry; likely ships as a subcommand of `aictl-server`.

## Verification

1. `cargo build` and `cargo build --release` — clean with and without `--features mcp`.
2. `cargo lint` (clippy pedantic per `.cargo/config.toml`) — no warnings.
3. `cargo test` — unit + integration tests pass, including the mock-server round-trip.
4. Manual (Phase 1):
   - Configure one stdio server (`@modelcontextprotocol/server-filesystem`); confirm `/tools` lists its tools with `(mcp)` suffix.
   - Prompt the LLM to read a file via the MCP server; confirm round-trip and correct result.
   - Kill the server process externally; confirm next call surfaces `[mcp error]` and `/mcp` shows `Failed`.
   - Add `mcp__filesystem__read_file` to `AICTL_SECURITY_DISABLED_TOOLS`; confirm dispatch is blocked at the security gate.
   - Flip `AICTL_MCP_ENABLED=false`; confirm no MCP tools appear and the banner line disappears.
   - Misconfigure a server (bad path); confirm aictl still starts, that server is `Failed`, other servers still work.
   - Build without `--features mcp`; confirm `--list-mcp` and `/mcp` print the rebuild hint instead of erroring.

## Open questions

- **Dep choice: `rmcp` vs. hand-rolled JSON-RPC** — resolve during Phase 1a. Start with `rmcp` for speed, revisit if binary size or dep churn is painful.
- **JSON-Schema validation lib** — `jsonschema` is popular but pulls in a lot of regex deps. If `rmcp` already re-exports a validator, use that; otherwise consider skipping strict validation and letting the server return a validation error (simpler, less binary bloat, at the cost of a round-trip per bad call).
- **`${keyring:NAME}` substitution** — should this be MCP-specific or a general config-value feature? Probably general — `keys::get_secret(name)` already exists; wire it into `config_get` for values matching the pattern. Out of scope for this plan, but flag it for a follow-up.
- **Prompts as skills vs. dedicated surface** — decide once we've looked at a handful of real catalogues (GitHub, Linear, Slack). The "prompts are multi-message" concern may or may not matter in practice.
- **Resource caching** — MCP servers can declare resources with TTLs. v1 doesn't cache; revisit if latency is painful for servers with large static resource sets.
- **Concurrency** — `tools/call` is inherently serial per server today (one in-flight RPC per JSON-RPC client). If the agent loop starts dispatching tools in parallel (see coding-agent roadmap entry), MCP calls on the same server will queue. Servers that support JSON-RPC batch requests could parallelize, but that's a v2 concern.
- **Windows** — not a target today. Defer stdio transport Windows quirks (named pipes, `.exe` resolution) until there's demand, matching the plugin plan.
