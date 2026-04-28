# Plan: `aictl-server` — HTTP LLM Proxy

## Scope update — 2026-04-28

This plan was originally written around an agent-endpoint MVP plus an OpenAI-compatible gateway. That direction has been narrowed: **`aictl-server` is now a pure LLM proxy with no agent capabilities.** Specifically, the server:

- Does **not** run the agent loop (`run::run_agent_turn`) or expose any endpoint that does.
- Does **not** dispatch tools, list/load agents or skills, manage sessions, or surface plugins/hooks/slash commands.
- Does **not** implement an `HttpUI` against `AgentUI` — there is no `AgentUI` consumer on the server side.
- **Does** expose the OpenAI-compatible gateway (`/v1/chat/completions`, `/v1/completions`, `/v1/models`) routing to every supported provider, with redaction and audit applied.
- **Does** require a master API key on every request (auto-generated on first startup if not configured) and **does** write a structured server log.

Everything that was the "agent endpoint" plan (formerly Phase 1, §§5–7, parts of §13–§15) is dropped from the active scope and kept below for reference only — it is no longer a target for implementation. Sections that still apply (gateway, errors, shutdown, deployment) are unchanged. New sections at the end of this document describe the master API key and the server log requirements.

If a future product need calls for HTTP-accessible agent loops, that becomes a separate plan, not a revival of the dropped phase.

## Context

Today `aictl` is a CLI-only program. Every interaction goes through the REPL or a single-shot `--message` invocation, and every provider call originates from a short-lived process on the user's machine. This plan adds a second binary, `aictl-server`, that exposes the LLM provider catalogue over a local (or optionally remote) HTTP endpoint as an **OpenAI-compatible passthrough**. Clients holding an OpenAI-format SDK can point at `aictl-server` and transparently consume Anthropic, Gemini, Grok, Mistral, DeepSeek, Kimi, Z.ai, Ollama, GGUF, or MLX models, with redaction, audit, key management, and a master-key gate handled centrally.

The server reuses `~/.aictl/config` as-is so a user can run the CLI and the server side-by-side against the same provider keys, while `--config <path>` / `AICTL_CONFIG` let operators run multiple server instances with different policies. Outbound redaction (`run::redact_outbound`) and audit logging continue to apply on the proxy path.

What the server deliberately does **not** offer: the agent loop, tool dispatch, agents, skills, plugins, hooks, sessions, slash commands, or any coding-mode workflow. Those are CLI/REPL features and remain CLI/REPL-only. Clients that need them use `aictl`.

## Prerequisite: Modular Architecture

This plan depends on [.claude/plans/modular-architecture.md](modular-architecture.md) having shipped through at least the workspace split. Specifically:

- `aictl-core` exists as a library crate with the `llm::call_<provider>` functions, `MODELS` catalogue, redaction pipeline, audit logger, key store, and stats writer as public API.
- Provider calls and the redaction pipeline are owned by `aictl-core` and reachable from any frontend.
- Feature flags (`gguf`, `mlx`, `redaction-ner`) live on `aictl-core`; the server enables whichever the deployment needs.

The server does **not** depend on `AgentUI` or `run::run_agent_turn` being part of the public surface — it never calls them. Until the workspace split lands, attempting to write `aictl-server` would drag REPL deps (`rustyline`/`termimad`/`crossterm`) into a daemon, which the workspace split is explicitly there to prevent.

## Goals & Non-goals

**Goals**
- Ship a second binary (`aictl-server`) from the same workspace that speaks HTTP on a configurable bind address.
- Be a pure LLM proxy: translate OpenAI-shaped requests into the configured provider's native format, call `aictl_core::llm::call_<provider>`, translate the response back, with redaction and audit layered in.
- Keep the CLI binary (`aictl`) unchanged in behavior, dependencies, and cold-start cost. No HTTP deps leak into `aictl-cli`.
- Streaming over Server-Sent Events using OpenAI's `data: {"choices":[{"delta":...}]}` shape.
- Security-first defaults: bind `127.0.0.1` only by default, master API key required on every request, CORS off, per-request body size cap, per-request timeout.
- Master API key auto-generated and persisted to `~/.aictl/config` on first startup if not already configured.
- Structured server log written to a configurable path; redaction applied to any payload preview.
- Audit every gateway request through `audit.rs`. Stats through `stats.rs`.
- Graceful shutdown: drain in-flight requests with a timeout, flush audit/stats/log, exit.
- Multiple instances on the same host can coexist via distinct `--config <path>` and `--bind` pairs.

**Non-goals**
- **Not an agent host.** No `/v1/chat` agent-loop endpoint, no SSE streaming of `AgentEvent`s, no tool dispatch, no `<tool name="...">` filtering — none of those code paths exist on the server. Clients that need agent capabilities use the CLI.
- **No agents, skills, sessions, plugins, hooks, or slash commands.** Their files on disk are ignored by the server; their HTTP endpoints do not exist.
- **No `/v1/tools/*`** direct tool invocation. The security gate (`security::validate_tool`) is not even wired up on the server because there are no tools to validate.
- Not a Coding Agent host. The dropped scope statement above already implies this; coding mode is doubly out of scope.
- Not a multi-tenant SaaS backend. No per-user isolation beyond what the single shared `~/.aictl/config` provides.
- No clustering, no inter-node replication. One process, one host.
- No WebSockets in v1. SSE covers streaming.
- No browser UI served by the binary. The server is JSON/SSE only.
- No reverse-proxy assumption. We do not read `X-Forwarded-*` headers or trust them.
- No rate-limiting in v1 beyond a concurrency cap.

## Approach: Phased rollout

### Phase 1 — OpenAI-compatible gateway (MVP)

