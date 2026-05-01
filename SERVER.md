# SERVER.md

`aictl-server` is an HTTP **LLM proxy**. It exposes the same provider catalogue that the CLI ships behind one **OpenAI-compatible** endpoint so any client that already speaks the OpenAI SDK can transparently call Anthropic, Gemini, Grok, Mistral, DeepSeek, Kimi, Z.ai, Ollama, GGUF, or MLX models.

Pure proxy. No agent loop, no tool dispatch, no agents/skills/sessions, no slash commands. Those are CLI-only and stay CLI-only — see [README.md](README.md) and [ARCH.md](ARCH.md). For agent capabilities over HTTP, use the CLI.

## Scope

**Does**

- Expose `POST /v1/chat/completions`, `POST /v1/completions`, `GET /v1/models`, `GET /v1/stats`, `GET /healthz`.
- Translate OpenAI-shaped requests into each provider's native format and back.
- Apply outbound redaction (`run::redact_outbound`) on every gateway request.
- Apply the prompt-injection guard (`security::detect_prompt_injection`) on every user message.
- Audit every gateway dispatch via `audit::log_tool` as `gateway:<provider>`.
- Require a master API key on every authenticated request (auto-generated on first launch if not configured).
- Stream responses over Server-Sent Events in OpenAI's `data: {"choices":[{"delta":...}]}` shape.
- Apply a global in-flight concurrency cap (`AICTL_SERVER_MAX_CONCURRENT_REQUESTS`) and an optional per-client-IP token-bucket rate limit (`AICTL_SERVER_RATE_LIMIT_RPM` / `AICTL_SERVER_RATE_LIMIT_BURST`).

**Does not**

- Run the agent loop or expose any endpoint that does.
- Dispatch tools. Tool calls returned by upstream providers are not executed server-side; the security gate (`security::validate_tool`) is not wired up because there is nothing to validate.
- Surface agents, skills, sessions, plugins, hooks, or slash commands.
- Terminate TLS in v1. Run nginx/Caddy in front for HTTPS.

## Install

### One-liner

```sh
curl -fsSL https://aictl.app/server/install.sh | sh
```

The installer detects the platform (macOS arm64/x86_64, Linux x86_64/arm64), downloads the matching binary from the latest GitHub release, drops it on `$PATH`, and prints next-step guidance. It is idempotent — re-running upgrades in place.

### From source

```sh
cargo install --git https://github.com/pwittchen/aictl.git --bin aictl-server
```

### From a cloned repo

If you've already cloned the repo locally and want to build from your working tree (the usual workflow when developing or testing a patch):

```sh
# Run directly without installing — handy while iterating.
cargo run --release --bin aictl-server

# Build the release binary; lands at target/release/aictl-server.
cargo build --release --bin aictl-server

# Install the workspace binary to ~/.cargo/bin so it's on $PATH.
cargo install --path crates/aictl-server

# With optional features (mirrors the CLI's feature flags).
cargo install --path crates/aictl-server --features "gguf mlx redaction-ner"
```

`cargo install --path` puts the binary at `~/.cargo/bin/aictl-server`. Make sure `~/.cargo/bin` is on your `$PATH` (rustup adds it by default). To uninstall: `aictl-server --uninstall` (removes the binary from `~/.cargo/bin/`, `~/.local/bin/`, `/usr/local/bin/`, and `$AICTL_INSTALL_DIR`; leaves `~/.aictl/` untouched), or `cargo uninstall aictl-server` for a cargo-managed install.

### First launch

```sh
aictl-server
```

On first launch the server generates a 32-byte master API key, persists it to `~/.aictl/config` as `AICTL_SERVER_MASTER_KEY`, and prints it once to stderr. Copy it — you'll need it on every request.

### Launch via the CLI shortcut

If you have both binaries installed, `aictl --serve` is a convenience shortcut that locates `aictl-server` and execs it. Trailing args are forwarded to the server:

```sh
aictl --serve                                # default 127.0.0.1:7878
aictl --serve -- --bind 0.0.0.0:7878 --quiet # forward server flags after `--`
```

