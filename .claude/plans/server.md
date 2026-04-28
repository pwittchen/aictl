# Plan: `aictl-server` — HTTP LLM Proxy

## Scope

`aictl-server` is a **pure LLM proxy**. It exposes the provider catalogue over HTTP via an OpenAI-compatible gateway and nothing else. Agent functionality stays in the CLI for now.

Specifically, the server:

- **Does** expose the OpenAI-compatible gateway (`/v1/chat/completions`, `/v1/completions`, `/v1/models`) routing to every supported provider.
- **Does** apply outbound redaction and audit on the proxy path.
- **Does** require a master API key on every request (auto-generated on first startup if not configured).
- **Does** write a structured server log.
- **Does not** run the agent loop (`run::run_agent_turn`) or expose any endpoint that does.
- **Does not** dispatch tools, list/load agents or skills, manage sessions, or surface plugins/hooks/slash commands.
- **Does not** implement an `HttpUI` against `AgentUI` — there is no `AgentUI` consumer on the server side.

If a future product need calls for HTTP-accessible agent loops, that becomes a separate plan.

## Context

Today `aictl` is a CLI-only program. Every interaction goes through the REPL or a single-shot `--message` invocation, and every provider call originates from a short-lived process on the user's machine. This plan adds a second binary, `aictl-server`, that exposes the LLM provider catalogue over a local (or optionally remote) HTTP endpoint as an **OpenAI-compatible passthrough**. Clients holding an OpenAI-format SDK can point at `aictl-server` and transparently consume Anthropic, Gemini, Grok, Mistral, DeepSeek, Kimi, Z.ai, Ollama, GGUF, or MLX models, with redaction, audit, key management, and a master-key gate handled centrally.

The server reuses `~/.aictl/config` as-is so a user can run the CLI and the server side-by-side against the same provider keys, while `--config <path>` / `AICTL_CONFIG` let operators run multiple server instances with different policies. Outbound redaction (`run::redact_outbound`) and audit logging continue to apply on the proxy path.

What the server deliberately does **not** offer: the agent loop, tool dispatch, agents, skills, plugins, hooks, sessions, slash commands, or any coding-mode workflow. Those are CLI/REPL features and remain CLI/REPL-only. Clients that need them use `aictl`.

## Prerequisite: Modular Architecture

This plan depends on [.claude/plans/modular-architecture.md](modular-architecture.md) having shipped through at least the workspace split. Specifically:

- `engine` exists as a library crate with the `llm::call_<provider>` functions, `MODELS` catalogue, redaction pipeline, audit logger, key store, and stats writer as public API.
- Provider calls and the redaction pipeline are owned by `engine` and reachable from any frontend.
- Feature flags (`gguf`, `mlx`, `redaction-ner`) live on `engine`; the server enables whichever the deployment needs.

The server does **not** depend on `AgentUI` or `run::run_agent_turn` being part of the public surface — it never calls them. Until the workspace split lands, attempting to write `aictl-server` would drag REPL deps (`rustyline`/`termimad`/`crossterm`) into a daemon, which the workspace split is explicitly there to prevent.

## Goals & Non-goals

**Goals**
- Ship a second binary (`aictl-server`) from the same workspace that speaks HTTP on a configurable bind address.
- Be a pure LLM proxy: translate OpenAI-shaped requests into the configured provider's native format, call `engine::llm::call_<provider>`, translate the response back, with redaction and audit layered in.
- Keep the CLI binary (`aictl`) unchanged in behavior, dependencies, and cold-start cost. No HTTP deps leak into `cli`.
- Streaming over Server-Sent Events using OpenAI's `data: {"choices":[{"delta":...}]}` shape.
- Security-first defaults: bind `127.0.0.1` only by default, master API key required on every request, CORS off, per-request body size cap, per-request timeout.
- Master API key auto-generated and persisted to `~/.aictl/config` on first startup if not already configured.
- Structured server log written to a configurable path; redaction applied to any payload preview.
- Audit every gateway request through `audit.rs`. Stats through `stats.rs`.
- Graceful shutdown: drain in-flight requests with a timeout, flush audit/stats/log, exit.
- Multiple instances on the same host can coexist via distinct `--config <path>` and `--bind` pairs.