`POST /v1/chat/completions` and `POST /v1/completions` accept the OpenAI request schema and route to whichever provider matches the `model` field. Streaming via SSE in OpenAI's `data: {"choices":[{"delta":...}]}` shape. Plus `GET /v1/models`, `GET /healthz`, `GET /v1/stats`. Master API key required on every authenticated request (auto-generated on first startup if not configured). Structured server log enabled out of the box.

### Phase 2 — operational hardening

Optional TLS termination via `rustls`. Rate limiting (`tower_governor` or equivalent) keyed on the master API key. `GET /metrics` Prometheus endpoint. Multi-config presets so a single binary launch can switch between pre-baked policies.

### Phase 3 — remote provider key passthrough (revisit)

Re-evaluate whether to support a per-request provider key override (`Authorization` forwarded upstream) for deployments that want the server to hold no provider keys. Not a v1 concern; revisit only on concrete demand.

### Dropped scope (formerly Phase 1 + Phase 3)

The agent endpoint (`POST /v1/chat`, `POST /v1/chat/stream`), session CRUD, agent/skill listings, direct tool invocation (`POST /v1/tools/{name}`), tool catalog, MCP resources/prompts, and the `HttpUI`/`AgentEvent` SSE machinery are all dropped from this plan. The detailed sections below that describe them are kept for historical reference and **clearly marked as dropped** — they are not implementation targets.

---

## Phase 1 — detailed design

> **Note:** the sections below were written when Phase 1 covered the agent endpoint plus the gateway. Sub-sections describing the agent loop, `HttpUI`, sessions, agents, skills, tool dispatch, and Phase-3 tool surfaces are **dropped** (see scope update at the top). Sub-sections covering crate skeleton, framework choice, configuration, errors, graceful shutdown, deployment, and the gateway translation layer (§8) still apply to the new Phase 1 (OpenAI gateway only). The new master-key and server-log requirements are documented in their own sections after this block.

### 1. Crate skeleton

Under the existing workspace (post-Modular-Architecture split):

```
crates/
├── aictl-core/          # existing library
├── aictl-cli/           # existing CLI binary
└── aictl-server/        # NEW
    ├── Cargo.toml
    └── src/
        ├── main.rs            # clap, server startup, graceful shutdown
        ├── config.rs          # server-specific config knobs
        ├── state.rs           # shared AppState (concurrency semaphore, shutdown handle)
        ├── auth.rs            # master-key middleware (constant-time compare)
        ├── master_key.rs      # load-or-generate the master API key on startup
        ├── log.rs             # structured server log writer (file or stderr)
        ├── error.rs           # typed ApiError + IntoResponse impl
        ├── openai.rs          # OpenAI request/response translation per provider
        ├── routes/
        │   ├── mod.rs
        │   ├── gateway.rs     # /v1/chat/completions, /v1/completions
        │   ├── models.rs      # /v1/models
        │   ├── health.rs      # /healthz
        │   └── stats.rs       # /v1/stats
        └── sse.rs             # SSE framing helpers (OpenAI delta shape)
```

There is intentionally no `ui.rs`, `routes/chat.rs`, `routes/agents.rs`, `routes/skills.rs`, or `routes/sessions.rs` — those would have served the dropped agent-endpoint scope.

`aictl-server/Cargo.toml` depends on `aictl-core = { path = "../aictl-core" }`. Runtime deps beyond what core already pulls in: `axum`, `tower`, `tower-http` (for `RequestBodyLimit`, `TraceLayer`, `TimeoutLayer`), `tokio` (inherits features from core), `futures`, `serde`/`serde_json` (already in core), `async-stream` for SSE body construction. No `rustyline`, `termimad`, `crossterm`, `indicatif`, `dialoguer`.

Binary name is `aictl-server` (`[[bin]] name = "aictl-server"`). No subcommand inside `aictl`. See the roadmap section "Separate binaries vs. one binary" for the rationale.

### 2. HTTP framework choice: axum

**Axum**, for four reasons:

- Already sits in the Tokio ecosystem. No new runtime, no new async abstractions. `aictl-core` already uses `tokio`, so axum slots in without a runtime mismatch.
- First-class SSE support (`axum::response::sse::Sse`, `KeepAlive`). The CLI's `StreamState` already produces `String` tokens; wrapping them in SSE `data:` frames is a few lines.
- Typed extractors (`Json<T>`, `State<AppState>`, `TypedHeader<Authorization<Bearer>>`) make the route signatures read like documentation.
- The same `tower::ServiceBuilder` middleware stack lets us layer on request-body limits, timeouts, auth, and tracing without hand-rolling wrappers.

