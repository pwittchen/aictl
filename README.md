# aictl
experimental general purpose AI agent

goal of this project is to create an experimental AI agent to test integrations & interactions with LLMS, agent-loop, human-in-the-loop patterns and tool calling

## Usage

```bash
aictl --provider <PROVIDER> --api-key <API_KEY> --model <MODEL> --message <MESSAGE>
```

### Parameters

| Flag | Short | Description |
|------|-------|-------------|
| `--provider` | `-p` | LLM provider (`openai` or `anthropic`) |
| `--api-key` | `-k` | API key for the selected provider |
| `--model` | `-m` | Model name (e.g. `gpt-4o`, `claude-sonnet-4-20250514`) |
| `--message` | `-M` | Message to send to the LLM |

### Examples

```bash
# OpenAI
aictl -p openai -k "sk-..." -m gpt-4o -M "What is Rust?"

# Anthropic
aictl -p anthropic -k "sk-ant-..." -m claude-sonnet-4-20250514 -M "What is Rust?"
```

### Build

```bash
cargo build --release
```

The binary will be at `target/release/aictl`.
