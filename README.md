# aictl 🤖

[![CI](https://github.com/pwittchen/aictl/actions/workflows/ci.yml/badge.svg)](https://github.com/pwittchen/aictl/actions/workflows/ci.yml)
[![RELEASE](https://github.com/pwittchen/aictl/actions/workflows/release.yml/badge.svg)](https://github.com/pwittchen/aictl/actions/workflows/release.yml)
[![DEPLOY WEBSITE](https://github.com/pwittchen/aictl/actions/workflows/deploy-website.yml/badge.svg)](https://github.com/pwittchen/aictl/actions/workflows/deploy-website.yml)

AI agent in your terminal — 52 built-in cloud models across 8 providers, plus any model available through Ollama, native GGUF inference via llama.cpp, or native MLX inference on Apple Silicon

Project website: [aictl.app](https://aictl.app) — source in [`website/`](website/).

> [!NOTE]
> The **aictl** is a general-purpose AI agent.
> Dedicated coding capabilities may be added in the future. If you are looking for an AI agent specialized in software development today,
> consider Claude Code, Codex, or opencode — they are purpose-built for that workflow.

![aictl screenshot](screenshot.png)

## Install

```bash
curl -sSf https://aictl.app/install.sh | sh
```

The installer downloads a prebuilt binary for your platform from the latest GitHub release and places it in `~/.local/bin/aictl`. If aictl is already installed at `~/.cargo/bin/aictl` (e.g. from a prior `cargo install`), the installer updates it in place at that location instead of the default `~/.local/bin/`. Set `AICTL_INSTALL_DIR` to pick a different location explicitly. If no prebuilt binary exists for your platform, the installer falls back to building from source with `cargo install`.

### Supported platforms

Prebuilt binaries are published for:

| OS | Architectures |
|---|---|
| Linux | `x86_64`, `aarch64` |
| macOS | `x86_64`, `aarch64` (Apple Silicon) |

Native Windows is not supported — aictl depends on a POSIX shell (`sh`) and Unix tools (`date`, `pbcopy`, etc.) for its built-in tool calls. Windows users can run aictl inside [WSL](https://learn.microsoft.com/windows/wsl/) using the Linux binary, which works normally.

Other platforms (FreeBSD, other BSDs, uncommon Linux architectures) can still build from source via the `cargo install` fallback path, provided a Rust toolchain is available.

### Prerequisites

Installing a prebuilt binary has no prerequisites beyond `curl`. Building from source (either via the installer fallback or manually) requires [Rust](https://www.rust-lang.org/tools/install) (edition 2024).

### From source

```bash
git clone git@github.com:pwittchen/aictl.git
cd aictl
cargo install --path .
```

This installs the `aictl` binary to `~/.cargo/bin/`.

### Build without installing

```bash
cargo build --release
```

The binary will be at `target/release/aictl`.

### Optional feature flags

Native local-model inference is gated behind cargo features so a plain `cargo build` / `cargo install` keeps a lightweight default (no C++ toolchain or Metal Toolchain required). Opt in per backend:

| Feature | What it enables | Platform | Extra build-time requirements |
|---------|-----------------|----------|-------------------------------|
| `gguf` | Native GGUF inference via `llama-cpp-2` | All | `cmake` + a working C/C++ compiler (Xcode Command Line Tools on macOS, `build-essential` on Debian/Ubuntu) |
| `mlx`  | Native MLX inference via `mlx-rs` (Apple's MLX framework) | macOS + Apple Silicon only | Full Xcode (not just CLT) with the Metal Toolchain installed |

Examples:

```bash
# GGUF only
cargo build --release --features gguf
cargo install --path . --features gguf

# MLX only (macOS Apple Silicon)
cargo build --release --features mlx
cargo install --path . --features mlx

# Both at the same time
cargo build --release --features "gguf mlx"
cargo install --path . --features "gguf mlx"
```

Without these features, the corresponding slash commands (`/gguf`, `/mlx`) and CLI flags (`--pull-gguf-model`, `--pull-mlx-model`, etc.) still work for **model management** (download / list / remove); only the inference path is disabled, and trying to run a local model prints a clear error telling you which feature to rebuild with.

The prebuilt binaries published on GitHub Releases (downloaded by `install.sh`) ship with `--features gguf` enabled on every platform, and additionally `--features "gguf mlx"` on the macOS aarch64 build — so one-liner installs get native inference out of the box where the platform supports it.

## Uninstall

### Binary release (installed via `install.sh`)

The install script places the binary at `~/.local/bin/aictl` (or `$AICTL_INSTALL_DIR` if you set it). Remove it with:

```bash
rm ~/.local/bin/aictl
```

### From source (installed via `cargo install`)

Cargo tracks its own installs, so the clean way is:

```bash
cargo uninstall aictl
```

This removes `~/.cargo/bin/aictl`. If `cargo uninstall` doesn't find it (e.g. installed under a different crate name), delete the binary directly:

```bash
rm ~/.cargo/bin/aictl
```

### Remove configuration and data (optional)

aictl stores all state under `~/.aictl/` — config file, saved agents, saved sessions. To wipe it completely:

```bash
rm -rf ~/.aictl
```

Skip this step if you plan to reinstall and want to keep your API keys, agents, and session history.

## Usage

```bash
aictl [--version] [--update] [--uninstall] [--config] [--provider <PROVIDER>] [--model <MODEL>] [--message <MESSAGE>] [--auto] [--quiet] [--unrestricted] [--incognito] [--agent <NAME>] [--list-agents] [--session <ID|NAME>] [--list-sessions] [--clear-sessions] [--lock-keys] [--unlock-keys] [--clear-keys] [--pull-gguf-model <SPEC>] [--list-gguf-models] [--remove-gguf-model <NAME>] [--clear-gguf-models] [--pull-mlx-model <SPEC>] [--list-mlx-models] [--remove-mlx-model <NAME>] [--clear-mlx-models]
```

Omit `--message` to enter interactive REPL mode with persistent conversation history.

### REPL Commands

The interactive REPL supports slash commands:

| Command | Description |
|---------|-------------|
| `/agent` | Manage agents (create manually, create with AI, view/load/delete, unload) |
| `/clear` | Clear conversation context |
| `/compact` | Summarize conversation into a compact context |
| `/context` | Show context usage (token and message counts vs limits) |
| `/copy` | Copy last response to clipboard |
| `/help` | Show available commands |
| `/info` | Show setup info (provider, model, behavior, memory, agent, version, OS, binary size) |
| `/gguf` | Manage native GGUF models (view downloaded, pull, remove, clear all) |
| `/mlx` | Manage native MLX models (Apple Silicon; view downloaded, pull, remove, clear all) |
| `/memory` | Switch memory mode: long-term (all messages) or short-term (sliding window) |
| `/security` | Show current security policy (blocked commands, CWD jail, timeouts, etc.) |
| `/session` | Manage sessions (show current info, set name, view/load/delete saved, clear all) |
| `/stats` | Manage usage statistics — view today/month/overall (sessions, calls, tokens, estimated cost) or clear all |
| `/behavior` | Switch between auto and human-in-the-loop mode during the session |
| `/model` | Switch model and provider during the session (persists to `~/.aictl/config`) |
| `/tools` | Show available tools |
| `/keys` | Manage API key storage — lock (config → keyring), unlock (keyring → config), or clear (both stores) |
| `/config` | Re-run the interactive configuration wizard |
| `/update` | Update to the latest version |
| `/uninstall` | Remove the aictl binary from `~/.cargo/bin/` and `~/.local/bin/` (asks for confirmation) |
| `/version` | Check current version against the latest available |
| `/exit` | Exit the REPL |

Press **Esc** during any LLM call or tool execution to interrupt the operation and return to the prompt. Conversation history is rolled back so the interrupted turn has no effect.

### Parameters

Only `--version` (`-v`) and `--help` (`-h`) have short flags. All other options use long form only, by convention.

| Flag | Description |
|------|-------------|
| `--version`, `-v` | Print version information |
| `--help`, `-h` | Print help |
| `--update` | Update to the latest version |
| `--uninstall` | Remove the aictl binary from `~/.cargo/bin/aictl`, `~/.local/bin/aictl`, and `$AICTL_INSTALL_DIR/aictl` (if set) and exit. Leaves `~/.aictl/` untouched |
| `--config` | Interactive configuration wizard — set provider, model, and API keys step by step |
| `--provider` | LLM provider (`openai`, `anthropic`, `gemini`, `grok`, `mistral`, `deepseek`, `kimi`, `zai`, `ollama`, `gguf`, or `mlx`). Falls back to `AICTL_PROVIDER` in `~/.aictl/config` |
| `--model` | Model name (e.g. `gpt-4o`). Falls back to `AICTL_MODEL` in `~/.aictl/config` |
| `--message` | Message to send (omit for interactive mode) |
| `--agent` | Load a saved agent by name (works in both single-shot and interactive modes) |
| `--list-agents` | Print saved agents from `~/.aictl/agents/` and exit |
| `--auto` | Run in autonomous mode (skip tool confirmation prompts) |
| `--quiet` | Suppress tool calls and reasoning, only print the final answer (requires `--auto`) |
| `--unrestricted` | Disable all security restrictions (use with caution) |
| `--incognito` | Start interactive REPL without saving any session (disables `/session`). Falls back to `AICTL_INCOGNITO` in `~/.aictl/config` |
| `--session` | Load a saved session by uuid or name on startup (interactive mode only) |
| `--list-sessions` | Print saved sessions from `~/.aictl/sessions/` and exit |
| `--clear-sessions` | Remove all saved sessions and exit |
| `--lock-keys` | Migrate plain-text API keys from `~/.aictl/config` into the system keyring and exit |
| `--unlock-keys` | Migrate API keys from the system keyring back into `~/.aictl/config` and exit |
| `--clear-keys` | Remove API keys from both `~/.aictl/config` and the system keyring and exit |
| `--pull-gguf-model` | Download a native GGUF model (spec: `hf:owner/repo/file.gguf`, `owner/repo:file.gguf`, or `https://…/file.gguf`). Saved under `~/.aictl/models/gguf/` and exits |
| `--list-gguf-models` | Print all downloaded native GGUF models and exit |
| `--remove-gguf-model` | Remove a downloaded native GGUF model by name and exit |
| `--clear-gguf-models` | Remove every downloaded native GGUF model and exit |
| `--pull-mlx-model` | Download a native MLX model (spec: `mlx:owner/repo` or `owner/repo`). Saved under `~/.aictl/models/mlx/<name>/` and exits |
| `--list-mlx-models` | Print all downloaded native MLX models and exit |
| `--remove-mlx-model` | Remove a downloaded native MLX model by name and exit |
| `--clear-mlx-models` | Remove every downloaded native MLX model and exit |

CLI flags take priority over config file values.

### Sessions

In interactive mode, each REPL run is a session. A new uuid is generated at startup and the conversation is persisted to `~/.aictl/sessions/<uuid>` as JSON after every agent turn and compaction. Session names (optional, unique) are stored in `~/.aictl/sessions/.names`. On exit, the session uuid (and name, if set) is printed.

Use `/session` to show current session info, assign a readable name, browse saved sessions (load or delete with confirmation), or clear all sessions. Pass `--session <uuid|name>` to resume an existing session on startup. Incognito mode (`--incognito` or `AICTL_INCOGNITO=true`) runs the REPL without creating or saving any session file; `/session` is disabled and displays a notice.

### Agents

Agents are reusable system prompt extensions that specialize the LLM for dedicated tasks or behaviors. Agent prompts are stored as plain text files in `~/.aictl/agents/`.

Use `/agent` to open the agent menu:

- **Create agent manually** — enter a name and type or paste the agent prompt text directly
- **Create agent with AI** — provide a name and brief description; the LLM generates the full agent prompt
- **View all agents** — browse saved agents, view their prompt, load an agent, or delete it
- **Unload agent** — remove the currently loaded agent (only shown when one is loaded)

Agents can also be loaded from the command line with `--agent <name>`, which works in both single-shot and interactive modes.

Agent names may contain only letters, numbers, underscores, and dashes. When an agent is loaded, its prompt is appended to the system prompt and the agent name appears in magenta brackets before the input prompt (e.g. `[my-agent] ❯`).

### Configuration

Configuration is loaded from `~/.aictl/config`. This is a single global config file.

Additionally, aictl loads a project prompt file from the current working directory (default: `AICTL.md`). If present, its contents are appended to the system prompt, allowing per-project instructions for the agent. The filename can be customized via `AICTL_PROMPT_FILE` in `~/.aictl/config`.

The quickest way to get started is the interactive wizard:

```bash
aictl --config
```

It walks you through selecting a provider, model, and entering API keys. You can also edit `~/.aictl/config` manually at any time.

#### Basic configuration

You need to configure API key for the provider and model you want to use. `AICTL_MEMORY` and `AICTL_INCOGNITO` params are optional.

| Key | Description |
|-----|-------------|
| `AICTL_PROVIDER` | Default provider (`openai`, `anthropic`, `gemini`, `grok`, `mistral`, `deepseek`, `kimi`, `zai`, `ollama`, `gguf`, or `mlx`) |
| `AICTL_MODEL` | Default model name |
| `AICTL_MEMORY` | Memory mode: `long-term` (all messages, default) or `short-term` (sliding window) |
| `AICTL_INCOGNITO` | Start interactive REPL without saving sessions. Accepts `true` or `false` (default: `false`) |
| `AICTL_PROMPT_FILE` | Filename for the project prompt file loaded from the current directory (default: `AICTL.md`) |
| `AICTL_TOOLS_ENABLED` | Enable or disable all tool calls. When `false`, the LLM can only respond with plain text (default: `true`) |
| `AICTL_AUTO_COMPACT_THRESHOLD` | Context usage percentage at which the REPL auto-compacts the conversation. Accepts an integer in `1..=100` (default: `80`) |
| `AICTL_LLM_TIMEOUT` | Per-call LLM response timeout in seconds. Applied to every provider (remote APIs, Ollama, native GGUF/MLX) and to the compaction and agent-generation calls. `0` disables the timeout. Default: `300` |
| `AICTL_MAX_ITERATIONS` | Maximum number of LLM calls allowed in a single agent turn before the loop aborts. Accepts a positive integer (default: `20`) |

#### API keys

`FIRECRAWL_API_KEY` is optional and is needed only if you want to use `search_web` tool.

Not all API keys are required. You need to provide only those, for which you set `AICTL_PROVIDER` and `AICTL_MODEL`.

If you want to use multiple LLM providers, then you need to provide appropriate keys.

| Key | Description |
|-----|-------------|
| `LLM_OPENAI_API_KEY` | API key for OpenAI |
| `LLM_ANTHROPIC_API_KEY` | API key for Anthropic |
| `LLM_GEMINI_API_KEY` | API key for Google Gemini |
| `LLM_GROK_API_KEY` | API key for xAI Grok |
| `LLM_MISTRAL_API_KEY` | API key for Mistral |
| `LLM_DEEPSEEK_API_KEY` | API key for DeepSeek |
| `LLM_KIMI_API_KEY` | API key for Kimi (Moonshot AI) |
| `LLM_ZAI_API_KEY` | API key for Z.ai |
| `LLM_OLLAMA_HOST` | Ollama server URL (default: `http://localhost:11434`) |
| `FIRECRAWL_API_KEY` | API key for Firecrawl (`search_web` tool) |

##### Where to get API keys

Each provider issues API keys through its own developer console. Sign up, create a key, then paste it into `~/.aictl/config` (or run `aictl --config`).

| Provider | Console URL |
|----------|-------------|
| OpenAI | [platform.openai.com/api-keys](https://platform.openai.com/api-keys) |
| Anthropic | [console.anthropic.com/settings/keys](https://console.anthropic.com/settings/keys) |
| Google Gemini | [aistudio.google.com/app/apikey](https://aistudio.google.com/app/apikey) |
| xAI Grok | [console.x.ai](https://console.x.ai) |
| Mistral | [console.mistral.ai/api-keys](https://console.mistral.ai/api-keys) |
| DeepSeek | [platform.deepseek.com/api_keys](https://platform.deepseek.com/api_keys) |
| Kimi (Moonshot) | [platform.moonshot.ai/console/api-keys](https://platform.moonshot.ai/console/api-keys) |
| Z.ai | [z.ai/manage-apikey/apikey-list](https://z.ai/manage-apikey/apikey-list) |
| Firecrawl | [firecrawl.dev/app/api-keys](https://firecrawl.dev/app/api-keys) |

Ollama, native GGUF, and native MLX run locally and require no API key.

#### Secure key storage (system keyring)

By default, API keys live as plain text in `~/.aictl/config`. aictl can also store them in the OS-native keyring — macOS Keychain or Linux Secret Service (gnome-keyring / KWallet via D-Bus) — and reads them transparently from whichever store has them.

The active backend appears in the welcome banner (`keys: Keychain (2 locked · 1 plain · 0 both)`) and `/security` shows the per-key location.

Migration is done from inside the REPL via the `/keys` interactive menu:

- **lock keys** — copies every plain-text key found in `~/.aictl/config` into the system keyring and removes the plain-text copy
- **unlock keys** — copies every keyring entry back into `~/.aictl/config` and deletes it from the keyring
- **clear keys** — removes the keys from both stores (asks for confirmation)

The same operations are available as one-shot CLI flags: `--lock-keys`, `--unlock-keys`, `--clear-keys`.

When the keyring backend is unavailable (e.g. headless Linux without a Secret Service daemon), aictl falls back to plain-text storage automatically and the banner shows `keys: plain text` in yellow.

#### Security configuration (optional)

| Key | Description |
|-----|-------------|
| `AICTL_SECURITY` | Master security switch (default: `true`) |
| `AICTL_SECURITY_INJECTION_GUARD` | Block user prompts that look like prompt-injection attempts (default: `true`) |
| `AICTL_SECURITY_CWD_RESTRICT` | Restrict file tools to working directory (default: `true`) |
| `AICTL_SECURITY_SHELL_ALLOWED` | Comma-separated whitelist of allowed shell commands (empty = all except blocked) |
| `AICTL_SECURITY_SHELL_BLOCKED` | Additional blocked shell commands (added to built-in defaults) |
| `AICTL_SECURITY_BLOCK_SUBSHELL` | Block `$()`, backticks, and process substitution (default: `true`) |
| `AICTL_SECURITY_BLOCKED_PATHS` | Additional blocked file paths (added to built-in defaults) |
| `AICTL_SECURITY_ALLOWED_PATHS` | Paths allowed outside the working directory |
| `AICTL_SECURITY_SHELL_TIMEOUT` | Shell command timeout in seconds (default: `30`) |
| `AICTL_SECURITY_MAX_WRITE` | Max file write size in bytes (default: `1048576` = 1 MB) |
| `AICTL_SECURITY_DISABLED_TOOLS` | Comma-separated tool names to disable (e.g. `exec_shell,search_web`) |
| `AICTL_SECURITY_BLOCKED_ENV` | Additional env vars to scrub from shell subprocesses |

Create `~/.aictl/config` (see `.aictl/config` in this repo for the reference):

```
AICTL_PROVIDER=anthropic
AICTL_MODEL=claude-sonnet-4-20250514
LLM_ANTHROPIC_API_KEY=sk-ant-...
FIRECRAWL_API_KEY=fc-...
```

The file format supports comments (`#`), quoted values, and optional `export` prefixes.

### Providers

aictl supports eleven LLM providers — eight remote APIs plus Ollama, native GGUF inference via llama.cpp, and native MLX inference on Apple Silicon:

#### OpenAI

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
| `gpt-5.4-nano` | $0.20 | $1.25 |
| `gpt-5.4-mini` | $0.75 | $4.50 |
| `gpt-5.4` | $2.50 | $15.00 |
| `gpt-5.4-pro` | $60.00 | $270.00 |
| `o4-mini` | $1.10 | $4.40 |
| `o3` | $2.00 | $8.00 |
| `o1` | $15.00 | $60.00 |

GPT-5.2 and GPT-5.4 use dual-tier pricing that doubles above the 272K context threshold; the table shows the short-context rates. The cost meter in aictl always reports the short-context price.

#### Anthropic

Requires `LLM_ANTHROPIC_API_KEY`. Supported models with cost estimates (input/output per 1M tokens):

| Model | Input | Output |
|-------|-------|--------|
| `claude-haiku-*` (3.x) | $0.25 | $1.25 |
| `claude-haiku-4-*` | $1.00 | $5.00 |
| `claude-sonnet-*` | $3.00 | $15.00 |
| `claude-opus-4-5-*` / `claude-opus-4-6-*` | $5.00 | $25.00 |
| `claude-opus-4-*` (older) | $15.00 | $75.00 |

#### Google Gemini

Requires `LLM_GEMINI_API_KEY`. Supported models with cost estimates (input/output per 1M tokens):

| Model | Input | Output |
|-------|-------|--------|
| `gemini-3.1-pro-preview` | $2.00 | $12.00 |
| `gemini-3.1-flash-lite-preview` | $0.25 | $1.50 |
| `gemini-2.5-pro` | $1.25 | $10.00 |
| `gemini-2.5-flash` | $0.15 | $0.60 |

Gemini 3.1 Pro uses dual-tier pricing that doubles above a 200K context threshold; the table shows the short-context rates. `gemini-2.0-flash` has been removed from the model list because Google is shutting it down on June 1, 2026.

#### xAI Grok

Requires `LLM_GROK_API_KEY`. Supported models with cost estimates (input/output per 1M tokens):

| Model | Input | Output |
|-------|-------|--------|
| `grok-4` | $3.00 | $15.00 |
| `grok-4-fast-reasoning` / `grok-4-fast-non-reasoning` | $0.20 | $0.50 |
| `grok-4-1-fast-reasoning` / `grok-4-1-fast-non-reasoning` | $0.20 | $0.50 |
| `grok-3` | $3.00 | $15.00 |
| `grok-3-mini` | $0.30 | $0.50 |

Grok 4 Fast variants ship with a 2M-token context window, the largest available across frontier models.

#### Mistral

Requires `LLM_MISTRAL_API_KEY`. Supported models with cost estimates (input/output per 1M tokens):

| Model | Input | Output |
|-------|-------|--------|
| `mistral-large-latest` | $2.00 | $6.00 |
| `mistral-medium-latest` | $0.40 | $2.00 |
| `mistral-small-latest` | $0.10 | $0.30 |
| `codestral-latest` | $0.30 | $0.90 |

#### DeepSeek

Requires `LLM_DEEPSEEK_API_KEY`. Supported models with cost estimates (input/output per 1M tokens):

| Model | Input | Output |
|-------|-------|--------|
| `deepseek-chat` | $0.27 | $1.10 |
| `deepseek-reasoner` | $0.55 | $2.19 |

#### Kimi

Requires `LLM_KIMI_API_KEY`. Supported models with cost estimates (input/output per 1M tokens):

| Model | Input | Output |
|-------|-------|--------|
| `kimi-k2.5` | $0.60 | $2.00 |
| `kimi-k2-0905-preview` | $0.60 | $2.00 |
| `kimi-k2-0711-preview` | $0.60 | $2.00 |
| `kimi-k2-turbo-preview` | $0.60 | $2.00 |
| `kimi-k2-thinking` | $0.60 | $2.00 |
| `kimi-k2-thinking-turbo` | $0.60 | $2.00 |
| `moonshot-v1-128k` | $0.60 | $2.00 |
| `moonshot-v1-32k` | $0.60 | $2.00 |
| `moonshot-v1-8k` | $0.60 | $2.00 |

#### Z.ai

Requires `LLM_ZAI_API_KEY`. Supported models with cost estimates (input/output per 1M tokens):

| Model | Input | Output |
|-------|-------|--------|
| `glm-5.1` | $1.40 | $4.40 |
| `glm-5-turbo` | $1.20 | $4.00 |
| `glm-5` | $0.72 | $2.30 |
| `glm-4.7` | $0.39 | $1.75 |
| `glm-4.7-flash` | $0.06 | $0.40 |

#### Ollama

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

#### Native GGUF (llama.cpp) — experimental

> **Experimental.** Native GGUF inference is a new, work-in-progress feature. It runs, it works, and it talks to the same tools the API providers do — but expect rough edges: small models struggle with tool-call formatting, chat templates are hard-coded to ChatML (so some models respond in a less natural style than their native template would produce), generation parameters are fixed, and performance tuning (GPU offload, context reuse across turns, speculative decoding) has not been wired up yet. The API-provider path remains the recommended default for day-to-day use. Please report issues at [github.com/pwittchen/aictl/issues](https://github.com/pwittchen/aictl/issues).

aictl can run GGUF models in-process via [`llama-cpp-2`](https://crates.io/crates/llama-cpp-2) — no Ollama server required. By default no local models are available; they must be downloaded explicitly by the user, one at a time, into `~/.aictl/models/gguf/`.

Native inference is gated behind the `gguf` cargo feature. **Prebuilt binaries published on GitHub Releases (the ones `install.sh` downloads) ship with `--features gguf` enabled**, so users who install via the one-liner get native GGUF inference out of the box — no extra steps required.

When building from source, the `gguf` feature is **off by default** to keep a plain `cargo install aictl` / `cargo build` working without a C/C++ toolchain. Opt in explicitly:

```bash
cargo install --path . --features gguf
# or
cargo build --release --features gguf
```

Building with `--features gguf` requires `cmake` and a working C/C++ compiler (Xcode Command Line Tools on macOS, `build-essential` on Debian/Ubuntu). The install-script fallback path (`cargo install --git ...`, triggered when no prebuilt binary exists for your platform) does **not** pass `--features gguf` and will therefore produce a binary without native inference — in that case, rebuild manually with the command above.

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

##### Tested GGUF models

The following models have been verified end-to-end (download, load, inference, tool calls) via the `/gguf` pull menu's predefined catalog:

| Model | Pull command |
|-------|--------------|
| `Qwen3-4B-Q4_K_M` | `aictl --pull-gguf-model lmstudio-community/Qwen3-4B-GGUF:Qwen3-4B-Q4_K_M.gguf` |

#### Native MLX (Apple Silicon) — experimental

> **Experimental.** Native MLX inference is a new feature limited to macOS on Apple Silicon (`aarch64`). Architecture coverage is currently Llama-family — Llama 3.x, Qwen 2.5, Mistral 7B v0.3, DeepSeek-R1 Distill Qwen — plus Gemma 2. Phi-3.5 and MoE models are rejected with a clear error. Llama 3.1/3.2 RoPE scaling is not yet applied (quality degrades past ~8K context), top-p sampling is omitted (temperature only), and the chat-template renderer falls back to ChatML when the per-model jinja template fails to render. Please report issues at [github.com/pwittchen/aictl/issues](https://github.com/pwittchen/aictl/issues).

aictl can run MLX models in-process via [`mlx-rs`](https://crates.io/crates/mlx-rs) — no Python, no `mlx_lm`, no separate server. Quantized 4-bit weights from the [`mlx-community`](https://huggingface.co/mlx-community) Hugging Face organization are loaded directly via `safetensors`. By default no local MLX models are available; they must be downloaded explicitly by the user into `~/.aictl/models/mlx/<name>/`.

Native inference is gated behind the `mlx` cargo feature. **Prebuilt binaries published on GitHub Releases include `--features mlx` on the macOS aarch64 build** in addition to `--features gguf`, so the one-liner installs get native MLX out of the box on Apple Silicon.

When building from source, the `mlx` feature is **off by default**. Opt in explicitly (Apple Silicon only):

```bash
cargo install --path . --features mlx
# or
cargo build --release --features mlx
```

Building with `--features mlx` requires the **Xcode Metal Toolchain** (full Xcode, not just the Command Line Tools). Install via Xcode → Settings → Components, or `xcodebuild -downloadComponent MetalToolchain`. Verify with `xcrun --find metal`.

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

##### Tested MLX models

The following models have been verified end-to-end (download, load, inference, tool calls) on Apple Silicon:

| Model | Pull command |
|-------|--------------|
| `mlx-community__DeepSeek-R1-Distill-Qwen-7B-4bit` | `aictl --pull-mlx-model mlx-community/DeepSeek-R1-Distill-Qwen-7B-4bit` |
| `mlx-community__Llama-3.2-3B-Instruct-4bit` | `aictl --pull-mlx-model mlx-community/Llama-3.2-3B-Instruct-4bit` |
| `mlx-community__gemma-2-9b-it-4bit` | `aictl --pull-mlx-model mlx-community/gemma-2-9b-it-4bit` |

### Cost estimates

The per-token tables above tell you what each model charges; they don't tell you what a realistic workday actually costs. For that, see [LLM_PRICING.md](LLM_PRICING.md) — it models two usage patterns (chat assistant and coding agent) and reports daily and monthly totals for every model in the catalog.

The headline numbers for intensive use (150 chat turns/day or 50 coding tasks/day, 22 working days/month, cached pricing):

| Usage pattern | Cheapest | Flagship cluster | Opus 4.6 |
|---|---|---|---|
| Chat | **$2.64/mo** (grok-4-fast) | ~$35–$48/mo | $69.74/mo |
| Coding agent | **$34.76/mo** (grok-4-fast) | ~$460–$525/mo | $874.50/mo |

A few things worth knowing before you budget:

- **Intensive coding agent use is roughly 60× more expensive than chat use** on any given model, because the agent loop re-sends the growing conversation history each iteration and produces long, code-heavy outputs. Tool call count is not the dominant factor.
- **Prompt caching cuts costs roughly in half**, but the "cached" column is only reliable for Anthropic — aictl explicitly writes to Anthropic's prompt cache via `cache_control` markers. OpenAI, Gemini, Grok, DeepSeek, and Kimi cache automatically server-side, so you'll hit cached rates during sustained sessions but not after idle gaps longer than the provider's TTL (typically 5–10 minutes). Z.ai GLM and Mistral have no cache handling in aictl, so they always bill at the full rate.
- **The cost meter that aictl prints after every turn** reflects actual cached vs. fresh tokens from each provider's response, so it's more accurate than any estimate. If you want to know what your specific workload really costs, run a few typical sessions and watch the per-turn summary.

### Agent Loop & Tool Calling

aictl runs an agent loop: the LLM can invoke tools, see their results, and continue reasoning until it produces a final answer.

By default, every tool call requires confirmation (y/N prompt). Use `--auto` to skip confirmation and run autonomously.

Available tools:

| Tool | Description |
|------|-------------|
| `exec_shell` | Execute a shell command via `sh -c` |
| `read_file` | Read the contents of a file |
| `write_file` | Write content to a file (first line = path, rest = content) |
| `remove_file` | Remove (delete) a file (regular files only, not directories) |
| `create_directory` | Create a directory and any missing parent directories |
| `list_directory` | List files and directories at a path with `[FILE]`/`[DIR]`/`[LINK]` prefixes |
| `search_files` | Search file contents by pattern (grep regex) with optional directory scope |
| `edit_file` | Apply a targeted find-and-replace edit to a file (exact unique match required) |
| `search_web` | Search the web via Firecrawl API (requires `FIRECRAWL_API_KEY`) |
| `find_files` | Find files matching a glob pattern (e.g. `**/*.rs`) with optional base directory |
| `fetch_url` | Fetch a URL and return readable text content (HTML tags stripped) |
| `extract_website` | Fetch a URL and extract only the main readable content (strips scripts, styles, nav, boilerplate) |
| `fetch_datetime` | Get the current date, time, timezone, and day of week |
| `fetch_geolocation` | Get geolocation data for an IP address (city, country, timezone, coordinates, ISP) via ip-api.com |
| `read_image` | Read an image from a file path or URL for vision analysis (PNG, JPEG, GIF, WebP, BMP, TIFF, SVG, ICO) |
| `generate_image` | Generate an image from a text description via DALL-E, Imagen, or Grok (auto-selects provider based on available keys; saves PNG to current directory) |
| `read_document` | Read a PDF, DOCX, or spreadsheet and extract content as markdown text. Supports `.pdf`, `.docx`, `.xlsx`, `.xls`, `.ods`. PDF text extracted directly; DOCX converted to markdown; spreadsheets converted to markdown tables (one per sheet) |

#### Image capabilities by provider

The `read_image` (vision/analysis) and `generate_image` tools depend on provider support:

| Provider | Image analysis (`read_image`) | Image generation (`generate_image`) |
|----------|-------------------------------|-------------------------------------|
| OpenAI | All models | DALL-E 3 |
| Anthropic | All models | -- |
| Gemini | All models | Imagen 4.0 |
| Grok | All models | Grok 2 Image |
| Mistral | All models | -- |
| DeepSeek | -- | -- |
| Kimi | kimi-k2.5 and moonshot-v1 variants | -- |
| Z.ai | -- (requires GLM vision models not in catalog) | -- |
| Ollama | Model-dependent (e.g. llava, llama3.2-vision) | -- |

**Image generation fallback**: `generate_image` auto-selects a provider based on available API keys. The active provider is tried first (if it supports generation), then falls back through OpenAI, Gemini, and Grok in order. This means you can generate images even when your active chat provider (e.g. Anthropic or Mistral) doesn't offer a generation API — as long as you have at least one of `LLM_OPENAI_API_KEY`, `LLM_GEMINI_API_KEY`, or `LLM_GROK_API_KEY` configured.

The tool-calling mechanism uses a custom XML format in the LLM response text (not provider-native tool APIs):

```xml
<tool name="exec_shell">
ls -la /tmp
</tool>
```

The agent loop runs for up to 20 iterations. LLM reasoning is printed to stderr; the final answer goes to stdout. Token usage, estimated cost, and execution time are always displayed after each response.

### Security

All tool calls pass through a configurable security policy (`src/security.rs`) before execution. By default:

- **Shell command blocking**: dangerous commands are blocked (`rm`, `sudo`, `dd`, `mkfs`, `nc`, etc.). Command substitution (`$(...)`, backticks) is blocked. Compound commands (`|`, `&&`, `||`, `;`) are split and each segment is validated independently.
- **CWD jail**: file tools (`read_file`, `write_file`, `remove_file`, `edit_file`, `create_directory`, `list_directory`, `search_files`, `find_files`) can only operate within the working directory. Path traversal via `..` is defeated by canonicalization.
- **Blocked paths**: sensitive paths are always blocked (`~/.ssh`, `~/.gnupg`, `~/.aictl`, `~/.aws`, `~/.config/gcloud`, `/etc/shadow`, `/etc/sudoers`).
- **Environment scrubbing**: shell subprocesses receive a clean environment — vars matching `*_KEY`, `*_SECRET`, `*_TOKEN`, `*_PASSWORD` are stripped so API keys cannot leak.
- **Shell timeout**: commands are killed after 30 seconds (configurable).
- **Write size limit**: file writes are capped at 1 MB (configurable).
- **Output sanitization**: tool results are sanitized to prevent prompt injection via `<tool>` tags.
- **Injection guard**: user prompts are scanned before being sent to the LLM. Inputs containing instruction-override phrases ("ignore previous instructions", "disable security", etc.) or forged role/tool tags (`<tool …>`, `<|system|>`, `### System:`, etc.) are blocked with a clear error. Disable with `AICTL_SECURITY_INJECTION_GUARD=false`.

Security denials are returned to the LLM as tool results (displayed in red) so it can adapt. Use `--unrestricted` to disable all security checks. Individual settings are configurable via `AICTL_SECURITY_*` keys in `~/.aictl/config`.

### Examples

```bash
# With defaults configured in ~/.aictl/config, just run:
aictl

# Or send a single message:
aictl --message "What is Rust?"

# Override provider/model from the command line:
aictl --provider openai --model gpt-4o --message "What is Rust?"

# Agent with tool calls (interactive confirmation)
aictl --message "List files in the current directory"

# Autonomous mode (no confirmation prompts)
aictl --auto --message "What OS am I running?"

# Quiet mode (only final answer, no tool calls or reasoning)
aictl --auto --quiet --message "What OS am I running?"
```

## Tests

```bash
cargo test
```

Unit tests cover core logic across six modules: `commands` (slash command parsing), `config` (config file parsing), `tools` (tool-call XML parsing), `ui` (formatting helpers), `llm` (cost estimation and model matching), and `security` (shell validation, path validation, output sanitization). The `session` module handles persistence of REPL conversations under `~/.aictl/sessions/`.

## Roadmap

See [ROADMAP.md](ROADMAP.md) for planned features and future direction, including new tools, UX improvements, desktop app plans, and coding agent capabilities.

## Architecture

See [ARCH.md](ARCH.md) for detailed ASCII diagrams covering:

- Module structure
- Startup flow
- Agent loop
- Tool execution dispatch
- LLM provider abstraction
- UI layer
- End-to-end data flow

## Claude Code Skills

This project includes [Claude Code](https://claude.ai/code) skills for common workflows. Run them as slash commands in a Claude Code session:

| Skill | Description |
|-------|-------------|
| `/commit` | Commit staged and unstaged changes with a clear commit message |
| `/update-docs` | Update README.md, CLAUDE.md, and ARCH.md to match the current project state |
| `/evaluate-rust-quality` | Audit code quality, idiomatic Rust usage, and best practices |
| `/evaluate-rust-security` | Audit security posture, injection risks, and credential handling |
| `/evaluate-rust-performance` | Audit performance patterns, allocations, and CLI responsiveness |
| `/project-stats-report` | Generate a project statistics report (LOC, commit activity, contributors, etc.) |

Evaluation reports are saved to `.claude/reports/` with timestamped filenames.

## License

This project is licensed under the [PolyForm Noncommercial License 1.0.0](LICENSE). It is free to use for non-commercial purposes, including personal use, research, education, and use by non-profit organizations. For commercial use, please contact [piotr@wittchen.io](mailto:piotr@wittchen.io).

## A note on how this was built

This project was built with AI assistance (Claude Code). The architecture, feature set, security model, provider coverage, and overall direction were designed and guided by me; the code itself was largely written by the model under that direction. I'm sharing it openly because I think the result is useful, and being transparent about how it was made matters more than pretending otherwise.
