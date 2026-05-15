# Configuration

Configuration is loaded from `~/.aictl/config`. This is a single global config file.

Additionally, aictl loads a project prompt file from the current working directory (default: `AICTL.md`). If present, its contents are appended to the system prompt, allowing per-project instructions for the agent. The filename can be customized via `AICTL_PROMPT_FILE` in `~/.aictl/config`. When the configured/default file is missing, aictl falls back to `CLAUDE.md` and then `AGENTS.md` so existing project instructions for other tools are reused automatically; the fallback chain can be disabled with `AICTL_PROMPT_FALLBACK=false`.

The quickest way to get started is the interactive wizard:

```bash
aictl --config
```

It walks you through selecting a provider, model, and entering API keys. You can also edit `~/.aictl/config` manually at any time.

## Basic configuration

You need to configure the API key for the provider and model you want to use. `AICTL_INCOGNITO` is optional.

| Key | Description |
|-----|-------------|
| `AICTL_PROVIDER` | Default provider (`openai`, `anthropic`, `gemini`, `grok`, `mistral`, `deepseek`, `kimi`, `zai`, `ollama`, `gguf`, `mlx`, or `aictl-server`) |
| `AICTL_MODEL` | Default model name |
| `AICTL_INCOGNITO` | Start interactive REPL without saving sessions. Accepts `true` or `false` (default: `false`) |
| `AICTL_PROMPT_FILE` | Filename for the project prompt file loaded from the current directory (default: `AICTL.md`) |
| `AICTL_PROMPT_FALLBACK` | When the primary prompt file is missing, fall back to `CLAUDE.md` then `AGENTS.md`. Accepts `true` or `false` (default: `true`) |
| `AICTL_TOOLS_ENABLED` | Enable or disable all tool calls. When `false`, the LLM can only respond with plain text (default: `true`) |
| `AICTL_AUTO_COMPACT_THRESHOLD` | Context usage percentage at which the REPL auto-compacts the conversation. Accepts an integer in `1..=100` (default: `80`) |
| `AICTL_LLM_TIMEOUT` | Per-call LLM response timeout in seconds. Applied to every provider (remote APIs, Ollama, native GGUF/MLX) and to the compaction and agent-generation calls. `0` disables the timeout. Default: `30` |
| `AICTL_MAX_ITERATIONS` | Maximum number of LLM calls allowed in a single agent turn before the loop aborts. Accepts a positive integer (default: `20`) |
| `AICTL_SKILLS_DIR` | Override the location of the skills directory (default: `~/.aictl/skills`) |
| `AICTL_MEMORY_ENABLED` | Enable or disable long-term memory. When `true` (default), saved facts in `~/.aictl/memory.json` are loaded into the system prompt of every conversation; the `save_memory` tool and `/remember` are write-enabled. Incognito mode is a stronger kill-switch and overrides this flag for both reads and writes |
| `AICTL_WORKING_DIR_CLI` | Persistent working directory for the CLI — used as the CWD jail root and the spawn dir for every tool call. Accepts absolute, relative, and `~`-prefixed paths. Overridden by `--cwd <PATH>`; falls back to `AICTL_WORKING_DIR` (legacy) and then the launch directory |
| `AICTL_WORKING_DIR` | Legacy unsuffixed fallback for the working directory. Kept working for existing configs; `AICTL_WORKING_DIR_CLI` wins when both are set |
| `AICTL_CLIENT_HOST` | Base URL of an upstream `aictl-server` (e.g. `http://127.0.0.1:7878`). Used only when the active provider is `aictl-server`; otherwise inert. Empty/unset = direct providers (the default) |
| `AICTL_CLIENT_MASTER_KEY` | Bearer token presented to the configured `aictl-server`. Same `/keys` lock/unlock/clear lifecycle as the provider keys. Distinct from the server's own `AICTL_SERVER_MASTER_KEY` (also covered by `/keys`) so a single host can run both roles unambiguously |

## API keys

`FIRECRAWL_API_KEY` is optional and is needed only if you want to use the `search_web_fc` tool. Without it, web search falls back to `search_web_ddg`, which uses the public DuckDuckGo Instant Answer API and requires no key.

Not all API keys are required. You need to provide only those for which you set `AICTL_PROVIDER` and `AICTL_MODEL`.

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
| `FIRECRAWL_API_KEY` | API key for Firecrawl (`search_web_fc` tool) |

### Where to get API keys

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

## Secure key storage (system keyring)

By default, API keys live as plain text in `~/.aictl/config`. aictl can also store them in the OS-native keyring — macOS Keychain or Linux Secret Service (gnome-keyring / KWallet via D-Bus) — and reads them transparently from whichever store has them.

The active backend appears in the welcome banner (`keys: Keychain (2 locked · 1 plain · 0 both)`) and `/security` shows the per-key location.

Migration is done from inside the REPL via the `/keys` interactive menu:

- **lock keys** — copies every plain-text key found in `~/.aictl/config` into the system keyring and removes the plain-text copy
- **unlock keys** — copies every keyring entry back into `~/.aictl/config` and deletes it from the keyring
- **clear keys** — removes the keys from both stores (asks for confirmation)

