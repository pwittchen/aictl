# Plan: Cross-Provider `/v1/messages` Gateway

## Context

Today `aictl-server` exposes `POST /v1/messages` as a pure passthrough to `api.anthropic.com` — the body is forwarded verbatim after master-key authentication, the prompt-injection guard runs on user text, and the redactor rewrites text surfaces (`crates/aictl-server/src/routes/messages.rs`). The handler hard-rejects any model that does not resolve to `Provider::Anthropic` with `400 model_not_found` (lines 66–74).

This works for Claude Code — Anthropic's CLI sends the native Messages shape, so passthrough preserves tool use, content blocks, prompt caching, system content arrays, and the `message_start` / `content_block_delta` SSE event sequence. But it means Claude Code (and any other native-Anthropic client routed through `ANTHROPIC_BASE_URL`) is stuck on Claude models. Users who want to run Claude Code against GPT-4o, Gemini, or a self-hosted Ollama model have no path.

This plan adds a second mode to `/v1/messages`: when the requested model resolves to a non-Anthropic provider, the handler translates the Anthropic-shaped request into the provider's native shape, dispatches directly to that provider's API (not through `aictl_core::llm::call_<provider>` — those use the engine's XML tool system and don't pipe native tool calls), then translates the response back into the Anthropic shape. Anthropic models keep the passthrough path verbatim with zero behavioral drift.

## Goals & Non-goals

**Goals**

- Make `/v1/messages` work with every provider the server already supports for `/v1/chat/completions`: OpenAI, Grok, Mistral, DeepSeek, Kimi, Z.ai, Gemini, plus the local trio (Ollama / GGUF / MLX) where tool-calling support exists.
- Preserve the Anthropic passthrough path **unchanged** — same byte-for-byte forwarding to `api.anthropic.com`, same support for prompt caching, extended thinking, `anthropic-beta` headers, content-block system prompts, fine-grained tool streaming.
- Translate the Anthropic Messages shape (top-level `system`, content blocks, `tool_use` / `tool_result`, image blocks) into each provider's native shape and back.
- Bridge the streaming SSE shapes — translate OpenAI's `choices[0].delta.content` chunks into Anthropic's structured event sequence (`message_start` → `content_block_start` → `content_block_delta` → `content_block_stop` → `message_delta` → `message_stop`).
- Bidirectionally translate tool calls: Anthropic `tool_use` / `tool_result` content blocks ↔ provider-native `tool_calls[]` / `role:"tool"` messages.
- Document trade-offs clearly: which Anthropic features survive cross-provider routing, which are silently dropped, which are rejected upfront.
- Gate the cross-provider mode behind `AICTL_SERVER_MESSAGES_CROSS_PROVIDER` (default **`false`**) so existing operators see no behavior change until they opt in.

**Non-goals**

