# Issues

## Security

- **Secure API key storage** `[config]` — Research and implement a more secure way of storing API keys instead of plain text. Consider using the system keyring with a plain-text environment variable as a fallback. Add a note in the welcome banner and `/info` output indicating whether keys are stored securely.

- **Key management commands** `[config] [ui]` — Once secure key storage is implemented, show the storage status of each API key in the `/info` and `/security` commands (whether the key is set and whether it is secured via keyring or stored in plain text). Add the following commands: `/lock-keys` (copy API keys to the keyring and remove them from the config file), `/unlock-keys` (copy keys back to the config file and remove them from the keyring), and `/clear-keys` (remove keys from both locations).

## LLM Providers

- **Add native local model support** `[llm]` — Load models directly from disk using ONNX or a similar format.

## Tools

- **Image processing - Analysis** `[tool]` — Add the ability to process and analyze images (preferably with base64 and existing APIs).
- **Image processing - Generation** `[tool]` — Add the ability to generate images.
- **Document processing** `[tool]` — Add support for reading PDF and DOCX files.
- **Spreadsheet processing** `[tool]` — Add support for reading XLSX files using the `calamine` crate.

## Bugs

- **Tool output sometimes printed instead of executed** `[bug]` — Occasionally the LLM generates a tool call (especially a shell command) but the agent prints it as a final response instead of executing it. This may be related to errors such as a nonexistent command. Update the agent loop so that tool output is never presented as a final result; instead, the agent should retry with a different approach.

## Other

- **Project website** `[marketing]` — Create a public-facing project website.
