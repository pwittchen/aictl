# Issues

## Agents

- **Manual Prompt Bug** - pasting copied prompt while creating new agent causes incorrect work. Prompt is copied partially and agent is being created too fast. It may be related to empty lines in prompt, but I'm not sure.

## Tools

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

### Coding Agent

Provide configurable mode, which will transofrm the general purpose agent into the coding agent. There should be additional skills/tools and prompts available for such mode, which won't be available in the "default" general purpose mode.
