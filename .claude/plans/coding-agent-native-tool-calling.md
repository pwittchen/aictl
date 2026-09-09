# Spec: Native tool calling per provider

> Report item **1** (Tier 1). The one structural change in the set. Sequence after the eval
> harness and context management so the switch is measurable and the transcript is bounded.

## Context

Every provider receives the same `<tool name="…">` text protocol.
`grep -rn "tool_calls" crates/aictl-core/src/llm/` returns nothing outside `mock.rs`. The
model is asked to emit XML inside prose, and the host parses it back out with
`tools::parse_tool_calls` ([`tools.rs`](../../crates/aictl-core/src/tools.rs)).

The cost is spread across the codebase and is all visible:

- `tools.rs` carries `TOOL_CLOSE_TAGS`, `STRAY_OPEN_TAGS`, `PARAM_ELEMENTS` and
  `normalize_tool_body` purely to survive models leaking the XML dialect they were
  post-trained on. Observed concretely with `claude-haiku-4-5` emitting
  `sh -c '<input>pwd</input>'` and a file literally named `<tool name="write_file">`.
- `security::check_path_with` rejects any path containing `<` or `>` as a backstop against
  that leakage reaching the filesystem.
- `llm::stream::StreamState` must hold back any buffered text that could be a prefix of
  `<tool name="` so half-formed tool XML never renders.
- The full tool catalogue (~4KB across `TOOL_COUNT = 36` tools) rides in the system prompt on
  **every** call, uncached in providers without prompt caching.
- Tool arguments are line-oriented free text, so every tool re-implements its own body
  parsing and every model must guess the grammar from prose examples.

Providers post-trained on native `tools[]` are measurably more reliable at emitting a
well-formed call than at freeform XML, and native calling supplies parallel calls, stable
tool-call IDs, and structured arguments for free.

**Most of the translation work already exists.**
[`crates/aictl-server/src/messages/translator/`](../../crates/aictl-server/src/messages/translator/)
converts an Anthropic-shaped request — including `tools[]`, `tool_use` / `tool_result`
blocks, and `tool_choice` — into OpenAI / Gemini / Ollama native shapes and back, with
per-provider streaming state machines under `translator/stream/`. Its intermediate
representation lives in
[`translator/ir.rs`](../../crates/aictl-server/src/messages/translator/ir.rs)
(`AnthropicRequest`, `AnthropicTool { name, description, input_schema }`, `ContentBlock`,
`ToolResultBlock`).

That code is in the **server** crate and the engine cannot depend on it (the dependency
direction is `server → core`). This spec's central design question is how to reuse it
without inverting that.

## Goals & Non-goals

**Goals**

- A `llm::ToolProtocol` enum — `Native` or `Xml` — resolved per (provider, model).
- JSON Schema definitions for all 36 built-in tools, plus schema passthrough for MCP tools
  (which already carry `input_schema`) and plugin tools (which have `schema_hint`).
- Native `tools[]` emission and `tool_calls[]` parsing for OpenAI-family (OpenAI, Grok,
  Mistral, DeepSeek, Kimi, Z.ai), Anthropic, and Gemini.
- The existing XML path preserved verbatim as the fallback for Ollama, GGUF, MLX, and any
  provider/model not on the native list — including the foreign-dialect tolerance.
- Tool-catalogue prose dropped from the system prompt when the protocol is `Native`
  (the schemas carry it), keeping it when `Xml`.
- Streaming works on both paths.
- One `ToolCall` type flows into `run::handle_tool_batch` regardless of protocol — the agent
  loop, security gate, hooks, audit, and redaction seams are untouched.

**Non-goals**

- No change to tool *semantics*. `edit_file`'s `<<< === >>>` grammar, `read_file --lines`,
  `search_files` flags all stay exactly as they are — a native call carries the same body
  text in a `body` string parameter (see §3 on the schema shape decision).
