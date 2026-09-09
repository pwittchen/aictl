# Spec: Extended thinking / reasoning effort

> Report item **3** (Tier 1). Small change, large quality delta for coding tasks. Shares the
> `ModelInfo` catalogue refactor with the native-tool-calling spec — do that refactor once.

## Context

No reasoning controls are wired anywhere in the engine:

- `grep -n "thinking" crates/aictl-core/src/llm/anthropic.rs` is empty. The Anthropic request
  struct ([`llm/anthropic.rs:11`](../../crates/aictl-core/src/llm/anthropic.rs)) carries
  `model`, `messages`, `system`, `max_tokens`, `temperature`, `stream` — no `thinking` block.
- `grep -n "reasoning" crates/aictl-core/src/llm/openai.rs` is empty. `OpenAiRequest`
  ([`llm/openai.rs:13`](../../crates/aictl-core/src/llm/openai.rs)) has no `reasoning_effort`.
- Gemini's `thinkingConfig` is likewise absent.

For coding work specifically, a large share of current model quality lives behind these
switches. An o-series or GPT-5-class model called without `reasoning_effort` runs at the
provider's default; Claude models called without a `thinking` block do no extended thinking at
all. The agent is leaving the single cheapest quality improvement on the table.

There is a second, subtler cost: `config::MAX_RESPONSE_TOKENS = 4096`
([`config.rs:56`](../../crates/aictl-core/src/config.rs)) is the hard `max_tokens` on every
call. Anthropic requires `max_tokens > thinking.budget_tokens`, so enabling thinking without
touching this constant produces an immediate 400.

## Goals & Non-goals

**Goals**

- A per-model capability declaration of what reasoning control the model accepts.
- A single user-facing effort knob — `off` / `low` / `medium` / `high` — mapped per provider
  to the right wire format.
- Correct `max_tokens` handling so thinking budgets do not collide with the response cap.
- Reasoning/thinking output surfaced through the existing `AgentUI::show_reasoning` seam
  rather than mixed into the answer.
- Sensible default: **`medium` in coding-agent mode, `off` otherwise**, so the cost increase
  is opt-in-by-context rather than global.
- Thinking blocks correctly preserved or dropped in the transcript per provider requirements.

**Non-goals**

- No per-turn effort switching by the model itself. The user sets it; the host applies it.
- No thinking-token cost modeling beyond what `TokenUsage` already reports (thinking tokens
  bill as output tokens on every provider we target, which `estimate_cost` already handles).
- No interleaved-thinking or tool-use-with-thinking beta features.
- No local-provider support. GGUF/MLX have no equivalent control.
- No server changes — `aictl-server`'s `/v1/messages` passthrough already forwards `thinking`
  verbatim for Anthropic, which is a separate, already-working path.

## Design

### 1. Capability declaration

Extends the `ModelInfo` catalogue introduced by the native-tool-calling spec (§7 there):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReasoningSupport {
    /// No reasoning control. Send nothing.
    None,
    /// Anthropic extended thinking: `thinking: {type:"enabled", budget_tokens:N}`.
    AnthropicThinking,
    /// OpenAI-family `reasoning_effort: "low"|"medium"|"high"`.
    OpenAiEffort,
    /// Gemini `generationConfig.thinkingConfig.thinkingBudget: N`.
    GeminiBudget,
    /// The model reasons unconditionally and rejects the control field
    /// (o1-class, some Grok reasoning models). Send nothing, but report
    /// "always on" in `--info` rather than "unsupported".
    Always,
}
```

`ReasoningSupport::Always` matters: sending `reasoning_effort` to a model that always reasons
is a 400 on some providers, and reporting it as "unsupported" to the user is misleading.

### 2. The effort knob

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReasoningEffort { Off, Low, Medium, High }

/// Resolve the effort level for this process.
///
/// `AICTL_REASONING_EFFORT` when set; otherwise `Medium` in coding-agent
/// mode and `Off` elsewhere.
pub fn reasoning_effort() -> ReasoningEffort;
```

Mapping to the wire, per `ReasoningSupport`:

