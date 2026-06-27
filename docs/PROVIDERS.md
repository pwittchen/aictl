# Providers

aictl supports eleven LLM providers — eight remote APIs plus Ollama, native GGUF inference via llama.cpp, and native MLX inference on Apple Silicon. Each provider needs its own API key in `~/.aictl/config` (see [CONFIG.md](CONFIG.md)).

Per-token pricing tables for each provider follow. For realistic chat and coding-agent workday cost estimates across the whole catalog, see [LLM_PRICING.md](LLM_PRICING.md).

## OpenAI

Requires `LLM_OPENAI_API_KEY`. Supported models with cost estimates (input/output per 1M tokens):

| Model | Input | Output |
|-------|-------|--------|
| `gpt-4.1-nano` | $0.10 | $0.40 |
| `gpt-4.1-mini` | $0.40 | $1.60 |
| `gpt-4.1` | $2.00 | $8.00 |
| `gpt-4o-mini` | $0.15 | $0.60 |
| `gpt-4o` | $2.50 | $10.00 |
| `gpt-5-mini` | $0.25 | $2.00 |
| `gpt-5` | $1.25 | $10.00 |
| `gpt-5.2` | $1.75 | $14.00 |
| `gpt-5.2-pro` | $30.00 | $180.00 |
| `gpt-5.3-codex` | $1.75 | $14.00 |
| `gpt-5.4-nano` | $0.20 | $1.25 |
| `gpt-5.4-mini` | $0.75 | $4.50 |
| `gpt-5.4` | $2.50 | $15.00 |
| `gpt-5.4-pro` | $60.00 | $270.00 |
| `gpt-5.5` | $5.00 | $30.00 |
| `gpt-5.5-pro` | $30.00 | $180.00 |
| `o4-mini` | $1.10 | $4.40 |
| `o3` | $2.00 | $8.00 |
| `o1` | $15.00 | $60.00 |

GPT-5.2, GPT-5.4, and GPT-5.5 use dual-tier pricing that doubles above the 272K context threshold; the table shows the short-context rates. The cost meter in aictl always reports the short-context price.

## Anthropic

Requires `LLM_ANTHROPIC_API_KEY`. Supported models with cost estimates (input/output per 1M tokens):

| Model | Input | Output |
|-------|-------|--------|
| `claude-haiku-*` (3.x) | $0.25 | $1.25 |
| `claude-haiku-4-*` | $1.00 | $5.00 |
| `claude-sonnet-*` | $3.00 | $15.00 |
| `claude-opus-4-5-*` / `claude-opus-4-6-*` / `claude-opus-4-7-*` / `claude-opus-4-8-*` | $5.00 | $25.00 |
| `claude-fable-5` | $10.00 | $50.00 |
| `claude-opus-4-*` (older) | $15.00 | $75.00 |

## Google Gemini

Requires `LLM_GEMINI_API_KEY`. Supported models with cost estimates (input/output per 1M tokens):

| Model | Input | Output |
|-------|-------|--------|
| `gemini-3.5-flash` | $1.50 | $9.00 |
| `gemini-3.1-pro-preview` | $2.00 | $12.00 |
| `gemini-3-flash-preview` | $0.50 | $3.00 |
| `gemini-3.1-flash-lite` | $0.25 | $1.50 |
| `gemini-3.1-flash-lite-preview` | $0.25 | $1.50 |
| `gemini-2.5-pro` | $1.25 | $10.00 |
| `gemini-2.5-flash` | $0.30 | $2.50 |
| `gemini-2.5-flash-lite` | $0.10 | $0.40 |

Gemini 3.1 Pro uses dual-tier pricing that doubles above a 200K context threshold; the table shows the short-context rates. `gemini-2.0-flash` has been removed from the model list because Google is shutting it down on June 1, 2026.

## xAI Grok

Requires `LLM_GROK_API_KEY`. Supported models with cost estimates (input/output per 1M tokens):

| Model | Input | Output |
|-------|-------|--------|
| `grok-4.3` | $1.25 | $2.50 |
| `grok-4.20-0309-reasoning` / `grok-4.20-0309-non-reasoning` / `grok-4.20-multi-agent-0309` | $1.25 | $2.50 |
| `grok-4` | $3.00 | $15.00 |
| `grok-build-0.1` | $1.00 | $2.00 |
| `grok-3-mini` | $0.30 | $0.50 |

Grok 4.20 ships with a 2M-token context window, the largest available across frontier models. Grok 4.3 (released April 30, 2026) is the new flagship at a 1M-token context window — pricing doubles above the 200K input threshold. `grok-build-0.1` (released May 20, 2026) is an agentic-coding model with a 256K-token context window and text+image input, served through the same OpenAI-compatible Chat Completions endpoint.

