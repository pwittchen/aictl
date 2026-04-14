# Issues

## UI

- **Merge commands** - merge `/stats` and `/clear-stats` into one `/stats` command with interactive menu for viewing and clearing stats; merge `/lock-keys`, '/unlock-keys`, `/clear-keys` into one `/keys` command with interactive menu for all these functionalities

- **Short-flags** - remove short-flags from non-interactive UI to keep it more clean and explicit, leave only `-v` shortflag for `--version` and `-h` for `--help` because this is widely used convention

## Infrastructure

- **Project domain configuration** — configure domain, so it'll point to the VPS via Cloudflare (with `cloudflared`)
- **Project website** `[marketing]` — Build a public-facing project website.

## Roadmap

### Modular architecture

Split the codebase into separate modules: `core` (shared logic), `cli`, `server` (currently empty), `desktop` (currently empty) to enable independent development of each target.

### Server

Expose program functionality via a REST API protected by a local API key / token (optional).

### Desktop

Provide a desktop app with the same capabilities as the CLI. macOS support is required; other platforms are a stretch goal.

### Coding Agent

Provide configurable mode, which will transofrm the general purpose agent into the coding agent. There should be additional skills/tools and prompts available for such mode, which won't be available in the "default" general purpose mode. Coding agent should work only in CLI app and be unavailable for server and desktop.