| Effort | AnthropicThinking (`budget_tokens`) | OpenAiEffort | GeminiBudget (`thinkingBudget`) |
|--------|-------------------------------------|--------------|----------------------------------|
| `Off` | field omitted | field omitted | `0` (explicit disable) |
| `Low` | `2048` | `"low"` | `2048` |
| `Medium` | `8192` | `"medium"` | `8192` |
| `High` | `24576` | `"high"` | `24576` |

The budget numbers are starting points, not tuned values. They are exactly the kind of thing
the eval harness exists to calibrate — see Open questions.

### 3. `max_tokens` interaction

Anthropic requires `max_tokens > thinking.budget_tokens`. Today every call sends
`MAX_RESPONSE_TOKENS = 4096`, which is below every budget above except `Off`.

Fix: `max_tokens` becomes a function of the effort level rather than a bare constant.

```rust
/// Response-token cap for a call, accounting for any thinking budget.
/// Thinking tokens are drawn from the same allowance on Anthropic, so the
/// cap must cover the budget plus room for the actual answer.
pub fn max_response_tokens(support: ReasoningSupport, effort: ReasoningEffort) -> u32 {
    match support {
        ReasoningSupport::AnthropicThinking => thinking_budget(effort) + MAX_RESPONSE_TOKENS,
        _ => MAX_RESPONSE_TOKENS,
    }
}
```

`MAX_RESPONSE_TOKENS` keeps its current value and meaning ("room for the answer itself").
OpenAI's `max_completion_tokens` and Gemini's `maxOutputTokens` do **not** include reasoning
tokens in the same way, so they keep the plain constant — verify per provider against current
docs during implementation rather than trusting this table.

### 4. Provider wiring

**Anthropic** ([`llm/anthropic.rs`](../../crates/aictl-core/src/llm/anthropic.rs)):

```rust
#[derive(Serialize)]
struct ThinkingConfig {
    #[serde(rename = "type")] kind: &'static str,   // "enabled"
    budget_tokens: u32,
}

struct AnthropicRequest {
    // … existing fields …
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<ThinkingConfig>,
}
```

Response handling: `content[]` gains blocks of `type: "thinking"` carrying a `thinking` string
and a `signature`. Two requirements:

- Thinking text goes to `AgentUI::show_reasoning`, never into the answer string.
- **Thinking blocks must be preserved verbatim in the transcript when the assistant turn also
  contains tool calls** — Anthropic rejects a follow-up request whose assistant turn dropped
  the signed thinking block. This interacts directly with the native-tool-calling spec's
  `Message` changes: a `thinking` field (text + signature) is added there alongside
  `tool_calls`, and is serialized back on subsequent requests. On the XML tool path there are
  no tool_use blocks, so thinking can be dropped safely.

Temperature must be unset (or `1`) when thinking is enabled — Anthropic rejects other values.

**OpenAI-family** ([`llm/openai.rs`](../../crates/aictl-core/src/llm/openai.rs) and the five
providers reusing its shapes): add

```rust
#[serde(skip_serializing_if = "Option::is_none")]
reasoning_effort: Option<&'static str>,
```

Reasoning summaries, where the provider returns them, route to `show_reasoning`. Note that
several OpenAI reasoning models also reject `temperature` — gate it the same way.

**Gemini** ([`llm/gemini.rs`](../../crates/aictl-core/src/llm/gemini.rs)): add
`generationConfig.thinkingConfig.thinkingBudget`. Gemini 2.5+ models think by default, so
`Off` must send an explicit `0` rather than omitting the field.

**Grok / Mistral / DeepSeek / Kimi / Z.ai**: these reuse OpenAI shapes. Their reasoning models
are marked `Always` or `OpenAiEffort` per the catalog; DeepSeek's reasoner returns a
`reasoning_content` field which routes to `show_reasoning`.

**server_proxy**: pass `reasoning_effort` through in the OpenAI-shaped body. The server
forwards to the upstream provider, so it works without server changes; verify no double
application.

### 5. Streaming