## Mistral

Requires `LLM_MISTRAL_API_KEY`. Supported models with cost estimates (input/output per 1M tokens):

| Model | Input | Output |
|-------|-------|--------|
| `mistral-large-latest` | $2.00 | $6.00 |
| `mistral-medium-latest` | $0.40 | $2.00 |
| `mistral-small-latest` | $0.10 | $0.30 |
| `magistral-medium-2509` | $2.00 | $5.00 |
| `magistral-small-2509` | $0.50 | $1.50 |
| `devstral-2512` | $0.40 | $2.00 |
| `labs-devstral-small-2512` | $0.10 | $0.30 |
| `codestral-latest` | $0.30 | $0.90 |
| `ministral-14b-latest` | $0.20 | $0.20 |
| `ministral-8b-latest` | $0.15 | $0.15 |
| `ministral-3b-latest` | $0.10 | $0.10 |

The Ministral 3 series (3B / 8B / 14B, released December 2025) are compact edge-class models priced with identical input and output rates.

## DeepSeek

Requires `LLM_DEEPSEEK_API_KEY`. Supported models with cost estimates (input/output per 1M tokens):

| Model | Input | Output |
|-------|-------|--------|
| `deepseek-v4-pro` | $1.74 | $3.48 |
| `deepseek-v4-flash` | $0.14 | $0.28 |
| `deepseek-chat` | $0.28 | $0.42 |
| `deepseek-reasoner` | $0.28 | $0.42 |

`deepseek-chat` and `deepseek-reasoner` are now legacy aliases that route to `deepseek-v4-flash` upstream — they remain in the catalog for backward compatibility. `deepseek-v4-pro` is the new flagship MoE (1.6T total / 49B active params, 1M context); the listed price is the standard list rate — DeepSeek is offering a 75% promotional discount through 2026-05-31.

## Kimi

Requires `LLM_KIMI_API_KEY`. Supported models with cost estimates (input/output per 1M tokens):

| Model | Input | Output |
|-------|-------|--------|
| `kimi-k2.7-code` | $0.95 | $4.00 |
| `kimi-k2.7-code-highspeed` | $1.90 | $8.00 |
| `kimi-k2.6` | $0.95 | $4.00 |
| `kimi-k2.6-thinking` | $0.95 | $4.00 |
| `kimi-k2.5` | $0.60 | $3.00 |
| `kimi-k2-0905-preview` | $0.60 | $2.50 |
| `kimi-k2-0711-preview` | $0.60 | $2.50 |
| `kimi-k2-turbo-preview` | $1.15 | $8.00 |
| `kimi-k2-thinking` | $0.60 | $2.50 |
| `kimi-k2-thinking-turbo` | $1.15 | $8.00 |
| `moonshot-v1-128k` | $2.00 | $5.00 |
| `moonshot-v1-32k` | $1.00 | $3.00 |
| `moonshot-v1-8k` | $0.20 | $2.00 |
| `moonshot-v1-128k-vision-preview` | $2.00 | $5.00 |
| `moonshot-v1-32k-vision-preview` | $1.00 | $3.00 |
| `moonshot-v1-8k-vision-preview` | $0.20 | $2.00 |

The `moonshot-v1-*-vision-preview` variants accept image inputs (vision) and share pricing with their text-only counterparts. The `kimi-k2.7-code-highspeed` variant trades a higher price for ~180 tok/s throughput on coding workloads.

## Z.ai

Requires `LLM_ZAI_API_KEY`. Supported models with cost estimates (input/output per 1M tokens):

| Model | Input | Output |
|-------|-------|--------|
| `glm-5.2` | $1.40 | $4.40 |
| `glm-5.1` | $1.40 | $4.40 |
| `glm-5-turbo` | $1.20 | $4.00 |
| `glm-5` | $1.00 | $3.20 |
| `glm-4.7` | $0.60 | $2.20 |
| `glm-4.7-flashx` | $0.07 | $0.40 |
| `glm-4.7-flash` | Free | Free |
| `glm-4.6` | $0.60 | $2.20 |
| `glm-4.5` | $0.60 | $2.20 |
| `glm-4.5-x` | $2.20 | $8.90 |
| `glm-4.5-airx` | $1.10 | $4.50 |
| `glm-4.5-air` | $0.20 | $1.10 |
| `glm-4.5-flash` | Free | Free |
| `glm-4-32b-0414-128k` | $0.10 | $0.10 |

## Ollama

