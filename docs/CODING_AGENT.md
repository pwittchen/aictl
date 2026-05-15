# Coding-agent mode (experimental)

> **Experimental.** This mode is available and works on small-to-medium tasks, but it is not battle-tested for production coding workflows. The prompt and the five-phase loop are still being tuned, and the agent has no IDE integration and no semantic indexing. For production-grade coding work — large refactors, day-to-day feature work in mature codebases, anything where reliability matters — prefer dedicated coding agents: [Claude Code](https://docs.claude.com/en/docs/claude-code/overview), [OpenAI Codex CLI](https://github.com/openai/codex), or [opencode](https://github.com/sst/opencode). Use aictl's coding-agent mode for quick edits, exploration, and one-off scripts.

Coding-agent mode swaps the general-purpose system prompt for a coding-specialist prompt that follows a five-phase workflow: **Explore → Plan → Code → Review → Test**. Same tools, tighter discipline — read code before changing it, plan before editing, run tests after changes, prefer minimal diffs.

## Turning it on

The mode is **off by default**. Flip it on in any of these ways:

- **CLI flag (one launch):** `aictl --coding-agent` (or `--no-coding-agent` to force off for a launch when the config key is `true`).
- **CLI slash command:** `/coding on | off | toggle | status` in the REPL (or bare `/coding` to open an interactive menu) — persists to `~/.aictl/config` so the desktop picks it up on next launch.
- **Config key:** `AICTL_CODING_AGENT=true` in `~/.aictl/config`.
- **Desktop:** Settings → Coding Agent toggle, or click the chevrons-in-square icon in the composer toolbar (between the memory and auto-accept icons).

When coding mode is on, the CLI REPL prompt shows a dim `[phase]` prefix tracking the current workflow phase, and `--info` adds a `coding-agent: on` line. Phase transitions follow what the model says (a `<phase>NAME</phase>` tag at the start of its turn) and what it does (an `edit_file` / `write_file` tool call infers `Code`, a final answer past `Code` infers `Review`, past `Review` infers `Test`). Use `/skip` to advance the phase manually (`/skip` alone advances by one; `/skip review` or `/skip test` skips that phase).

The mode also enforces a **definition of done**: the agent must run the project's build, lint, and test commands after every change (and report pass/fail counts) before declaring success — if it can't run them itself, it tells the user the exact commands to run. Documentation is part of the same gate: when the change is user-visible and the project already has a `README.md` or other docs, the agent updates the affected sections; when the project has no `README.md` at all, the agent creates one with a project name, build/install instructions, and a minimal usage example.

## Sharper tool surface

The mode benefits from a sharper **tool surface** shared with the general agent (additive — old grammars keep working):

- **`edit_file`** accepts multiple `<<< … === … >>>` blocks per call (applied top-to-bottom and atomic — any block failure aborts the write, no partial state on disk), each optionally scoped by `@N` / `@N-M` to a 1-based inclusive line range; on a zero-hit exact match it retries with whitespace normalized per line and applies the change only if exactly one fuzzy candidate exists, otherwise it surfaces an "N candidates" error rather than guessing.
- **`search_files`** and **`find_files`** shell out to `rg` (ripgrep) when it's on `PATH` — `.gitignore` is respected by default, with flags `--regex` / `--literal`, `--case sensitive|smart|insensitive`, `--type rust|py|js|…`, `--context <N>`, `--max <N>`, `--no-ignore` on search and `--type <lang>` on find. When `rg` isn't available the tools fall back to the pure-Rust path so no install is required.
- **`read_file`** takes an optional second-line `--lines [N|N-M]` flag that returns the requested slice (or the whole file when bare `--lines` is passed) with `NNNNN: ` line-number prefixes — paired with `edit_file`'s `@N-M` directive so the model can pin a change to a specific line range without ambiguity.
- **`test`** runs the project's test command and returns a structured `Passed / Failed / Skipped` summary with per-failure detail. Empty body auto-detects the runner (cargo / npm / pytest / go / gradle / maven / ctest / make); a `<filter>` body narrows (`cargo test <f>`, `pytest -k <f>`, `npm test -- <f>`, `go test -run <f>`, `./gradlew test --tests <f>`); `--cmd <command>` overrides entirely. In coding-agent mode the host parses the runner output and, on failure, injects a `<test_failure>` user turn carrying the failing test names + messages + locations so the model can act without re-reading the prose tail — capped by `AICTL_CODING_TEST_RETRIES`.
- **Parallel read-only tool dispatch.** One model response can carry multiple `<tool>` blocks when every call is read-only (`read_file`, `list_directory`, `search_files`, `find_files`, `git status|log|blame|diff`, `lint_file`, `check_port`, `system_info`, `fetch_url`, `extract_website`, `read_document`, `read_image`, `json_query`, `csv_query`, `calculate`, `fetch_datetime`, `fetch_geolocation`, `clipboard read`, `diff_files`, `checksum`, `list_processes`); the host runs them concurrently via `tokio::task::JoinSet` and joins the per-tool results into one `<tool_results>` envelope in source order, so a five-file Explore burst takes one LLM round-trip instead of five. Capped by `AICTL_CODING_PARALLEL_TOOLS_MAX` (default 4, clamped to 16; `0` disables and forces serial); side-effect calls (`write_file`, `edit_file`, `exec_shell`, `test`, …) still run alone — if the model batches a side-effect with reads, the host runs only the side-effect and rejects the reads so the model re-emits them next turn. Surfaces on the `parallel:` line in `--info`.

## Repo context block

When coding mode is on, the agent's system prompt gains a **`<repo_context>` block** at the start of every session — current branch, last 5 commits, dirty files, top-level directory layout, and the resolved build / lint / test commands. The model can skip the discovery round of `git status` / `git log` / `ls` calls it would otherwise make. The block is cached per working directory and busted automatically after every `write_file` / `edit_file` / `remove_file` / `create_directory`; `/coding refresh` forces a re-read mid-session.

## Review hook

Coding mode also enforces a **structured Review hook** before any final answer is released. When the model emits a no-tool-call response and the session has actually edited files, the host runs the project's build command and `lint_file` on each changed file. On clean output the answer is released with a `[review: clean — build + lint passed]` banner prepended; on failure the host pushes a `<review_result>` user turn carrying the build / lint output tails and the loop continues — the model gets another shot. The retry budget is `AICTL_CODING_REVIEW_RETRIES` (default 2) after which the answer is released with a `[review: N attempt(s); failures may remain]` banner.

## Tuning knobs

Optional tuning knobs in `~/.aictl/config`:

- `AICTL_CODING_PLAN_APPROVE=true` — pause for confirmation before the Code phase.
- `AICTL_CODING_SKIP_REVIEW=true` / `AICTL_CODING_SKIP_TEST=true` — bypass those phases.
- `AICTL_CODING_TEST_RETRIES=3` — bound the Code → Test re-loop on `test`-tool failures.
- `AICTL_CODING_REVIEW_RETRIES=2` — bound the Code → Review re-loop on Review-hook failures.
- `AICTL_CODING_LINTER` / `AICTL_CODING_TEST_CMD` / `AICTL_CODING_BUILD_CMD` — override the auto-detected linter / test / build commands. Auto-detection covers Cargo / npm / Python / Go / Gradle / Maven / CMake / Make from project markers in the working directory; wrapper scripts (`gradlew`, `mvnw`) are preferred over system `gradle` / `mvn`, and `clang-tidy` is preferred over `cppcheck` when `compile_commands.json` is present.
- `AICTL_CODING_REPO_CONTEXT=false` — suppress the `<repo_context>` block entirely (useful on very large repos where the directory tree adds prompt bloat); `AICTL_CODING_REPO_CONTEXT_TREE_DEPTH` / `_TREE_MAX` tune the tree depth and entry cap.

Coding-agent mode composes with the rest of the prompt extension points: a loaded agent (`--agent rust-expert`) still appends its persona block on top of the coding-specialist base, a `/<skill>` invocation still injects its body for one turn, and `AICTL.md` / `~/.aictl/AICTL.md` still apply. The server (`aictl-server`) is unaffected — it has no agent loop, so `AICTL_CODING_AGENT` is ignored there even when set in a shared config.
