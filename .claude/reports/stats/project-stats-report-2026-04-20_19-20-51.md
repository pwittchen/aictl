# Project Stats Report -- 2026-04-20 19:20:51

## Overview

- Name: `aictl`
- Version: `0.27.1`
- Edition: `2024`
- Description: AI agent for the terminal
- Current branch: `master`
- Latest commit: `e381175` — "Add short promo text for X" (2026-04-20T19:08:36+02:00, Piotr Wittchen)

## Commit Activity

- Total commits: 702
- First commit: 2026-03-20
- Latest commit: 2026-04-20
- Elapsed calendar days: 31
- Active development days (unique commit days): 25
- Commits in last 7 days: 228
- Commits in last 30 days: 698
- Branches: 3
- Tags: 71

Commits per month (observed):

| Month    | Commits |
| -------- | ------- |
| 2026-03  | 176     |
| 2026-04  | 526     |

## Contributors

| Contributor                                                             | Commits |
| ----------------------------------------------------------------------- | ------- |
| Piotr Wittchen <piotr@wittchen.io>                                      | 632     |
| github-actions[bot] <github-actions[bot]@users.noreply.github.com>      | 70      |

## Lines of Code

Tool used: `cloc` 2.08 (tokei unavailable). Excluded: `target/`, `node_modules/`, `.git/`.

| Language      | Files | Code   | Comments | Blanks |
| ------------- | ----- | ------ | -------- | ------ |
| Rust          | 82    | 25,494 | 2,795    | 2,859  |
| Markdown      | 35    | 3,772  | 0        | 1,286  |
| HTML          | 4     | 1,351  | 0        | 79     |
| CSS           | 2     | 714    | 18       | 121    |
| Bourne Shell  | 1     | 213    | 10       | 29     |
| YAML          | 3     | 211    | 4        | 31     |
| JSON          | 3     | 89     | 0        | 0      |
| JavaScript    | 2     | 77     | 4        | 5      |
| TypeScript    | 2     | 77     | 6        | 12     |
| TOML          | 2     | 46     | 23       | 2      |
| **Total**     | 136   | 32,044 | 2,860    | 4,424  |

## Project Structure

- Rust source files: 82
- Runtime dependencies declared in `Cargo.toml`: 30 (9 optional, behind `gguf` / `mlx` / `redaction-ner` features)
- Dev dependencies: 0 (no `[dev-dependencies]` section — tests live inline under `#[cfg(test)]`)
- Feature flags: `gguf`, `mlx`, `redaction-ner` (all default off)
- Test functions (`#[test]` + `#[tokio::test]`): 635 across 38 files
- TODO/FIXME/HACK/XXX markers in `src/`: 0

Top 5 largest Rust source files (by line count):

| File                              | Lines |
| --------------------------------- | ----- |
| `src/security.rs`                 | 1,630 |
| `src/security/redaction.rs`       | 1,513 |
| `src/tools/csv_query.rs`          | 1,095 |
| `src/ui.rs`                       | 1,021 |
| `src/run.rs`                      | 908   |

## Repository Size

- Working tree (excluding `target/` and `.git/`): 2.2 MB
- `.git` directory: 25 MB
- `target/` (build artifacts, not counted in working tree): 25 GB

## Notes

- 702 commits compressed into 25 active development days (31 calendar days) indicates a concentrated burst of work since the repo's first commit on 2026-03-20.
- Bot author (`github-actions[bot]`) contributes 70 commits (~10%), consistent with automated release / version-bump workflows given 71 tags.
- Zero TODO/FIXME/HACK/XXX markers in `src/` — noteworthy for a codebase of ~25.5k Rust LOC.
- Ratio of test functions to source files (635 / 82 ≈ 7.7 tests per file) suggests tests are colocated with implementation modules rather than a separate `tests/` tree.