- No change to the agent loop's dispatch, parallelism, or approval logic.
- No `tool_choice` forcing at v1. The model decides when to call.
- No structured-output / JSON-mode work.
- No server changes. `aictl-server` keeps its own translator for its own `/v1/messages`
  route; this spec does not merge them (see §2).

## Design

### 1. `ToolProtocol` resolution

```rust
// crates/aictl-core/src/llm.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolProtocol { Native, Xml }

/// Resolve the tool-call protocol for a (provider, model) pair.
///
/// Native is opt-out per model rather than opt-in per provider: a provider
/// whose API supports `tools[]` may still host a model that is bad at it.
pub fn tool_protocol(provider: &str, model: &str) -> ToolProtocol;
```

Resolution order:

1. `AICTL_TOOL_PROTOCOL` config key (`native` | `xml` | `auto`, default `auto`) — a global
   override, and the kill switch.
2. Local providers (`Provider::is_local()` — Ollama, GGUF, MLX) → `Xml`. Ollama's native
   tool support is uneven across models and the local path is where the XML tolerance earns
   its keep.
3. A `NATIVE_TOOL_MODELS` predicate over the `MODELS` catalog
   ([`llm.rs:47`](../../crates/aictl-core/src/llm.rs)) — see §7 on why this becomes a proper
   capability table rather than another prefix-match function.
4. Default `Xml`.

### 2. Where the translation code lives

The server's translator cannot be imported by core. Three options were weighed:

- **(a) Move `translator/ir.rs` + per-provider modules into `aictl-core`** and have the
  server depend on them. Correct long-term shape, largest diff, and the server's IR carries
  Anthropic-API concerns (`cache_control`, `thinking`, PDF blocks, feature gating) the engine
  does not want.
- **(b) Duplicate the shapes in core.** Fast, and guaranteed to drift — exactly the mistake
  `llm/server_proxy.rs` deliberately avoided by reusing `llm/openai`'s `pub(crate)` structs.
- **(c) New `aictl-core::llm::tools_native` module owning a *minimal* IR** — just
  `NativeTool { name, description, input_schema }` and `NativeToolCall { id, name, arguments }`
  — with per-provider serialization living next to each existing provider module.

**Choose (c).** The engine's needs are a strict subset of the server's: no `cache_control`,
no thinking blocks, no PDF, no feature gating, no cross-provider *request* translation
(the engine owns the request shape from the start). The server's translator solves a harder
problem — faithfully round-tripping someone else's Anthropic request — and coupling to it
would drag that surface into the engine.

What is reused is **knowledge, not code**: the per-provider quirks already discovered in
`translator/openai_family.rs`, `translator/gemini.rs`, and `translator/stream/*.rs` (argument
accumulation across deltas, Gemini's `functionCall`/`functionResponse` shape, index-keyed
streaming fragments) are transcribed into the engine's implementation. A comment in each new
module points at its server counterpart so the two stay reviewable together.

### 3. Tool schemas — the shape decision

The tools' current bodies are line-oriented free text. Two possible schema shapes:

**(A) Faithful per-tool schemas** — `edit_file` gets `{path, edits: [{old, new, line_range}]}`,
`search_files` gets `{pattern, dir, regex, case, type, context, max}`, etc.

**(B) A single `body` string per tool** — `{"body": "src/foo.rs\n--lines 40-60"}` — the exact
text the existing parsers already accept.

**Choose (B) for v1, with (A) as a follow-up for the five tools that benefit most.**

Rationale: (B) is a pure transport change. Every tool's parser, every security-gate body
inspection (`search_files` dir extraction, `git` subcommand classification in
`tools::is_parallelizable`, `clipboard read`/`write` split), and every existing test keeps
working unchanged, and the risk of the migration collapses to "does the model fill in `body`
correctly?" — which it does, because the prose examples in the schema `description` are the
same examples it sees in the prompt today.