Thinking deltas arrive as their own event types (`content_block_delta` with
`thinking_delta` on Anthropic; reasoning summary deltas on OpenAI). `StreamState` routes them
to a distinct `StreamEvent::Reasoning(String)` rather than `Delta`, so they never render as
answer text. `InteractiveUI` renders them dimmed via the existing `show_reasoning` styling;
`PlainUI` suppresses them in `text`/`json` formats exactly as it does other reasoning chatter.

### 6. Configuration

| Key | Default | Meaning |
|-----|---------|---------|
| `AICTL_REASONING_EFFORT` | unset → `medium` in coding mode, `off` otherwise | `off` \| `low` \| `medium` \| `high` |
| `AICTL_REASONING_SHOW` | `true` | Render thinking/reasoning text through `show_reasoning`; `false` consumes it silently |

### 7. CLI / desktop surface

- `/behavior` gains a second dimension. Today it is a two-item menu (human-in-the-loop /
  auto) in [`commands/behavior.rs`](../../crates/aictl-cli/src/commands/behavior.rs); it
  becomes a two-section menu: *approval* (unchanged) and *reasoning effort* (off / low /
  medium / high, with the current model's support state shown and unsupported levels
  greyed out).
- `--reasoning <off|low|medium|high>` — one-launch override.
- `--info` gains `reasoning: medium (anthropic thinking, budget 8192)` or
  `reasoning: unsupported by <model>` or `reasoning: always on`.
- `/model` menu annotates reasoning-capable models with a small marker, the same way vision
  capability is surfaced today.
- Desktop: a `reasoning_effort_status` / `reasoning_effort_set` Tauri command pair and a
  four-way selector under Settings → General, next to the coding-agent toggle.

### 8. Integration points

| File | Change |
|------|--------|
| `crates/aictl-core/src/llm.rs` | `ReasoningSupport` on `ModelInfo`; `ReasoningEffort`; `thinking_budget`; `max_response_tokens` |
| `crates/aictl-core/src/config.rs` | `AICTL_REASONING_EFFORT`, `AICTL_REASONING_SHOW`, `reasoning_effort()` |
| `crates/aictl-core/src/llm/anthropic.rs` | `thinking` request field; thinking-block parsing; temperature gate; transcript preservation |
| `crates/aictl-core/src/llm/openai.rs` | `reasoning_effort` field; reasoning-summary routing |
| `crates/aictl-core/src/llm/{grok,mistral,deepseek,kimi,zai}.rs` | Reuse the OpenAI field; DeepSeek `reasoning_content` routing |
| `crates/aictl-core/src/llm/gemini.rs` | `thinkingConfig`; explicit `0` for `Off` |
| `crates/aictl-core/src/llm/stream.rs` | `StreamEvent::Reasoning`; per-provider thinking-delta routing |
| `crates/aictl-core/src/llm/server_proxy.rs` | Pass effort through |
| `crates/aictl-cli/src/commands/behavior.rs` | Two-section menu |
| `crates/aictl-cli/src/main.rs` | `--reasoning` flag |
| `crates/aictl-cli/src/commands/{info,model}.rs` | Surface support + level |
| `crates/aictl-desktop/src/commands/` | Two Tauri commands + Settings UI |
| `docs/PROVIDERS.md`, `docs/CONFIG.md`, `docs/USAGE.md`, `CLAUDE.md` | Document the knob and per-provider mapping |

### 9. Testing

**Unit**

- `reasoning_effort()`: unset + coding mode → `Medium`; unset + normal → `Off`; explicit key
  wins in both; unparseable value falls back to the default rather than erroring.
- `max_response_tokens`: Anthropic + `High` exceeds the budget; every other support type
  returns `MAX_RESPONSE_TOKENS` unchanged.
- Request serialization goldens per provider: `Off` omits the field entirely (Gemini sends
  `0`); each level maps to the documented value; `ReasoningSupport::None` and `Always` never
  emit the field regardless of effort.
- Temperature gating: Anthropic request with thinking enabled omits `temperature`.
- Anthropic response parsing: a `content[]` with `thinking` + `text` + `tool_use` blocks
  yields the answer text only, the thinking text via the reasoning channel, and preserves the
  thinking block (with signature) in the assistant `Message`.