- Not a unified middleware framework. The translator is purpose-built for the Anthropic ↔ provider direction. We are not building a generic protocol translator (LiteLLM, OpenRouter, etc. already exist if that's what the user wants).
- Not a server-side agent loop. The handler still does one request → one provider call → one response. Multi-turn flow, tool execution, retries — all of that stays in the client (Claude Code).
- Not feature parity with Anthropic on non-Anthropic providers. Prompt caching, extended thinking, the memory tool, fine-grained tool streaming, PDF blocks, and `anthropic-beta` features are Anthropic-only and will be either stripped (with a one-time warning) or rejected (with `400`), per a configurable feature-gate policy.
- Not changing `/v1/chat/completions`. That route stays OpenAI-shape-in, OpenAI-shape-out. Tool support on `/v1/chat/completions` is a separate roadmap item.
- Not adding new providers. We translate to the providers `aictl_core` already supports. Adding a new provider is a separate change in `aictl-core`.
- Not splitting CLI vs server config. The cross-provider mode is server-only — the CLI's `--provider aictl-server` path already reaches non-Anthropic providers via `/v1/chat/completions`.

## How it differs from the existing `/v1/chat/completions` gateway

|                          | `/v1/messages` (cross-provider, new)             | `/v1/chat/completions` (today)          |
|--------------------------|--------------------------------------------------|------------------------------------------|
| Request shape (client)   | Native Anthropic                                 | OpenAI                                   |
| Response shape (client)  | Native Anthropic                                 | OpenAI                                   |
| Tool calls               | Anthropic `tool_use` / `tool_result` blocks      | Rejected (`tools_unsupported_for_provider`) |
| Streaming events         | Anthropic SSE event sequence                     | OpenAI `data: {...}` chunks              |
| Content blocks           | Yes (text + image + tool blocks)                 | No (flat `content` string)               |
| Dispatch                 | Direct provider HTTP (native tools needed)       | `aictl_core::llm::call_<provider>` (text in / text out) |
| Reuses engine code       | Provider HTTP client, redactor, injection guard, audit | All of the above plus `llm::call_<provider>` |
| Audit event              | `gateway:anthropic` (passthrough), `gateway:messages:<provider>` (translated) | `gateway:<provider>`                     |

The cross-provider path **cannot** reuse `aictl_core::llm::call_<provider>` because those functions speak the engine's internal text-in / text-out abstraction and use a hand-rolled XML tool format (`<tool name="...">`). Claude Code expects native `tool_use` content blocks back, so the translator owns the full HTTP round-trip with each provider.

## Architecture

### Routing decision

`crates/aictl-server/src/routes/messages.rs::messages` becomes a thin dispatcher:

```text
1. Authenticate (existing master-key gate).
2. Parse body, extract `model`.
3. Resolve provider via openai::resolve_provider(model).
4. If provider == Anthropic:
     → call messages::passthrough::forward (today's code, lifted as-is).
5. Else if AICTL_SERVER_MESSAGES_CROSS_PROVIDER == true:
     → call messages::translator::translate_and_dispatch.
6. Else:
     → 400 model_not_found (today's behavior).
```

The flag default is `false` so first-launch operators see no change. The CLI / Docker / install paths advertise the flag in the `--help` output once shipped.

### Module layout

```
crates/aictl-server/src/
  routes/
    messages.rs                  # thin dispatcher (Anthropic vs translator vs reject)
  messages/                      # new
    mod.rs                       # exports passthrough + translator
    passthrough.rs               # current passthrough code, lifted from routes/messages.rs
    translator/
      mod.rs                     # translate_and_dispatch entry; provider dispatch fan-out
      anthropic_ir.rs            # parsed-Anthropic intermediate (serde structs)
      feature_gate.rs            # strip / reject / warn unsupported Anthropic features
      openai_family.rs           # OpenAI / Grok / Mistral / DeepSeek / Kimi / Z.ai (shared shape)
      gemini.rs                  # Gemini native shape
      ollama.rs                  # Ollama (native tool calling on models that support it)
      stream/
        mod.rs                   # Anthropic SSE event types + emitter
        openai_to_anthropic.rs   # OpenAI delta → Anthropic event state machine
        gemini_to_anthropic.rs   # Gemini stream chunks → Anthropic events
      tests/
        request.rs               # request-translation round-trips
        response.rs               # response-translation round-trips
        stream.rs                # SSE state machine fixtures
        feature_gate.rs          # gate policy decisions
```

The OpenAI-family adapter is one module because Grok / Mistral / DeepSeek / Kimi / Z.ai all speak the OpenAI shape with provider-specific base URLs and model names. The shared code translates Anthropic → OpenAI request, dispatches against the correct base URL, parses the OpenAI response, translates back. Only the URL + auth header + model-name validation differ per provider.

Gemini and Ollama each have their own native shape — `generateContent` for Gemini, `/api/chat` for Ollama — so they get dedicated adapters.

GGUF and MLX are skipped in phase 1. They run in-process via `aictl_core::llm::call_gguf` / `call_mlx`, those entry points don't accept native tools, and adding native tool calling to in-process inference is its own redesign. Cross-provider `/v1/messages` rejects GGUF and MLX models with `400 model_unsupported_for_cross_provider` and a pointer to the documentation. Documented as a known gap; revisit in phase 2.

### Intermediate representation

A `messages::translator::anthropic_ir` module defines serde-deserializable structs that mirror the Anthropic Messages request:

```rust
pub struct AnthropicRequest {
    pub model: String,
    pub messages: Vec<AnthropicMessage>,
    pub system: Option<SystemPrompt>,        // String | Vec<TextBlock>
    pub tools: Option<Vec<AnthropicTool>>,
    pub tool_choice: Option<AnthropicToolChoice>,
    pub max_tokens: u32,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub top_k: Option<u32>,                  // dropped on OpenAI-family
    pub stop_sequences: Option<Vec<String>>,
    pub stream: Option<bool>,
    pub metadata: Option<AnthropicMetadata>,
    // Unsupported-on-translation: passed to feature_gate.
    pub thinking: Option<serde_json::Value>,
    pub cache_control_seen: bool,
}

pub struct AnthropicMessage {
    pub role: AnthropicRole,                 // user | assistant
    pub content: AnthropicContent,           // String | Vec<ContentBlock>
}

pub enum ContentBlock {
    Text { text: String, cache_control: Option<CacheControl> },
    Image { source: ImageSource },           // base64 | url
    ToolUse { id: String, name: String, input: serde_json::Value },
    ToolResult { tool_use_id: String, content: ToolResultContent, is_error: Option<bool> },
    Document { source: DocumentSource },     // PDF — rejected on translation
}
```

Parsing happens once at the top of `translate_and_dispatch`. Every downstream adapter (`openai_family`, `gemini`, `ollama`) consumes the same IR and emits its provider-native shape.

### Provider dispatch (no engine reuse)

Each adapter does its own HTTP work via `aictl_core::config::http_client()`:

```text
openai_family::dispatch(ir, provider) →
  1. ir → OpenAiChatRequest (translate fields + content blocks + tools).
  2. Resolve base_url + key_name from provider.
  3. POST {base_url}/v1/chat/completions with Authorization: Bearer {key}.
  4. If stream:
       Pipe response.bytes_stream() through openai_to_anthropic::translate.
     Else:
       Parse OpenAiChatResponse, translate to AnthropicResponse, return JSON.
```

Per-provider base URLs:

| Provider  | Base URL                                       | Key name             |
|-----------|------------------------------------------------|----------------------|
| OpenAI    | `https://api.openai.com`                       | `LLM_OPENAI_API_KEY` |
| Grok      | `https://api.x.ai`                             | `LLM_GROK_API_KEY`   |
| Mistral   | `https://api.mistral.ai`                       | `LLM_MISTRAL_API_KEY`|
| DeepSeek  | `https://api.deepseek.com`                     | `LLM_DEEPSEEK_API_KEY`|
| Kimi      | `https://api.moonshot.ai` (or `.cn`)           | `LLM_KIMI_API_KEY`   |
| Z.ai      | `https://api.z.ai/api/paas/v4`                 | `LLM_ZAI_API_KEY`    |
| Gemini    | `https://generativelanguage.googleapis.com`    | `LLM_GEMINI_API_KEY` |
| Ollama    | `http://127.0.0.1:11434` (or `LLM_OLLAMA_HOST`)| _none_               |

URLs are read from the same `LLM_*_BASE_URL` config keys the engine already supports, so a user pinning Mistral to a private endpoint (or pointing OpenAI at LiteLLM) keeps that override.

## Translation matrices

### Request: Anthropic → OpenAI-family

| Anthropic field                                | OpenAI field                                        | Notes                                                  |
|------------------------------------------------|------------------------------------------------------|--------------------------------------------------------|
| `system: "string"`                             | `messages[0] = {role:"system", content:"string"}`   | Single message.                                        |
| `system: [{type:"text", text:...}, …]`         | `messages[0] = {role:"system", content: "joined"}`  | Cache-control markers dropped; texts joined with `\n\n`. |
| `messages[*]: {role, content:"string"}`        | `messages[*]: {role, content:"string"}`             | 1:1.                                                   |
| `messages[*].content: [TextBlock, …]`          | `content: "joined"` or `content: [{type:"text", …}]`| Multi-text joined unless image blocks present.         |
| `messages[*].content: [ImageBlock]`            | `content: [{type:"image_url", image_url:{url:"data:…;base64,…"}}]` | Base64 → data URL; URL source passes through.          |
| `messages[*].content: [ToolUse, …]`            | `assistant` message with `tool_calls: [{id, type:"function", function:{name, arguments: JSON.stringify(input)}}]` | Bundle all tool_use blocks on the same assistant turn. |
| `messages[*].content: [ToolResult, …]`        | One `{role:"tool", tool_call_id, content}` message per result | Anthropic packs many in one user message; OpenAI splits. |
| `tools[]: {name, description, input_schema}`   | `tools[]: {type:"function", function:{name, description, parameters}}` | Schema passed through; `strict` not set.               |
| `tool_choice: "auto"`                          | `tool_choice: "auto"`                               | Direct.                                                |
| `tool_choice: "any"`                           | `tool_choice: "required"`                           | Best-effort mapping.                                   |
| `tool_choice: {type:"tool", name}`             | `tool_choice: {type:"function", function:{name}}`   | Direct.                                                |
| `max_tokens`                                   | `max_tokens`                                        | Required in Anthropic; optional in OpenAI.             |
| `temperature`, `top_p`                         | same                                                | Direct.                                                |
| `top_k`                                        | —                                                   | Dropped (OpenAI doesn't support it).                    |
| `stop_sequences`                               | `stop`                                              | Direct.                                                |
| `metadata.user_id`                             | `user`                                              | Direct.                                                |
| `cache_control: {type:"ephemeral"}`            | —                                                   | Stripped; feature_gate logs once per request.          |
| `thinking: {...}`                              | —                                                   | Stripped; warning audit event.                         |
| `anthropic-beta` header                        | —                                                   | Not forwarded.                                          |

### Response: OpenAI → Anthropic

| OpenAI                                  | Anthropic                                                         |
|-----------------------------------------|--------------------------------------------------------------------|
| `id`                                    | `id` (prefixed with `msg_` if not already).                       |
| `model`                                 | `model`.                                                          |
| `choices[0].message.content` (string)   | `content: [{type:"text", text}]`.                                 |
| `choices[0].message.tool_calls[]`       | `content: [{type:"tool_use", id, name, input: JSON.parse(args)}, …]` (one per call). |
| Both `content` + `tool_calls` present   | `content: [{type:"text", text}, {type:"tool_use", …}, …]` in declared order. |
| `choices[0].finish_reason: "stop"`      | `stop_reason: "end_turn"`.                                        |
| `choices[0].finish_reason: "length"`    | `stop_reason: "max_tokens"`.                                      |
| `choices[0].finish_reason: "tool_calls"`| `stop_reason: "tool_use"`.                                        |
| `choices[0].finish_reason: "content_filter"` | `stop_reason: "end_turn"` + `"stop_filter": "content_filter"` audit. |
| `usage.prompt_tokens`                   | `usage.input_tokens`.                                             |
| `usage.completion_tokens`               | `usage.output_tokens`.                                            |
| `usage.prompt_tokens_details.cached_tokens` (if present) | `usage.cache_read_input_tokens` (best-effort).               |
| (absent on OpenAI)                      | `usage.cache_creation_input_tokens: 0` (always; no caching on this path). |

### Streaming: OpenAI SSE → Anthropic SSE

State machine in `messages::translator::stream::openai_to_anthropic::translate`:

```text
[init]
  emit: event: message_start
        data: {"type":"message_start","message":{"id":"msg_…","type":"message","role":"assistant","content":[],"model":"…","usage":{"input_tokens":0,"output_tokens":0}}}

[per-chunk]
  - On first delta.content (non-empty):
      if no block open: emit content_block_start (index 0, type:"text")
      block_state = TextOpen(0)
      emit content_block_delta (index 0, type:"text_delta", text: delta)
  - On subsequent delta.content:
      emit content_block_delta (index N, type:"text_delta", text: delta)
  - On delta.tool_calls[i] first appearance:
      if TextOpen(j): emit content_block_stop (index j); block_state = Closed
      emit content_block_start (index next, type:"tool_use", id, name)
      tool_state[i] = ToolOpen(next, buffer="")
      if arguments fragment present: emit input_json_delta
  - On delta.tool_calls[i] subsequent fragments:
      emit content_block_delta (index = tool_state[i].index, type:"input_json_delta", partial_json: fragment)
      tool_state[i].buffer += fragment
  - On final chunk (finish_reason set):
      close all open blocks: emit content_block_stop for each
      emit message_delta (delta:{stop_reason, stop_sequence:null}, usage:{output_tokens: total})
      emit message_stop

[error mid-stream]
  emit: event: error
        data: {"type":"error","error":{"type":"api_error","message":"…"}}
```

The state machine needs to handle:
- Multiple tool calls interleaved (index tracking per `tool_calls[i].index`).
- Empty content (no delta.content, just tool_calls) — skip the text block entirely.
- Provider keep-alive lines (`data: [DONE]`) — terminate cleanly.
- Split-token edge cases — OpenAI emits `delta.tool_calls[0].function.arguments` as `"{\"a\":"` then `"\"b\"}"`, and the Anthropic shape wants the same fragments as `input_json_delta`. We forward verbatim; the *client* (Claude Code) reassembles. That matches Anthropic's native behavior for `input_json_delta`.

Implementation: a `tokio_stream` adapter that consumes `Result<Bytes, _>` and produces `Result<axum::response::sse::Event, _>`. SSE parsing is hand-rolled — line-based, handles `data: ` prefix and double-newline framing.

### Request/response: Anthropic ↔ Gemini

Gemini's `generateContent` / `streamGenerateContent` shape is structurally similar to Anthropic's:

| Anthropic                         | Gemini                                              |
|-----------------------------------|------------------------------------------------------|
| `system`                          | `systemInstruction.parts[0].text`                    |
| `messages[*]`                     | `contents[*]` with `role: "user"\|"model"`           |
| `content` text                    | `parts[*].text`                                      |
| `content` image (base64)          | `parts[*].inlineData: {mimeType, data}`              |
| `content` tool_use                | `parts[*].functionCall: {name, args}`                |
| `content` tool_result             | `parts[*].functionResponse: {name, response}`        |
| `tools[]`                         | `tools[0].functionDeclarations[]`                    |
| `tool_choice: "auto"`             | `toolConfig.functionCallingConfig.mode: "AUTO"`      |
| `tool_choice: "any"`              | `toolConfig.functionCallingConfig.mode: "ANY"`       |
| `tool_choice: {type:"tool", name}`| `mode: "ANY"` + `allowedFunctionNames: [name]`       |
| `max_tokens`                      | `generationConfig.maxOutputTokens`                   |
| `temperature`                     | `generationConfig.temperature`                       |
| `stop_sequences`                  | `generationConfig.stopSequences`                     |

Gemini streaming uses SSE-ish chunks of `{candidates:[{content:{parts:[…]}, finishReason}]}`. A separate state machine (`stream/gemini_to_anthropic.rs`) bridges to the Anthropic event shape. Same emission contract as the OpenAI bridge.

### Request/response: Anthropic ↔ Ollama

Ollama's `/api/chat` shape sits between OpenAI and Anthropic — `messages[]` array, `tools[]` similar to OpenAI, but with `images: [base64]` instead of content blocks. Native tool calling is available on Ollama models that declare `tools` capability (Qwen 2.5, Llama 3.1+, Mistral Nemo, etc.); we forward tools and pass through the `message.tool_calls[]` field on response.

If the model doesn't support tools, Ollama returns plain text and we surface a clear runtime error rather than silently dropping. Detection: `GET /api/show?name=<model>` → check `capabilities`. We probe once per process per model and cache.

Streaming: Ollama emits newline-delimited JSON (one object per line), not SSE. A dedicated parser in `stream/ollama_to_anthropic.rs` reads NDJSON and emits Anthropic SSE events.

## Feature-gate policy

`AICTL_SERVER_MESSAGES_FEATURE_GATE` controls how the translator handles Anthropic-only features that have no clean equivalent on the target provider:

| Mode         | Behavior                                                                          |
|--------------|------------------------------------------------------------------------------------|
| `strip` (default) | Silently drop the unsupported field. Emit one `feature_dropped` audit event per request with the list. Client gets a successful response.   |
| `warn`       | Same as strip, but include a `aictl-warning` response header listing dropped features. |
| `reject`     | Return `400 feature_unsupported_for_provider` with the list. Operators who want strict parity flip this on.                                  |

Features the gate handles:

| Feature                          | OpenAI-family | Gemini       | Ollama       |
|----------------------------------|---------------|--------------|--------------|
| `cache_control`                  | stripped      | stripped     | stripped     |
| `thinking`                       | stripped      | stripped     | stripped     |
| `anthropic-beta` header          | ignored       | ignored      | ignored      |
| `metadata.user_id`               | passed (`user`) | stripped   | stripped     |
| PDF `document` block             | rejected      | rejected     | rejected     |
| `service_tier`                   | stripped      | stripped     | stripped     |
| `top_k`                          | stripped      | passed       | passed       |
| Image (base64)                   | passed (data URL) | passed (`inlineData`) | passed (`images[]`) |
| Image (URL)                      | passed        | rejected     | rejected     |

The gate runs **before** the redactor and injection guard — strip-mode mutates the IR; reject-mode short-circuits with `400`.

## Security & auditing

- **Master-key gate** unchanged — same `Authorization: Bearer …` check on every request.
- **Prompt-injection guard** runs on every user-role text surface in the IR, regardless of target provider. Same code path as today (`security::detect_prompt_injection`).
- **Redaction** runs on every text surface in the IR after the feature gate. Block-mode aborts with `400 redaction_blocked`. Redact-mode rewrites in place.
- **CWD jail** is irrelevant — translated requests dispatch to provider HTTPS endpoints, not local processes.
- **Audit** — every dispatch logs:
  - Anthropic passthrough: `gateway:anthropic` (unchanged).
  - Cross-provider: `gateway:messages:<provider>` (new). Input preview is the joined user text; result is the per-request UUID.
  - Feature gate: separate `feature_dropped` events listing the stripped fields and the provider.
- **MCP / CWD / tool restrictions** — irrelevant; this is a proxy path with no tool dispatch on the server side.

The translated request is signed with the operator's per-provider key (`LLM_OPENAI_API_KEY`, `LLM_GEMINI_API_KEY`, …) from the same `keys::get_secret` lookup the existing `/v1/chat/completions` route uses. No new secrets management.

## Configuration surface

New server-only config keys:

| Key                                            | Default     | Effect                                                                  |
|------------------------------------------------|-------------|-------------------------------------------------------------------------|
| `AICTL_SERVER_MESSAGES_CROSS_PROVIDER`         | `false`     | Master switch. When `false`, non-Anthropic models on `/v1/messages` get today's `400 model_not_found`. |
| `AICTL_SERVER_MESSAGES_FEATURE_GATE`           | `strip`     | `strip` / `warn` / `reject` for unsupported Anthropic features.         |
| `AICTL_SERVER_MESSAGES_TRANSLATE_PROVIDERS`    | `*`         | Optional comma-separated allow-list (`openai,gemini,ollama`). `*` = any non-Anthropic provider. Operators who want to restrict cost exposure pin this. |

Existing per-provider `LLM_*_API_KEY` and `LLM_*_BASE_URL` config keys are reused as-is — no new key per provider.

The Anthropic passthrough path is unaffected by any of the three new keys.

## Phasing

**Phase 1 — OpenAI-family non-streaming, no tools (~200 LOC)**
- Build the routing dispatcher and the IR.
- Implement `openai_family::dispatch` for non-streaming, text-only.
- Wire `AICTL_SERVER_MESSAGES_CROSS_PROVIDER` flag.
- Tests: round-trip a basic prompt through GPT-4o-mini.

**Phase 2 — OpenAI-family streaming (~400 LOC)**
- Implement `stream/openai_to_anthropic.rs` state machine.
- Tests: fixture-driven (record real OpenAI streams, replay through translator, assert Anthropic event sequence byte-for-byte).

**Phase 3 — OpenAI-family tools (~300 LOC)**
- Bidirectional `tool_use` ↔ `tool_calls` translation.
- `tool_result` packing/unpacking.
- Tests: multi-turn tool dialog round-trips.

**Phase 4 — Gemini (~400 LOC)**
- `gemini.rs` adapter + `stream/gemini_to_anthropic.rs`.
- Per-feature gate adjustments (Gemini supports inline images but not URL images).
- Tests: Gemini 2.0 Flash through the translator.

**Phase 5 — Ollama (~300 LOC)**
- `ollama.rs` adapter + `stream/ollama_to_anthropic.rs` (NDJSON parser).
- Capability probe + cache.
- Tests: against a local Ollama running Qwen 2.5.

**Phase 6 — Feature gate polish + docs (~100 LOC)**
- `AICTL_SERVER_MESSAGES_FEATURE_GATE` modes wired.
- `AICTL_SERVER_MESSAGES_TRANSLATE_PROVIDERS` allow-list wired.
- README / SERVER.md / server.html updates lift restrictions and document the new dual-mode behavior.

Each phase is independently shippable behind the master flag (`AICTL_SERVER_MESSAGES_CROSS_PROVIDER=true`) with the unimplemented providers returning the existing `400 model_not_found`.

Total estimate: ~1.7k LOC of translator code + ~600 LOC of tests. At this repo's AI-assisted velocity, plausibly 6–10 hours of focused work end-to-end.

## Testing

**Unit (per-translator):**
- Round-trip every field in the translation matrix. `proptest` for the text-block-flatten path.
- Tool-call translation: Anthropic `tool_use` block → OpenAI `tool_calls[]` → Anthropic again should be stable.
- Feature gate: strip / reject / warn modes each produce the expected outcome.

**Streaming state machine:**
- Fixture-driven. Record real OpenAI / Gemini / Ollama streams via curl (anonymized) into `crates/aictl-server/tests/fixtures/streams/`. Replay through the translator. Assert the emitted Anthropic SSE event sequence equals an expected fixture (also captured from real Anthropic streams for the same prompt).
- Edge cases: empty stream, stream-with-only-tool-calls, stream-with-error-mid-flight, stream-with-multiple-tool-calls.

**Integration:**
- Run the server with `AICTL_SERVER_MESSAGES_CROSS_PROVIDER=true` and a mock provider HTTP server (extend the existing `mockito` patterns in `crates/aictl-server/tests/`). Send native-Anthropic requests with `model: "gpt-4o-mini"`, `model: "gemini-2.0-flash"`, etc. Assert Anthropic-shaped responses.

**End-to-end (manual gate before shipping):**
- Run real Claude Code against the server with `ANTHROPIC_BASE_URL=http://127.0.0.1:7878` and `ANTHROPIC_MODEL=gpt-4o-mini`. Verify:
  - Basic text round-trip works.
  - Tool dispatch (Claude Code's edit_file, run_command, etc.) works through the translator.
  - Multi-turn tool dialogs preserve `tool_use_id` → `tool_result` linkage.
  - Streaming feels responsive (no buffering pathology).
- Repeat with `ANTHROPIC_MODEL=gemini-2.0-flash`.
- Repeat with `ANTHROPIC_MODEL=qwen2.5-coder:14b` (Ollama).

## Trade-offs to document

These land in `README.md`, `SERVER.md`, and `website/server.html`:

**Anthropic passthrough (unchanged) — what you get on Anthropic models:**
- Full prompt caching (90% input-token discount on cached prefixes).
- Extended thinking (`thinking` parameter).
- Fine-grained tool streaming.
- All `anthropic-beta` features.
- Native PDF content blocks.
- Byte-for-byte Anthropic SSE event shape.

**Cross-provider — what you give up when routing to non-Anthropic:**
- **No prompt caching** on the cross-provider path. Each request pays full input-token cost. (Gemini has context caching, OpenAI has prompt caching, but neither aligns with Anthropic's `cache_control` block markers — wiring them is out of scope.)
- **No extended thinking.** OpenAI o1/o3 use a different API shape; the translator does not map between them. Use OpenAI's `/v1/chat/completions` path if you need reasoning models.
- **No fine-grained tool streaming.** Provider-level chunking is preserved but the granularity (e.g., character-level deltas) may be coarser than Anthropic's.
- **No `anthropic-beta` features.** Memory tool, computer use, etc. — Anthropic-only.
- **PDF content blocks rejected.** Strip mode: `400 feature_unsupported_for_provider`. Extract text upstream.
- **`cache_control` markers stripped.** No effect on cost; documented in `feature_dropped` audit events.
- **Stop reasons may not round-trip exactly.** `content_filter`, `length`, `tool_calls` map best-effort.
- **Streaming event timing may differ.** Anthropic emits a more structured event sequence than OpenAI's flat deltas; the translator approximates but does not perfectly replicate latency profiles.
- **GGUF / MLX rejected.** No native tool calling in the in-process backends. Use Ollama if you want local + tools.

**Audit / security — unchanged across both paths:**
- Master-key gate on every request.
- Prompt-injection guard on every user message.
- Redaction on every text surface (`system`, `messages[*].content[*].text`).
- Per-request UUID + audit log entry.

## Open questions

- **Cost transparency.** Should the cross-provider response include a server-injected note (e.g., `aictl-warning: prompt caching disabled`) when the request had `cache_control` markers and was routed cross-provider? Default to `warn` mode so users see this once and can flip to `strip` for silent operation.
- **Default provider when `ANTHROPIC_MODEL` is `claude-sonnet-4-6` but the operator only has an OpenAI key.** Reject (today's behavior, just at a different layer) or remap (dangerous, surprising). Plan: reject; operators must explicitly set `ANTHROPIC_MODEL=gpt-4o-mini`.
- **Per-provider quota.** A single Claude Code session translating through to OpenAI can easily burn through quota on long contexts (no caching). Should we add a `AICTL_SERVER_MESSAGES_MAX_INPUT_TOKENS` cap? Phase 6 stretch goal.
- **Streaming-only providers.** Gemini supports non-streaming responses; Ollama always streams internally even if `stream: false` is requested. Translator surfaces a unified non-streaming shape by buffering internally — document the latency cost.

## What lands when

| Doc                                                | Now (planning)                                                                  | After phase 1                  | After phase 6 |
|----------------------------------------------------|----------------------------------------------------------------------------------|---------------------------------|---------------|
| `.claude/plans/messages-cross-provider.md` (this)  | Full plan.                                                                       | Update with phase outcomes.    | Move to `.claude/plans/done/`. |
| `ROADMAP.md`                                        | Add "Cross-provider /v1/messages" section linking here.                          | Update phase checkboxes.        | Remove entry.  |
| `README.md` (Claude Code section)                  | Add "Planned: cross-provider routing" callout with trade-off summary + link to SERVER.md. | Update to reflect shipped state. | Same.        |
| `SERVER.md` (`/v1/messages`)                       | Add a "Cross-provider routing (planned)" subsection with the full trade-off table. | Mark features shipped per phase. | Drop "(planned)". |
| `website/server.html` (Claude Code section)        | Add a "Cross-provider routing (planned)" card alongside the current Anthropic-only one. | Update card to reflect shipped state. | Same. |
