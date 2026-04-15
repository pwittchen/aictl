# Issues

## Infrastructure

- **Project website** `[marketing]` — publish website from the `website/` dir

## Roadmap

### Modular architecture

Split the codebase into separate modules: `core` (shared logic), `cli`, `desktop` (currently empty) to enable independent development of each target.

### Desktop

Provide a desktop app with the same capabilities as the CLI. macOS support is required; other platforms are a stretch goal.

### Coding Agent

Provide configurable mode, which will transofrm the general purpose agent into the coding agent. There should be additional skills/tools and prompts available for such mode, which won't be available in the "default" general purpose mode. Coding agent should work only in CLI app and be unavailable for server and desktop.