- Transcript round-trip: an assistant message carrying thinking + tool_calls re-serializes
  with the signature intact.
- Streaming: `thinking_delta` events never reach `StreamEvent::Delta`.

**Integration**

- Mock provider emitting thinking blocks: the final answer contains no thinking text, and
  `show_reasoning` was called.
- `AICTL_REASONING_SHOW=false` suppresses the reasoning output but still preserves the block
  in the transcript.

**Eval**

Run the 20-task set at `off` / `medium` / `high` on one Anthropic and one OpenAI model. This
is the calibration for the budget table in §2 and the primary justification for the default.
Record cost alongside pass rate — the interesting number is pass-rate-per-dollar, not pass
rate.

### 10. Rollout

Three PRs (after the `ModelInfo` refactor lands):

1. **Knob + capability table + `max_tokens` handling.** No provider emits anything yet;
   `--info` reports the resolved level. Pure plumbing, fully testable.
2. **Anthropic thinking**, including the transcript-preservation requirement. The highest
   value and the highest risk; lands alone.
3. **OpenAI-family + Gemini**, plus the `/behavior` menu, `--reasoning` flag, and desktop
   surface.

### 11. Verification

1. `cargo build --workspace` and `cargo lint` clean.
2. `cargo test` clean including per-provider serialization goldens.
3. `AICTL_REASONING_EFFORT=off` produces byte-identical request bodies to today for every
   provider (regression gate).
4. A real coding task on `claude-sonnet-5` at `medium` completes with thinking visible in the
   reasoning channel and absent from the answer.
5. A multi-turn Anthropic session with thinking + tool calls does not 400 on the second turn
   (the transcript-preservation regression).
6. Eval comparison table committed at `off` vs `medium` vs `high`.
7. CI gate: `grep -rE 'ReasoningEffort|thinking_budget' crates/aictl-server/src/` empty.

### 12. Risks

- **Silent cost increase.** Defaulting coding mode to `medium` raises per-turn cost
  materially. Mitigation: `--info` and `/behavior` both show the active level; the first
  release note calls it out; `/stats` already tracks cost so the change is visible to users.
- **Anthropic transcript rejection.** Dropping a signed thinking block from an assistant turn
  that also called a tool causes a 400 on the *next* request — a failure that only appears in
  multi-turn tool sessions and is easy to miss in single-turn testing. Mitigation: it is an
  explicit integration test (verification step 5), and it is the reason PR 2 lands alone.
- **`max_tokens` inflation truncating budgets.** Getting the Anthropic arithmetic wrong
  produces either a 400 or silently truncated answers. Mitigation: unit test asserts
  `max_tokens > budget_tokens` at every level.
- **Provider docs drift.** Which models accept which field changes with every release. The
  `sync-models` skill already walks providers for new models; it gains a step to check the
  reasoning column. Mitigation: `ReasoningSupport::None` is the safe default for unlisted
  models, so a drift produces "no reasoning" rather than an error.
- **Reasoning text leaking into the answer** on a provider whose streaming shape we misread —
  visible and embarrassing. Mitigation: golden streaming fixtures per provider, and the
  answer-text assertion in the integration test.

### 13. Open questions

- **Budget values in §2 are guesses.** 8192 for `medium` is a plausible starting point, not a
  measured one. The eval run in §9 sets them.
- **Should effort scale with phase?** High during Plan, low during Code is intuitively right
  and would cut cost meaningfully. It also adds a per-call decision the user cannot see.
  Lean: not at v1; revisit once phase tracking is proven reliable enough to key spend on.
- **Should `Off` be the coding-mode default after all?** Depends entirely on the
  pass-rate-per-dollar number. The spec commits to `medium` provisionally so the eval has a
  hypothesis to falsify.
- **DeepSeek `reasoning_content` in the transcript** — does it need preserving across turns
  like Anthropic's signed blocks, or is it purely informational? Verify against current docs;
  default to dropping it (informational) unless a multi-turn failure says otherwise.
