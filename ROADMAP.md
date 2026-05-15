# Roadmap

## Docs

- Simplify README.md docs and move large docs into the separate `*.md` file

## Coding-agent mode

Phase 1 (master switch, `SYSTEM_PROMPT_CODING`, `WorkflowPhase`, CLI surface, desktop toggle) and Phase 2 (smarter `edit_file` with multi-block / line-scope / fuzzy fallback, ripgrep-backed `search_files` / `find_files`, opt-in `--lines` slice and numbering on `read_file`) have shipped. Follow-up phases:

- **Phase 3 — test loop and self-review polish**: dedicated `test` tool with structured output parsing, automatic context injection (git branch, recent commits, dir tree) at session start, structured pre-final-answer Review hook to replace the v1 prompt-driven Review.
- **Phase 4 — parallel tool execution and streaming**: parallel tool execution via `tokio::JoinSet`, streaming refinements specific to coding mode (real-time `[phase]` updates).