(A) is where the reliability win actually lives for `edit_file` in particular, but doing both
at once means a structural transport change and 36 parser rewrites in one PR. The follow-up
promotes `edit_file`, `search_files`, `read_file`, `test`, and `exec_shell` to faithful
schemas once the transport is proven, keeping `body` as the shape for the other 31.

Schema generation:

```rust
// crates/aictl-core/src/tools/schema.rs
/// JSON Schema for a built-in tool. Derived from BUILTIN_TOOLS' name +
/// description plus a per-tool `body` description carrying the grammar
/// examples that today live in the system prompt.
pub fn schema_for(name: &str) -> Option<NativeTool>;

/// Every tool the model may call this turn: built-ins minus
/// `disabled_tools`, plus MCP catalogue entries (schema passthrough),
/// plus plugin tools (`schema_hint` or a bare `body` string).
pub fn native_catalogue() -> Vec<NativeTool>;
```

The per-tool `body` descriptions are **lifted verbatim** from the tool-catalogue section of
`SYSTEM_PROMPT` so there is exactly one source of grammar text. `SYSTEM_PROMPT` is
refactored to build its catalogue section from the same table, which incidentally fixes the
existing hazard that a new tool can be added to `BUILTIN_TOOLS` and forgotten in the prompt.

### 4. Provider wiring

Each `call_*` function gains a protocol-aware branch. Signature change:

```rust
pub async fn call_openai(
    api_key: &str, model: &str, messages: &[Message],
    on_token: Option<TokenSink>,
    tools: Option<&[NativeTool]>,          // new — None keeps today's XML path
) -> Result<(String, Vec<NativeToolCall>, TokenUsage), AictlError>;
```

Returning `(text, calls, usage)` rather than `(text, usage)` unifies both paths: on the XML
path `calls` is empty and `run` parses the text as today; on the native path `text` is the
assistant's prose and `calls` is populated. The XML parse stays in `run`, not in the provider
modules, so there is one place where a `ToolCall` is born.

Per-provider specifics:

| Provider | Request | Response | Streaming |
|----------|---------|----------|-----------|
| OpenAI-family (openai, grok, mistral, deepseek, kimi, zai) | `tools: [{type:"function", function:{name, description, parameters}}]` | `choices[0].message.tool_calls[]` | `delta.tool_calls[]` fragments keyed by `index`; accumulate `function.arguments` string across deltas |
| Anthropic | `tools: [{name, description, input_schema}]` | `content[]` blocks of `type: "tool_use"` | `content_block_start` (`tool_use`) → `input_json_delta` partial JSON → `content_block_stop` |
| Gemini | `tools: [{functionDeclarations: [...]}]` | `candidates[0].content.parts[].functionCall` | parts arrive whole per chunk |

Tool *results* go back as native blocks too:

- OpenAI-family: a `{role: "tool", tool_call_id, content}` message per result.
- Anthropic: a user message with `tool_result` content blocks carrying `tool_use_id`.
- Gemini: a `functionResponse` part.

This means `Message` needs to represent a tool result distinctly rather than as a user
message containing `<tool_result>` text. Minimal change:

```rust
pub enum Role { System, User, Assistant, Tool }   // Tool is new
pub struct Message {
    pub role: Role,
    pub content: String,
    pub images: Vec<String>,
    /// Native tool-call correlation. Empty on the XML path.
    pub tool_call_id: Option<String>,     // new — set on Role::Tool messages
    pub tool_calls: Vec<NativeToolCall>,  // new — set on Role::Assistant messages
}
```

`Role::Tool` messages serialize as plain user messages on the XML path (preserving today's
`<tool_result>` envelope text), so session files written by either protocol remain loadable
by the other. Session-format compatibility is a hard requirement — a user flipping models
mid-session must not lose their transcript. `session.rs` gains a migration that treats
missing `tool_call_id` / `tool_calls` fields as defaults (serde `#[serde(default)]`).

### 5. Streaming