Resolution order for the server binary: a sibling of the current `aictl` executable, then `$PATH`, then `~/.cargo/bin/`, then `~/.local/bin/`, then `$AICTL_INSTALL_DIR`. If nothing is found, the CLI prints a clear "not installed" message with the install one-liner.

## Usage

```sh
# Server runs on http://127.0.0.1:7878 by default.
aictl-server

# In another terminal:
curl http://127.0.0.1:7878/v1/chat/completions \
  -H "Authorization: Bearer $AICTL_SERVER_MASTER_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "claude-sonnet-4-6",
    "messages": [{"role": "user", "content": "Hello"}]
  }'
```

### From the OpenAI Python SDK

```python
from openai import OpenAI

client = OpenAI(
    api_key="<AICTL_SERVER_MASTER_KEY>",
    base_url="http://127.0.0.1:7878/v1",
)

reply = client.chat.completions.create(
    model="claude-sonnet-4-6",
    messages=[{"role": "user", "content": "Hello"}],
)
print(reply.choices[0].message.content)
```

The same SDK call works against any model in `GET /v1/models` — the server picks the right upstream provider and substitutes its own configured key.

### Connecting `aictl-cli` to `aictl-server`

The CLI can route every non-local LLM call through this server instead of holding upstream provider keys itself. Set two values in the *CLI's* `~/.aictl/config`:

```ini
AICTL_CLIENT_HOST=http://127.0.0.1:7878
AICTL_CLIENT_MASTER_KEY=<value of AICTL_SERVER_MASTER_KEY from the server>
```

Or pass them per-launch without persisting:

```sh
aictl --client-url http://127.0.0.1:7878 --client-master-key sk-aictl-…
```

The `AICTL_CLIENT_*` vs `AICTL_SERVER_*` split is deliberate: a single host may run both roles, and the CLI side stores the *connection* key (what it presents to *some* server) under a name distinct from the server's *own* master key. Locking via `/keys` moves `AICTL_CLIENT_MASTER_KEY` into the OS keyring just like any provider key. Local providers (Ollama / GGUF / MLX) always bypass the server. `/balance` reads the server's `/v1/stats` aggregate when routing is active.

## Configuration

`aictl-server` reads the same `~/.aictl/config` file the CLI reads. Server-only knobs are prefixed `AICTL_SERVER_*` and sit alongside the existing CLI keys.

| Key | Default | Description |
|-----|---------|-------------|
| `AICTL_SERVER_BIND` | `127.0.0.1:7878` | Bind address. Non-loopback values still require the master key; a startup warning is printed. |
| `AICTL_SERVER_MASTER_KEY` | _(auto-generated)_ | Bearer token required on every authenticated request. Auto-generated on first launch and persisted here. |
| `AICTL_SERVER_REQUEST_TIMEOUT` | `120` | Per-request wall-clock timeout (seconds). `0` disables. |
| `AICTL_SERVER_BODY_LIMIT_BYTES` | `2097152` | Per-request body cap (2 MiB). |
| `AICTL_SERVER_MAX_CONCURRENT_REQUESTS` | `32` | Global concurrency semaphore. Returns 503 when saturated. |
| `AICTL_SERVER_SHUTDOWN_TIMEOUT` | `20` | Drain grace period on SIGTERM (seconds). |
| `AICTL_SERVER_SSE_KEEPALIVE` | `15` | SSE keepalive comment interval (seconds). `0` disables. |
| `AICTL_SERVER_LOG_LEVEL` | `info` | `trace`/`debug`/`info`/`warn`/`error`. |
| `AICTL_SERVER_LOG_FILE` | `~/.aictl/server.log` | JSON-Lines log file. Empty disables the file sink (terminal sink stays on). |
| `AICTL_SERVER_LOG_BODIES` | `true` | Log redacted request/response bodies. `false` drops body lines at the source. |
| `AICTL_SERVER_CORS_ORIGINS` | _(empty)_ | Comma-separated origin list. Empty = CORS off. |
| `AICTL_SERVER_RATE_LIMIT_RPM` | `0` | Per-client-IP requests per minute. `0` disables (only the global concurrency cap applies). |
| `AICTL_SERVER_RATE_LIMIT_BURST` | `0` | Token-bucket capacity (max consecutive requests). `0` falls back to the RPM value, so the bucket holds one minute of tokens. |

