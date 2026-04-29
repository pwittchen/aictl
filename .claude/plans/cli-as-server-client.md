# `aictl-cli` as `aictl-server` client — development plan

Allow `aictl-cli` to route every LLM call through a single `aictl-server` instance instead of talking to each provider directly. The user points the CLI at a server URL plus a master key; the CLI then uses the server's OpenAI-compatible `/v1/chat/completions` endpoint as its only upstream and lets the server fan out to the underlying providers.

This plan operationalizes the **CLI as aictl-server client** section of [`ROADMAP.md`](../../ROADMAP.md) — once it ships, that roadmap entry comes out.

---

## 1. Scope

**Purely additive.** When the new `server_url` config key is empty (the default), the CLI behaves exactly as it does today: every provider key resolves locally, every `llm::call_<provider>` runs against the provider's own endpoint. No regression, no behavior change.

When `server_url` is set:

- All non-local LLM calls route through `${server_url}/v1/chat/completions` with `Authorization: Bearer ${server_master_key}`.
- Local providers (`Ollama`, `GGUF`, `MLX`) **never** go through the server — they stay local-process.
- The model catalogue, pricing, redaction, security gate, audit log, sessions, agents, skills, MCP, plugins, hooks, slash commands, and the agent loop itself all stay CLI-side.
- `/balance` and `--list-balances` hit `${server_url}/v1/stats` instead of probing each provider individually.

The `Provider` enum is **not** changed. The dispatch decision is a single `if let Some(url) = server_url()` branch at the top of provider dispatch — model selection, pricing, balance UI, and the rest of the system keep working off the existing catalogue.

---

## 2. Goals & non-goals

### Goals