`llm::stream::StreamState` keeps its XML hold-back logic unchanged — it is still needed on
the `Xml` path, and it is what makes GGUF/MLX usable. On the `Native` path the hold-back is
skipped entirely (there is no XML to hide) and the state machine instead accumulates
tool-call fragments, emitting a `StreamEvent::ToolCallsComplete(Vec<NativeToolCall>)` at the
end of the assistant message.

`drive_openai_compatible_stream` gains the OpenAI `delta.tool_calls` accumulator. The
Anthropic and Gemini streaming paths get equivalents. The `<phase>` scan added in Phase 4
stays on both paths.

### 6. Agent-loop integration

`run::run_agent_turn`'s parse step becomes protocol-aware:

```rust
let calls: Vec<tools::ToolCall> = if native_calls.is_empty() {
    tools::parse_tool_calls(&response)          // XML path, unchanged
} else {
    native_calls.iter().map(tools::ToolCall::from_native).collect()
};
```

`ToolCall::from_native` maps `{name, arguments}` to the existing `{name, input}` by reading
the `body` field out of the arguments object (shape B). Everything downstream —
`handle_tool_batch`, `is_parallelizable`, `split_context_dependency`, the security gate,
hooks, audit, the duplicate-call guard, the Review hook — is unchanged.

Native parallel tool calls arrive as a `Vec` already, which is exactly what
`handle_tool_batch` takes. The Phase 4 batching work is what makes this drop in cleanly.

### 7. Model capability table

Both this spec and the reasoning-effort spec need per-model capability data, and
`MODELS: &[(&str, &str, &str)]` (provider, model, key-name) cannot carry it. Rather than add
a third prefix-matching function alongside `is_vision_capable` and `context_limit`, promote
the catalogue:

```rust
pub struct ModelInfo {
    pub provider: &'static str,
    pub name: &'static str,
    pub api_key_config_key: &'static str,
    pub context: u64,
    pub vision: bool,
    pub native_tools: bool,
    pub reasoning: ReasoningSupport,   // defined by the reasoning-effort spec
}
pub const MODELS: &[ModelInfo] = &[ … ];
```

`context_limit` and `is_vision_capable` become lookups with their current prefix logic as the
fallback for unlisted models (Ollama/GGUF/MLX names, and models newer than the catalog).
Their existing tests must pass unchanged — that is the migration's safety net.

**This is a prerequisite refactor shared with the reasoning-effort spec.** Do it once, in its
own PR, before either feature. It also removes a real maintenance hazard: `sync-models` today
must remember to touch three separate functions.

### 8. Configuration

| Key | Default | Meaning |
|-----|---------|---------|
| `AICTL_TOOL_PROTOCOL` | `auto` | `native` \| `xml` \| `auto`. Global override / kill switch |

No per-provider key at v1 — `auto` plus the capability table covers the cases, and a global
`xml` is the escape hatch when a provider ships a broken release.

### 9. CLI / desktop surface

- `--info` gains `tool-protocol: native (anthropic/claude-sonnet-5)`.
- `/tools` gains a header line stating the active protocol, and on the native path notes
  that the catalogue is sent as schemas rather than prompt text.
- `/model` menu: no change (protocol is derived, not chosen).
- Desktop: no new UI. The protocol is visible in the existing settings/info surface only.

### 10. Integration points

