# LLM Pricing

Estimated daily and monthly API cost of running aictl intensively for
a working day (~8 hours of active use), broken down by the two usage
patterns aictl is typically used for: as a general chat assistant and
as a coding agent. Monthly figures assume **22 working days per month**
(a typical work-month with weekends and a few days off — not a full
30-day calendar month).

Rates are the short-context list prices baked into `src/llm.rs` as of
April 2026. Providers with dual-tier pricing above a long-context
threshold (GPT-5.2/5.4, Gemini 3.1 Pro) are priced at the short-context
rate — aictl's cost meter always reports that tier.

All figures are in USD. Token counts are estimates; your mileage will
vary with question length, tool output size, and how often you run
`/clear` or `/compact`.

## Methodology

### Scenario A — Chat assistant

You're using aictl in interactive REPL mode as a general-purpose
knowledge tool: asking questions, doing quick lookups, running the
occasional web fetch, datetime query, or small file read. Most turns
don't trigger a tool call at all.

- 150 turns / day
- ~6K input tokens per turn (system prompt + short history + maybe a
  small tool result)
- ~400 output tokens per turn
- **Daily totals: ~0.9M input, ~60K output**

### Scenario B — Coding agent

You're using aictl with the full tool catalog to read, search, edit,
and shell-execute across a real codebase. Each "task" is a multi-turn
agent loop that reads several files, runs commands, and makes edits.

- 50 tasks / day
- 10 iterations per task on average (the agent loop caps at 20)
- Input grows across iterations as file contents and tool results
  accumulate in history and the full conversation is re-sent each
  iteration
- ~295K cumulative input tokens per task
- ~15K output tokens per task (long, code-heavy responses)
- **Daily totals: ~15M input, ~750K output**

### Caching assumption

The "with prompt caching" columns assume the provider supports prompt
caching and that a typical fraction of input tokens hit the cache:

- **70% cache hit** in the chat scenario — system prompt and recent
  history get reused across turns
- **80% cache hit** in the coding scenario — each agent-loop iteration
  reuses the growing history, so caching works especially well

The cache-read discount used is the one baked into
`cache_read_multiplier` in `src/llm.rs`: **10%** for Anthropic and
OpenAI GPT-5.x, **25%** for Gemini, Grok, DeepSeek, and Kimi. Z.ai's
GLM prompt cache is not yet modeled by the aictl cost meter, so its
"with cache" figure is identical to the no-cache figure.

## Scenario A — Chat assistant

| Model | Daily (no cache) | Daily (cached) | Monthly (no cache) | Monthly (cached) |
|---|---|---|---|---|
| `claude-opus-4-6` | $6.00 | $3.17 | $132.00 | $69.74 |
| `claude-sonnet-4-6` | $3.60 | $1.90 | $79.20 | $41.80 |
| `claude-haiku-4-5` | $1.20 | $0.63 | $26.40 | $13.86 |
| `gpt-5.4` | $3.15 | $1.73 | $69.30 | $38.06 |
| `gpt-5.4-mini` | $0.95 | $0.52 | $20.90 | $11.44 |
| `gpt-5.4-nano` | $0.26 | $0.14 | $5.72 | $3.08 |
| `gemini-3.1-pro-preview` | $2.52 | $1.58 | $55.44 | $34.76 |
| `gemini-3.1-flash-lite-preview` | $0.32 | $0.20 | $7.04 | $4.40 |
| `grok-4` | $3.60 | $2.18 | $79.20 | $47.96 |
| `grok-4-fast-reasoning` | $0.21 | $0.12 | $4.62 | $2.64 |
| `deepseek-chat` | $0.31 | $0.18 | $6.82 | $3.96 |
| `glm-5.1` | $1.52 | $1.52 | $33.44 | $33.44 |

## Scenario B — Coding agent

