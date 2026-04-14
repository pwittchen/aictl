# Issues

## LLM Providers

- **Native local model support** `[llm]` — Load and run models directly from disk (e.g. GGUF via `llama.cpp` bindings or ONNX runtime) without a separate server. Document per-model requirements — some (Gemma, Llama) may need an HF key; most GGUF models can be downloaded without one. First, gather information, which models can be handled in such a way and which require hugging face API key.

## Infrastructure

- **Project domain configuration** — configure domain, so it'll point to the VPS via Cloudflare (with `cloudflared`)
- **Project website** `[marketing]` — Build a public-facing project website.

## Roadmap

### Modular architecture

Split the codebase into separate modules: `core` (shared logic), `cli`, `server` (currently empty), `desktop` (currently empty) to enable independent development of each target.

### Server

Expose program functionality via a REST API protected by a local API key / token (optional).

### Desktop

Provide a desktop app with the same capabilities as the CLI. macOS support is required; other platforms are a stretch goal.

### Coding Agent

Provide configurable mode, which will transofrm the general purpose agent into the coding agent. There should be additional skills/tools and prompts available for such mode, which won't be available in the "default" general purpose mode.