| File | Change |
|------|--------|
| `crates/aictl-core/src/llm.rs` | `ModelInfo` catalogue (§7); `ToolProtocol`; `tool_protocol()`; `NativeTool` / `NativeToolCall` |
| `crates/aictl-core/src/llm/tools_native.rs` | **New** — minimal IR + shared serialization helpers |
| `crates/aictl-core/src/llm/{openai,anthropic,gemini,grok,mistral,deepseek,kimi,zai}.rs` | `tools` parameter; native request emission; `tool_calls` parsing |
| `crates/aictl-core/src/llm/stream.rs` | Native-path fragment accumulator; `StreamEvent::ToolCallsComplete`; skip XML hold-back when native |
| `crates/aictl-core/src/llm/server_proxy.rs` | Pass `tools[]` through — the server speaks OpenAI shape, so this is nearly free |
| `crates/aictl-core/src/tools/schema.rs` | **New** — `schema_for`, `native_catalogue` |
| `crates/aictl-core/src/tools.rs` | `ToolCall::from_native`; catalogue table shared with the prompt |
| `crates/aictl-core/src/config.rs` | `AICTL_TOOL_PROTOCOL`; build the prompt's tool catalogue from the shared table; omit it entirely when native |
| `crates/aictl-core/src/run.rs` | Protocol-aware parse step; thread `tools` into every provider call |
| `crates/aictl-core/src/session.rs` | `#[serde(default)]` on the two new `Message` fields; round-trip test |
| `crates/aictl-cli/src/commands/{info,tools}.rs` | Surface the active protocol |
| `docs/PROVIDERS.md`, `docs/TOOLS.md`, `docs/CONFIG.md`, `CLAUDE.md` | Document the two protocols and the table |

### 11. Testing

**Unit**

- `tool_protocol`: local providers → `Xml` always, even with `AICTL_TOOL_PROTOCOL=native`
  (the override does not apply to providers whose API has no `tools[]`); catalog hit →
  `Native`; unknown model → `Xml`; global override honored for cloud providers.
- `ModelInfo` migration: every existing `context_limit` and `is_vision_capable` test passes
  unchanged.
- `schema_for`: all 36 built-ins produce valid JSON Schema; disabled tools are excluded from
  `native_catalogue`; an MCP tool's `input_schema` passes through byte-identical.
- `ToolCall::from_native`: `{"body": "src/x.rs"}` → `input == "src/x.rs"`; missing `body` →
  a `ToolCall` whose input is empty (and whose tool will reject it with its normal error, not
  a panic); arguments that arrive as a JSON *string* rather than an object (an OpenAI quirk)
  are parsed then read.
- Per-provider request serialization: golden-file tests on the emitted JSON body for each of
  the three families, asserting tool names, descriptions, and schema shape.
- Per-provider response parsing: golden fixtures of real response bodies → expected
  `Vec<NativeToolCall>`.
- Streaming accumulators: OpenAI fragments split mid-argument across three deltas reassemble
  to valid JSON; Anthropic `input_json_delta` sequence reassembles; Gemini whole-part case.
- `Message` serde: a session file written pre-change deserializes with the new fields
  defaulted; a native-protocol session loads on the XML path with `<tool_result>` text intact.

**Integration** (mock-LLM harness)

- Mock provider in native mode returns a `tool_calls[]` with two reads → the loop dispatches
  a parallel batch identically to the XML path, and the audit log entries are indistinguishable.
- A tool denied by the security gate on the native path produces a `Role::Tool` result
  message carrying the denial, correlated by `tool_call_id`.
- Protocol flip mid-session: run two turns native, set `AICTL_TOOL_PROTOCOL=xml`, run a third;
  the transcript stays coherent and the third turn's tool call succeeds.

**Eval**

The full 20-task set, run on the same three models before and after, is the acceptance
criterion. Expected signal: fewer `20t` iteration-cap failures and a drop in
`looks_like_malformed_tool_call` occurrences (add a counter to the audit log for this).

### 12. Rollout

Five PRs, strictly ordered:

1. **`ModelInfo` catalogue refactor** (§7). Zero behavior change; all existing tests pass.
2. **Schemas + shared catalogue table** (§3). `native_catalogue()` exists and is tested; the
   system prompt is rebuilt from the same table. Still no native dispatch.
3. **OpenAI-family native path** — the widest coverage per unit of work (six providers share
   one shape). Gated by `AICTL_TOOL_PROTOCOL`, default `auto` with the catalog listing only
   OpenAI-family models as `native_tools: true` initially.
