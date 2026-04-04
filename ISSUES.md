# Issues

## Security

- **Secure API key storage** `[config]` — Research and implement a more secure way of storing API keys instead of plain text. Consider using the system keyring with a plain-text environment variable as a fallback. Add a note in the welcome banner and `/info` output indicating whether keys are stored securely.

- **Key management commands** `[config] [ui]` — Once secure key storage is implemented, show the storage status of each API key in the `/info` and `/security` commands (whether the key is set and whether it is secured via keyring or stored in plain text). Add the following commands: `/lock-keys` (copy API keys to the keyring and remove them from the config file), `/unlock-keys` (copy keys back to the config file and remove them from the keyring), and `/clear-keys` (remove keys from both locations).

## Configuration

- **Agent prompt profiles** `[config]` — Add support for managing multiple agent prompts that users can switch between depending on the use case. Prompt definitions would live in `~/.aictl/agents/`. Consider allowing selection via a CLI parameter, a naming convention, or both. Add commands for managing agents: create, use, discard, and delete.

- **Session persistence and restoration** `[func]` — Allow users to persist and restore conversation sessions. Sessions would be stored in `~/.aictl/sessions/` with a name and timestamp (use a random name if none is provided). Persistence should be opt-in via a `--session` or `--memory` flag. On restoration, the session history should be read, compacted, and used for future conversation context. Consider tying sessions to agent profiles (related to the agent prompt profiles issue above).

- **Update config file path** `[config]` — Move the config file from `~/.aictl` to `~/.aictl/config`. A dedicated project directory opens up more possibilities for structured storage (sessions, agent profiles, etc.).

## LLM Providers

- **Add Grok support** `[llm]`
- **Add Mistral support** `[llm]`
- **Add Z.ai support** `[llm]`
- **Add DeepSeek support** `[llm]`
- **Add Ollama support** `[llm]`
- **Add native local model support** `[llm]` — Load models directly from disk using ONNX or a similar format.

## Tools

- **Image processing** `[tool]` — Add the ability to process and analyze images.
- **Document processing** `[tool]` — Add support for reading PDF and DOCX files.
- **Spreadsheet processing** `[tool]` — Add support for reading XLSX files using the `calamine` crate.

## Bugs

- **Tool output sometimes printed instead of executed** `[bug]` — Occasionally the LLM generates a tool call (especially a shell command) but the agent prints it as a final response instead of executing it. This may be related to errors such as a nonexistent command. Update the agent loop so that tool output is never presented as a final result; instead, the agent should retry with a different approach.

## Other

- **Project website** `[marketing]` — Create a public-facing project website.