Ollama runs models locally — no API key required. Install Ollama from [ollama.com](https://ollama.com), pull a model, and start the server:

```bash
ollama pull llama3.2
ollama serve
```

Then configure aictl to use it:

```
AICTL_PROVIDER=ollama
AICTL_MODEL=llama3.2:latest
```

Available models are detected automatically from your local Ollama instance via the REST API. The `/model` command shows only models you have pulled locally. If Ollama is not running, it will not appear in the model menu.

By default, aictl connects to `http://localhost:11434`. To use a different address, set `LLM_OLLAMA_HOST` in `~/.aictl/config`.

All Ollama models are free (self-hosted), so cost estimation shows $0.00.

Any model string can be passed via `--model`; cost estimation uses pattern matching on the model name and falls back to zero if unrecognized.

## Native GGUF (llama.cpp) — experimental

> **Experimental.** Native GGUF inference is a new, work-in-progress feature. It runs, it works, and it talks to the same tools the API providers do — but expect rough edges: small models struggle with tool-call formatting, chat templates are hard-coded to ChatML (so some models respond in a less natural style than their native template would produce), generation parameters are fixed, and performance tuning (GPU offload, context reuse across turns, speculative decoding) has not been wired up yet. The API-provider path remains the recommended default for day-to-day use. Please report issues at [github.com/pwittchen/aictl/issues](https://github.com/pwittchen/aictl/issues).

aictl can run GGUF models in-process via [`llama-cpp-2`](https://crates.io/crates/llama-cpp-2) — no Ollama server required. By default no local models are available; they must be downloaded explicitly by the user, one at a time, into `~/.aictl/models/gguf/`.

Native inference is gated behind the `gguf` cargo feature. **Prebuilt binaries published on GitHub Releases (the ones `install.sh` downloads) ship with `--features gguf` enabled**, so users who install via the one-liner get native GGUF inference out of the box — no extra steps required.

When building from source, the `gguf` feature is **off by default** to keep a plain `cargo install aictl` / `cargo build` working without a C/C++ toolchain. Opt in explicitly (see [INSTALL.md](INSTALL.md#optional-feature-flags)).

**Model management** (works in every build, even without `--features gguf`):

```bash
# Pull a GGUF model from Hugging Face
aictl --pull-gguf-model hf:bartowski/Llama-3.2-3B-Instruct-GGUF/Llama-3.2-3B-Instruct-Q4_K_M.gguf

# Shorthand form
aictl --pull-gguf-model bartowski/Llama-3.2-3B-Instruct-GGUF:Llama-3.2-3B-Instruct-Q4_K_M.gguf

# Direct URL
aictl --pull-gguf-model https://example.com/path/model.gguf

# List, remove, clear
aictl --list-gguf-models
aictl --remove-gguf-model Llama-3.2-3B-Instruct-Q4_K_M
aictl --clear-gguf-models
```

Inside the REPL, `/gguf` opens an interactive menu with the same operations (view downloaded / pull / remove / clear all). Downloads stream to `~/.aictl/models/gguf/<name>.gguf.part` with a progress bar and are atomically renamed on completion, so an interrupted download never leaves a half-written model in place.

Once a model is downloaded it appears in the `/model` picker under the **Native GGUF** header, alongside Ollama models. Configure it as the default:

```
AICTL_PROVIDER=gguf
AICTL_MODEL=Llama-3.2-3B-Instruct-Q4_K_M
```

Inference runs on a `tokio::spawn_blocking` task, so it doesn't block the async runtime. Cost always shows $0.00. Messages are flattened into a ChatML-style prompt, which works well for modern instruction-tuned models; per-model chat templates may be added later. If you try to use a GGUF model in a build without `--features gguf`, aictl prints a clear error telling you to rebuild.

### Tested GGUF models

The following models have been verified end-to-end (download, load, inference, tool calls) via the `/gguf` pull menu's predefined catalog:

| Model | Pull command |
|-------|--------------|
| `Qwen3-4B-Q4_K_M` | `aictl --pull-gguf-model lmstudio-community/Qwen3-4B-GGUF:Qwen3-4B-Q4_K_M.gguf` |

## Native MLX (Apple Silicon) — experimental

> **Experimental.** Native MLX inference is a new feature limited to macOS on Apple Silicon (`aarch64`). Architecture coverage is currently Llama-family — Llama 3.x, Qwen 2.5, Mistral 7B v0.3, DeepSeek-R1 Distill Qwen — plus Gemma 2. Phi-3.5 and MoE models are rejected with a clear error. Llama 3.1/3.2 RoPE scaling is not yet applied (quality degrades past ~8K context), top-p sampling is omitted (temperature only), and the chat-template renderer falls back to ChatML when the per-model jinja template fails to render. Please report issues at [github.com/pwittchen/aictl/issues](https://github.com/pwittchen/aictl/issues).

aictl can run MLX models in-process via [`mlx-rs`](https://crates.io/crates/mlx-rs) — no Python, no `mlx_lm`, no separate server. Quantized 4-bit weights from the [`mlx-community`](https://huggingface.co/mlx-community) Hugging Face organization are loaded directly via `safetensors`. By default no local MLX models are available; they must be downloaded explicitly by the user into `~/.aictl/models/mlx/<name>/`.

The macOS Apple Silicon prebuilt binary on GitHub Releases ships with `--features mlx` enabled and includes a sibling `mlx.metallib` file placed next to the binary at install time (MLX's first runtime fallback is `<exec_dir>/mlx.metallib`). Other platform releases contain only the `aictl` binary — they don't support MLX.

Native inference is gated behind the `mlx` cargo feature. When building from source, the `mlx` feature is **off by default**. Opt in explicitly (Apple Silicon only — see [INSTALL.md](INSTALL.md#optional-feature-flags)).

**Model management** (works in every build, even without `--features mlx` and even on non-Apple-Silicon hosts):

```bash
# Pull an MLX model from Hugging Face (mlx-community)
aictl --pull-mlx-model mlx:mlx-community/Llama-3.2-3B-Instruct-4bit

# Shorthand form
aictl --pull-mlx-model mlx-community/Qwen2.5-7B-Instruct-4bit

# List, remove, clear
aictl --list-mlx-models
aictl --remove-mlx-model mlx-community__Llama-3.2-3B-Instruct-4bit
aictl --clear-mlx-models
```

Inside the REPL, `/mlx` opens an interactive menu with the same operations plus a curated catalog of popular `mlx-community` repos. Downloads stream multi-file safetensors directories with a per-file progress bar.

Once a model is downloaded it appears in the `/model` picker under the **MLX (Apple Silicon)** header. Configure it as the default:

```
AICTL_PROVIDER=mlx
AICTL_MODEL=mlx-community__Llama-3.2-3B-Instruct-4bit
```

Inference runs on a `tokio::spawn_blocking` task, so it doesn't block the async runtime. Cost always shows $0.00. If you try to use an MLX model in a build without `--features mlx`, or on a non-Apple-Silicon host, aictl prints a clear error explaining the constraint.

### Tested MLX models

The following models have been verified end-to-end (download, load, inference, tool calls) on Apple Silicon:

| Model | Pull command |
|-------|--------------|
| `mlx-community__DeepSeek-R1-Distill-Qwen-7B-4bit` | `aictl --pull-mlx-model mlx-community/DeepSeek-R1-Distill-Qwen-7B-4bit` |
| `mlx-community__Llama-3.2-3B-Instruct-4bit` | `aictl --pull-mlx-model mlx-community/Llama-3.2-3B-Instruct-4bit` |
| `mlx-community__gemma-2-9b-it-4bit` | `aictl --pull-mlx-model mlx-community/gemma-2-9b-it-4bit` |

## Cost estimates

The per-token tables above tell you what each model charges; they don't tell you what a realistic workday actually costs. For that, see [LLM_PRICING.md](LLM_PRICING.md) — it models two usage patterns (chat assistant and coding agent) and reports daily and monthly totals for every model in the catalog.

The headline numbers for intensive use (150 chat turns/day or 50 coding tasks/day, 22 working days/month, cached pricing):

| Usage pattern | Cheapest | Flagship cluster | Opus 4.6 |
|---|---|---|---|
| Chat | **$3.08/mo** (gpt-5.4-nano) | ~$35–$48/mo | $69.74/mo |
| Coding agent | **$39.16/mo** (gpt-5.4-nano) | ~$460–$525/mo | $874.50/mo |

A few things worth knowing before you budget:

- **Intensive coding agent use is roughly 60× more expensive than chat use** on any given model, because the agent loop re-sends the growing conversation history each iteration and produces long, code-heavy outputs. Tool call count is not the dominant factor.
- **Prompt caching cuts costs roughly in half**, but the "cached" column is only reliable for Anthropic — aictl explicitly writes to Anthropic's prompt cache via `cache_control` markers. OpenAI, Gemini, Grok, DeepSeek, and Kimi cache automatically server-side, so you'll hit cached rates during sustained sessions but not after idle gaps longer than the provider's TTL (typically 5–10 minutes). Z.ai GLM and Mistral have no cache handling in aictl, so they always bill at the full rate.
- **The cost meter that aictl prints after every turn** reflects actual cached vs. fresh tokens from each provider's response, so it's more accurate than any estimate. If you want to know what your specific workload really costs, run a few typical sessions and watch the per-turn summary.
