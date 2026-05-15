# Plan: Coding Agent — Java/Kotlin and C linter/test/build detection

## Context

Phase 1 ([`coding-agent.md`](coding-agent.md)) shipped `coding::detect_linter` and `coding::detect_test_cmd` in [`crates/aictl-core/src/coding.rs`](../../crates/aictl-core/src/coding.rs). Today they cover Rust, Node, Python, and Go — the four ecosystems most aictl contributors actively use. Two ecosystems still fall through to the no-detection branch:

- **Java / Kotlin**: a Gradle or Maven project at the working-directory root has no markers `coding::detect_*` recognizes. The Review phase emits `lint_file` calls (which work fine — `lint_file` is per-file and doesn't need project-level detection) but the Test phase logs "no test command detected" and skips. The Phase-3 Review hook (when [`coding-agent-phase-3.md`](coding-agent-phase-3.md) ships) will also skip the project build for the same reason.
- **C** (and C++ to the extent it shares build systems): same story. A `CMakeLists.txt` or a hand-rolled `Makefile` produces no detection, so the Test phase silently does nothing useful.

This plan plugs the gap. Scope is narrow on purpose — it's not a project-language survey, it's adding two new detection branches (and one for C) to functions that already exist, plus the matching `detect_build_cmd` from Phase 3 once that lands. The change is local to `coding.rs`, the existing `tools/lint.rs` extension list, and a small prompt sweep.

## Goals & Non-goals

**Goals**

- Extend `coding::detect_linter`, `coding::detect_test_cmd`, and (once [`coding-agent-phase-3.md`](coding-agent-phase-3.md) ships) `coding::detect_build_cmd` to recognize Gradle and Maven projects and emit appropriate commands.
- Same for C projects with CMake or Make.
- Prefer the wrapper script (`gradlew`, `mvnw`) when present so the model uses the project-pinned tool version instead of whatever happens to be on `PATH`.
- For C, prefer `clang-tidy` when `compile_commands.json` is present (the database carries the per-file flags the linter needs); fall back to `cppcheck` otherwise.
- Update the `lint_file` per-file linter registry ([`tools/lint.rs:45`](../../crates/aictl-core/src/tools/lint.rs)) to add Java / Kotlin entries (the existing C/C++ block stays; we revisit only to add `clang-tidy` when a compile DB is present).
- Update the coding-mode and base system prompts to mention the new detected languages so the model knows Gradle / Maven / CMake commands are first-class citizens.

**Non-goals**

- No build-system *selection* UI. If a repo has both Gradle and a `pom.xml` (e.g. a polyglot monorepo), the precedence below picks one deterministically. The user overrides with `AICTL_CODING_TEST_CMD` if the default is wrong.
- No multi-module orchestration. Gradle's `:moduleA:test` shape is documented in the prompt, but the *default* command is the root task (`./gradlew test`); the user / model can narrow it.
- No coverage, no benchmarks, no static-analysis tools beyond the linter list (no SpotBugs, no PMD, no ErrorProne plugins).
- No build-system installation. The detector returns a *command*; if `gradle` or `cmake` isn't installed the command fails at exec time with a clear error — same shape as today's `cargo`-less environment.
- No C++-specific paths beyond what the existing `lint.rs` group already covers (`.cpp`, `.cc`, …). The CMake branch covers both C and C++ projects.
- No server changes — coding-mode detection is host-side.
- No new tools. The work is purely augmenting the three `coding::detect_*` functions plus a small `lint.rs` patch.

## Design

### 1. Detection precedence

Precedence is "primary then secondary" for each ecosystem so a repo with both Gradle and Maven (rare but real — migration in flight) still gets a deterministic command. Order overall, top-to-bottom inside each `detect_*` function:

1. **User override**: `AICTL_CODING_LINTER` / `AICTL_CODING_TEST_CMD` / `AICTL_CODING_BUILD_CMD` (unchanged from today).
2. **Rust** (`Cargo.toml`) — unchanged.
3. **Node** (`package.json`) — unchanged.
4. **Python** (`pyproject.toml` / `requirements.txt` / `pytest.ini`) — unchanged.
5. **Go** (`go.mod`) — unchanged.
6. **Gradle** (`build.gradle` or `build.gradle.kts` or `settings.gradle` or `settings.gradle.kts`) — new.
7. **Maven** (`pom.xml`) — new.
8. **CMake** (`CMakeLists.txt`) — new.
9. **Make** (`Makefile`) — new.

The order matches "what aictl contributors most likely work on" first, then the JVM cluster, then the C cluster. Putting Gradle before Maven mirrors industry convention; putting CMake before Make mirrors that modern C/C++ projects standardize on CMake and Make-only setups tend to be older or smaller.

### 2. Gradle detection

```rust
fn detect_gradle(working_dir: &Path) -> Option<&'static str> {
    let has_gradle = working_dir.join("build.gradle").is_file()
        || working_dir.join("build.gradle.kts").is_file()
        || working_dir.join("settings.gradle").is_file()
        || working_dir.join("settings.gradle.kts").is_file();
    if !has_gradle { return None; }
    // Wrapper present? Use it — pinned project version is the contract
    // the project author signed up for. Falls back to `gradle` only when
    // the wrapper isn't there.
    let wrapper = working_dir.join("gradlew").is_file();
    Some(if wrapper { "wrapper" } else { "system" })
}

// Linter:   ./gradlew check    or    gradle check
// Test:     ./gradlew test     or    gradle test
// Build:    ./gradlew build    or    gradle build  (also runs check + test;
//                                                   for v1 we keep them
//                                                   separate so phases stay
//                                                   distinct)
```

Why `check` for lint, not `lint`? Gradle's `check` is the standard verification umbrella — it runs whatever is configured: Checkstyle, ktlint, Detekt, SpotBugs, JaCoCo coverage thresholds, etc. We don't need to know which the project uses; the umbrella is the contract.

On Windows, `gradlew` is `gradlew.bat`. The detector checks both; the returned command uses `./gradlew` either way and lets the shell resolve. (We're not targeting Windows in v1 — see [`project_desktop_macos_only.md`](../../.claude/.. /memory/project_desktop_macos_only.md) in the user's memory, the desktop is macOS-only initially; the CLI is broader. Document the bat shape in the prompt so a future Windows user can wire it manually.)

### 3. Maven detection

```rust
fn detect_maven(working_dir: &Path) -> Option<&'static str> {
    if !working_dir.join("pom.xml").is_file() { return None; }
    let wrapper = working_dir.join("mvnw").is_file();
    Some(if wrapper { "wrapper" } else { "system" })
}

// Linter:   ./mvnw verify  -or-  mvn verify       (verify runs Surefire + Failsafe + any
//                                                  plugin-bound checks)
// Test:     ./mvnw test    -or-  mvn test
// Build:    ./mvnw package -or-  mvn package      (compile + test + jar)
```

`verify` is the right linter umbrella for the same reason `check` is right for Gradle — it runs the configured plugins (Checkstyle Maven plugin, SpotBugs Maven plugin, etc.). Some projects only attach analysis to `verify`, not to `compile`, so picking `verify` covers the common case.

### 4. CMake detection

```rust
fn detect_cmake(working_dir: &Path) -> Option<CMakeShape> {
    if !working_dir.join("CMakeLists.txt").is_file() { return None; }
    // Out-of-source build directory: prefer `build/`, fall back to
    // `cmake-build-debug/` (CLion default) or `out/build/`. If none
    // exists, the build command will *create* `build/` on first run.
    let build_dir = ["build", "cmake-build-debug", "out/build"]
        .into_iter()
        .map(|d| working_dir.join(d))
        .find(|p| p.is_dir());
    let has_compile_db = build_dir
        .as_ref()
        .map(|d| d.join("compile_commands.json").is_file())
        .unwrap_or(false);
    Some(CMakeShape { build_dir, has_compile_db })
}

// Linter:   clang-tidy --quiet -p build           (when compile_commands.json present)
//           cppcheck --enable=warning --quiet .   (otherwise)
// Test:     ctest --test-dir build --output-on-failure
//           — falls back to `make -C build test` if `ctest` isn't on PATH.
// Build:    cmake --build build                   (cmake handles "no build dir yet" by
//                                                  configuring first when paired with
//                                                  `cmake -S . -B build`; v1 emits the
//                                                  two-step command:
//                                                  "cmake -S . -B build && cmake --build build")
```

The `compile_commands.json` check determines which linter the *project-level* `detect_linter` returns. The per-file `lint_file` tool already prefers `clang-format --dry-run` and `cppcheck` ([`tools/lint.rs:215`](../../crates/aictl-core/src/tools/lint.rs)); we leave that registry alone — `lint_file` is per-file and doesn't need the compile DB to spot-check single files. The project-level `detect_linter` is what reaches for `clang-tidy` because that's where the DB pays off.

### 5. Make detection

```rust
fn detect_make(working_dir: &Path) -> Option<MakeShape> {
    if !working_dir.join("Makefile").is_file() { return None; }
    let body = std::fs::read_to_string(working_dir.join("Makefile")).ok().unwrap_or_default();
    let has_test = makefile_has_target(&body, "test");
    let has_check = makefile_has_target(&body, "check");
    Some(MakeShape { has_test, has_check })
}

// Linter:   make check       (when `check` target exists)
//           (none)            (otherwise — Make has no umbrella; lint_file per file still works)
// Test:     make test         (when `test` target exists)
//           make check        (when only `check` exists)
//           (none)            (otherwise)
// Build:    make              (the default target; standard convention)
```

`makefile_has_target` is a tiny regex-free string scan: look for a line that starts with `<target>:` (allowing the optional `.PHONY:` decoration). Cheap-and-correct for the common shapes; rare false positives (e.g. a comment containing `test:`) only cause us to try the target, which fails with a clear error.

### 6. Combined `detect_*` shape

`detect_linter` and `detect_test_cmd` already exist in [`crates/aictl-core/src/coding.rs`](../../crates/aictl-core/src/coding.rs) and follow an early-return-on-match pattern. We extend each with the new branches in the precedence order from §1:

```rust
pub fn detect_linter(working_dir: &Path) -> Option<String> {
    if let Some(cmd) = crate::config::coding_linter_override() { return Some(cmd); }
    // (existing Rust/Node/Python/Go branches unchanged)
    if let Some(shape) = detect_gradle(working_dir) {
        return Some(if shape == "wrapper" { "./gradlew check".into() } else { "gradle check".into() });
    }
    if let Some(shape) = detect_maven(working_dir) {
        return Some(if shape == "wrapper" { "./mvnw verify".into() } else { "mvn verify".into() });
    }
    if let Some(c) = detect_cmake(working_dir) {
        return Some(if c.has_compile_db {
            "clang-tidy --quiet -p build".into()
        } else {
            "cppcheck --enable=warning --quiet .".into()
        });
    }
    if let Some(m) = detect_make(working_dir) {
        if m.has_check { return Some("make check".into()); }
    }
    None
}

pub fn detect_test_cmd(working_dir: &Path) -> Option<String> {
    if let Some(cmd) = crate::config::coding_test_cmd_override() { return Some(cmd); }
    // (existing Rust/Node/Python/Go branches unchanged)
    if let Some(shape) = detect_gradle(working_dir) {
        return Some(if shape == "wrapper" { "./gradlew test".into() } else { "gradle test".into() });
    }
    if let Some(shape) = detect_maven(working_dir) {
        return Some(if shape == "wrapper" { "./mvnw test".into() } else { "mvn test".into() });
    }
    if detect_cmake(working_dir).is_some() {
        return Some("ctest --test-dir build --output-on-failure".into());
    }
    if let Some(m) = detect_make(working_dir) {
        if m.has_test { return Some("make test".into()); }
        if m.has_check { return Some("make check".into()); }
    }
    None
}
```

`detect_build_cmd` (introduced by [`coding-agent-phase-3.md`](coding-agent-phase-3.md)) gets the same treatment — the patch is part of *this* plan if Phase 3 has already shipped, otherwise it ships as part of Phase 3 with the JVM/C branches already included.

### 7. `lint_file` per-file additions

The per-file `lint_file` tool's `LINTERS` table ([`tools/lint.rs:45`](../../crates/aictl-core/src/tools/lint.rs)) currently has no Java or Kotlin entries. Add:

```rust
LinterGroup {
    extensions: &["java"],
    candidates: &[
        LinterCmd { binary: "google-java-format", args: &["--dry-run", "--set-exit-if-changed"], label: "google-java-format --dry-run" },
        LinterCmd { binary: "checkstyle", args: &["-c", "/google_checks.xml"], label: "checkstyle (google)" },
        // `javac -Xlint` as a last resort — needs the file to compile in
        // isolation, which rarely works without a classpath, so it's last
        // and likely to fail noisily. Document this in the prompt.
        LinterCmd { binary: "javac", args: &["-Xlint", "-d", "/tmp"], label: "javac -Xlint" },
    ],
},
LinterGroup {
    extensions: &["kt", "kts"],
    candidates: &[
        LinterCmd { binary: "ktlint", args: &[], label: "ktlint" },
        LinterCmd { binary: "ktfmt", args: &["--dry-run"], label: "ktfmt --dry-run" },
    ],
},
```

The existing `c / cpp / cc / cxx / h / hpp / hh / hxx` group ([`lint.rs:215`](../../crates/aictl-core/src/tools/lint.rs)) already covers C/C++. We *don't* add `clang-tidy` to that group because clang-tidy without a compile database has very limited utility on a per-file basis — it tends to spew "use of undeclared identifier" errors when called on a file with no `-I` flags. Project-level `detect_linter` already picks `clang-tidy` when the database is present; the per-file tool sticks with `clang-format` + `cppcheck`.

### 8. Prompt updates

`SYSTEM_PROMPT_CODING` ([`config.rs`](../../crates/aictl-core/src/config.rs)) — the Test- and Review-phase guidance gets a one-line addition listing the new commands:

```
Test phase: Run the project's test command via exec_shell. Examples:
`cargo test`, `npm test`, `pytest`, `go test ./...`, `./gradlew test`,
`./mvnw test`, `ctest --test-dir build --output-on-failure`, `make test`.
```

Same for Review-phase build/lint examples. The base `SYSTEM_PROMPT` gets the equivalent shorter sentence in the tool catalogue's `lint_file` blurb (since the *per-file* linter is the user-visible tool there; the project-level commands are an exec_shell concern).

`README.md` — under "Coding-agent mode", a one-liner: "Auto-detects Cargo / npm / Python / Go / Gradle / Maven / CMake / Make for build, lint, and test commands."

`CLAUDE.md` — update the existing coding-agent paragraph: "Auto-detection in `coding::detect_linter` / `detect_test_cmd` / `detect_build_cmd` covers Rust, Node, Python, Go, Gradle, Maven, CMake, and Make; project-level commands prefer wrappers (`gradlew`, `mvnw`) when present and prefer `clang-tidy` for C/C++ when `compile_commands.json` exists."

### 9. Runtime shape and integration points

| File | Change |
|------|--------|
| `crates/aictl-core/src/coding.rs` | Add `detect_gradle` / `detect_maven` / `detect_cmake` / `detect_make` private helpers; extend `detect_linter` and `detect_test_cmd` to call them; extend `detect_build_cmd` similarly (depends on whether [`coding-agent-phase-3.md`](coding-agent-phase-3.md) has landed) |
| `crates/aictl-core/src/tools/lint.rs` | Add `java` and `kt`/`kts` linter groups; leave the C/C++ group unchanged |
| `crates/aictl-core/src/config.rs` | Revise the Test- and Review-phase paragraphs in `SYSTEM_PROMPT_CODING`; revise the `lint_file` blurb in `SYSTEM_PROMPT` |
| `README.md` | One-line "Auto-detects …" sweep |
| `CLAUDE.md` | One-line update to the coding-agent paragraph |
| `ROADMAP.md` | Remove the JVM/C bullet once shipped |

### 10. Testing

**Unit tests** in `coding.rs`'s `#[cfg(test)]` mod:

- `detect_linter` with a fixture working dir containing:
  - `build.gradle` only → `"gradle check"`.
  - `build.gradle` + `gradlew` → `"./gradlew check"`.
  - `build.gradle.kts` + `gradlew` → `"./gradlew check"` (Kotlin DSL still hits Gradle).
  - `pom.xml` only → `"mvn verify"`.
  - `pom.xml` + `mvnw` → `"./mvnw verify"`.
  - `CMakeLists.txt` with `build/compile_commands.json` → `"clang-tidy --quiet -p build"`.
  - `CMakeLists.txt` without compile DB → `"cppcheck --enable=warning --quiet ."`.
  - `Makefile` with `check:` target → `"make check"`.
  - `Makefile` with no relevant targets → `None`.
  - User override (`AICTL_CODING_LINTER=foo`) wins over every detection.
- `detect_test_cmd` mirror table.
- `detect_build_cmd` mirror table (when Phase 3 has landed).
- Precedence: a synthetic dir with `Cargo.toml` + `build.gradle` returns the Cargo command (Rust wins).
- Precedence within JVM: `build.gradle` + `pom.xml` returns Gradle (matches industry default for migration repos).
- `makefile_has_target` table: target present, target with `.PHONY:` decoration, target in a comment (false positive accepted), target named `test-foo` (no false match).

**Manual smoke**

1. In a real Gradle project (e.g. `spring-petclinic`), `aictl --coding-agent` shows `coding-test: ./gradlew test` in `--info`. Ask the agent "run the tests". Verify the dispatch goes through the wrapper and the model surfaces pass/fail.
2. In a Maven project, repeat. Verify `./mvnw verify` is picked when both `mvnw` and `pom.xml` are present.
3. In a CMake project with a configured `build/` containing `compile_commands.json`, `--info` shows `coding-lint: clang-tidy --quiet -p build`. Remove the database, restart, verify it falls back to `cppcheck`.
4. In a Make-only project (e.g. classic `coreutils`), verify `--info` shows `coding-test: make test` when the Makefile has that target.
5. Open a `Foo.java` file and run `/<skill> lint_file Foo.java` (or have the agent issue the tool call) — verify the per-file linter picks `google-java-format --dry-run` first if installed, falls through to `checkstyle`, then to `javac -Xlint`.

**CI gates**

```bash
# JVM/C symbols stay inside aictl-core::coding (not leaking into the server).
grep -rE 'detect_gradle|detect_maven|detect_cmake|detect_make' crates/aictl-server/src/   # must be empty
```

No additional CI gate beyond the unit tests above — detection is a pure-Rust path with no subprocess involvement.

## Rollout phases

One PR is enough. The patch is concentrated in `coding.rs` (four new private helpers + four new branches across three functions) and `tools/lint.rs` (two new linter groups). Tests, prompt sweep, README/CLAUDE.md updates, and the ROADMAP bullet removal land in the same commit.

If reviewer appetite prefers smaller diffs, split into two PRs:

1. **`coding.rs` detection** — the Gradle/Maven/CMake/Make branches + prompt sweep. The model now sees the new commands surface via `detect_*`; the per-file `lint_file` is unchanged.
2. **`tools/lint.rs` Java/Kotlin groups** — the per-file additions. Independent from PR 1.

## Verification

Sign-off requires:

1. `cargo build --workspace` clean on default features and `--all-features`.
2. `cargo lint` clean.
3. `cargo test` clean including the new unit tests in `coding.rs`.
4. Manual smoke checklist above.
5. The `--info` banner shows the expected commands in real JVM / C fixture repos.
6. Existing detection branches (Rust/Node/Python/Go) keep matching first when their markers are present.

## Risks

- **`./gradlew` and `./mvnw` permission bits**: a freshly cloned repo can have wrappers without `+x`. The detector returns `./gradlew test`; the shell then fails with "permission denied". Mitigation: in the Review phase prompt mention this caveat ("if `./gradlew` is not executable, run `chmod +x ./gradlew` first") and let the model react. We don't auto-chmod from the detector — silently mutating file permissions on detection would be surprising.
- **CMake build dir nonexistent**: the test command `ctest --test-dir build` fails when `build/` hasn't been configured. Mitigation: the Test phase prompt mentions the prerequisite ("ctest assumes you've already run `cmake -S . -B build && cmake --build build`"). The build command itself produces the dir, so the natural flow Build → Test handles it. Document.
- **Compile database staleness**: `compile_commands.json` reflects the build that produced it. After a structural change (new source file, new include path), the DB is stale and `clang-tidy` may complain. Mitigation: the prompt notes "if clang-tidy complains about missing includes, run the build first to refresh `compile_commands.json`."
- **Gradle daemon noise**: the first `./gradlew` invocation starts the daemon and takes 10–30 s. Mitigation: this is a Gradle property, not our concern; the model surfaces the duration to the user normally. We don't try to skip it.
- **Polyglot monorepos**: a repo with `Cargo.toml` at the root and a `pom.xml` in a subdirectory returns the Cargo command (root-only detection). Mitigation: the user runs aictl from the subdirectory and gets Maven, *or* sets `AICTL_CODING_TEST_CMD` explicitly. Don't try to be clever about multi-module heuristics.
- **`google-java-format` is not on the typical macOS dev box**: the linter falls through to `checkstyle` which is also often missing, then to `javac -Xlint` which almost always fails on a single-file lint. Mitigation: document the install pointers in the prompt's `lint_file` blurb so the model can tell the user. The fallback chain is correct in shape; what's missing is the binaries the user hasn't installed.

## Scope boundaries with other plans

- **Phase 1 (`coding-agent.md`)**: prerequisite. `detect_linter` and `detect_test_cmd` already exist and are read by the Review/Test phase prompt blocks.
- **Phase 2 (`coding-agent-phase-2.md`)**: orthogonal. The smarter `edit_file` / search tools don't depend on language detection.
- **Phase 3 (`coding-agent-phase-3.md`)**: tight coupling. `detect_build_cmd` is introduced in Phase 3. This plan piggybacks on the same scheme. Sequencing options:
  - **If Phase 3 lands first**: this plan adds the four JVM/C branches to `detect_build_cmd` in addition to the existing two `detect_*` functions.
  - **If this plan lands first**: `detect_build_cmd` doesn't exist yet, so we ship the JVM/C branches in `detect_linter` and `detect_test_cmd` only; Phase 3's PR adds the matching branches when it lands.
  - Either order works; pick based on which PR is closer to ready.
- **Phase 4 (`coding-agent-phase-4.md`)**: orthogonal. Parallel tool dispatch doesn't interact with detection.

## Open questions

- **Wrapper precedence on Windows**: `gradlew.bat` and `mvnw.cmd` exist alongside the POSIX wrappers. v1 ignores them (CLI is broader than the macOS-only desktop, but Windows isn't a target yet). When we revisit, the detector returns the right invocation per OS via `cfg!(target_os = "windows")`.
- **`make check` vs. `make test` precedence**: today the precedence is `test` then `check`. Some old-school projects only define `check`. Lean keeping the order — most projects with both put the heavier suite in `test`.
- **Should we sniff Gradle's `kotlinOptions` to disambiguate Java vs. Kotlin?**: lean no — the detector doesn't care which language the project compiles. The commands (`./gradlew check`, `./gradlew test`) are language-agnostic. Per-file linting picks the right tool based on extension.
- **CMake configure step in the build command**: today's proposal returns `cmake --build build`, which assumes `build/` is already configured. An alternative is to detect "no build dir" and return `cmake -S . -B build && cmake --build build`. Lean keeping the simpler shape; the user/model can configure first or set `AICTL_CODING_BUILD_CMD` to the two-step shell.
- **Bazel and Buck**: not in scope. These are niche enough at aictl's user base that adding them speculatively is over-fitting. Revisit if real demand surfaces.
- **`mvn` aliases**: `mvn verify` runs `compile + test + verify` plugins. Some projects only run analysis in `package` or `install`. We pick `verify` as the umbrella, which is the documented Maven convention; users with non-standard bindings override via `AICTL_CODING_LINTER`.
