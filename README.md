# aictl
experimental general purpose AI agent

goal of this project is to create an experimental AI agent to test integrations & interactions with LLMS, agent-loop, human-in-the-loop patterns and tool calling

## Usage

```bash
aictl --provider <PROVIDER> --model <MODEL> --message <MESSAGE>
```

### Parameters

| Flag | Short | Description |
|------|-------|-------------|
| `--provider` | `-p` | LLM provider (`openai` or `anthropic`) |
| `--api-key` | `-k` | API key (optional, falls back to env var) |
| `--model` | `-m` | Model name (e.g. `gpt-4o`, `claude-sonnet-4-20250514`) |
| `--message` | `-M` | Message to send to the LLM |

### API Keys

The API key can be provided via the `--api-key` flag or through environment variables:

| Provider | Environment Variable |
|----------|---------------------|
| `openai` | `OPENAI_API_KEY` |
| `anthropic` | `ANTHROPIC_API_KEY` |

If both are set, the `--api-key` flag takes precedence.

### Examples

```bash
# Using environment variables
export OPENAI_API_KEY="sk-..."
aictl -p openai -m gpt-4o -M "What is Rust?"

export ANTHROPIC_API_KEY="sk-ant-..."
aictl -p anthropic -m claude-sonnet-4-20250514 -M "What is Rust?"

# Using the --api-key flag directly
aictl -p openai -k "sk-..." -m gpt-4o -M "What is Rust?"
```

### Build

```bash
cargo build --release
```

The binary will be at `target/release/aictl`.