Provider keys (`LLM_OPENAI_API_KEY`, `LLM_ANTHROPIC_API_KEY`, …) live under their existing CLI names — the server reads them via `keys::get_secret`, so keyring-stored keys work the same as plain-text fallback.

### Server-scoped security and redaction

The server can run a different security / redaction posture than the CLI on the same host without forking `~/.aictl/config`. For every flag that makes sense in a pure HTTP proxy, an `AICTL_SERVER_*` form takes precedence over the matching `AICTL_*` form when the engine is loaded inside `aictl-server`. Unset server overrides fall through to the shared key, so a single-host setup needs no duplication.

Tool-dispatch knobs (CWD jail, shell allow/block lists, blocked env vars, disabled tools, max-write byte cap, shell timeout) are intentionally **not** mirrored: the server does not run tools, so those flags have no meaning here.

| Server key | Falls back to | Default | Description |
|-----|-----|-----|-----|
| `AICTL_SERVER_SECURITY` | `AICTL_SECURITY` | `true` | Master enable for the security subsystem (the prompt-injection guard + audit). `false`/`0` turns it off entirely. |
| `AICTL_SERVER_SECURITY_INJECTION_GUARD` | `AICTL_SECURITY_INJECTION_GUARD` | `true` | Run `detect_prompt_injection` on every user message before dispatch. |
| `AICTL_SERVER_SECURITY_AUDIT_LOG` | `AICTL_SECURITY_AUDIT_LOG` | `true` | Append `gateway:<provider>` entries to `~/.aictl/audit/<request-id>`. |
| `AICTL_SERVER_SECURITY_REDACTION` | `AICTL_SECURITY_REDACTION` | `off` | `off` / `redact` / `block`. `redact` rewrites detected secrets in-place; `block` returns 400 `redaction_blocked`. |
| `AICTL_SERVER_SECURITY_REDACTION_LOCAL` | `AICTL_SECURITY_REDACTION_LOCAL` | `false` | When `false`, local-provider dispatches (Ollama / GGUF / MLX from the server's host) skip the redaction pass. Set `true` to enforce redaction even on in-host traffic. |
| `AICTL_SERVER_REDACTION_DETECTORS` | `AICTL_REDACTION_DETECTORS` | _(empty = all)_ | Comma-separated subset of `api_key, aws, jwt, private_key, connection_string, credit_card, iban, email, phone, high_entropy`. |
| `AICTL_SERVER_REDACTION_EXTRA_PATTERNS` | `AICTL_REDACTION_EXTRA_PATTERNS` | _(empty)_ | Semicolon-separated `NAME=REGEX` pairs → rewritten as `[REDACTED:NAME]`. |
| `AICTL_SERVER_REDACTION_ALLOW` | `AICTL_REDACTION_ALLOW` | _(empty)_ | Semicolon-separated allowlist regexes — matches survive Layer-A/B redaction. |
| `AICTL_SERVER_REDACTION_NER` | `AICTL_REDACTION_NER` | `false` | Enable Layer-C NER. Requires the `redaction-ner` cargo feature plus a pulled model. |
| `AICTL_SERVER_REDACTION_NER_MODEL` | `AICTL_REDACTION_NER_MODEL` | `onnx-community/gliner_small-v2.1` | NER model name (or `owner/repo`). The server can ship a different model from the CLI without forking config. |

### CLI flags

| Flag | Description |
|------|-------------|
| `--bind <addr:port>` | Override `AICTL_SERVER_BIND`. |
| `--master-key <value>` | Use this key for this launch only (not persisted). |
| `--quiet` | Suppress startup banner. |
| `--log-level <level>` | Override `AICTL_SERVER_LOG_LEVEL`. |
| `--log-file <path>` | Override `AICTL_SERVER_LOG_FILE`. |

`--unrestricted` is intentionally absent — the server does not dispatch tools, so there is nothing to gate.

## Master-key handling

1. `--master-key <value>` wins for the current launch (not persisted).
2. Otherwise the persisted `AICTL_SERVER_MASTER_KEY` is used.
3. Otherwise 32 bytes of OS randomness are generated, base64url-encoded, persisted to `~/.aictl/config`, and printed once to stderr and the structured log.

Rotate by editing `~/.aictl/config` (set or remove the entry — removal causes the next launch to regenerate). Comparison at the auth boundary is constant-time.

## REST API

Every authenticated request must carry `Authorization: Bearer <master-key>`. Unauthenticated requests get a `401` with an OpenAI-shaped error envelope. `GET /healthz` is the only auth-free route.

### `POST /v1/chat/completions`

OpenAI-shaped request schema. The `model` field selects the provider — exact match against the catalogue from `GET /v1/models`. `stream: true` returns SSE.

Request:

```json
{
  "model": "claude-sonnet-4-6",
  "messages": [{"role": "user", "content": "Hello"}],
  "stream": false
}
```

Response (`stream: false`):

```json
{
  "id": "chatcmpl-…",
  "object": "chat.completion",
  "created": 1714411200,
  "model": "claude-sonnet-4-6",
  "choices": [{
    "index": 0,
    "message": {"role": "assistant", "content": "Hi!"},
    "finish_reason": "stop"
  }],
  "usage": {"prompt_tokens": 8, "completion_tokens": 2, "total_tokens": 10}
}
```

Streaming response (`stream: true`):

```
data: {"id":"chatcmpl-…","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"role":"assistant"}}]}

data: {"id":"chatcmpl-…","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"content":"Hi"}}]}

data: {"id":"chatcmpl-…","object":"chat.completion.chunk","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}

data: [DONE]

```

**Tool-calling passthrough** — not implemented in this phase. Requests with a non-empty `tools` array, a non-`null` non-`"none"` `tool_choice`, or any legacy `functions` field get a `400 tools_unsupported_for_provider`.

### `POST /v1/completions`

Legacy text-completion API. The prompt is wrapped into a single user message and routed through the same provider-selection logic.

```json
{
  "model": "gpt-4o-mini",
  "prompt": "Once upon a time"
}
```

### `GET /v1/models`

Lists every model from `aictl_core::llm::MODELS` plus locally available Ollama / GGUF / MLX models. The `available` field is `true` when the upstream API key is configured (or, for local providers, when the model file is present).

### `GET /healthz`

No auth. Returns `{"status":"ok","version":"…","uptime_secs":…,"active_requests":N}`.

### `GET /v1/stats`

Authenticated. Returns the `aictl_core::stats` aggregates (today / month / overall).

## Errors

Every error response is `{"error": {"code": "…", "message": "…"}}` — the same shape OpenAI uses, so SDK error handlers keep working.

| HTTP | Error code | Cause |
|------|------------|-------|
| 400 | `prompt_injection` | The prompt-injection guard tripped. |
| 400 | `redaction_blocked` | Outbound message contained sensitive data (block mode). |
| 400 | `model_not_found` | No provider knows how to serve the requested model. |
| 400 | `body_malformed` | Request body did not match the expected schema. |
| 400 | `tools_unsupported_for_provider` | Tool-calling fields are not supported in this phase. |
| 401 | `auth_invalid` | Missing or wrong `Authorization: Bearer` header. |
| 403 | `provider_auth_failed` | Upstream provider rejected the substituted key. |
| 413 | `body_too_large` | Request body exceeded `AICTL_SERVER_BODY_LIMIT_BYTES`. |
| 429 | `rate_limited` | Per-IP token bucket exhausted (set via `AICTL_SERVER_RATE_LIMIT_RPM`). Response carries `Retry-After: <seconds>`. |
| 503 | `provider_unavailable` | Upstream provider failed (5xx, empty response, stream error). |
| 503 | `provider_key_not_configured` | No API key for the resolved provider in `~/.aictl/config`. |
| 503 | `concurrency_cap_reached` | Global semaphore saturated. |
| 504 | `gateway_timeout` | Per-request timeout expired. |

## Security model

- **Master-key gate**: every authenticated request must present `Authorization: Bearer <master-key>`. Comparison is constant-time; both wrong-token and missing-token map to the same 401 body.
- **Network bind**: defaults to `127.0.0.1`. Non-loopback binds emit a startup warning. Operators are responsible for putting TLS in front when exposing beyond localhost.
- **CORS**: off by default. Set `AICTL_SERVER_CORS_ORIGINS` to opt in.
- **Body cap**: 2 MiB by default; oversized bodies get 413.
- **Concurrency cap**: 32 in-flight requests by default; saturated cap returns 503 immediately rather than queueing.
- **Rate limit (optional)**: per-client-IP token bucket via `AICTL_SERVER_RATE_LIMIT_RPM` and `AICTL_SERVER_RATE_LIMIT_BURST`. Off by default. Saturation returns 429 with a `Retry-After: <seconds>` header. Buckets are keyed by the request's source IP (read from the socket — `X-Forwarded-*` is not trusted), so the limiter only behaves as expected when the server is reached directly. Behind a reverse proxy every request appears to come from the proxy's IP — terminate the limit at the proxy or trust the proxy to set its own.
- **Redaction**: `aictl_core::run::redact_outbound` runs on every gateway request, with the same regex bank, entropy pass, and optional NER as the CLI. Local providers (Ollama/GGUF/MLX) skip unless `AICTL_SECURITY_REDACTION_LOCAL=true`.
- **Prompt-injection guard**: `aictl_core::security::detect_prompt_injection` runs on every user message; matches surface as 400 `prompt_injection` so poisoned prompts can't burn the operator's tokens.
- **Audit**: every dispatch is logged as `gateway:<provider>` to `~/.aictl/audit/` via `audit::log_tool`, with the per-request UUID as the session id.

The master key grants full proxy access — there is no second tier of credentials. Rotate by editing the config file.

## Rate limiting

Two layers cooperate:

1. **Global concurrency cap** — `AICTL_SERVER_MAX_CONCURRENT_REQUESTS` (default `32`). A `tokio::Semaphore` bounds in-flight requests. Saturation returns 503 immediately rather than queueing. Always on.
2. **Per-client-IP token bucket** — opt-in via `AICTL_SERVER_RATE_LIMIT_RPM` (`0` disables, the default). Each unique client IP gets its own bucket; saturation returns 429 with a `Retry-After: <seconds>` header.

### Configuration

| Knob | Meaning |
|------|---------|
| `AICTL_SERVER_RATE_LIMIT_RPM` | Steady-state requests per minute per IP. `0` disables. |
| `AICTL_SERVER_RATE_LIMIT_BURST` | Bucket capacity (max consecutive requests). `0` falls back to RPM, so the bucket holds one minute of tokens. |

Tokens refill linearly at `RPM / 60` per second. The bucket starts full so the first burst is allowed up to the configured capacity.

### Examples

```ini
# 60 requests/min sustained; bucket capacity defaults to RPM (60 tokens),
# so an idle client can fire 60 immediate requests, then drips at 1/s.
AICTL_SERVER_RATE_LIMIT_RPM=60

# 600 requests/min sustained, but limit any single burst to 50 consecutive
# requests (tighter than the default 600-token capacity for this RPM).
AICTL_SERVER_RATE_LIMIT_RPM=600
AICTL_SERVER_RATE_LIMIT_BURST=50
```

### Behavior

- The limiter sits **behind** the auth gate, so unauthenticated traffic gets `401` *without* burning a bucket entry.
- `GET /healthz` is exempt entirely — it sits outside both auth and rate-limit middleware so liveness probes stay free.
- Buckets are keyed by the request's source IP (read from the socket — `X-Forwarded-*` is **not** trusted). Behind a reverse proxy every request appears to come from the proxy's IP. Terminate the limit at the proxy, or accept that the per-IP bucket is effectively a global cap behind a single proxy.
- The internal map is bounded: when more than 10,000 distinct buckets accumulate, idle buckets older than two minutes are evicted on the next request.
- Startup logs `event=rate_limit_enabled` with the resolved RPM and burst when the limiter is active.
- 429 events log `event=rate_limited` with `client_ip` and `retry_after_secs`.

## Server log

Two sinks fan out from one event source:

1. **File sink** — JSON-Lines at `AICTL_SERVER_LOG_FILE` (default `~/.aictl/server.log`).
2. **Terminal sink** — human-readable, ANSI-colored on TTY, written to stderr. Auto-disables colors on non-TTY or when `NO_COLOR` is set.

Levels: `trace`/`debug`/`info`/`warn`/`error`. Body lines are gated by `AICTL_SERVER_LOG_BODIES` (default `true`); turning it off drops body lines at the source.

Rotation is the operator's responsibility (`logrotate`, journald). The file is opened append-only; SIGTERM flushes via the buffered writer's `Drop`.

## Deployment

### systemd

```ini
[Unit]
Description=aictl-server
After=network.target

[Service]
Type=simple
User=aictl
ExecStart=/usr/local/bin/aictl-server
Restart=on-failure
NoNewPrivileges=yes
ProtectSystem=strict
ReadWritePaths=/var/lib/aictl

[Install]
WantedBy=multi-user.target
```

### nginx reverse proxy (TLS + SSE)

```
server {
  listen 443 ssl http2;
  server_name aictl.example.com;

  location /v1/ {
    proxy_pass http://127.0.0.1:7878;
    proxy_http_version 1.1;
    proxy_set_header Connection "";
    proxy_buffering off;
    proxy_read_timeout 600;
  }
}
```

`proxy_buffering off` is required for SSE streaming to flush deltas live instead of being buffered until the connection closes.

### Docker

```dockerfile
FROM debian:stable-slim
COPY aictl-server /usr/local/bin/aictl-server
COPY --chown=aictl:aictl aictl-config /home/aictl/.aictl/config
EXPOSE 7878
ENTRYPOINT ["/usr/local/bin/aictl-server"]
```

## FAQ

**Is rate limiting available?** Yes. A per-client-IP token bucket is configurable via `AICTL_SERVER_RATE_LIMIT_RPM` and `AICTL_SERVER_RATE_LIMIT_BURST`; off by default. Saturation surfaces as 429 with a `Retry-After` header. The global concurrency cap (`AICTL_SERVER_MAX_CONCURRENT_REQUESTS`) operates independently and remains the primary backpressure mechanism.

**Can I forward a per-request provider key?** Not in this phase. Phase 3 revisits whether to support a `X-Provider-Authorization` header (or a `provider_key` body field) for deployments that want the server to hold no provider keys.

**Why no agent endpoints?** Pure-proxy is the entire point. Agent loops over HTTP would have a different auth model, session story, and tool-approval protocol; that becomes a separate plan if a concrete demand surfaces.

**Can I run two servers on one host?** Yes — distinct `--bind` values plus distinct config trees (e.g. `HOME=/var/lib/aictl-prod aictl-server`). Native `--config <path>` support depends on the modular-architecture loader change landing.

## Verification (developers)

The server enforces a hard separation from the CLI's interactive surface. CI greps:

```sh
grep -rE 'rustyline::|termimad::|indicatif::|crossterm::|dialoguer::' crates/aictl-server/src/
grep -rE 'run_agent_turn|run_agent_single|AgentUI|ToolApproval' crates/aictl-server/src/
```

Both must return empty. Any future change that pulls a REPL dep or reaches into the agent loop violates the proxy-only contract — fix the change, not the grep.
