# Project Stats Report -- 2026-05-15 23:41:02

## Overview

- **Name:** aictl (Cargo workspace)
- **Version:** 0.46.0 (workspace-shared)
- **Edition:** 2024
- **Current branch:** master
- **Latest commit:** `e3f1314 updated roadmap (2026-05-15T23:39:51+02:00, Piotr Wittchen)`
- **Workspace members:** `aictl-cli`, `aictl-core`, `aictl-server`, `aictl-desktop`
- **Default members (built by bare `cargo build`):** `aictl-cli`, `aictl-core`, `aictl-server` (desktop excluded — Tauri toolchain)

## Commit Activity

- **Total commits:** 1196
- **First commit:** 2026-03-20T19:50:28+01:00
- **Latest commit:** 2026-05-15T23:39:51+02:00
- **Elapsed days:** ~56 (calendar days between first and latest)
- **Active development days:** 49 (unique commit dates)
- **Commits in last 7 days:** 165
- **Commits in last 30 days:** 669
- **Branches (local + remote refs):** 3
- **Tags:** 120

### Commits per month (last 12)

| Month    | Commits |
|----------|---------|
| 2026-03  | 176     |
| 2026-04  | 653     |
| 2026-05  | 367     |

## Contributors

| Contributor                                              | Commits |
|----------------------------------------------------------|---------|
| Piotr Wittchen <piotr@wittchen.io>                       | 1081    |
| github-actions[bot] <github-actions[bot]@users.noreply…> | 117     |

## Lines of Code

Tool used: **cloc 2.08** (excludes `target`, `node_modules`, `.git`, `dist`, `build`).

| Language     | Files | Code   | Comments | Blanks |
|--------------|-------|--------|----------|--------|
| Rust         | 179   | 54,189 | 8,301    | 5,708  |
| TypeScript   | 31    | 15,132 | 1,343    | 827    |
| Markdown     | 73    | 9,702  | 0        | 3,135  |
| CSS          | 3     | 7,631  | 477      | 1,211  |
| JSON         | 14    | 6,076  | 0        | 13     |
| HTML         | 5     | 4,621  | 0        | 268    |
| YAML         | 3     | 512    | 118      | 54     |
| Bourne Shell | 4     | 445    | 39       | 68     |
| JavaScript   | 1     | 367    | 37       | 55     |
| TOML         | 7     | 183    | 62       | 31     |
| Dockerfile   | 2     | 75     | 72       | 28     |
| Python       | 1     | 56     | 22       | 13     |
| Text         | 2     | 42     | 0        | 14     |
| XML          | 2     | 22     | 5        | 0      |
| Make         | 1     | 9      | 0        | 4      |
| **Total**    | 328   | 99,062 | 10,476   | 11,429 |

Total counted lines (code + comments + blanks): **120,967**.

## Project Structure

- **Rust source files (across all crates):** 179
- **`crates/**/src/*.rs` files:** 176
- **Workspace crates:** 4 (cli, core, server, desktop)
- **Test functions (`#[test]` / `#[tokio::test]`):** 1,091
- **TODO / FIXME / HACK / XXX markers (Rust/TS/TSX in `crates/`):** 36

### Dependency counts (direct, per crate)

| Crate         | Runtime deps | Dev deps | Optional / feature-gated                                                                                       |
|---------------|--------------|----------|----------------------------------------------------------------------------------------------------------------|
| aictl-core    | 21 + 6 optional | 1     | `gguf`, `mlx`, `redaction-ner` (gates `llama-cpp-2`, `mlx-rs`, `tokenizers`, `minijinja`, `safetensors`, `gline-rs`, `orp`) |
| aictl-cli     | 12           | 1        | `gguf`, `mlx`, `redaction-ner` (passthrough to core)                                                            |
| aictl-server  | 17           | 1        | `gguf`, `mlx`, `redaction-ner` (passthrough)                                                                    |
| aictl-desktop | 17 + 1 optional + 1 build-dep | 0 | `voice` (gates `whisper-rs`); plus `gguf` / `mlx` / `redaction-ner` passthrough                          |

### Top 5 largest Rust source files

| Lines | File                                                |
|-------|-----------------------------------------------------|
| 2,401 | `crates/aictl-core/src/security/redaction.rs`       |
| 2,227 | `crates/aictl-core/src/security.rs`                 |
| 2,156 | `crates/aictl-core/src/run.rs`                      |
| 1,426 | `crates/aictl-core/src/coding.rs`                   |
| 1,364 | `crates/aictl-cli/src/repl.rs`                      |

## Repository Size

- **Working tree (excluding `target/`, `.git/`, `node_modules/`):** ~12 MB
  - `crates/` (excluding `node_modules` and `dist`): ~5 MB
  - `crates/aictl-desktop/webview/dist`: 6.8 MB (built frontend bundle, checked into worktree)
  - `website/`: 1.8 MB
  - `.claude/`: 720 KB
  - `docs/`: 264 KB
  - `Cargo.lock`: 204 KB
  - other top-level dirs and files: ~200 KB total
- **`.git/`:** 13 MB
- **Build artifacts (not part of the worktree):**
  - `crates/aictl-desktop/webview/node_modules`: 93 MB
  - `target/`: 23 GB

## Notes

- All non-bot commits are from a single contributor; the bot accounts for 117 of 1,196 commits (~9.8%, mostly CI / release tagging given the 120 tags).
- Throughput averages 1,196 / 49 ≈ **24.4 commits per active day** over the project's lifetime; the last 7 days alone added 165 commits (~23.6/day).
- 120 tags vs 1,196 commits → roughly one tagged release every ~10 commits; consistent with an automated release workflow.
- Rust dominates the codebase (54,189 LOC, 55%); TypeScript (15,132 LOC) plus HTML (4,621) and CSS (7,631) reflect the desktop webview and the marketing website under `website/`.
- Three files in `aictl-core` cross the 2,000-line threshold (`redaction.rs`, `security.rs`, `run.rs`) — each is a documented hot path in CLAUDE.md.