**Non-goals**
- **Not an agent host.** No agent-loop endpoint, no SSE streaming of `AgentEvent`s, no tool dispatch, no `<tool name="...">` filtering — none of those code paths exist on the server. Clients that need agent capabilities use the CLI.
- **No agents, skills, sessions, plugins, hooks, or slash commands.** Their files on disk are ignored by the server; their HTTP endpoints do not exist.
- **No `/v1/tools/*`** direct tool invocation. The security gate (`security::validate_tool`) is not even wired up on the server because there are no tools to validate.
- Not a Coding Agent host. Coding mode is doubly out of scope — there is no agent loop to enter coding mode on.
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

---

## Phase 1 — detailed design

### 1. Crate skeleton

Under the existing workspace (post-Modular-Architecture split):

```
crates/
├── engine/             # existing library
├── cli/                # existing CLI binary
└── server/             # NEW (package: aictl-server)
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

There is intentionally no `ui.rs`, `routes/chat.rs`, `routes/agents.rs`, `routes/skills.rs`, `routes/sessions.rs`, or `routes/tools.rs` — those would have served an agent endpoint, which is out of scope.

`crates/server/Cargo.toml` depends on `engine = { path = "../engine" }`. Runtime deps beyond what engine already pulls in: `axum`, `tower`, `tower-http` (for `RequestBodyLimit`, `TraceLayer`, `TimeoutLayer`), `tokio` (inherits features from engine), `futures`, `serde`/`serde_json` (already in engine), `async-stream` for SSE body construction. No `rustyline`, `termimad`, `crossterm`, `indicatif`, `dialoguer`.

Binary name is `aictl-server` (`[[bin]] name = "aictl-server"`). No subcommand inside `aictl`. See the roadmap section "Separate binaries vs. one binary" for the rationale.

### 2. HTTP framework choice: axum

**Axum**, for four reasons:

- Already sits in the Tokio ecosystem. No new runtime, no new async abstractions. `engine` already uses `tokio`, so axum slots in without a runtime mismatch.
- First-class SSE support (`axum::response::sse::Sse`, `KeepAlive`). The CLI's `StreamState` already produces `String` tokens; wrapping them in SSE `data:` frames is a few lines.
- Typed extractors (`Json<T>`, `State<AppState>`, `TypedHeader<Authorization<Bearer>>`) make the route signatures read like documentation.
- The same `tower::ServiceBuilder` middleware stack lets us layer on request-body limits, timeouts, auth, and tracing without hand-rolling wrappers.

Alternatives considered and rejected: **actix-web** (second runtime, doesn't compose with the existing tokio-based engine without extra care), **hyper directly** (no extractor ergonomics, every route becomes boilerplate), **rocket** (still maturing async story and a more opinionated dev experience than we want for infra code).

Pin axum to `0.8.x`. Upgrade cost is low but semver breaks between minor lines are real; lock it explicitly in `Cargo.toml`.

### 3. Configuration

Every server knob lives in `~/.aictl/config` (or the `--config <path>` override). CLI flags override config. The CLI-style `config_get`/`config_set` helpers in `engine::config` work unchanged; the server adds a narrow set of new keys:

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

The existing `engine::config` loader uses `OnceLock<RwLock<HashMap>>` and hard-codes the `~/.aictl/config` path. As part of Modular Architecture Phase 3 that loader should already take an override path; if not, adjust it before writing the server. The server plan assumes the override path is wired.

### 4. Authentication and network binding

The master API key (`server_master_key`) is required on every request **regardless of bind address**. There is no loopback-bypass. The CLI's "user owns the machine" model does not transfer here: the moment a process listens on a port, anything on the loopback interface (other users on a shared host, browser-based attacks via DNS rebinding) can reach it. A single key is the simplest defense and costs nothing.

**Master-key handling on startup** (`master_key.rs`):

1. If `--master-key <value>` is provided, use it for this launch. Do not persist it.
2. Else if `server_master_key` is set in the active config, use it.
3. Else generate a new key: 32 bytes of OS-randomness, base64url-encoded with no padding. Persist via `engine::config::config_set("server_master_key", …)` and print exactly once to stderr (and to `server_log_file`) at startup, prefixed with a clear marker:

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

### 5. Server log

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

### 6. Request concurrency and timeout

A global `tokio::sync::Semaphore` seeded with `server_max_concurrent_requests` gates every non-health route. On saturation the request returns 503 immediately rather than queueing; clients that care about backpressure handle retry themselves. The semaphore permit is held for the full request duration, including the streaming response body.

Each request gets a `tokio::time::timeout(server_request_timeout)` wrapped around the upstream provider future. On expiry the HTTP response is terminated with a 504 (or, for streams already in progress, a final SSE `error` frame with `code: "timeout"`) and the semaphore permit is released.

Per-turn provider timeouts (`AICTL_LLM_TIMEOUT`, default 30s) continue to apply inside the engine. The request timeout is the outer cap covering the full request lifecycle.

### 7. OpenAI-compatible gateway — endpoints

All request bodies are JSON unless stated otherwise. All responses are JSON or SSE (`text/event-stream`). Timestamps are RFC 3339 strings.

#### `POST /v1/chat/completions`

Accepts the standard OpenAI request schema (`messages`, `model`, `stream`, `tools`, `temperature`, `max_tokens`, `top_p`, etc.). The `model` field determines the target provider:

- Prefix or exact-match lookup against `engine::llm::MODELS` decides the provider (`Anthropic`, `OpenAI`, `Gemini`, `Grok`, `Mistral`, `DeepSeek`, `Kimi`, `Z.ai`, `Ollama`, `GGUF`, `MLX`).
- The server translates the OpenAI request into the provider's native format, calls `engine::llm::call_<provider>`, and translates the response back into the OpenAI response schema.
- For `stream: true`, SSE frames mirror OpenAI's `data: {"choices":[{"delta":...}]}` shape and the stream concludes with `data: [DONE]`.

Gateway behavior:

- **Redaction** runs on the outbound `messages[]` content via `run::redact_outbound`. Local providers (Ollama/GGUF/MLX) skip unless `AICTL_SECURITY_REDACTION_LOCAL=true`, same as the CLI.
- **Audit** logs every gateway request as `ToolCall { name: "gateway:<provider>", input: "<redacted prompt>", ... }` so the existing audit trail covers both surfaces. The session id used for audit is the per-request UUID; there is no persistent session.
- **Tool-call translation**: OpenAI's `tools` / `tool_choice` fields are passed through to providers that support them natively (Anthropic, OpenAI, Gemini). For providers without native tool support, the server rejects tool-including requests with 400. No attempt to paper over provider capability differences — that's the caller's decision. The server does **not** execute the returned tool calls; it returns them to the client just like the upstream provider would. Tool execution is the client's responsibility.
- **`Authorization: Bearer sk-...`** header from the client is *not* forwarded to the upstream provider. The server substitutes its own configured key. Clients send the aictl server master key in `Authorization`; the server picks the right upstream key from `~/.aictl/config`.

#### `POST /v1/completions`

Legacy text-completion API. Same translation rules as `/v1/chat/completions`. Providers that have only chat APIs (Anthropic, Gemini, etc.) get the prompt wrapped into a single user message; the translation rule is documented in `docs/server.md`.

#### `GET /v1/models`

Lists every model from `engine::llm::MODELS` plus any locally available Ollama / GGUF / MLX models (same detection as the CLI's `--list-models`).

```json
{
  "object": "list",
  "data": [
    {
      "id": "claude-opus-4-7",
      "object": "model",
      "owned_by": "Anthropic",
      "context_window": 1000000,
      "available": true
    },
    { "id": "gpt-4.1", "object": "model", "owned_by": "OpenAI", "context_window": 1047576, "available": true },
    { "id": "llama3.1:latest", "object": "model", "owned_by": "Ollama", "context_window": 131072, "available": true }
  ]
}
```

The shape stays close to OpenAI's `/v1/models` so SDKs that introspect the catalogue keep working. `available` is an aictl-specific extension; SDKs that don't know about it ignore the field.

#### `GET /healthz`

No auth. Returns `{"status":"ok","version":"...","uptime_secs":...,"active_requests":N}`. Used by container orchestrators and external monitors.

#### `GET /v1/stats`

Authenticated. Returns `engine::stats` aggregates — tokens per provider, request counts, estimated spend — for the configured config's stats file.

### 8. Security gate, redaction, and audit

The server is a thin HTTP transport over `engine::llm::*` with redaction and audit layered in. There is no `security::validate_tool` call site here because the server does not dispatch tools — tool calls returned by upstream providers go straight back to the client.

- **Redaction**: `run::redact_outbound` runs right before provider dispatch on every gateway request. Local providers (Ollama/GGUF/MLX) skip unless `AICTL_SECURITY_REDACTION_LOCAL=true`.
- **Audit**: every gateway dispatch is logged via `audit::log_tool` as `gateway:<provider>` with the per-request UUID as the session id. Audit always runs, even with redaction in pass-through mode.
- **CWD jail**: not relevant. There is no working-directory-scoped tool execution.
- **Prompt-injection guard**: `detect_prompt_injection` runs on every incoming `messages[]` body. On match, the request is rejected with 400 `code: "prompt_injection"`. This protects against poisoned prompts being forwarded upstream on the operator's billing.

### 9. Errors

One typed error enum maps to HTTP status codes:

```rust
pub enum ApiError {
    BadRequest { code: &'static str, message: String },        // 400
    Unauthorized,                                               // 401
    Forbidden { reason: &'static str },                         // 403
    NotFound { what: &'static str },                            // 404
    PayloadTooLarge { limit: u64 },                             // 413
    UnprocessableEntity { code: &'static str, message: String },// 422
    TooManyRequests,                                            // 429 (reserved for Phase 2 rate limits)
    InternalError { trace_id: String },                         // 500
    ServiceUnavailable { reason: &'static str },                // 503
    GatewayTimeout,                                             // 504
}
```

Every error body is `{"error": {"code": "...", "message": "..."}}` — stable machine-parseable shape that mirrors OpenAI's error envelope so SDK error handlers keep working. `code` values are documented in `docs/server.md`.

Stable error codes for the gateway:
- `prompt_injection`, `provider_unavailable`, `provider_rate_limited`, `model_not_found`, `redaction_blocked`, `body_too_large`, `body_malformed`, `tools_unsupported_for_provider`, `auth_missing`, `auth_invalid`.

Internal errors log the full error with a generated trace ID; the response surfaces only the trace ID for the operator to correlate.

### 10. Graceful shutdown

SIGINT / SIGTERM triggers:

1. Stop accepting new connections (`axum_server::Handle::graceful_shutdown`).
2. Wait up to `server_shutdown_timeout` seconds for in-flight requests to complete.
3. On timeout, cancel remaining requests with a final `error` SSE frame (for streams) or 503 (for non-streaming).
4. Flush audit / stats / log writers (file-backed with `BufWriter`; explicit flush to be safe).
5. Release provider HTTP clients (drops `reqwest::Client`).
6. Exit 0.

A `/v1/shutdown` admin endpoint is deferred. Operators signal the process.

### 11. Observability

Structured logging via `tracing` + `tracing-subscriber` with the `json` feature for production, `fmt` for dev. Fields on every request span: `method`, `path`, `status`, `elapsed_ms`, `request_id`, `model` (if any), `provider` (if any). Log levels controlled by `server_log_level`.

Metrics are out of scope for Phase 1 (no Prometheus endpoint). If operators ask for it, Phase 2 adds `GET /metrics` via `metrics-exporter-prometheus`. Until then, `/v1/stats` covers the LLM-specific telemetry, and request-level metrics come out of structured logs.

### 12. Integration points

| File / location | Change |
|-----------------|--------|
| `crates/server/` | **New** — entire crate per the skeleton in §1 |
| `Cargo.toml` (workspace root) | Add `crates/server` to `members` |
| `crates/engine/src/config.rs` | If not already done in Modular Architecture, accept an optional config-path argument at init time |
| `crates/engine/src/llm/*.rs` | No change; providers are already callable from any frontend post-Modular-Architecture |
| `crates/engine/src/llm/mod.rs` | Confirm `MODELS` and `call_<provider>` symbols are `pub`. May need to expose a small `provider_for_model(&str) -> Option<Provider>` helper if one doesn't already exist |
| `crates/engine/src/run.rs` | Confirm `redact_outbound` is reachable as a public symbol. No agent-loop changes |
| `docs/server.md` | **New** — full API reference, request/response examples, deployment notes, OpenAI-shape mapping per provider |
| `README.md` | Add "HTTP server" feature mention; link to `docs/server.md` |
| `ARCH.md` | New "aictl-server" section under "Workspace layout" |
| `CLAUDE.md` | Add `crates/server` to the module map |
| `ROADMAP.md` | Remove the "Server" section once Phase 1 ships |
| Website (`website/index.html`, `website/guides.html`) | "Server mode" section under Extensibility |

### 13. Testing strategy

- **Unit tests (`crates/server/src/**`)**:
  - `auth.rs`: token present/missing, constant-time compare, missing-header behavior.
  - `master_key.rs`: load existing, generate-and-persist on first run, `--master-key` precedence.
  - `openai.rs`: round-trip translation per provider for `/v1/chat/completions` request and response shapes; tool-call passthrough where supported, 400 where not.
  - `sse.rs`: frame construction matches OpenAI delta shape; keepalive timing (use `tokio::time::pause` for determinism); final `[DONE]` frame.
  - `error.rs`: every `ApiError` variant maps to the documented status + code, OpenAI-shaped envelope.
- **Integration tests (`crates/server/tests/`)**:
  - Spin the server on an ephemeral port with a mock LLM provider (already used by `engine` tests).
  - `test_chat_completions_non_streaming`: POST `/v1/chat/completions` with `stream: false`, assert response shape matches OpenAI schema.
  - `test_chat_completions_streaming`: POST with `stream: true`, consume SSE, assert `data: {...}` deltas and final `data: [DONE]`.
  - `test_models_lists_catalogue`: `GET /v1/models` returns the engine catalogue plus locally detected models.
  - `test_auth_missing_header_401` / `test_auth_wrong_token_401` / `test_auth_correct_token_200`.
  - `test_master_key_generated_on_first_start`: empty config, server starts, key is persisted, second launch reuses it.
  - `test_body_size_cap`: POST body larger than `server_body_limit_bytes` → 413.
  - `test_concurrent_cap`: fill the semaphore; next request → 503.
  - `test_request_timeout`: slow mock provider → 504 (or terminating SSE error frame).
  - `test_cors_off_default`: `OPTIONS` request without `server_cors_origins` set → 404, not 204.
  - `test_prompt_injection`: known-bad prompt → 400 `prompt_injection`.
  - `test_redaction_runs_outbound`: prompt containing a fake API key reaches the mock provider with the key redacted; audit log records the redacted form.
  - `test_gateway_routes_to_anthropic`: OpenAI-shaped request with `model=claude-sonnet-4-6` reaches the Anthropic mock.
  - `test_gateway_rejects_missing_model`: 400 `model_not_found`.
  - `test_gateway_rejects_tools_on_unsupported_provider`: e.g., `model=llama3` + `tools=[...]` → 400 `tools_unsupported_for_provider`.
- **Smoke test in CI**: spin the server, send one POST `/v1/chat/completions`, verify it works against a mock provider.
- **Load test (manual, pre-release)**: `wrk` or `oha` against `/v1/chat/completions` with a mock provider at 100 rps for 60s. Look for memory leaks, file-handle growth, audit-writer contention.

### 14. Deployment posture

Not strictly in scope for the plan, but worth recording:

- Binary is statically linkable on Linux with `--features vendored-tls` passthrough if rustls is ever added. For Phase 1 we assume native TLS via reverse proxy.
- `docker/Dockerfile.server` ships a minimal image: `FROM debian:stable-slim`, copy the binary, copy a blank `/home/aictl/.aictl/config` template, `EXPOSE 7878`, `ENTRYPOINT ["/usr/local/bin/aictl-server"]`. No shell inside the image.
- `systemd/aictl-server.service`: runs as `aictl:aictl`, `ProtectSystem=strict`, `ReadWritePaths=/var/lib/aictl`, `NoNewPrivileges=yes`.
- Brew formula ships `aictl-server` alongside `aictl` but in a separate bottle so CLI-only users don't drag the axum deps.

---

## Phase 2 — operational hardening (outline)

- Optional TLS termination via `rustls` behind `server_tls_cert` / `server_tls_key`. Operators who don't want a reverse proxy get TLS natively.
- Rate limiting via `tower_governor` or equivalent. Token-bucket per bearer token + per IP. Surfaces as `429 too_many_requests`.
- `GET /metrics` Prometheus endpoint via `metrics-exporter-prometheus`. Emits per-request latency histograms, per-provider request counters, in-flight gauge.
- Multi-config presets: `--config-preset <name>` loads `~/.aictl/configs/<name>` so a single binary can switch between pre-baked policies at launch.

---

## Phase 3 — remote provider key passthrough (outline)

Re-evaluate whether to support a per-request provider key override. Two shapes considered:

- **Header passthrough**: client sends `X-Provider-Authorization: Bearer sk-...`; server forwards verbatim to the upstream provider, ignoring the configured key. Server still requires its master key in `Authorization`.
- **Per-request body field**: a `provider_key` field in the request body. Same effect, less header juggling.

Either shape breaks redaction's "keys are server-managed secrets" assumption (the redactor's regex bank includes patterns that would match the forwarded key) and complicates audit attribution. Defer until a concrete user asks. If shipped, document that audit records contain only the prefix of forwarded keys, never the full value.

---

## Risks

- **Engine assumptions about the process being interactive**: `engine` should be frontend-agnostic after Modular Architecture, but a forgotten `eprintln!` or `dialoguer::` inside a provider call would corrupt SSE output or block indefinitely. The Modular Architecture plan's grep gate (Phase 1 output) prevents this; the server adds a second grep in its CI config to keep the invariant honest.
- **Shared global state across requests**: `engine::config::CONFIG` and `security::POLICY` are process-globals. The gateway path doesn't touch `session::CURRENT` (no sessions on the server), so the worst race risk is around config mutation. Mitigation: treat `config_set` as launch-time-only on the server; do not expose any endpoint that mutates config.
- **SSE through proxies**: nginx and some CDNs buffer responses by default, defeating streaming. Document the required nginx config (`proxy_buffering off`; `proxy_http_version 1.1`; `proxy_read_timeout` raised) in `docs/server.md`. Mitigation in-server: send a keepalive comment immediately after headers so proxies commit to the stream.
- **Audit log contention**: `audit::log_tool` serializes writes through a `Mutex<BufWriter<File>>`. Under high request concurrency this is a bottleneck. If it becomes a measurable problem, Phase 2 swaps to per-session writers or an async writer task with a bounded channel. Not solving in Phase 1.
- **Provider rate limits surfaced as 500s**: if Anthropic rate-limits us, the current `engine::llm::call_anthropic` returns a generic error. Mitigation: extend the error enum in `engine::llm` to distinguish rate-limit from other failures, so the server can respond with `429 provider_rate_limited` instead of 500. Plan touches `engine` minimally; prefer adding a single `EngineError::RateLimited` variant with provider + retry-after, and letting every provider populate it.
- **OpenAI schema drift**: OpenAI's tool-calling schema has changed several times (functions → tools → parallel tool calls). We target the current `tools`/`tool_choice` shape and document the supported subset. Clients using the legacy `functions` shape get a 400 with a one-line migration hint.
- **Streaming cancellation**: the client closing its SSE connection should cancel the upstream provider call. Axum's `Sse` body exposes disconnect detection; the handler should `select!` the upstream future against the body's disconnect future. Easy to implement; call out in code review so it's not missed.
- **Binary identity**: `aictl-server --version` vs `aictl --version`. Both should report the same workspace version. Wire a shared `VERSION` constant in `engine` and have both binaries read it.

## Scope boundaries with other plans

- **Modular Architecture** (`modular-architecture.md`): blocker. Server depends on the workspace split having shipped.
- **Desktop** (roadmap "Desktop" section): parallel consumer of `engine`. The desktop frontend implements `AgentUI`; the server does not. They share only `engine`'s LLM surface.
- **MCP** (`mcp-support.md`): no overlap. The server does not run the agent loop, so MCP tools never surface here. MCP stays a CLI feature.
- **Plugins** (`plugin-system.md`): no overlap. Plugins are tools; the server does not dispatch tools.
- **Coding Agent** (roadmap): no overlap. The server has no agent loop, so coding mode is structurally absent rather than rejected.

If a future plan calls for HTTP-accessible agent loops, that's a new plan (likely "aictl-agent-server" or similar), with its own auth model, session story, tool-approval protocol, and `HttpUI` against `AgentUI`. It would not be a revival of an earlier draft of this document.

## Verification

Per-phase gate:

| Phase | Build | Lint | Test | Additional |
|-------|-------|------|------|------------|
| 1 | `cargo build --workspace` | `cargo lint --workspace` | `cargo test --workspace` | `aictl-server` starts on loopback, OpenAI SDK pointed at the server drives a multi-provider chat successfully; redaction + audit confirmed; all integration tests pass |
| 2 | same | same | same | TLS termination works end-to-end; rate-limit returns 429 under burst; Prometheus scrape works |
| 3 | same | same | same | Per-request provider key passthrough works; audit logs the prefix only |

Final sign-off for Phase 1 requires:

1. `cargo build --workspace` clean on default features and `--all-features`.
2. `cargo lint --workspace` clean.
3. `cargo test --workspace` clean including every integration test in §13.
4. Grep for forbidden symbols in `crates/server`:
   ```bash
   grep -rE 'rustyline::|termimad::|indicatif::|crossterm::|dialoguer::' crates/server/src/
   ```
   Returns empty.
5. Grep for agent-loop entry points in `crates/server`:
   ```bash
   grep -rE 'run_agent_turn|run_agent_single|AgentUI|ToolApproval' crates/server/src/
   ```
   Returns empty. The server must not reach into the agent loop.
6. Smoke: launch `aictl-server` against a fresh `~/.aictl/config` with a single Anthropic key, point the OpenAI Python SDK at the server, run `client.chat.completions.create(model="claude-sonnet-4-6", ...)`, confirm the reply and that redaction ran (inspect audit log).
7. Master-key generation: launch with no `server_master_key` configured, confirm key is generated, printed once, persisted to config, and a second launch reuses it silently.
8. Shutdown: `SIGTERM` drains an in-flight streaming request within `server_shutdown_timeout`, flushes audit + log, exits 0.

## Open questions

- **Multi-tenancy**: should a single server instance support multiple `~/.aictl/config` presets selected per request? For v1 the answer is no — operators run one process per config. Revisit if there's demand for a per-request `X-AICTL-Config` header. Tying that into `config::CONFIG` (`OnceLock<RwLock<HashMap>>`) would require either (a) a per-request config overlay passed into every engine call, which is invasive, or (b) an external process manager that spawns one `aictl-server` per config. (b) is simpler and is the documented recommendation for Phase 1.
- **Per-request provider key override**: covered by Phase 3 above. Defer until a concrete user asks.
- **WebSocket transport**: SSE covers streaming today. WebSockets become interesting if/when the server needs bidirectional messages (e.g., live tool-approval callbacks), but tool execution is out of scope, so this is unlikely to surface for the proxy-only server.
- **Session-style continuity for the gateway**: `/v1/chat/completions` is conversational but stateless by convention — the client owns the message history. We keep it stateless. If a stateful variant is ever needed, that's the same line of thinking as agent-over-HTTP and would belong to a separate plan.
- **Release cadence**: `aictl-server` ships lock-step with `aictl`. Both consume `engine` at the same commit; version numbers stay unified. The release job needs two binary artifacts and two Brew bottles.
- **Telemetry opt-out**: `engine` doesn't phone home today. The server shouldn't either. If analytics ever appear, they are opt-in and configured in the shared config file, not server-specific.
