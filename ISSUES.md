# Issues

## Tools

- **PDF and DOCX reading** `[tool]` — Extract text from PDF and DOCX files so the agent can reason over documents.

- **XLSX reading** `[tool]` — Read spreadsheets via the `calamine` crate. Return cell contents in a structured format.

## LLM Providers

- **Native local model support** `[llm]` — Load and run models directly from disk (e.g. GGUF via `llama.cpp` bindings or ONNX runtime) without a separate server. Document per-model requirements — some (Gemma, Llama) may need an HF key; most GGUF models can be downloaded without one.

## Infrastructure

- **Project domain configuration** — configure domain, so it'll point to the VPS via Cloudflare (with `cloudflared`)
- **Project website** `[marketing]` — Build a public-facing project website.

## Other

- **Stats** - add some stats to the app like number of requests, tool calls, model usage, costs - keep overall stats, per month and per day

## Roadmap

### Modular architecture

Split the codebase into separate modules: `core` (shared logic), `cli`, `server` (currently empty), `desktop` (currently empty) to enable independent development of each target.

### Server

Expose program functionality via a REST API protected by a local API key.

### Desktop

Provide a desktop app with the same capabilities as the CLI. macOS support is required; other platforms are a stretch goal.

### Coding Agent

Provide configurable mode, which will transofrm the general purpose agent into the coding agent. There should be additional skills/tools and prompts available for such mode, which won't be available in the "default" general purpose mode.
