# Roadmap

## Docs

- Simplify README.md docs and move large docs into the separate `*.md` file

## Coding-agent mode

Phase 1 (master switch, `SYSTEM_PROMPT_CODING`, `WorkflowPhase`, CLI surface, desktop toggle), Phase 2 (smarter `edit_file` with multi-block / line-scope / fuzzy fallback, ripgrep-backed `search_files` / `find_files`, opt-in `--lines` slice and numbering on `read_file`), Phase 3 (dedicated `test` tool with structured parsing + host-driven retry loop, `<repo_context>` block injected at session start, structured pre-final-answer Review hook that runs the project build + `lint_file` on changed paths and re-loops the model on failure), and Phase 4 (parallel read-only tool dispatch via `tokio::JoinSet` capped at `AICTL_CODING_PARALLEL_TOOLS_MAX`, mid-stream `<phase>` tag updates that flip the REPL `[phase]` prefix as soon as the tag streams in) have shipped.
