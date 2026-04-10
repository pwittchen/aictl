# Issues

## Bugs

- **Malformed tool calls treated as final answer** `[agent-loop]` — When `parse_tool_call()` returns `None`, the response is surfaced to the user as a final answer even if it contains a malformed tool call. The agent loop should detect this and retry rather than printing raw tool XML.

## Config

- **Auto-compact Configuration** - allow to configure auto-compact feature

## Security

- **Secure API key storage** `[config]` — API keys are stored as plain text in `~/.aictl/config`. Integrate system keyring (e.g. `keyring` crate) with plain-text fallback. Show storage backend in the welcome banner and `/info`.

- **Key management commands** `[config] [ui]` — Depends on secure key storage. Add `/lock-keys` (migrate to keyring, remove from config), `/unlock-keys` (migrate back), `/clear-keys` (remove from both). Show per-key storage status in `/info` and `/security`.

## Tools

- **Image analysis** `[tool] [llm]` — Send images to vision-capable models. Accept file paths or URLs, encode as base64, pass via the provider's vision API.

- **Image generation** `[tool]` — Generate images via an external API (e.g. DALL-E, Stable Diffusion). Save to disk and return the file path.

- **PDF and DOCX reading** `[tool]` — Extract text from PDF and DOCX files so the agent can reason over documents.

- **XLSX reading** `[tool]` — Read spreadsheets via the `calamine` crate. Return cell contents in a structured format.

## LLM Providers

- **Native local model support** `[llm]` — Load and run models directly from disk (e.g. GGUF via `llama.cpp` bindings or ONNX runtime) without a separate server. Document per-model requirements — some (Gemma, Llama) may need an HF key; most GGUF models can be downloaded without one.

## Infrastructure

- **Project domain** — Register a domain and point it to the VPS.
- **Project website** `[marketing]` — Build a public-facing project website.

## Roadmap

### Modular architecture

Split the codebase into separate modules: `core` (shared logic), `cli`, `server` (currently empty), `desktop` (currently empty) to enable independent development of each target.

### Server

Expose program functionality via a REST API protected by a local API key.

### Desktop

Provide a desktop app with the same capabilities as the CLI. macOS support is required; other platforms are a stretch goal.
