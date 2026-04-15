# Project Stats Report -- 2026-04-15 17:37:09

## Overview

- Name: `aictl`
- Version: 0.23.2
- Edition: 2024
- Current branch: `master`
- Latest commit: `86dc1b3 Add /project-stats-report skill` (2026-04-15T17:36:42+02:00, Piotr Wittchen)

## Commit Activity

- Total commits: 514
- First commit: 2026-03-20T19:50:28+01:00
- Latest commit: 2026-04-15T17:36:42+02:00
- Elapsed days: 26
- Active development days (unique commit days): 20
- Commits in last 7 days: 172
- Commits in last 30 days: 514
- Branches (local + remote): 3
- Tags: 53

### Commits per month (last 12 months)

| Month    | Commits |
|----------|---------|
| 2026-03  | 176     |
| 2026-04  | 338     |

## Contributors

| Contributor | Commits |
|-------------|---------|
| Piotr Wittchen <piotr@wittchen.io> | 462 |
| github-actions[bot] <github-actions[bot]@users.noreply.github.com> | 52 |

## Lines of Code

Tool used: `cloc` 2.08.

| Language      | Files | Code   | Comments | Blank |
|---------------|-------|--------|----------|-------|
| Rust          | 23    | 12,817 | 913      | 1,378 |
| Markdown      | 21    | 2,287  | 0        | 722   |
| Bourne Shell  | 1     | 186    | 8        | 27    |
| YAML          | 2     | 141    | 0        | 23    |
| JSON          | 1     | 40     | 0        | 0     |
| TOML          | 2     | 37     | 9        | 2     |
| **Total**     | **50**| **15,508** | **930** | **2,152** |

## Project Structure

- Rust source files in `src/`: 22
- Total Rust LOC (incl. blanks/comments): 15,089
- Dependencies (runtime, from `Cargo.toml`): 19 (5 optional)
- Dev dependencies: 0
- Declared features: `default`, `gguf`, `mlx`
- Test functions (`#[test]` / `#[tokio::test]`): 189 across 9 files
- TODO/FIXME/HACK/XXX markers in `src/`: 0

### Top 5 largest source files

| File | Lines |
|------|-------|
| `src/commands.rs` | 4,193 |
| `src/tools.rs` | 2,006 |
| `src/main.rs` | 1,466 |
| `src/llm_mlx.rs` | 1,403 |
| `src/security.rs` | 1,325 |

## Repository Size

- Working tree (excluding `target/` and `.git/`): 1.1 MB
- `.git` directory: 16 MB

## Notes

- Project history is young (26 elapsed days) but very active: 514 commits with 20 distinct commit days and 53 tags, indicating a high release cadence.
- Two files (`commands.rs`, `tools.rs`) account for ~41% of Rust LOC; both are natural concentration points (REPL command dispatch and tool execution).
- No TODO/FIXME/HACK/XXX markers in `src/` — codebase is free of in-line deferral notes.
- Test surface is substantial (189 test functions) relative to 22 source modules.