Alternatives considered and rejected: **actix-web** (second runtime, doesn't compose with the existing tokio-based engine without extra care), **hyper directly** (no extractor ergonomics, every route becomes boilerplate), **rocket** (still maturing async story and a more opinionated dev experience than we want for infra code).

Pin axum to `0.8.x`. Upgrade cost is low but semver breaks between minor lines are real; lock it explicitly in `Cargo.toml`.

### 3. Configuration

Every server knob lives in `~/.aictl/config` (or the `--config <path>` override). CLI flags override config. The CLI-style `config_get`/`config_set` helpers in `aictl-core::config` work unchanged; the server adds a narrow set of new keys:

```
server_bind=127.0.0.1:7878           # default; any non-loopback change still requires the master key
server_master_key=<key>              # required on every request; auto-generated on first startup if absent
server_cors_origins=                 # comma-separated; empty = CORS off (default)
server_request_timeout=120           # per-request wall-clock timeout, seconds; 0 disables
server_body_limit_bytes=2097152      # per-request body cap; 2 MiB default
server_max_concurrent_requests=32    # global concurrency semaphore; rejects 503 when saturated
server_shutdown_timeout=20           # drain grace period on SIGTERM, seconds
server_sse_keepalive=15              # SSE keepalive interval, seconds (0 disables)
server_log_level=info                # trace|debug|info|warn|error
server_log_file=~/.aictl/server.log  # structured log destination; empty = stderr only
```

CLI flags on `aictl-server` mirror the important ones and follow the existing long-form-only convention:

```
aictl-server \
  --config <path>           # override ~/.aictl/config
  --bind <addr:port>        # override server_bind
  --master-key <value>      # override server_master_key (prefer config for persistence)
  --quiet                   # suppress startup banner on stderr
  --log-level <level>       # override server_log_level
  --log-file <path>         # override server_log_file
```

Note: `--unrestricted` is **not** a server flag. It exists in the CLI to disable `security::validate_tool`, but the server does not dispatch tools, so the flag has nothing to gate.

The existing `aictl-core::config` loader uses `OnceLock<RwLock<HashMap>>` and hard-codes the `~/.aictl/config` path. As part of Modular Architecture Phase 3 that loader should already take an override path; if not, adjust it before writing the server. The server plan assumes the override path is wired.

### 4. Authentication and network binding

The master API key (`server_master_key`) is required on every request **regardless of bind address**. There is no loopback-bypass. The CLI's "user owns the machine" model does not transfer here: the moment a process listens on a port, anything on the loopback interface (other users on a shared host, browser-based attacks via DNS rebinding) can reach it. A single key is the simplest defense and costs nothing.

**Master-key handling on startup** (`master_key.rs`):

1. If `--master-key <value>` is provided, use it for this launch. Do not persist it.
2. Else if `server_master_key` is set in the active config, use it.
3. Else generate a new key: 32 bytes of OS-randomness, base64url-encoded with no padding. Persist via `aictl_core::config::config_set("server_master_key", …)` and print exactly once to stderr (and to `server_log_file`) at startup, prefixed with a clear marker:

   ```
   [server] generated new master API key — set Authorization: Bearer <key>
   [server] persisted to ~/.aictl/config (server_master_key)
   ```

   Subsequent launches reuse the persisted key silently.

Operators rotate by removing the config entry (next launch regenerates) or by writing a new value manually. There is no rotation API in v1 — the surface stays small.

Every request except `GET /healthz` requires `Authorization: Bearer <master-key>`. Comparison is constant-time (`subtle::ConstantTimeEq` or equivalent) to avoid timing oracles. Missing header → 401; wrong key → 401 with an identical response body so the distinction can't be enumerated.

**Bind defaults**: `127.0.0.1:7878`. Operators who set a non-loopback bind get the same auth requirement (the master key). A startup warning is printed when `server_bind` resolves to a non-loopback address so accidental `0.0.0.0` from a container template is at least visible in the log.

**CORS**: off by default. If `server_cors_origins` is set, `tower-http::cors::CorsLayer` adds the configured origins with credentials allowed.

**TLS**: not terminated by the server in v1. Operators put nginx/Caddy in front if they need HTTPS. Rustls termination is a Phase 2 optional add-on.

### 4a. Server log

The server writes a structured request log on top of (and separate from) the existing `audit::log_tool` records. Where the audit log is a per-session JSONL trail of every provider dispatch (kept for parity with the CLI), the **server log** is the operational record an admin actually reads when something goes wrong.

Destination resolution: `--log-file` overrides `server_log_file`; empty value (or unset) means stderr only; otherwise the file is opened append-only with a `BufWriter`.

Each log line is one JSON object (`tracing-subscriber`'s `json` formatter). Required fields:

- `timestamp` (RFC 3339, UTC)
- `level` (`info` / `warn` / `error` etc.)
- `request_id` (per-request UUID; surfaced in `X-Request-Id` response header)
- `method`, `path`, `status`
- `elapsed_ms`
- `model` (when known), `provider` (when known)
- `upstream_request_id` (from the provider's response, when surfaced)
- `bytes_in`, `bytes_out`
- `client_ip` (from the socket — `X-Forwarded-*` is not trusted)
- `error_code` and `trace_id` on failures

Server-internal events (startup banner, master-key generation marker, shutdown drain progress) log at `info`. Failed-auth attempts log at `warn` with the source IP. Provider errors log at `error` with the trace ID.

Redaction note: the log never includes raw prompt content or full response bodies. If a payload preview is helpful (e.g., for malformed-JSON diagnostics), it is run through the same `redaction` pipeline that protects outbound provider calls before being attached. Bodies that fail redaction are logged as `<redacted>` rather than risking PII leakage to the file.

Log level is set by `server_log_level` (`trace` / `debug` / `info` / `warn` / `error`) or `--log-level`. Default `info`. Rotation is the operator's responsibility (`logrotate` or systemd journal); the server does not rotate the file itself in v1.

### 5. The `HttpUI` — adapting `AgentUI` to HTTP

The agent loop in `aictl-core::run` calls `ui.show_answer`, `ui.stream_chunk`, `ui.confirm_tool`, `ui.warn`, etc. The CLI implements these with terminal writes. The server's `HttpUI` implements them by pushing typed `AgentEvent` values into a `tokio::mpsc::Sender`, and the route handler consumes that channel either (a) by collecting all events and returning a single JSON response for non-streaming, or (b) by converting each event into an SSE frame and yielding from an `async_stream`.

Event shape:

```rust
#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    Reasoning { text: String },
    StreamChunk { text: String },
    ToolCallAuto { name: String, input: String, id: u64 },
    ToolCallPending { name: String, input: String, id: u64 },  // awaiting client approval
    ToolResult { id: u64, output: String, truncated: bool },
    Answer { text: String },
    Warning { text: String },
    TokenUsage { input: u64, output: u64, cached: u64, model: String },
    Error { code: String, message: String },
    Done { total_tokens: u64, tool_calls: u32, elapsed_ms: u64 },
}
```

The CLI's `AgentUI::confirm_tool` returns synchronously. In HTTP land there is no interactive terminal to block on, so the `HttpUI` implementation of `confirm_tool` consults the per-request `auto` flag from the JSON body:

- `auto: true` ⇒ return `ToolApproval::ApproveOnce` without prompting. Matches CLI `--auto`.
- `auto: false` ⇒ return `ToolApproval::Deny` and emit a `ToolCallPending` event so the client knows the request was halted. The client can resume by re-POSTing with the explicit approval (see below). v1 is "deny by default"; interactive approval is a Phase 3 concern once a protocol for client-side approval replies is designed. Most server deployments will run `auto: true`.

The existing `ToolApproval` enum (`ApproveOnce`, `ApproveAll`, `Deny`) stays in `aictl-core`; `HttpUI` only produces `ApproveOnce` or `Deny` in v1.

`stream_suspend` / `stream_end` map onto SSE frame boundaries: when the streaming session ends (tool call detected, provider call complete, error), the SSE body closes with a final `Done` event followed by `[DONE]` in OpenAI's convention for Phase 2.

### 6. Request concurrency and timeout

A global `tokio::sync::Semaphore` seeded with `server_max_concurrent_requests` gates every non-health route. On saturation the request returns 503 immediately rather than queueing; clients that care about backpressure handle retry themselves. The semaphore permit is held for the full request duration, including the streaming response body.

Each request gets a `tokio::time::timeout(server_request_timeout)` wrapped around the agent-loop future. On expiry the HTTP response is terminated with a 504-equivalent final event (`Error { code: "timeout", ... }`) and the semaphore permit is released.

Per-turn provider timeouts (`AICTL_LLM_TIMEOUT`, default 30s) continue to apply inside the engine. The request timeout is the outer cap covering the whole agent loop including tool dispatch.

### 7. Endpoints — request/response shapes

All request bodies are JSON unless stated otherwise. All responses are JSON or SSE (`text/event-stream`). Timestamps are RFC 3339 strings.

#### `POST /v1/chat` — single-shot agent turn (non-streaming)

Request:
```json
{
  "prompt": "summarize TODO.md",
  "model": "claude-sonnet-4-6",
  "agent": "code-reviewer",
  "skill": "review",
  "session_id": "abc123",
  "auto": true,
  "memory": "persistent",
  "unrestricted": false,
  "working_dir": "/Users/op/projects/foo"
}
```

- `prompt` — required. Same text the user would type at the REPL.
- `model` — optional. Falls back to the configured default model, same rule as the CLI.
- `agent`, `skill` — optional. Same semantics as `--agent` / `--skill`.
- `session_id` — optional. If omitted, a fresh session is created; if present but not found, 404.
- `auto` — required. `true` to bypass tool approval; `false` for deny-by-default in Phase 1.
- `memory` — `persistent` | `ephemeral`. Ephemeral skips the session writeback.
- `unrestricted` — forbidden unless the server was launched with `--unrestricted`. The request flag is a per-request opt-in, not an escalation.
- `working_dir` — optional. Must be an absolute path; the server validates it exists, is a directory, and is readable. Passes into the prompt-file lookup (`AICTL.md` → `CLAUDE.md` → `AGENTS.md`) so per-request working directories become meaningful.

Response:
```json
{
  "session_id": "abc123",
  "answer": "The TODO list has 5 items: ...",
  "tool_calls": [
    { "name": "read_file", "input": "/.../TODO.md", "output_preview": "1. ...", "duration_ms": 45 }
  ],
  "usage": { "input_tokens": 1234, "output_tokens": 567, "cached_tokens": 0, "estimated_cost_usd": 0.003 },
  "model": "claude-sonnet-4-6",
  "provider": "Anthropic",
  "elapsed_ms": 2104,
  "iterations": 3
}
```

Errors as typed `ApiError` with a stable `code` field (see §10).

#### `POST /v1/chat/stream` — streaming agent turn (SSE)

Same request body as `/v1/chat`. Response is `text/event-stream` with one `data:` frame per `AgentEvent`, concluding with a `done` frame:

```
event: stream_chunk
data: {"type":"stream_chunk","text":"Hello"}

event: stream_chunk
data: {"type":"stream_chunk","text":", world"}

event: tool_call_auto
data: {"type":"tool_call_auto","name":"read_file","input":"/etc/hostname","id":1}

event: tool_result
data: {"type":"tool_result","id":1,"output":"host.local\n","truncated":false}

event: answer
data: {"type":"answer","text":"Your hostname is host.local."}

event: done
data: {"type":"done","total_tokens":1801,"tool_calls":1,"elapsed_ms":1420}
```

Keepalive comments (`: keepalive\n\n`) are sent every `server_sse_keepalive` seconds so proxies don't drop the connection.

The CLI's `StreamState` filter is reused verbatim: anything that could prefix `<tool name="` is held back until it's unambiguous, so no tool XML ever reaches the client. This is the same invariant the CLI enforces; we inherit the guarantee by going through the engine.

#### `GET /v1/models`

Lists every model from `aictl_core::llm::MODELS` plus any locally available Ollama / GGUF / MLX models (same detection as `/models` in the CLI).

```json
{
  "models": [
    {
      "id": "claude-opus-4-7",
      "provider": "Anthropic",
      "context_window": 1000000,
      "capabilities": ["chat", "tools", "streaming", "images"],
      "available": true
    },
    { "id": "gpt-4.1", "provider": "OpenAI", "context_window": 1047576, "available": true },
    { "id": "llama3.1:latest", "provider": "Ollama", "context_window": 131072, "available": true }
  ]
}
```

Availability depends on (a) key presence for cloud providers, (b) local binary/model presence for Ollama/GGUF/MLX.

#### `GET /v1/agents`, `GET /v1/skills`

Enumerate installed agents/skills including frontmatter. Matches the CLI's `--list-agents` / `--list-skills` output.

```json
{
  "agents": [
    { "name": "code-reviewer", "description": "...", "source": "aictl-official", "category": "development" }
  ]
}
```

#### `GET /v1/sessions`, `GET /v1/sessions/{id}`, `DELETE /v1/sessions/{id}`, `POST /v1/sessions`

CRUD over `aictl_core::session`. `POST` creates a named session; response returns the `id`. `GET /v1/sessions/{id}` returns session metadata plus the message history (optionally redacted — see §9). `DELETE` removes it. Incognito sessions (`is_incognito`) are not listed in the index.

#### `GET /healthz`

No auth. Returns `{"status":"ok","version":"...","uptime_secs":...,"active_requests":N}`. Used by container orchestrators and external monitors.

#### `GET /v1/stats`

Authenticated. Returns `aictl_core::stats` aggregates — sessions started, tokens per provider, tool-call counts, estimated spend — for the configured config's stats file.

### 8. OpenAI-compatible gateway (Phase 2)

`POST /v1/chat/completions` accepts the standard OpenAI request schema (`messages`, `model`, `stream`, `tools`, `temperature`, `max_tokens`, `top_p`). The `model` field determines the target provider:

- Prefix or exact-match lookup against `aictl_core::llm::MODELS` decides the provider (`Anthropic`, `OpenAI`, `Gemini`, `Grok`, `Mistral`, `DeepSeek`, `Kimi`, `Z.ai`, `Ollama`, `GGUF`, `MLX`).
- The server translates the OpenAI request into the provider's native format, calls `aictl_core::llm::call_<provider>`, and translates the response back into the OpenAI response schema.
- For streaming (`stream: true`), SSE frames mirror OpenAI's `data: {"choices":[{"delta":...}]}` shape.

This is raw provider passthrough — no agent loop, no tool execution. It exists so existing OpenAI-SDK apps can swap their base URL and pick up every aictl-supported provider for free, with redaction and audit applied centrally.

Gateway-specific behavior:
- **Redaction** still runs on the outbound `messages[]` content. Local providers (Ollama/GGUF/MLX) skip unless `AICTL_SECURITY_REDACTION_LOCAL=true`, same as the CLI.
- **Audit** logs every gateway request as `ToolCall { name: "gateway:<provider>", input: "<redacted prompt>", ... }` so the existing audit trail covers both surfaces.
- **Tool-call translation**: OpenAI's `tools` / `tool_choice` fields are passed through to providers that support them natively (Anthropic, OpenAI, Gemini). For providers without native tool support, the server rejects tool-including requests with 400. No attempt to paper over provider capability differences — that's the caller's decision.
- **`Authorization: Bearer sk-...`** header from the client is *not* forwarded to the upstream provider. The server substitutes its own configured key. Clients send the aictl server token in `Authorization`; the server picks the right upstream key from `~/.aictl/config`.

`POST /v1/completions` follows the same shape for the legacy text-completion API. Providers that have only chat APIs (Anthropic, Gemini, etc.) get the prompt wrapped into a single user message; the translation rule is documented in `docs/server.md`.

### 9. Security gate, redaction, and audit

Identical to the CLI. The server is a thin transport over the same engine, so:

- Every tool call passes through `aictl_core::security::validate_tool` before execution and `sanitize_output` on return.
- `--unrestricted` disables validation exactly as it does in the CLI. Audit and redaction still run.
- Outbound redaction (`run::redact_outbound`) runs right before provider dispatch in the agent-loop path *and* right before provider dispatch in the gateway path. No change to the redactor itself; the server just makes sure both code paths call it.
- The CWD jail in `SecurityPolicy.working_dir` is scoped per-request via the `working_dir` field in the request body. Each request constructs a `SecurityPolicy` snapshot with its own `working_dir` instead of reading `std::env::current_dir()`. The global `POLICY` stays — per-request scoping overlays per-request working directories on top of the shared policy.
- Audit entries go to `~/.aictl/audit/<session-id>` as they do today. For ephemeral sessions, audit still runs against the ephemeral session id; the file is cleaned up at request end if `memory: "ephemeral"`.
- The prompt-injection guard (`detect_prompt_injection`) runs on every incoming user message. On match, the request is rejected with 400 `code: "prompt_injection"` — not an HTTP-level 451 or anything fancy, just a typed error.

Response bodies returning session history should apply redaction to the displayed content *only if* `server_redact_history=true` is set. The server doesn't auto-redact session history by default because the caller presumably has the same privilege as the CLI user; operators who expose `/v1/sessions/{id}` to less-privileged callers flip the flag.

### 10. Errors

One typed error enum maps to HTTP status codes:

```rust
pub enum ApiError {
    BadRequest { code: &'static str, message: String },        // 400
    Unauthorized,                                               // 401
    Forbidden { reason: &'static str },                         // 403
    NotFound { what: &'static str },                            // 404
    Conflict { message: String },                               // 409
    PayloadTooLarge { limit: u64 },                             // 413
    UnprocessableEntity { code: &'static str, message: String },// 422
    TooManyRequests,                                            // 429 (reserved for Phase 4 rate limits)
    InternalError { trace_id: String },                         // 500
    ServiceUnavailable { reason: &'static str },                // 503
    GatewayTimeout,                                             // 504
}
```

Every error body is `{"error": {"code": "...", "message": "..."}}` — stable machine-parseable shape. `code` values are documented in `docs/server.md` so client libraries can dispatch on them.

Stable error codes covering the agent loop:
- `prompt_injection`, `tool_denied`, `tool_missing_args`, `tool_timeout`, `provider_unavailable`, `provider_rate_limited`, `model_not_found`, `session_not_found`, `redaction_blocked`, `body_too_large`, `body_malformed`, `coding_mode_rejected`, `working_dir_invalid`.

Internal errors log the full error with a generated trace ID; the response surfaces only the trace ID for the operator to correlate.

### 11. Graceful shutdown

SIGINT / SIGTERM triggers:

1. Stop accepting new connections (`axum_server::Handle::graceful_shutdown`).
2. Wait up to `server_shutdown_timeout` seconds for in-flight requests to complete.
3. On timeout, cancel remaining requests with a final `Error { code: "server_shutting_down" }` event (for SSE) or 503 (for non-streaming).
4. Flush audit and stats writers (both are file-backed with `BufWriter` today; `drop` flushes on scope exit, but we call explicit flush to be safe).
5. Release provider HTTP clients (drops `reqwest::Client`), close MCP server connections if MCP support is active.
6. Exit 0.

A `/v1/shutdown` admin endpoint is deferred. Operators signal the process.

### 12. Observability

Structured logging via `tracing` + `tracing-subscriber` with the `json` feature for production, `fmt` for dev. Fields on every request span: `method`, `path`, `status`, `elapsed_ms`, `request_id`, `session_id` (if any), `model` (if any). Log levels controlled by `server_log_level`.

Metrics are out of scope for Phase 1 (no Prometheus endpoint). If operators ask for it, Phase 4 adds `GET /metrics` via `metrics-exporter-prometheus`. Until then, `/v1/stats` covers the LLM-specific telemetry, and request-level metrics come out of structured logs.

### 13. Coding mode rejection

`aictl-core` exposes whatever coding-mode enablement flag the Coding Agent roadmap entry lands (likely `AICTL_CODING_WORKFLOW` plus a request flag). `aictl-server` rejects any request whose body enables coding mode with `400 coding_mode_rejected`. This is enforced as a middleware on every route that hits the agent loop, not just the chat endpoint.

### 14. Integration points

| File / location | Change |
|-----------------|--------|
| `crates/aictl-server/` | **New** — entire crate per the skeleton in §1 |
| `Cargo.toml` (workspace root) | Add `crates/aictl-server` to `members` |
| `crates/aictl-core/src/config.rs` | If not already done in Modular Architecture, accept an optional config-path argument at init time |
| `crates/aictl-core/src/run.rs` | No behavior change; verify `run_agent_turn` is `pub`, not `pub(crate)` (Modular Architecture Phase 4 already does this) |
| `crates/aictl-core/src/session.rs` | Verify `load_messages`, `save_messages`, `set_current`, `current_id` are `pub` — they already are; confirm no regression |
| `crates/aictl-core/src/security.rs` | Add a builder that takes a per-request `working_dir` override so `HttpUI` requests get proper CWD jailing |
| `crates/aictl-core/src/llm/*.rs` | No change; providers are already callable from any frontend post-Modular-Architecture |
| `docs/server.md` | **New** — full API reference, request/response examples, deployment notes |
| `README.md` | Add "HTTP server" feature mention; link to `docs/server.md` |
| `ARCH.md` | New "aictl-server" section under "Workspace layout" |
| `CLAUDE.md` | Add `crates/aictl-server` to the module map |
| `ROADMAP.md` | Remove the "Server" section once Phase 2 ships |
| Website (`website/index.html`, `website/guides.html`) | "Server mode" section under Extensibility |

### 15. Testing strategy

- **Unit tests (`aictl-server/src/**`)**:
  - `auth.rs`: token present/missing, constant-time compare, loopback-vs-remote bind enforcement.
  - `ui.rs::HttpUI`: channel semantics, deny-by-default `confirm_tool`.
  - `sse.rs`: frame construction, keepalive timing (use `tokio::time::pause` for determinism).
  - `error.rs`: every `ApiError` variant maps to the documented status + code.
- **Integration tests (`crates/aictl-server/tests/`)**:
  - Spin the server on an ephemeral port with a mock LLM provider (already used by `aictl-core` tests).
  - `test_chat_non_streaming`: POST `/v1/chat`, assert answer + usage + elapsed_ms.
  - `test_chat_streaming`: POST `/v1/chat/stream`, consume SSE, assert event order and no `<tool name=` leakage in any `stream_chunk`.
  - `test_session_continuity`: two requests with the same `X-AICTL-Session`; assert second sees first's messages.
  - `test_ephemeral_session`: `memory: "ephemeral"`, verify no session file written.
  - `test_coding_mode_rejected`: request with coding-mode flag → 400.
  - `test_loopback_no_token_ok` / `test_remote_requires_token` / `test_wrong_token_401`.
  - `test_body_size_cap`: POST body larger than `server_body_limit_bytes` → 413.
  - `test_concurrent_cap`: fill the semaphore; next request → 503.
  - `test_request_timeout`: slow mock provider → 504 event / status.
  - `test_cors_off_default`: `OPTIONS` request without `server_cors_origins` set → 404, not 204.
  - `test_security_gate`: tool call rejected by `AICTL_SECURITY_DISABLED_TOOLS` surfaces as `tool_denied`.
  - `test_prompt_injection`: known-bad prompt → 400 `prompt_injection`.
  - `test_unrestricted_requires_launch_flag`: request flag without launch flag → 403.
- **Gateway integration tests (Phase 2)**:
  - `test_gateway_openai_route_to_anthropic`: OpenAI-shaped request with `model=claude-sonnet-4-6` reaches the Anthropic mock with redaction applied.
  - `test_gateway_streaming_sse_delta_format`: verify frames match OpenAI's delta shape.
  - `test_gateway_rejects_missing_model`: 400 `model_not_found`.
  - `test_gateway_rejects_tool_on_unsupported_provider`: e.g., `model=llama3` + `tools=[...]` → 400.
- **Smoke test in CI**: spin the server, send one POST, verify it works. Same spirit as the CLI smoke test.
- **Load test (manual, pre-release)**: `wrk` or `oha` against `/v1/chat` with a mock provider at 100 rps for 60s. Look for memory leaks, file-handle growth, audit-writer contention.

### 16. Deployment posture

Not strictly in scope for the plan, but worth recording:

- Binary is statically linkable on Linux with `--features vendored-tls` passthrough if rustls is ever added. For Phase 1 we assume native TLS via reverse proxy.
- `docker/Dockerfile.server` ships a minimal image: `FROM debian:stable-slim`, copy the binary, copy a blank `/home/aictl/.aictl/config` template, `EXPOSE 7878`, `ENTRYPOINT ["/usr/local/bin/aictl-server"]`. No shell inside the image.
- `systemd/aictl-server.service`: runs as `aictl:aictl`, `ProtectSystem=strict`, `ReadWritePaths=/var/lib/aictl`, `NoNewPrivileges=yes`.
- Brew formula ships `aictl-server` alongside `aictl` but in a separate bottle so CLI-only users don't drag the axum deps.

---

## Phase 2 — OpenAI gateway (detailed)

Covered in §8 above. Phase 2 is a single route module (`routes/gateway.rs`) plus a request/response translation layer (`src/openai.rs`). It reuses:

- All existing `llm::call_<provider>` functions.
- `redact_outbound` for outbound content.
- `audit::log_tool` with `gateway:<provider>` naming.
- The SSE framing helpers from Phase 1.

No new security knobs; the gateway inherits everything the agent endpoint has.

One Phase 2-specific risk: OpenAI's tool-calling schema has drifted several times (functions → tools → parallel tool calls). We target the current `tools`/`tool_choice` shape and document the supported subset. Clients using the legacy `functions` shape get a 400 with a one-line migration hint.

---

## Phase 3 — direct tool invocation + resources (outline)

- `POST /v1/tools/{name}` with a JSON body equivalent to the XML tool-body the agent produces. Security gate applies in full. Useful for clients that want to use the tool registry without running the agent loop.
- `GET /v1/tools` — catalog of every registered tool (built-in + MCP + plugin), with descriptions and input schemas.
- `POST /v1/tools/approve` — companion endpoint to Phase 1's `ToolCallPending` flow. Lets clients resume a halted tool call by explicit approval. Requires designing a pending-call state store (in-memory, TTL'd, indexed by session + pending-call ID).
- `GET /v1/resources` and `POST /v1/resources/read` — once MCP client support lands, expose MCP-sourced resources.
- `GET /v1/prompts` — likewise for MCP prompts.

Phase 3 starts only once interactive approval is a concrete client requirement. Until then, `auto: true` covers the deployment pattern (server as a trusted backend) and `auto: false` covers the "deny then halt" pattern.

---

## Phase 4 — remote catalogue + extras (outline)

- `GET /v1/agents/remote`, `GET /v1/skills/remote` — expose the curated catalogues already fetched via `aictl-core::agents::remote` and `aictl-core::skills::remote`.
- `POST /v1/agents/pull`, `POST /v1/skills/pull` — pull a named entry into `~/.aictl/agents/` or `~/.aictl/skills/`. Auth required; gated by a `server_remote_pull=false` master switch so admins can lock it down.
- Optional TLS termination via `rustls` behind `server_tls_cert` / `server_tls_key`. Operators who don't want a reverse proxy get TLS natively.
- Rate limiting via `tower_governor` or equivalent. Token-bucket per bearer token + per IP.
- `GET /metrics` Prometheus endpoint.
- Multi-config presets: `--config-preset <name>` loads `~/.aictl/configs/<name>` so a single binary can switch between pre-baked policies at launch.

---

## Risks

- **Engine assumptions about the process being interactive**: `aictl-core` should be frontend-agnostic after Modular Architecture, but a forgotten `eprintln!` or `dialoguer::` inside a provider call would corrupt SSE output or block indefinitely. The Modular Architecture plan's grep gate (Phase 1 output) prevents this; the server adds a second grep in its CI config to keep the invariant honest.
- **Shared global state across requests**: `aictl-core::config::CONFIG`, `security::POLICY`, and `session::CURRENT` are process-globals. Two concurrent requests racing on `session::set_current()` would corrupt each other's state. Mitigation: treat `session::set_current` and the other `CURRENT`-touching APIs as single-tenant-only for Phase 1; scope the session explicitly per request by passing the session ID to `run_agent_turn` and *not* calling `set_current`. This is already the cleaner design — `CURRENT` exists for the CLI's "implicit current session" ergonomics and has no place on the server hot path.
- **MCP server lifetime**: if MCP client support ships, MCP servers are per-process. The HTTP server needs to start them once and reuse across requests; spawning per-request would be absurdly slow. Call out in `docs/server.md` that MCP server processes outlive individual HTTP requests.
- **SSE through proxies**: nginx and some CDNs buffer responses by default, defeating streaming. Document the required nginx config (`proxy_buffering off`; `proxy_http_version 1.1`; `proxy_read_timeout` raised) in `docs/server.md`. Mitigation in-server: send a keepalive comment immediately after headers so proxies commit to the stream.
- **Audit log contention**: `audit::log_tool` serializes writes through a `Mutex<BufWriter<File>>`. Under high request concurrency this is a bottleneck. If it becomes a measurable problem, Phase 4 swaps to per-session writers or an async writer task with a bounded channel. Not solving in Phase 1.
- **Provider rate limits surfaced as 500s**: if Anthropic rate-limits us, the current `aictl-core::llm::call_anthropic` returns a generic error. Mitigation: extend the error enum in `aictl-core::llm` to distinguish rate-limit from other failures, so the server can respond with 429 instead of 500. Plan touches `aictl-core` minimally; prefer adding a single `AictlError::RateLimited` variant with provider + retry-after, and letting every provider populate it.
- **Coding-agent regressions**: a later coding-agent feature might implicitly rely on CLI affordances (raw-mode key listener, termimad). The Modular Architecture refactor already forbids that, but a reviewer should double-check every coding-agent patch doesn't reintroduce frontend-specific assumptions that the server can't satisfy. Mechanical check: the server's CI builds `aictl-core` without CLI deps; any such regression fails the build.
- **Binary identity**: `aictl-server --version` vs `aictl --version`. Both should report the same workspace version. Wire a shared `VERSION` constant in `aictl-core` and have both binaries read it.

## Scope boundaries with other plans

- **Modular Architecture** (`modular-architecture.md`): blocker. Server depends on Phases 3–5 having shipped. Everything in `aictl-core`'s public surface is what the server consumes.
- **Desktop** (roadmap "Desktop" section): parallel consumer of `aictl-core`. Both crates implement `AgentUI`; they share no code with each other beyond the engine. One plan does not block the other.
- **MCP** (`mcp-support.md`): the server surfaces MCP-sourced tools automatically once `aictl-core` has MCP client support. Nothing server-specific required in the MCP plan beyond making sure MCP server lifecycle is process-scoped, not request-scoped.
- **Plugins** (`plugin-system.md`): same — plugins live in `aictl-core`, the server sees them in the registry and dispatches them as it would built-in tools.
- **Coding Agent** (roadmap): deliberately excluded. The server rejects coding-mode requests with 400. If user feedback later demands coding mode over HTTP, that requires a new plan tackling tool approval, long-running workflows, and multi-turn state on the server; not Phase 1 material.

## Verification

Per-phase gate:

| Phase | Build | Lint | Test | Additional |
|-------|-------|------|------|------------|
| 1 | `cargo build --workspace` | `cargo lint --workspace` | `cargo test --workspace` | `aictl-server` starts on loopback, `/v1/chat` round-trip works, `/v1/chat/stream` delivers SSE with no tool-XML leak, all integration tests pass |
| 2 | same | same | same | OpenAI SDK pointed at the server drives a multi-provider chat successfully; redaction + audit confirmed |
| 3 | same | same | same | `/v1/tools/{name}` invokes a built-in tool directly; pending-call approval flow round-trips |
| 4 | same | same | same | remote catalogue reachable; Prometheus scrape works if enabled |

Final sign-off for Phase 1 + 2 requires:

1. `cargo build --workspace` clean on default features and `--all-features`.
2. `cargo lint --workspace` clean.
3. `cargo test --workspace` clean including every integration test in §15.
4. Grep for forbidden symbols in `aictl-server`:
   ```bash
   grep -rE 'rustyline::|termimad::|indicatif::|crossterm::|dialoguer::' crates/aictl-server/src/
   ```
   Returns empty.
5. Smoke: launch `aictl-server` against a fresh `~/.aictl/config` with a single Anthropic key, POST a chat request from `curl`, confirm the answer matches the equivalent CLI invocation.
6. Smoke (gateway): point the OpenAI Python SDK at the server, run `client.chat.completions.create(model="claude-sonnet-4-6", ...)`, confirm the reply and that redaction ran (inspect audit log).
7. Remote-bind safety: `server_bind=0.0.0.0:7878` with empty `server_token` fails startup with the documented error.
8. Shutdown: `SIGTERM` drains an in-flight streaming request within `server_shutdown_timeout`, flushes audit, exits 0.

## Open questions

- **Multi-tenancy**: should a single server instance support multiple `~/.aictl/config` presets selected per request? For v1 the answer is no — operators run one process per config. Revisit if there's demand for a per-request `X-AICTL-Config` header. Tying that into `config::CONFIG` (`OnceLock<RwLock<HashMap>>`) would require either (a) a per-request config overlay passed into every engine call, which is invasive, or (b) an external process manager that spawns one `aictl-server` per config. (b) is simpler and is the documented recommendation for Phase 1.
- **Per-request provider key override**: some deployments want "server holds no keys; client sends a provider key in a header". Plausible, but it breaks redaction's "keys are server-managed secrets" assumption and audits become harder to attribute. Defer until a concrete user asks.
- **WebSocket transport**: SSE covers streaming today. WebSockets become interesting if/when the server needs bidirectional messages beyond the agent loop (e.g., live tool-approval callbacks). Phase 3 problem.
- **Session auth vs request auth**: currently a single token grants access to all sessions. Should session IDs be treated as capabilities that anyone knowing the ID can reach? For Phase 1 yes, since the server is already behind a bearer token. Session-level auth is a Phase 4 concern tied to multi-tenancy.
- **Streaming cancellation**: the client closing its SSE connection should cancel the upstream provider call. Axum's `Sse` body exposes disconnect detection; the handler should `select!` the engine future against the body's disconnect future. Easy to implement; call out in the code review so it's not missed.
- **Sessions for gateway mode**: `/v1/chat/completions` is conversational but stateless by convention. Do we offer a stateful variant under `/v1/chat/completions` with `X-AICTL-Session`, or keep gateway mode stateless and point stateful callers at `/v1/chat`? Lean toward the latter: mixing stateful and stateless behavior on the OpenAI-shaped path would surprise SDK users.
- **Release cadence**: `aictl-server` ships lock-step with `aictl`? Almost certainly yes — both consume `aictl-core` at the same commit. Version numbers stay unified. But the release job needs two binary artifacts and two Brew bottles.
- **Telemetry opt-out**: `aictl-core` doesn't phone home today. The server shouldn't either. If analytics ever appear, they are opt-in and configured in the shared config file, not server-specific.