4. **Anthropic native path.**
5. **Gemini native path**, plus dropping the tool catalogue from the prompt when native, plus
   the `--info` / `/tools` surface.

Each of 3–5 is independently revertable by flipping one column in the catalog to `false`.

### 13. Verification

1. `cargo build --workspace` (default + `--all-features`) and `cargo lint` clean.
2. `cargo test` clean, including golden-file tests for all three families.
3. `AICTL_TOOL_PROTOCOL=xml` reproduces today's behavior bit-for-bit on the integration
   suite (regression gate — this must hold through every PR).
4. A real session against each of the three families completes a multi-tool task natively;
   the audit log shows the same tool names and outcomes as the XML path.
5. System prompt shrinks measurably when native: assert
   `build_system_prompt().len()` on a native model is smaller than on an XML model by
   roughly the catalogue size.
6. CI gates:
   ```bash
   grep -rE 'ToolProtocol|native_catalogue|NativeToolCall' crates/aictl-server/src/  # must be empty
   grep -rn 'parse_tool_calls' crates/aictl-core/src/llm/                            # must be empty
   ```

### 14. Risks

- **Session-format compatibility.** The `Message` change is the highest-risk piece: a bad
  serde migration corrupts users' saved sessions. Mitigation: `#[serde(default)]` on both new
  fields, an explicit round-trip test against a checked-in pre-change session fixture, and
  PR 1–2 landing the struct change ahead of any behavior change so it soaks.
- **Provider quirks not caught by fixtures.** Real APIs deviate from docs (empty
  `arguments: ""`, duplicate indices, `tool_calls` alongside non-empty `content`). Mitigation:
  transcribe the quirks already handled in `crates/aictl-server/src/messages/translator/`,
  and make `from_native` total — never panic, always produce a `ToolCall` the normal error
  path can reject.
- **Losing the foreign-dialect tolerance.** The XML normalization exists because a model
  leaked another dialect. On the native path that class of bug reappears as malformed JSON
  arguments. Mitigation: `from_native` tolerates string-encoded argument objects and missing
  fields; the XML path stays for exactly the providers where the tolerance was needed.
- **The `body`-string schema underdelivers.** If evals show no reliability gain from (B),
  the win was in faithful schemas all along and the follow-up becomes mandatory rather than
  optional. Mitigation: this is *why* the eval harness ships first — the decision becomes
  data rather than argument.
- **Prompt-caching interaction.** Dropping the catalogue from the system prompt changes the
  cached prefix for Anthropic, invalidating warm caches once on rollout. One-time cost, worth
  noting in the release notes.
- **Two translators drift.** The engine's native path and the server's translator solve
  overlapping problems separately. Mitigation: cross-referencing comments in both, and a
  note in `CLAUDE.md` that a provider quirk fixed in one should be checked against the other.

### 15. Open questions

- **Shape (A) vs (B)** is deliberately deferred to eval data. If (B) shows a gain and (A)
  shows more, the follow-up is scoped to five tools.
- **Should `tool_choice` ever be forced?** Forcing a tool call during the Explore phase might
  stop models that answer from memory instead of reading. Tempting, and easy to overdo.
  Lean: not at v1; revisit with eval evidence on tasks 13–15.
- **Ollama native tools.** Some Ollama models do support `tools[]`. Excluded at v1 because
  the failure mode is silent (the model ignores the field and emits prose). Revisit with a
  per-model opt-in once someone reports a model where it works.
- **Does `server_proxy` need protocol negotiation?** The server accepts OpenAI-shaped
  requests including `tools[]`, so passing them through should work — but the server's
  `/v1/chat/completions` route translates to the engine's `Vec<Message>` and dispatches via
  `call_<provider>`, which will now itself be protocol-aware. Verify there is no double
  translation before enabling native through the proxy; default it to `Xml` until confirmed.
