# Issues

## Security

- **Secure API key storage** `[config]` — Research and implement a more secure way of storing API keys instead of plain text. Consider using the system keyring with a plain-text environment variable as a fallback. Add a note in the welcome banner and `/info` output indicating whether keys are stored securely.

- **Key management commands** `[config] [ui]` — Once secure key storage is implemented, show the storage status of each API key in the `/info` and `/security` commands (whether the key is set and whether it is secured via keyring or stored in plain text). Add the following commands: `/lock-keys` (copy API keys to the keyring and remove them from the config file), `/unlock-keys` (copy keys back to the config file and remove them from the keyring), and `/clear-keys` (remove keys from both locations).

- **All tools switch** `[config]` `[tools]` - Add possibility tu turn off all tools via single param in the config `AICTL_TOOLS_ENABLED`. When this parameter is not set, it falls back to `true` by default.

## Configuration

- **Agent prompt profiles** `[config]` — Add support for managing multiple agent prompts that users can switch between depending on the use case. Prompt definitions would live in `~/.aictl/agents/`. Consider allowing selection via a CLI parameter, a naming convention, or both. Add commands for managing agents: create, use, discard, and delete.

- **Entry file per directory** `[config]` - Add entry file per directory - e.g `AICTL.md`, which will add additional prompt configured by the user while running agent from a specific dir. Similar concept to `CLAUDE.md` or `AGENTS.md`. Add possibility to configure entry file name (e.g. when user use claude code or codex by default, he can configure convetion name from other tool and use it in this tool)

- **Configuration command** `[config]` - Crate `/configure` command, which will allow to configure program and persist user choices. This command can be invoked anytime while using the app or automatically when the app detects that there is no configuration.

## LLM Providers

- **Add native local model support** `[llm]` — Load models directly from disk using ONNX or a similar format.

## Tools

- **Image processing** `[tool]` — Add the ability to process and analyze images.
- **Document processing** `[tool]` — Add support for reading PDF and DOCX files.
- **Spreadsheet processing** `[tool]` — Add support for reading XLSX files using the `calamine` crate.

## Bugs

- **Tool output sometimes printed instead of executed** `[bug]` — Occasionally the LLM generates a tool call (especially a shell command) but the agent prints it as a final response instead of executing it. This may be related to errors such as a nonexistent command. Update the agent loop so that tool output is never presented as a final result; instead, the agent should retry with a different approach.

## Other

- **Project website** `[marketing]` — Create a public-facing project website.
