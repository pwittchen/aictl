# Roadmap

## Docs

- Simplify README.md docs and move large docs into the separate `*.md` file

## Coding-agent mode

Phase 1 (master switch, `SYSTEM_PROMPT_CODING`, `WorkflowPhase`, CLI surface, desktop toggle) has shipped. Follow-up phases:

- **Phase 2 — better edit and search**: smarter `edit_file` (multi-edit, line-number addressing, fuzzy match), ripgrep-backed `search_files` / `find_files`, read-file-with-line-numbers and selective reading.
- **Phase 3 — test loop and self-review polish**: dedicated `test` tool with structured output parsing, automatic context injection (git branch, recent commits, dir tree) at session start, structured pre-final-answer Review hook to replace the v1 prompt-driven Review.
- **Phase 4 — parallel tool execution and streaming**: parallel tool execution via `tokio::JoinSet`, streaming refinements specific to coding mode (real-time `[phase]` updates).

Cross-cutting follow-ups for `coding::detect_linter` / `coding::detect_test_cmd` — today these projects fall through to the no-detection branch, so the Review and Test phases skip silently:

- **Java / Kotlin**: Gradle as the primary build system (`build.gradle`, `build.gradle.kts`, `settings.gradle{,.kts}`, `gradlew` wrapper → `./gradlew check` / `./gradlew test`) and Maven as the secondary (`pom.xml` → `mvn verify` / `mvn test`).
- **C**: CMake as the primary (`CMakeLists.txt` → `cmake --build build` plus `ctest --test-dir build`) and plain Make as the secondary (`Makefile` → `make` / `make test` or `make check` when the target exists); prefer `clang-tidy` for the linter when a `compile_commands.json` is present, falling back to `cppcheck`.
