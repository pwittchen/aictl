# aictl

[WORK IN PROGRESS] 🚧 experimental general purpose AI agent for terminal

## Issues

[ISSUES.md](ISSUES.md)

## Usage

```bash
aictl --provider <PROVIDER> --model <MODEL> --message <MESSAGE> [--auto]
```

### Parameters

| Flag | Short | Description |
|------|-------|-------------|
| `--provider` | `-p` | LLM provider (`openai` or `anthropic`) |
| `--model` | `-m` | Model name (e.g. `gpt-4o`, `claude-sonnet-4-20250514`) |
| `--message` | `-M` | Message to send to the LLM |
| `--auto` | | Run in autonomous mode (skip tool confirmation prompts) |

### API Keys

API keys are loaded from a `.env` file in the current directory or from system environment variables. The `.env` file takes priority over system env vars.

| Provider | Environment Variable |
|----------|---------------------|
| `openai` | `OPENAI_API_KEY` |
| `anthropic` | `ANTHROPIC_API_KEY` |

Create a `.env` file (see `.env.example`):

```
OPENAI_API_KEY=sk-...
ANTHROPIC_API_KEY=sk-ant-...
```

### Agent Loop & Tool Calling

aictl runs an agent loop: the LLM can invoke tools, see their results, and continue reasoning until it produces a final answer.

By default, every tool call requires confirmation (y/N prompt). Use `--auto` to skip confirmation and run autonomously.

Available tools:

| Tool | Description |
|------|-------------|
| `shell` | Execute a shell command via `sh -c` |
| `read_file` | Read the contents of a file |
| `write_file` | Write content to a file (first line = path, rest = content) |
| `list_directory` | List files and directories at a path with `[FILE]`/`[DIR]`/`[LINK]` prefixes |
| `search_files` | Search file contents by pattern (grep regex) with optional directory scope |
| `edit_file` | Apply a targeted find-and-replace edit to a file (exact unique match required) |

The tool-calling mechanism uses a custom XML format in the LLM response text (not provider-native tool APIs):

```xml
<tool name="shell">
ls -la /tmp
</tool>
```

The agent loop runs for up to 20 iterations. LLM reasoning is printed to stderr; the final answer goes to stdout.

### Examples

```bash
# Using a .env file (recommended)
echo 'OPENAI_API_KEY=sk-...' > .env
aictl -p openai -m gpt-4o -M "What is Rust?"

# Using environment variables
export ANTHROPIC_API_KEY="sk-ant-..."
aictl -p anthropic -m claude-sonnet-4-20250514 -M "What is Rust?"

# Agent with tool calls (interactive confirmation)
aictl -p anthropic -m claude-sonnet-4-20250514 -M "List files in the current directory"

# Autonomous mode (no confirmation prompts)
aictl -p anthropic -m claude-sonnet-4-20250514 --auto -M "What OS am I running?"
```

## Install

### Prerequisites

- [Rust](https://www.rust-lang.org/tools/install) (edition 2024)

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