| Model | Daily (no cache) | Daily (cached) | Monthly (no cache) | Monthly (cached) |
|---|---|---|---|---|
| `claude-opus-4-6` | $93.75 | $39.75 | $2,062.50 | $874.50 |
| `claude-sonnet-4-6` | $56.25 | $23.85 | $1,237.50 | $524.70 |
| `claude-haiku-4-5` | $18.75 | $7.95 | $412.50 | $174.90 |
| `gpt-5.4` | $48.75 | $21.75 | $1,072.50 | $478.50 |
| `gpt-5.4-mini` | $14.63 | $6.53 | $321.86 | $143.66 |
| `gpt-5.4-nano` | $3.94 | $1.78 | $86.68 | $39.16 |
| `gemini-3.1-pro-preview` | $39.00 | $21.00 | $858.00 | $462.00 |
| `gemini-3.1-flash-lite-preview` | $4.88 | $2.63 | $107.36 | $57.86 |
| `grok-4` | $56.25 | $29.25 | $1,237.50 | $643.50 |
| `grok-4-fast-reasoning` | $3.38 | $1.58 | $74.36 | $34.76 |
| `deepseek-chat` | $4.88 | $2.45 | $107.36 | $53.90 |
| `glm-5.1` | $24.30 | $24.30 | $534.60 | $534.60 |

## Takeaways

- **Chat usage is cheap on almost everything.** Even the flagship
  models (GPT-5.4, Gemini 3.1 Pro, Grok 4, Claude Sonnet 4.6) cost
  roughly $35–$48/month cached and $55–$80/month uncached. Only Opus
  4.6 approaches meaningful monthly cost for chat use (~$70/month
  cached, $132/month uncached).
- **Coding agents cost 10–20× more than chat.** Cumulative history and
  long outputs are the dominant drivers, not the tool calls themselves.
  Flagship-tier models land in the $460–$640/month range with caching
  and $860–$1,240/month without. Claude Opus 4.6 alone runs
  ~$875/month cached and over $2,000/month without — budget like a
  second SaaS subscription, not a utility.
- **Prompt caching cuts coding-agent costs roughly in half** on
  providers that support it (Anthropic, OpenAI, Gemini, Grok,
  DeepSeek). If you're evaluating aictl for intensive coding work,
  the sticker prices without caching are misleading — aictl already
  uses Anthropic's explicit prompt cache and accounts for cached
  reads on every provider.
- **The value plays for heavy coding use** all come in under
  **$60/month cached** for 50 tasks × 10 iterations × 22 working days:
  `grok-4-fast-reasoning` ($34.76/month), `deepseek-chat` ($53.90),
  `gemini-3.1-flash-lite-preview` ($57.86), and `gpt-5.4-nano`
  ($39.16). These are viable "always-on" coding assistants even for
  individuals.
- **The flagship cluster** — GPT-5.4, Gemini 3.1 Pro, and Claude
  Sonnet 4.6 — sits around **$460–$525/month cached** for intensive
  coding use. Pick based on quality and speed; cost barely
  discriminates between them.
- **Claude Opus 4.6** is the only model where intensive use is
  genuinely expensive: ~$875/month cached, ~$2,063/month uncached.
  Worth reserving for hard problems rather than running as a daily
  driver.

## TL;DR — mental model

If you're picking a mental model for what aictl costs per month:

| | Cheapest cached | Flagship cluster cached | Opus 4.6 cached |
|---|---|---|---|
| **Chat** | $2.64 (grok-4-fast) | ~$35–$48 | $69.74 |
| **Coding** | $34.76 (grok-4-fast) | ~$460–$525 | $874.50 |

The spread is: **chat = essentially free on value models, ~$50 on
flagships, ~$70 on Opus**. **Coding = $35 on value models, ~$500 on
flagships, ~$875 on Opus**. The ~60× jump from chat to coding for
any given model is the real headline.

These are the cached figures because aictl explicitly drives
Anthropic's prompt cache, and the other providers cache automatically
during sustained sessions. The uncached numbers in the detailed
tables above represent the worst case if caching silently breaks —
for Anthropic specifically, that worst case is roughly 2.4× the
cached figure.
