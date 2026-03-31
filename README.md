# aictl 🤖 

[![CI](https://github.com/pwittchen/aictl/actions/workflows/ci.yml/badge.svg)](https://github.com/pwittchen/aictl/actions/workflows/ci.yml)
[![RELEASE](https://github.com/pwittchen/aictl/actions/workflows/release.yml/badge.svg)](https://github.com/pwittchen/aictl/actions/workflows/release.yml)

general purpose AI agent for terminal

![aictl screenshot](screenshot.png)

## Install

```bash
curl -sSf https://raw.githubusercontent.com/pwittchen/aictl/master/install.sh | sh
```

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

## Usage

```bash
aictl [--provider <PROVIDER>] [--model <MODEL>] [--message <MESSAGE>] [--auto] [--quiet]
```

Omit `--message` to enter interactive REPL mode with persistent conversation history.

### REPL Commands

The interactive REPL supports slash commands:

| Command | Description |
|---------|-------------|
| `/clear` | Clear conversation context |
| `/compact` | Summarize conversation into a compact context |
| `/context` | Show context usage (token and message counts vs limits) |
| `/copy` | Copy last response to clipboard |
| `/help` | Show available commands |
| `/info` | Show setup info (provider, model, mode, version, OS, binary size) |
| `/mode` | Switch between auto and human-in-the-loop mode during the session |
| `/model` | Switch model and provider during the session (persists to `~/.aictl`) |
| `/tools` | Show available tools |
| `/exit` | Exit the REPL |

Press **Esc** during any LLM call or tool execution to interrupt the operation and return to the prompt. Conversation history is rolled back so the interrupted turn has no effect.

### Parameters

| Flag | Short | Description |
|------|-------|-------------|
| `--provider` | `-p` | LLM provider (`openai` or `anthropic`). Falls back to `AICTL_PROVIDER` in `~/.aictl` |
| `--model` | `-M` | Model name (e.g. `gpt-4o`). Falls back to `AICTL_MODEL` in `~/.aictl` |
| `--message` | `-m` | Message to send (omit for interactive mode) |
| `--auto` | `-a` | Run in autonomous mode (skip tool confirmation prompts) |
| `--quiet` | `-q` | Suppress tool calls and reasoning, only print the final answer (requires `--auto`) |

CLI flags take priority over config file values.

### Configuration

Configuration is loaded from `~/.aictl`. This is a single global config file — the program works the same regardless of the current working directory.

| Key | Description |
|-----|-------------|
| `AICTL_PROVIDER` | Default provider (`openai` or `anthropic`) |
| `AICTL_MODEL` | Default model name |
| `OPENAI_API_KEY` | API key for OpenAI |
| `ANTHROPIC_API_KEY` | API key for Anthropic |
| `FIRECRAWL_API_KEY` | API key for Firecrawl (`search_web` tool) |

Create `~/.aictl` (see `.aictl.example`):

```
AICTL_PROVIDER=anthropic
AICTL_MODEL=claude-sonnet-4-20250514
ANTHROPIC_API_KEY=sk-ant-...
FIRECRAWL_API_KEY=fc-...
```

The file format supports comments (`#`), quoted values, and optional `export` prefixes.

### Providers

aictl supports two LLM providers:

#### OpenAI

Requires `OPENAI_API_KEY`. Supported models with cost estimates (input/output per 1M tokens):

| Model | Input | Output |
|-------|-------|--------|
| `gpt-4.1-nano` | $0.10 | $0.40 |
| `gpt-4.1-mini` | $0.40 | $1.60 |
| `gpt-4.1` | $2.00 | $8.00 |
| `gpt-4o-mini` | $0.15 | $0.60 |
| `gpt-4o` | $2.50 | $10.00 |
| `gpt-5-mini` | $0.25 | $2.00 |
| `gpt-5` | $1.25 | $10.00 |
| `o4-mini` | $1.10 | $4.40 |
| `o3` | $2.00 | $8.00 |
| `o1` | $15.00 | $60.00 |

#### Anthropic

Requires `ANTHROPIC_API_KEY`. Supported models with cost estimates (input/output per 1M tokens):

| Model | Input | Output |
|-------|-------|--------|
| `claude-haiku-*` (3.x) | $0.25 | $1.25 |
| `claude-haiku-4-*` | $1.00 | $5.00 |
| `claude-sonnet-*` | $3.00 | $15.00 |
| `claude-opus-4-5-*` / `claude-opus-4-6-*` | $5.00 | $25.00 |
| `claude-opus-4-*` (older) | $15.00 | $75.00 |

Any model string can be passed via `--model`; cost estimation uses pattern matching on the model name and falls back to zero if unrecognized.

### Agent Loop & Tool Calling

aictl runs an agent loop: the LLM can invoke tools, see their results, and continue reasoning until it produces a final answer.

By default, every tool call requires confirmation (y/N prompt). Use `--auto` to skip confirmation and run autonomously.

Available tools:

| Tool | Description |
|------|-------------|
| `exec_shell` | Execute a shell command via `sh -c` |
| `read_file` | Read the contents of a file |
| `write_file` | Write content to a file (first line = path, rest = content) |
| `list_directory` | List files and directories at a path with `[FILE]`/`[DIR]`/`[LINK]` prefixes |
| `search_files` | Search file contents by pattern (grep regex) with optional directory scope |
| `edit_file` | Apply a targeted find-and-replace edit to a file (exact unique match required) |
| `search_web` | Search the web via Firecrawl API (requires `FIRECRAWL_API_KEY`) |
| `find_files` | Find files matching a glob pattern (e.g. `**/*.rs`) with optional base directory |
| `fetch_url` | Fetch a URL and return readable text content (HTML tags stripped) |
| `extract_website` | Fetch a URL and extract only the main readable content (strips scripts, styles, nav, boilerplate) |
| `fetch_datetime` | Get the current date, time, timezone, and day of week |
| `fetch_geolocation` | Get geolocation data for an IP address (city, country, timezone, coordinates, ISP) via ip-api.com |

The tool-calling mechanism uses a custom XML format in the LLM response text (not provider-native tool APIs):

```xml
<tool name="exec_shell">
ls -la /tmp
</tool>
```

The agent loop runs for up to 20 iterations. LLM reasoning is printed to stderr; the final answer goes to stdout. Token usage, estimated cost, and execution time are always displayed after each response.

### Examples

```bash
# With defaults configured in ~/.aictl, just run:
aictl

# Or send a single message:
aictl -m "What is Rust?"

# Override provider/model from the command line:
aictl -p openai -M gpt-4o -m "What is Rust?"

# Agent with tool calls (interactive confirmation)
aictl -m "List files in the current directory"

# Autonomous mode (no confirmation prompts)
aictl --auto -m "What OS am I running?"

# Quiet mode (only final answer, no tool calls or reasoning)
aictl --auto -q -m "What OS am I running?"
```

## Tests

```bash
cargo test
```

Unit tests cover core logic across five modules: `commands` (slash command parsing), `config` (config file parsing), `tools` (tool-call XML parsing), `ui` (formatting helpers), and `llm` (cost estimation and model matching).

## Architecture

See [ARCH.md](ARCH.md) for detailed ASCII diagrams covering:

- Module structure
- Startup flow
- Agent loop
- Tool execution dispatch
- LLM provider abstraction
- UI layer
- End-to-end data flow

## Known Issues & Ideas

See [ISSUES.md](ISSUES.md) for a list of known issues and planned improvements.

## License

This project is licensed under the [PolyForm Noncommercial License 1.0.0](LICENSE). It is free to use for non-commercial purposes, including personal use, research, education, and use by non-profit organizations. For commercial use, please contact [piotr@wittchen.io](mailto:piotr@wittchen.io).
