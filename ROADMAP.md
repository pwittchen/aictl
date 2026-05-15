# Roadmap

## Docs

- Simplify README.md docs and move large docs into the separate `*.md` file

## Coding-agent mode

Phase 1 (master switch, `SYSTEM_PROMPT_CODING`, `WorkflowPhase`, CLI surface, desktop toggle), Phase 2 (smarter `edit_file` with multi-block / line-scope / fuzzy fallback, ripgrep-backed `search_files` / `find_files`, opt-in `--lines` slice and numbering on `read_file`), and Phase 3 (dedicated `test` tool with structured parsing + host-driven retry loop, `<repo_context>` block injected at session start, structured pre-final-answer Review hook that runs the project build + `lint_file` on changed paths and re-loops the model on failure) have shipped. Follow-up phases:

- **Phase 4 — parallel tool execution and streaming**: parallel tool execution via `tokio::JoinSet`, streaming refinements specific to coding mode (real-time `[phase]` updates).
