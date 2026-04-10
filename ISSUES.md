# Issues

## Bugs

- **Tool output printed instead of executed** `[agent-loop]` — When `parse_tool_call()` returns `None`, the response is treated as a final answer even if it contains a failed tool call. The agent loop should detect malformed or failed tool calls and retry with a different approach rather than surfacing raw tool output to the user.

## Security

- **Secure API key storage** `[config]` — API keys are currently stored as plain text in `~/.aictl/config`. Implement system keyring integration (e.g. `keyring` crate) with plain-text fallback. Show storage status in the welcome banner and `/info` output.

- **Key management commands** `[config] [ui]` — Depends on secure key storage. Show per-key storage status (keyring vs plain text) in `/info` and `/security`. Add commands: `/lock-keys` (migrate keys to keyring, remove from config), `/unlock-keys` (migrate back to config), `/clear-keys` (remove from both).

## Tools

- **Image analysis** `[tool] [llm]` — Send images to vision-capable LLMs for analysis. Accept file paths or URLs, encode as base64, and pass via the provider's vision API.

- **Image generation** `[tool]` — Generate images via an external API (e.g. DALL-E, Stable Diffusion). Save output to disk and display the file path.

- **PDF and DOCX reading** `[tool]` — Extract text content from PDF and DOCX files so the agent can reason over documents.

- **XLSX reading** `[tool]` — Read spreadsheet data using the `calamine` crate. Return cell contents in a structured format the agent can process.

## LLM Providers

- **Native local model support** `[llm]` — Load and run models directly from disk (e.g. GGUF via `llama.cpp` bindings or ONNX runtime) without requiring a separate server like Ollama. Provide requirements for the specific models, because some of them may require different path (e.g. necessary HF key) - especially Gemma or Llama. Rest of the models probably can be downloaded as GGUF without HF key.

## Other

- **Project domain** — Configure a project domain and connect it to the VPS.
- **Project website** `[marketing]` — Create a public-facing project website.

## Roadmap

### Refactoring

- **Modules** - split program code into modules: core (shared code), cli, server (currently empty), desktop (currently empty) for the purpose of the future development of the server and desktop apps

### Server

- **Server** - create separate module, which allows to expose program functionality on the server with REST API protected behind the local API key

### Desktop

- **Desktop app** - create desktop app, which provides the same functionality as CLI (preferable for all popular OSes, but for macOS it's required)
