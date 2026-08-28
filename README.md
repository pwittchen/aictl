# <img src="crates/aictl-desktop/icons/icon.png" alt="aictl icon" width="40" height="40" valign="top" /> aictl

[![CI](https://github.com/pwittchen/aictl/actions/workflows/ci.yml/badge.svg)](https://github.com/pwittchen/aictl/actions/workflows/ci.yml)
[![RELEASE](https://github.com/pwittchen/aictl/actions/workflows/release.yml/badge.svg)](https://github.com/pwittchen/aictl/actions/workflows/release.yml)
[![DEPLOY WEBSITE](https://github.com/pwittchen/aictl/actions/workflows/deploy-website.yml/badge.svg)](https://github.com/pwittchen/aictl/actions/workflows/deploy-website.yml)

Native AI agent for your terminal and macOS desktop.
Run cloud and local models through one secure runtime.

Project website: [aictl.app](https://aictl.app) — source in [`website/`](website/).

**User guides** (main components):

- Desktop (macOS): https://aictl.app/desktop.html
- Terminal: https://aictl.app/terminal.html
- Server: https://aictl.app/server.html

You can use **aictl** on your desktop and in your terminal — with a single configuration and feature parity between CLI and desktop app — or as an HTTP server with LLM proxy and security features.

[![screenshot](screenshot.png)](https://aictl.app)

## Install

```bash
curl -sSf https://aictl.app/install.sh | sh
```

For build-from-source, Docker, optional cargo features (`gguf`, `mlx`, `redaction-ner`), the macOS desktop app, and uninstall instructions, see [docs/INSTALL.md](docs/INSTALL.md).

## Quick start with CLI

```bash
# Interactive REPL (after running aictl --config once to set provider + key):
aictl

# Single-shot message:
aictl --message "What is Rust?"

# Autonomous mode (no tool-call confirmation prompts):
aictl --auto --message "List the largest files in this directory"
```

The first launch will walk you through provider, model, and API-key setup. State (config, sessions, audit log, agents, skills) lives under `~/.aictl/`.

## Desktop app (macOS)

A native macOS desktop app (`aictl-desktop`) is available as a Tauri-based frontend over the same `aictl-core` engine the CLI uses — same providers, tools, agents, skills, and config. Download the latest `.dmg` from [aictl.app/desktop.html](https://aictl.app/desktop.html) (or the [GitHub releases](https://github.com/pwittchen/aictl/releases) page), open it, and drag `aictl.app` into `/Applications`. Apple Silicon and Intel builds are published per release. For build-from-source instructions see [docs/INSTALL.md](docs/INSTALL.md#desktop-app-macos).

## HTTP server (`aictl-server`)

A second binary in this workspace, `aictl-server`, exposes the same provider catalogue over an OpenAI-compatible HTTP endpoint with redaction, prompt-injection blocking, audit, and a master-key gate. Pure proxy — no agent loop, no tools, no agents/skills/sessions. See [docs/SERVER.md](docs/SERVER.md) for the full reference.

```sh
curl -fsSL https://aictl.app/server/install.sh | sh
aictl-server     # listens on 127.0.0.1:7878 by default; prints master key on first launch
aictl --serve    # convenience shortcut from the CLI; forwards trailing args after `--`
```

`aictl-server` can also be used as the upstream for the CLI itself (centralized provider keys + audit on the server, single master key on every client laptop), or as [Claude Code's Anthropic-compatible inference provider](docs/SERVER.md#connecting-claude-code) — including opt-in cross-provider routing that translates Claude Code's `POST /v1/messages` to/from OpenAI, Gemini, Ollama, and the rest.

## Documentation

| Topic | File |
|---|---|
| Install, build, Docker, desktop app | [docs/INSTALL.md](docs/INSTALL.md) |
| CLI flags, REPL slash commands, sessions, examples | [docs/USAGE.md](docs/USAGE.md) |
| Config keys, API keys, secure key storage, security knobs | [docs/CONFIG.md](docs/CONFIG.md) |
| Supported providers and per-token pricing | [docs/PROVIDERS.md](docs/PROVIDERS.md) |
| Realistic chat / coding-agent workday cost estimates | [docs/LLM_PRICING.md](docs/LLM_PRICING.md) |
| Built-in tools, image capabilities, security gate | [docs/TOOLS.md](docs/TOOLS.md) |
| Agents, skills, plugins, hooks, MCP servers | [docs/EXTENSIONS.md](docs/EXTENSIONS.md) |
| Coding-agent mode (experimental) | [docs/CODING_AGENT.md](docs/CODING_AGENT.md) |
| HTTP server (`aictl-server`) reference | [docs/SERVER.md](docs/SERVER.md) |
| Architecture diagrams | [docs/ARCH.md](docs/ARCH.md) |
| Roadmap | [ROADMAP.md](ROADMAP.md) |
| Design notes | [docs/DESIGN.md](docs/DESIGN.md) |

## Tests

```bash
cargo test
```

Unit tests cover core logic across six modules: `commands` (slash command parsing), `config` (config file parsing), `tools` (tool-call XML parsing), `ui` (formatting helpers), `llm` (cost estimation and model matching), and `security` (shell validation, path validation, output sanitization).

## Claude Code Skills

This project includes [Claude Code](https://claude.ai/code) skills for common workflows in `.claude/skills/`:

| Skill | Description |
|-------|-------------|
| `/commit` | Commit staged and unstaged changes with a clear commit message |
| `/update-docs` | Update README.md, CLAUDE.md, and the `docs/` tree to match the current project state |
| `/evaluate-rust-quality` | Audit code quality, idiomatic Rust usage, and best practices |
| `/evaluate-rust-security` | Audit security posture, injection risks, and credential handling |
| `/evaluate-rust-performance` | Audit performance patterns, allocations, and CLI responsiveness |
| `/project-stats-report` | Generate a project statistics report (LOC, commit activity, contributors, etc.) |
| `/sync-models` | Check each provider for newly released models and update the supported set and README |
| `/create-hook` | Add a lifecycle hook to `~/.aictl/hooks.json` (event, matcher, command, timeout) |
| `/add-mcp-server` | Connect an MCP server by adding an entry to `~/.aictl/mcp.json` |

Evaluation reports are saved to `.claude/reports/` with timestamped filenames.

## Support & consulting

**aictl** is free to use for non-commercial purposes (see [License](#license)). The CLI, desktop app, and HTTP server are open source and developed in the open — no paid tiers, no feature gating, no telemetry.

If you or your team need help with any of the following, get in touch at [piotr@wittchen.io](mailto:piotr@wittchen.io):

- **Hosted `aictl-server`** — managed multi-user proxy with shared keys, audit logs, and per-user budgets, deployed on your infra or mine.
- **On-prem deployment & setup** — installing, configuring, and integrating aictl into your stack (corporate proxies, SSO, internal MCP servers, custom redaction rules).
- **Custom features & integrations** — provider adapters, MCP servers, plugins, redaction detectors, or domain-specific agents and skills tailored to your workflow.
- **Custom AI Agents development** — design and build bespoke AI agents (tools, prompts, evaluations, deployment) tailored to your domain, whether on top of aictl or as standalone systems.
- **Commercial licensing** — a license for use inside a for-profit organization (the default [PolyForm Noncommercial License](LICENSE) does not cover this).
- **Support contracts** — SLA-backed support, security reviews, and prioritized bug fixes.

## License

This project is licensed under the [PolyForm Noncommercial License 1.0.0](LICENSE). It is free to use for non-commercial purposes, including personal use, research, education, and use by non-profit organizations. For commercial use, please contact [piotr@wittchen.io](mailto:piotr@wittchen.io).