The same operations are available as one-shot CLI flags: `--lock-keys`, `--unlock-keys`, `--clear-keys`.

When the keyring backend is unavailable (e.g. headless Linux without a Secret Service daemon), aictl falls back to plain-text storage automatically and the banner shows `keys: plain text` in yellow.

On macOS, each signed aictl binary (CLI, server, desktop) has its own Keychain ACL. The first time a different binary reads a key that another locked, macOS prompts for your login password to authorize access — click *Always Allow* to suppress future prompts for that key/binary pair. The prompt is the system asking you to grant cross-binary access, not aictl itself.

## Security configuration

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
| `AICTL_SECURITY_DISABLED_TOOLS` | Comma-separated tool names to disable (e.g. `exec_shell,search_web_fc`) |
| `AICTL_SECURITY_BLOCKED_ENV` | Additional env vars to scrub from shell subprocesses |
| `AICTL_SECURITY_AUDIT_LOG` | Append one JSON line per tool invocation to `~/.aictl/audit/<session-id>` (default: `true`) |
| `AICTL_SECURITY_REDACTION` | Outbound-message redaction mode: `off` (default), `redact`, or `block`. In `redact` mode each credential/PII match is swapped for `[REDACTED:<KIND>]` on the wire; in `block` mode the turn aborts with a scrubbed error. |
| `AICTL_SECURITY_REDACTION_LOCAL` | Also redact when sending to local providers (Ollama / GGUF / MLX). Default `false` — data never leaves the machine for these, so there's no privacy gain. |
| `AICTL_REDACTION_DETECTORS` | Comma-separated subset of built-in detectors (empty = all): `api_key, aws, aws_secret, jwt, private_key, connection_string, credit_card, iban, email, phone, url_secret, ssn, pesel, ip_address, mac_address, high_entropy`. |
| `AICTL_REDACTION_EXTRA_PATTERNS` | Semicolon-separated `NAME=REGEX` pairs. Each match is replaced with `[REDACTED:NAME]` (e.g. `CUSTOMER_ID=CUST-\d{8};TICKET=JIRA-\d{4,}`). |
| `AICTL_REDACTION_ALLOW` | Semicolon-separated regexes; any detection whose span is covered by an allowlist hit is dropped. Useful for documentation examples or internal IDs that trip the entropy scanner. |
| `AICTL_REDACTION_NER` | Enable the optional Layer-C NER pass (person / location / organization). Requires the `redaction-ner` cargo feature and a pulled model. Default `false`. |
| `AICTL_REDACTION_NER_MODEL` | NER model spec (`owner/repo` or `hf:owner/repo`). Default: `onnx-community/gliner_small-v2.1`. |
| `AICTL_PLUGINS_ENABLED` | Master switch for the plugin subsystem (default: `false`). Plugins are third-party code; they will not auto-load until you opt in. |
| `AICTL_PLUGINS_DIR` | Override the plugin discovery root (default: `~/.aictl/plugins`). Used mainly by tests and isolated installs. |
| `AICTL_PLUGINS_DISABLED` | Comma-separated plugin names to skip at load time. Useful for silencing one third-party plugin without editing its manifest. |
| `AICTL_HOOKS_FILE` | Override the hooks config path (default: `~/.aictl/hooks.json`). Used mainly by tests and isolated installs. |
| `AICTL_MCP_ENABLED` | Master switch for the MCP subsystem (default: `false`). MCP servers are third-party processes; they will not auto-spawn until you opt in. |
| `AICTL_MCP_CONFIG` | Override the MCP config path (default: `~/.aictl/mcp.json`). |
| `AICTL_MCP_TIMEOUT` | Default per-call RPC timeout in seconds for `tools/call` (default: `30`). Per-server overrides via `timeout_secs` in `mcp.json` win when set. |
| `AICTL_MCP_STARTUP_TIMEOUT` | `initialize` handshake timeout per server, in seconds (default: `10`). Hung servers are marked `Failed` and skipped — startup never blocks on a bad server. |
| `AICTL_MCP_DISABLED` | Comma-separated MCP server names to skip at load time, even when their `enabled` flag is `true`. |
| `AICTL_MCP_DENY_SERVERS` | Comma-separated MCP server names whose every tool is blocked at the security gate, even when the master switch is on. |

## Example config file

Create `~/.aictl/config` (see `.aictl/config` in this repo for the reference):

```
AICTL_PROVIDER=anthropic
AICTL_MODEL=claude-sonnet-4-6
LLM_ANTHROPIC_API_KEY=sk-ant-...
FIRECRAWL_API_KEY=fc-...
```

The file format supports comments (`#`), quoted values, and optional `export` prefixes.

## See also

- [PROVIDERS.md](PROVIDERS.md) — supported providers, models, and pricing.
- [TOOLS.md](TOOLS.md) — built-in tools, including the security gate.
- [SERVER.md](SERVER.md) — `aictl-server` configuration (separate `AICTL_SERVER_*` keys).