- **One server, one credential** — operators configure provider keys in one place (the server's `~/.aictl/config`) and every CLI host points at that server with a single master key. Adding a new provider on the server is invisible to clients.
- **Zero regression** — `cargo build --workspace`, `cargo lint --workspace`, `cargo test --workspace`, and the existing CLI behavior all stay green when `server_url` is unset.
- **Same security posture** — when the server is the upstream, the CLI's security gate, redaction, audit, and session storage all keep running locally; the server adds a *second* layer of redaction/audit on its side. Defense in depth, not a substitution.
- **Streaming preserved** — token streaming from server to CLI uses the same SSE shape `aictl-server` already speaks. The CLI's `StreamState` and `on_token` callback don't change.
- **Master key follows the existing key model** — plain text in `~/.aictl/config` by default, moved to keyring when the user locks keys via `/keys`. Same `keys::get_secret(name)` resolution as every other API key.

### Non-goals (v1)

- No automatic failover from server → direct providers if the server is down. Fail loudly. The user picked a routing mode.
- No multi-server load balancing or per-model server selection. One URL, all non-local models.
- No CLI-side support for the server's `/v1/completions` (legacy) endpoint — `/v1/chat/completions` covers everything the CLI emits.
- No tool-execution offload. Tools still run on the CLI host. The server is purely an LLM transport.
- MCP, plugins, hooks, agents, skills stay CLI-local — the server doesn't host these subsystems and we aren't building bridges in v1.
- No multi-credential support (e.g., per-CLI-user master keys against one server). One CLI host, one master key.

---

## 3. Configuration

### New config keys

Two keys land in `~/.aictl/config`, both mirroring the storage convention of the existing provider API keys:

| Key | Type | Default | Notes |
|---|---|---|---|
| `server_url` | string | empty | Base URL of the `aictl-server` instance, e.g. `http://127.0.0.1:7878`. Empty = "use direct providers" (current behavior). Trailing slash is tolerated and stripped at load time. |
| `server_master_key` | string | empty | Master API key for the server. **Plain text by default** (matches every other API key). When the user locks keys via `/keys`, this value moves to the OS keyring under the name `AICTL_SERVER_MASTER_KEY` and the plain config entry is cleared. |

`keys::get_secret("AICTL_SERVER_MASTER_KEY")` performs the existing keyring-first / config-fallback resolution, so the call site doesn't care which storage holds the key.

### CLI flags

Long-form only, matching project convention:

```
aictl --server-url <URL>          # override server_url for this invocation
aictl --server-master-key <KEY>   # override server_master_key for this invocation
```

Flags override config, never persist. The `--list-models`, `--list-balances`, single-shot `--message`, and REPL all respect the override.

### Slash commands

- `/config` lists `server_url` and `server_master_key` alongside the existing keys, with the same set/unset/show semantics.
- `/keys` gains an `aictl-server master key` row alongside the provider keys, with the same lock / unlock / clear behavior. Locking moves the value into the keyring; unlocking copies it back to the plain config; clear removes it from both stores.
- `/info` and the status banner show the active routing mode:
  - When `server_url` is set: `routing: aictl-server (http://127.0.0.1:7878)`.
  - When unset: `routing: direct providers`.

### Loading order (per request)

1. CLI flag (`--server-url`, `--server-master-key`).
2. `~/.aictl/config` (`server_url`, `server_master_key`).
3. Keyring (`AICTL_SERVER_MASTER_KEY` only — the URL never goes into the keyring).
4. If neither URL is set, fall through to the existing direct-provider path.
5. If a URL is set but the master key is missing, abort with a clear error: `server_url is set but server_master_key is empty — set it via /keys or --server-master-key`.

---

## 4. Routing decision

### Where the branch lives

The dispatch branch lives in **one place** to keep the rest of the system unchanged: `crates/aictl-core/src/run.rs`, inside the helper that picks which `llm::call_<provider>` to invoke. Today that helper matches on `Provider`; with the server in the mix it becomes:

```rust
async fn dispatch_llm(
    provider: Provider,
    model: &str,
    messages: &[Message],
    on_token: Option<TokenSink>,
) -> Result<(String, TokenUsage), AictlError> {
    if let Some((url, key)) = active_server() {
        if !provider.is_local() {
            return server_proxy::call(&url, &key, model, messages, on_token).await;
        }
    }
    match provider {
        Provider::OpenAi    => llm::openai::call_openai(&get_secret("OPENAI_API_KEY")?, model, messages, on_token).await,
        Provider::Anthropic => llm::anthropic::call_anthropic(...).await,
        // … existing arms unchanged
        Provider::Ollama    => llm::ollama::call_ollama(...).await,  // always local
        Provider::Gguf      => llm::gguf::call_gguf(...).await,      // always local
        Provider::Mlx       => llm::mlx::call_mlx(...).await,        // always local
    }
}
```

`active_server()` is a tiny helper that reads the resolved `server_url` + master key from `config` / `keys::get_secret`, returns `None` when either is missing.

`Provider::is_local()` is a new one-line method returning `true` for `Ollama`, `Gguf`, `Mlx`. Trivial, easy to grep, easy to extend if a new local provider appears.

### Why a dispatch helper rather than a new `Provider::Server` variant

- The `Provider` enum drives **model catalogue lookup, pricing, balance probing, and the model picker UI**. None of those should change just because the user routed traffic through a server — a `claude-opus-4-7` is still an Anthropic model with Anthropic pricing whether it goes direct or via the server.
- A new variant would force every match site (`MODELS` lookup, `/balance`, pricing, agent loop, etc.) to grow a new arm with no useful behavior. A dispatch branch keeps the model concept and the transport concept separate.

### What the branch does *not* touch

- `MODELS` catalogue and model→provider resolution stay identical.
- The `<tool>` XML protocol, security gate, redaction (still runs locally before egress), audit log, and session writer stay identical.
- `StreamState` and the `on_token` callback shape stay identical — the proxy emits the same `String` tokens any other provider does.
- `AICTL_LLM_TIMEOUT` keeps wrapping each call.

---

## 5. The `server_proxy` module

A new module: `crates/aictl-core/src/llm/server_proxy.rs`.

### Public surface

```rust
pub async fn call(
    server_url: &str,
    master_key: &str,
    model: &str,
    messages: &[Message],
    on_token: Option<TokenSink>,
) -> Result<(String, TokenUsage), AictlError>;
```

Mirrors `llm::openai::call_openai` deliberately — the server speaks the OpenAI shape, and reusing the shape means we lift the request/response types from `llm::openai` rather than duplicating them. In practice:

- Request body: the same `OpenAiRequest` / `OpenAiMessage` types currently defined in `llm/openai.rs`. We hoist them to a small `llm/openai_shape.rs` (or `pub(crate)` them out of `openai.rs`) so both modules share the structs without one importing the other's private items.
- Endpoint: `POST ${server_url}/v1/chat/completions`.
- Header: `Authorization: Bearer ${master_key}`.
- Streaming: same SSE consumption logic as `openai.rs` (`data: {...}` deltas + `data: [DONE]` terminator). The server already produces this shape per `SERVER.md`.
- Token usage: server forwards `usage` in the final non-stream chunk and in the trailing `chunk` of the SSE stream when `stream_options.include_usage = true` (which the CLI already requests on the OpenAI path). Same parser.

### What the module does **not** do

- Does not translate model names. The CLI sends `model: "claude-opus-4-7"`; the server does the routing.
- Does not handle tool-call XML. The CLI's tool protocol is XML-in-content, not OpenAI `tools[]`, so the proxy doesn't need to involve the OpenAI tool shape at all. The server treats the body as opaque chat content; the CLI's tool parser handles the response on the way back.
- Does not cache, retry, or fall back. One request, one upstream, one response.

### Errors

The proxy maps server-side error envelopes (`{"error":{"code":"...","message":"..."}}`) into the existing `AictlError`:

| Server `code` | CLI `AictlError` | UX |
|---|---|---|
| `auth_missing` / `auth_invalid` | `AictlError::Auth` | "server rejected master key — check `/keys`" |
| `prompt_injection` | `AictlError::Security` | "server's prompt-injection guard rejected this request" |
| `model_not_found` | `AictlError::Provider` | "server doesn't know model `<name>`; check `aictl --server-url … --list-models`" |
| `provider_rate_limited` | `AictlError::RateLimit` | upstream message |
| `provider_unavailable` | `AictlError::Provider` | upstream message |
| `body_too_large` | `AictlError::Provider` | "request exceeded server's `AICTL_SERVER_BODY_LIMIT_BYTES`" |
| any 5xx | `AictlError::Provider` | "server returned 5xx — see server log" |

Network-level errors (connection refused, DNS failure, TLS error) surface as `AictlError::Network` with a hint pointing at `/info` to verify the URL.

---

## 6. Health check on first use

Before the first proxied request of the process, the CLI does a `GET ${server_url}/healthz` with a short timeout (3 s). Result handling:

- 200 → cache "server reachable" for the rest of the process; never probe again.
- Non-2xx → print `server reachable but unhealthy ({status}) — proceeding anyway` and continue. Don't block the user.
- Network error → fail loudly: `server unreachable at ${server_url}: ${err}`. Single retry hint: `aictl --server-url ""` for a one-shot bypass.

The probe runs **once per CLI process**, not per request. In single-shot mode (`--message`), it adds ~10–30 ms to startup; in REPL mode, only the first turn pays.

The probe is skipped entirely when the server URL is unset. It does **not** count as a `/v1/chat/completions` request and does not touch audit / stats.

---

## 7. `/balance` and `/v1/stats`

When a server is configured, the CLI's balance flow changes shape:

- `/balance` and `--list-balances` stop probing each provider's own balance endpoint and instead call `GET ${server_url}/v1/stats` once. The server returns its aggregated stats; the CLI renders them as the existing balance table.
- Local providers (Ollama / GGUF / MLX) are not probed in either mode (already true today); their rows just say "local".
- `/stats` (the local stats command) keeps reading `~/.aictl/stats` exactly as today — local stats record what *this* CLI host did, the server stats record what *that* server saw. They are different counters by design.

### Implementation

`llm/balance.rs` grows a `fetch_server_stats(url, key) -> Vec<ProviderBalance>` function and a top-level branch:

```rust
pub async fn list_balances() -> Result<Vec<ProviderBalance>, AictlError> {
    if let Some((url, key)) = active_server() {
        return fetch_server_stats(&url, &key).await;
    }
    // existing per-provider probe loop, unchanged
}
```

The server's `/v1/stats` response shape is documented in `SERVER.md` — the CLI mirrors that shape into `ProviderBalance` so the rendering layer doesn't change.

---

## 8. Module map and integration points

| File / location | Change |
|---|---|
| `crates/aictl-core/src/llm/server_proxy.rs` | **New** — `pub async fn call(...)` per §5 |
| `crates/aictl-core/src/llm/mod.rs` | Add `pub mod server_proxy;` |
| `crates/aictl-core/src/llm/openai.rs` | Hoist `OpenAiRequest`, `OpenAiMessage`, `OpenAiContent`, `OpenAiResponse`, `OpenAiUsage`, the SSE delta parser, and `build_messages` to `pub(crate)` (or move to a shared `llm/openai_shape.rs`) so `server_proxy` reuses them without copy-paste |
| `crates/aictl-core/src/llm/balance.rs` | Add `fetch_server_stats(...)` + the top-level routing branch in `list_balances` (§7) |
| `crates/aictl-core/src/llm/mod.rs` | Add `Provider::is_local(&self) -> bool` returning `true` for `Ollama` / `Gguf` / `Mlx` |
| `crates/aictl-core/src/run.rs` | Wrap the existing provider-dispatch site in the `if let Some((url, key)) = active_server()` branch (§4). No other changes to the agent loop |
| `crates/aictl-core/src/config.rs` | Document `server_url`. Add `pub fn server_url() -> Option<String>` (reads config + strips trailing slash) and `pub fn active_server() -> Option<(String, String)>` (combines URL + `keys::get_secret`) |
| `crates/aictl-core/src/keys.rs` | Add `AICTL_SERVER_MASTER_KEY` to the list of well-known keys the keyring lock/unlock cycle iterates over so locking moves it into the keyring like any provider key |
| `crates/aictl-cli/src/main.rs` | Add `--server-url <URL>` and `--server-master-key <KEY>` clap arguments, wire them into the per-process config overlay (same mechanism used by the existing key flags) |
| `crates/aictl-cli/src/commands/keys.rs` | Surface the `aictl-server master key` row in the `/keys` menu |
| `crates/aictl-cli/src/commands/config.rs` | List `server_url` and `server_master_key` in `/config` |
| `crates/aictl-cli/src/commands/info.rs` | Show the active routing mode (server URL vs. direct) |
| `crates/aictl-cli/src/ui.rs` | Status banner shows `via aictl-server` when routing is active |
| `README.md` | Add a "Use aictl-server as the upstream" subsection with the two-config-key example |
| `ARCH.md` | Mention `llm/server_proxy.rs` in the module map and explain the routing branch |
| `CLAUDE.md` | Add `server_proxy.rs` to the module map; document the `server_url` / `server_master_key` config keys; note `Provider::is_local` |
| `SERVER.md` | Add a "Connecting `aictl-cli` to `aictl-server`" section pointing back at this routing flow |
| `ROADMAP.md` | Remove the **CLI as aictl-server client** section once Phase 1 ships |

No changes touch the security gate, redaction pipeline, audit log, session writer, hooks, plugins, MCP, agents, or skills.

---

## 9. UX walkthroughs

### First-time setup (plain text storage — default)

```
$ aictl
> /config server_url http://127.0.0.1:7878
server_url set
> /config server_master_key sk-aictl-…
server_master_key set
> /info
…
routing: aictl-server (http://127.0.0.1:7878)
…
> hello
[server reachable, OK]
…response…
```

`server_master_key` is now plain text in `~/.aictl/config` alongside `OPENAI_API_KEY` / `ANTHROPIC_API_KEY` / etc. — same posture as every other key by default.

### Locking keys

```
> /keys
…
[ ] OpenAI                    (in config, plain text)
[ ] Anthropic                 (in config, plain text)
[ ] aictl-server master key   (in config, plain text)

→ Lock all
locked 3 keys to keyring
> /keys
[*] OpenAI                    (in keyring)
[*] Anthropic                 (in keyring)
[*] aictl-server master key   (in keyring)
```

After locking, `~/.aictl/config` no longer contains any of the keys — including `server_master_key`. `keys::get_secret("AICTL_SERVER_MASTER_KEY")` resolves from the keyring transparently.

### One-shot override (no persistence)

```
$ aictl --server-url http://prod-server:7878 --server-master-key sk-… --message "summarize this"
```

Neither value is written to config or keyring. Single-shot only.

### Disabling without deleting credentials

```
> /config server_url ""
server_url cleared — routing: direct providers
```

The master key stays where it was (config or keyring). Setting `server_url` again restores routing without re-entering the key.

---

## 10. Risks

- **Double redaction**: redaction runs on the CLI before egress *and* on the server before its own egress. The CLI redactor sees the raw user prompt; the server redactor sees the already-redacted CLI output. This is harmless (idempotent on already-redacted strings) but worth noting in the audit trail — the CLI audit logs the pre-redaction form, the server audit logs what reached its proxy. Operators reconciling logs across the two surfaces should expect this.
- **Streaming through the server**: SSE through a Rust→HTTP→Rust hop adds buffering risk. The server already documents `proxy_buffering off` for nginx; for direct CLI→server traffic with no proxy in between, axum's SSE flush behavior is fine. We add an integration test that asserts deltas arrive incrementally rather than batched (§11).
- **Master key in `--server-master-key` flag**: shows up in shell history and `ps`. Document the flag as suitable for scripts and ephemeral overrides; the persistent path is `/keys`. Same caveat already applies to the existing `--api-key` style flags if any (audit before shipping).
- **Health check false negative**: the probe is best-effort — a 503 at the moment of probe but a 200 a second later means the user gets a misleading warning. The probe never blocks, only warns, so the cost is a single noisy log line. Acceptable.
- **`OpenAiRequest` shape drift**: hoisting the OpenAI request/response structs out of `openai.rs` for reuse couples the proxy to the OpenAI shape evolution. If the server later supports an `aictl`-specific extension (e.g., per-request provider override from §3 of the server plan), the proxy and the openai module will need to diverge. We accept the coupling for v1; a follow-up extracts a server-only request shape if needed.
- **Local provider misclassification**: if a future provider is added without updating `Provider::is_local`, the routing branch will incorrectly send it to the server. Mitigation: a unit test that asserts every variant is covered (see §11) — `match` exhaustiveness in `is_local` plus a static assertion on the variant count.
- **Master key in keyring on a headless host**: the keyring may not be available (no Secret Service on a fresh server, locked Keychain on macOS in CI). The existing `/keys` flow handles this — locking falls back to plain text with a warning. Same fallback applies to `AICTL_SERVER_MASTER_KEY`. No new behavior.

---

## 11. Testing strategy

### Unit tests (`crates/aictl-core/src/llm/server_proxy.rs`)

- `request_shape_matches_openai`: serialize a request and assert it matches the OpenAI request schema exactly (snapshot test against the same fixture used by `openai.rs`).
- `bearer_header_set`: assert `Authorization: Bearer <key>` is present.
- `streaming_assembles_deltas`: feed a canned SSE byte stream, assert the assembled `String` and `TokenUsage` match.
- `error_envelope_maps_codes`: feed each documented `code` value, assert the right `AictlError` variant is produced.

### Unit tests (`crates/aictl-core/src/llm/mod.rs`)

- `provider_is_local_exhaustive`: explicit match over every `Provider` variant; new variants force a code change here.

### Unit tests (`crates/aictl-core/src/run.rs`)

- `dispatch_uses_server_when_configured`: stub `active_server()` to `Some(...)`, run a non-local-provider call, assert the proxy was invoked.
- `dispatch_skips_server_for_local_providers`: same stub, run an `Ollama` call, assert the local module was invoked.
- `dispatch_skips_server_when_unconfigured`: `active_server()` → `None`, run any call, assert the local module was invoked.

### Integration tests (`crates/aictl-cli/tests/`)

Spin up a real `aictl-server` on an ephemeral port (the server crate already exposes a test harness — reuse it):

- `test_cli_routes_through_server`: configure `server_url` + `server_master_key`, run a single-shot `--message`, assert the server's audit log contains a `gateway:openai` (or whichever provider) entry.
- `test_cli_streaming_through_server`: same setup with a slow mock provider, assert streamed tokens arrive incrementally on the CLI's `on_token` callback (deltas separated by ≥10 ms gaps, not batched).
- `test_cli_balance_uses_server_stats`: configure server, run `--list-balances`, assert the table is populated from `/v1/stats` rather than per-provider probes (mock providers' balance endpoints are never hit).
- `test_cli_local_provider_skips_server`: configure server + a fake Ollama, run a `model=llama3.1` call, assert the server's audit log is empty and the local Ollama mock was hit instead.
- `test_cli_health_probe_warns_on_5xx`: server returns 503 on `/healthz` once, then 200, then accepts the chat call. Assert a warning is emitted, the chat still succeeds, and only one `/healthz` is requested per process.
- `test_cli_master_key_from_keyring`: lock `AICTL_SERVER_MASTER_KEY` into a mock keyring, clear plain config, run a single-shot, assert the request reaches the server with the correct bearer.
- `test_cli_flag_overrides_config`: persist `server_url=A` in config, pass `--server-url=B` on the command line, assert traffic reaches `B`.
- `test_cli_no_server_url_unchanged`: empty config, no flags — assert the existing direct-provider path is used (sanity gate against regression).

### Smoke

- Manual: `aictl-server` running locally; CLI configured; run a multi-turn REPL session with tool calls (`read_file`, `exec_shell`). Confirm: tool calls execute on the CLI, LLM calls go through the server, audit logs on both sides line up, redaction visible in the server's audit, sessions persist locally.

### Regression gate

A grep gate added to CI:

```bash
# server-routing must stay opt-in — direct provider calls must remain reachable
grep -rE 'server_proxy::call' crates/aictl-core/src/ | grep -v 'run.rs\|server_proxy.rs'
```

Should return empty — only `run.rs`'s dispatch helper and the proxy module itself ever name the proxy.

---

## 12. Phased rollout

### Phase 1 — MVP (this plan)

Everything above. Target: two config keys, one routing branch, one new `server_proxy.rs`, balance via `/v1/stats`, health check, `/keys` integration, docs.

### Phase 2 — quality of life (deferred)

- `/server` slash command: a focused menu for "set URL", "set key", "test connection", "view server stats", "clear" without the broader `/config` and `/keys` UI.
- Inline server health indicator in the status banner (green/red dot updating on the periodic ping the REPL already runs for `/balance`).
- Per-session opt-in: a `--via-server` / `--via-direct` flag that chooses routing for one session without touching config. Useful for A/B comparing latency or debugging.
- Telemetry parity: surface the server's `request_id` (returned in `X-Request-Id`) in the CLI's audit log so cross-host log reconciliation gets a join key.

### Phase 3 — advanced (revisit on demand)

- Multiple servers with per-model routing (e.g., "send `claude-*` to server A, `gpt-*` to server B"). Keep on the shelf until a concrete user asks.
- Automatic failover server → direct providers when the server is down. Currently rejected (silent fallback hides outages); revisit if operators ask for it explicitly.
- Per-CLI-host master keys distinct from the shared CLI master key (multi-tenant server deployments). Out of scope today — the server is single-tenant in v1.

---

## 13. Verification

Per-phase gate:

| Phase | Build | Lint | Test | Additional |
|---|---|---|---|---|
| 1 | `cargo build --workspace` clean on default features and `--all-features` | `cargo lint --workspace` clean | `cargo test --workspace` clean including every test in §11 | Smoke walkthrough from §11; `/keys` lock/unlock moves `AICTL_SERVER_MASTER_KEY` between plain config and keyring without leaking the value |

Final sign-off for Phase 1 requires:

1. Build, lint, and tests green per the table.
2. Grep regression gate (§11) returns empty.
3. With `server_url` empty, every existing CLI flow (REPL, single-shot, `--list-models`, `--list-balances`, agent management, slash commands) behaves identically to the pre-change `master` branch — verified by running the existing CLI integration tests against the patched binary.
4. With `server_url` set:
   - A single-shot `--message` round-trips through `aictl-server`, the server's audit log contains the expected `gateway:<provider>` entry, and the CLI's audit log contains the corresponding tool-call trail.
   - Streaming arrives incrementally (no buffering).
   - `/balance` populates from `/v1/stats`.
   - Local providers (`Ollama`) bypass the server.
   - Locking `aictl-server master key` via `/keys` moves it to the keyring; `~/.aictl/config` no longer contains `server_master_key`; the next request still succeeds.
5. Documentation coherent: `README.md`, `ARCH.md`, `CLAUDE.md`, `SERVER.md` all describe the new path consistently and the corresponding `ROADMAP.md` section is removed.

---

## 14. Open questions

- **Audit deduplication across surfaces.** When a CLI request goes through the server, both sides write an audit entry. Should the CLI's audit entry include the server's `request_id` (from `X-Request-Id`) so an operator can join the two logs? Phase 2 candidate; Phase 1 keeps them independent.
- **Health probe cadence.** Once-per-process is the simplest design. If REPL sessions run for hours and the server restarts mid-session, the next chat call fails with a network error rather than a probe warning. Acceptable in v1; if it becomes annoying, Phase 2 adds a periodic re-probe (every N minutes, configurable, default off).
- **`--list-balances` granularity.** The server's `/v1/stats` aggregates by provider; if a user wants per-API-key breakdown the server would need to expose that. Out of scope today since the server is single-tenant.
- **What about `/v1/models`?** The CLI today builds its model list from the static `MODELS` catalogue, not from a network call. We could optionally fetch `${server_url}/v1/models` to surface server-side detection of locally available Ollama / GGUF / MLX models (which the *server* sees, not the CLI). Phase 2 idea; Phase 1 sticks with the static catalogue.
- **Master key rotation UX.** Today rotation is "edit the value via `/keys` or `/config`". If multiple CLI hosts share one server and the operator rotates the server key, every CLI host needs the new value pushed manually. Out of scope for the CLI plan; lives with the server's deployment story.
