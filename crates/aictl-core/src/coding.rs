//! Coding-agent mode helpers.
//!
//! Hosts the [`WorkflowPhase`] state machine the CLI's REPL drives, plus
//! the linter / test command auto-detection used by the Review and Test
//! phases. The engine itself is phase-agnostic — frontends own a
//! `WorkflowPhase` and feed it to `crate::run::build_system_prompt_with`
//! as a per-turn hint.
//!
//! The whole module is gated on
//! [`crate::config::coding_agent_enabled`]: when the master switch is
//! off, the helpers are still callable (the CLI keeps the state machine
//! ready in case the user flips the switch mid-session), but no
//! coding-specific prose lands in the system prompt.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::OnceLock;

/// One of the five phases the coding-agent workflow moves through.
///
/// State transitions live in the host (the CLI's REPL driver in v1) so
/// the engine stays phase-agnostic. The model can also self-report a
/// phase by emitting a `<phase>NAME</phase>` tag at the start of its
/// turn; [`WorkflowPhase::parse_tag`] is the canonical parser.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WorkflowPhase {
    Explore,
    Plan,
    Code,
    Review,
    Test,
}

impl WorkflowPhase {
    /// Lower-case label for prompts, logs, and UI indicators.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Explore => "explore",
            Self::Plan => "plan",
            Self::Code => "code",
            Self::Review => "review",
            Self::Test => "test",
        }
    }

    /// Parse a phase label (case-insensitive). Returns `None` for any
    /// unknown string so a typo from the model doesn't silently change
    /// state.
    #[must_use]
    pub fn from_label(label: &str) -> Option<Self> {
        match label.trim().to_ascii_lowercase().as_str() {
            "explore" => Some(Self::Explore),
            "plan" => Some(Self::Plan),
            "code" => Some(Self::Code),
            "review" => Some(Self::Review),
            "test" => Some(Self::Test),
            _ => None,
        }
    }

    /// Extract a `<phase>NAME</phase>` tag from the start of a model
    /// response. Returns the parsed phase and the remaining text with
    /// the tag stripped (so the host can show clean output to the user).
    ///
    /// Tolerant of leading whitespace and case variations. Only the
    /// very first tag — anything later in the body is left alone, so
    /// the model can mention the word "phase" later without re-parsing.
    #[must_use]
    pub fn parse_tag(text: &str) -> Option<(Self, String)> {
        let trimmed = text.trim_start();
        let rest = trimmed.strip_prefix("<phase>")?;
        let end = rest.find("</phase>")?;
        let label = &rest[..end];
        let phase = Self::from_label(label)?;
        let after = &rest[end + "</phase>".len()..];
        let leading_ws_len = text.len() - trimmed.len();
        let mut stripped = String::with_capacity(text.len());
        stripped.push_str(&text[..leading_ws_len]);
        stripped.push_str(after.trim_start_matches('\n'));
        Some((phase, stripped))
    }

    /// Advance one phase. `Test` stays at `Test` — the host decides
    /// whether a re-loop bumps back to `Code`.
    #[must_use]
    pub fn next(self) -> Self {
        match self {
            Self::Explore => Self::Plan,
            Self::Plan => Self::Code,
            Self::Code => Self::Review,
            Self::Review | Self::Test => Self::Test,
        }
    }

    /// Skip one phase, honoring the user's `AICTL_CODING_SKIP_*` flags.
    /// Used by the CLI's `/skip review` and `/skip test` shortcuts.
    #[must_use]
    pub fn skip_to_after(self, skip_review: bool, skip_test: bool) -> Self {
        let mut current = self.next();
        if skip_review && current == Self::Review {
            current = current.next();
        }
        if skip_test && current == Self::Test {
            // Test is terminal — staying at Test or jumping past it
            // both collapse to Test for the indicator.
        }
        current
    }
}

/// Per-turn prompt fragment the host hands to
/// `crate::run::build_system_prompt_with`. Keeps the prose in one place
/// so the CLI and any future frontend that gains a phase tracker emit
/// the same hints.
///
/// `None` means "no phase guidance for this turn" (e.g. coding-agent
/// mode is off, or the host doesn't track phases).
#[must_use]
pub fn phase_hint(phase: WorkflowPhase) -> &'static str {
    match phase {
        WorkflowPhase::Explore => {
            "Current phase: Explore. Read code before changing it. Prefer read_file, search_files, find_files, list_directory, and git status / git log / git blame / git diff. Do not edit files in this phase. Also identify the project's build, lint, and test commands now — you will need them in Review and Test."
        }
        WorkflowPhase::Plan => {
            "Current phase: Plan. Produce a numbered plan: what you will change, where (file paths), and why. Keep it short and concrete. Note open questions explicitly."
        }
        WorkflowPhase::Code => {
            "Current phase: Code. Apply minimal, focused edits via edit_file or write_file. Match existing code style. Read a file once more right before editing if you're unsure of the exact text. When the last intended edit lands, move to Review and run the build — do NOT declare the task done here."
        }
        WorkflowPhase::Review => {
            "Current phase: Review. (1) Run `git diff` to confirm only the intended files changed. (2) Run the project build command via exec_shell (e.g. `cargo build`, `npm run build`, `tsc --noEmit`, `go build ./...`) and confirm it exits 0 — fix build errors before continuing. (3) Run lint_file on each changed file (or the project linter via exec_shell). (4) If the change affects user-visible behavior (new command/flag/config key, new public API, changed defaults, new dependencies), update the README and any other documentation the change invalidated — or, if no README.md exists at all, create one with a project name, build/install instructions, and a minimal usage example. If any of (1)–(3) fail, return to Code. Otherwise move to Test."
        }
        WorkflowPhase::Test => {
            "Current phase: Test. Run the project's test command via exec_shell (e.g. `cargo test`, `npm test`, `pytest`, `go test ./...`). Parse output and report pass/fail counts to the user. On any failure, fix the root cause and re-test. The coding task is NOT done until tests pass — or, if no test command exists, you have told the user explicitly that tests were skipped and given them the commands to run."
        }
    }
}

/// Detected (or user-configured) linter command for the current
/// project. Returns the override from [`crate::config::coding_linter_override`]
/// when set, otherwise probes the working directory for common project
/// markers.
#[must_use]
pub fn detect_linter(working_dir: &Path) -> Option<String> {
    if let Some(cmd) = crate::config::coding_linter_override() {
        return Some(cmd);
    }
    if working_dir.join("Cargo.toml").is_file() {
        if working_dir.join(".cargo/config.toml").is_file() {
            return Some("cargo lint".to_string());
        }
        return Some("cargo clippy --all-targets -- -D warnings".to_string());
    }
    if working_dir.join("package.json").is_file() {
        if working_dir.join("node_modules/.bin/eslint").exists() {
            return Some("npx eslint .".to_string());
        }
        if working_dir.join("tsconfig.json").is_file() {
            return Some("npx tsc --noEmit".to_string());
        }
    }
    if working_dir.join("pyproject.toml").is_file()
        || working_dir.join("requirements.txt").is_file()
    {
        return Some("ruff check .".to_string());
    }
    if working_dir.join("go.mod").is_file() {
        return Some("go vet ./...".to_string());
    }
    if let Some(wrapper) = detect_gradle(working_dir) {
        return Some(if wrapper {
            "./gradlew check".to_string()
        } else {
            "gradle check".to_string()
        });
    }
    if let Some(wrapper) = detect_maven(working_dir) {
        return Some(if wrapper {
            "./mvnw verify".to_string()
        } else {
            "mvn verify".to_string()
        });
    }
    if let Some(c) = detect_cmake(working_dir) {
        return Some(if c.has_compile_db {
            "clang-tidy --quiet -p build".to_string()
        } else {
            "cppcheck --enable=warning --quiet .".to_string()
        });
    }
    if let Some(m) = detect_make(working_dir)
        && m.has_check
    {
        return Some("make check".to_string());
    }
    None
}

/// Detected (or user-configured) test command for the current project.
///
/// `None` means "no test command detected" — the Test phase logs a
/// one-line note and skips. Users can wire one up with
/// `AICTL_CODING_TEST_CMD`.
#[must_use]
pub fn detect_test_cmd(working_dir: &Path) -> Option<String> {
    if let Some(cmd) = crate::config::coding_test_cmd_override() {
        return Some(cmd);
    }
    if working_dir.join("Cargo.toml").is_file() {
        return Some("cargo test".to_string());
    }
    if working_dir.join("package.json").is_file()
        && package_json_has_test_script(&working_dir.join("package.json"))
    {
        return Some("npm test".to_string());
    }
    if working_dir.join("pyproject.toml").is_file() || working_dir.join("pytest.ini").is_file() {
        return Some("pytest".to_string());
    }
    if working_dir.join("go.mod").is_file() {
        return Some("go test ./...".to_string());
    }
    if let Some(wrapper) = detect_gradle(working_dir) {
        return Some(if wrapper {
            "./gradlew test".to_string()
        } else {
            "gradle test".to_string()
        });
    }
    if let Some(wrapper) = detect_maven(working_dir) {
        return Some(if wrapper {
            "./mvnw test".to_string()
        } else {
            "mvn test".to_string()
        });
    }
    if detect_cmake(working_dir).is_some() {
        return Some("ctest --test-dir build --output-on-failure".to_string());
    }
    if let Some(m) = detect_make(working_dir) {
        if m.has_test {
            return Some("make test".to_string());
        }
        if m.has_check {
            return Some("make check".to_string());
        }
    }
    None
}

/// Detect a Gradle project at `working_dir`. Returns `Some(true)` when a
/// `gradlew` wrapper is present alongside the build script, `Some(false)`
/// when only system `gradle` will work, and `None` when no Gradle markers
/// are present. The wrapper is preferred so the model uses the
/// project-pinned tool version.
fn detect_gradle(working_dir: &Path) -> Option<bool> {
    let has_gradle = working_dir.join("build.gradle").is_file()
        || working_dir.join("build.gradle.kts").is_file()
        || working_dir.join("settings.gradle").is_file()
        || working_dir.join("settings.gradle.kts").is_file();
    if !has_gradle {
        return None;
    }
    Some(working_dir.join("gradlew").is_file())
}

/// Detect a Maven project at `working_dir`. Returns `Some(true)` when an
/// `mvnw` wrapper is present, `Some(false)` for system-`mvn` only, and
/// `None` when no `pom.xml` is present.
fn detect_maven(working_dir: &Path) -> Option<bool> {
    if !working_dir.join("pom.xml").is_file() {
        return None;
    }
    Some(working_dir.join("mvnw").is_file())
}

/// Shape of a detected `CMake` project.
struct CMakeShape {
    /// Whether a `compile_commands.json` sits inside one of the common
    /// build directories. Drives the linter choice — `clang-tidy` when
    /// present, `cppcheck` otherwise.
    has_compile_db: bool,
}

/// Detect a `CMake` project at `working_dir`. Looks for `CMakeLists.txt`
/// and, when present, probes the conventional build directories
/// (`build/`, `cmake-build-debug/`, `out/build/`) for a
/// `compile_commands.json`.
fn detect_cmake(working_dir: &Path) -> Option<CMakeShape> {
    if !working_dir.join("CMakeLists.txt").is_file() {
        return None;
    }
    let build_dir = ["build", "cmake-build-debug", "out/build"]
        .into_iter()
        .map(|d| working_dir.join(d))
        .find(|p| p.is_dir());
    let has_compile_db = build_dir
        .as_ref()
        .is_some_and(|d| d.join("compile_commands.json").is_file());
    Some(CMakeShape { has_compile_db })
}

/// Shape of a detected Make project — which conventional targets the
/// `Makefile` defines.
struct MakeShape {
    has_test: bool,
    has_check: bool,
}

/// Detect a Make-only project at `working_dir`. Reads `Makefile` (when
/// present) and looks for `test:` and `check:` target lines so the
/// caller can pick the right invocation.
fn detect_make(working_dir: &Path) -> Option<MakeShape> {
    let path = working_dir.join("Makefile");
    if !path.is_file() {
        return None;
    }
    let body = std::fs::read_to_string(&path).unwrap_or_default();
    Some(MakeShape {
        has_test: makefile_has_target(&body, "test"),
        has_check: makefile_has_target(&body, "check"),
    })
}

/// True when `body` contains a Makefile rule for `target` — i.e. a line
/// that, after optional leading whitespace and an optional `.PHONY:`
/// decoration, starts with `<target>:` followed by whitespace or end of
/// line. Comments and lines where the target name is a prefix of a
/// longer target (e.g. `test-foo`) are rejected.
fn makefile_has_target(body: &str, target: &str) -> bool {
    for raw in body.lines() {
        let line = raw.trim_start();
        if line.starts_with('#') {
            continue;
        }
        let rest = if let Some(after_phony) = line.strip_prefix(".PHONY:") {
            // `.PHONY: test foo` declares `test` as phony but is not the
            // rule itself; treat presence in the phony list as a match
            // (close enough — projects that declare a phony target
            // almost always define its body too).
            for tok in after_phony.split_whitespace() {
                if tok == target {
                    return true;
                }
            }
            continue;
        } else {
            line
        };
        let Some(after) = rest.strip_prefix(target) else {
            continue;
        };
        // Next char must be `:` to be a rule head; the colon must be
        // followed by whitespace, end-of-line, or another `:` (double-colon
        // rules) so `test-foo:` doesn't match when target is `test`.
        let mut chars = after.chars();
        if chars.next() != Some(':') {
            continue;
        }
        match chars.next() {
            None => return true,
            Some(c) if c.is_whitespace() || c == ':' => return true,
            _ => {}
        }
    }
    false
}

/// Detected (or user-configured) build command for the current project.
///
/// Used by the host-side Review hook (the build step) and surfaced in
/// the `<repo_context>` block. Falls back to `None` when no project
/// markers are found — the Review hook then skips the build step and
/// records "no build command detected" in the review result.
#[must_use]
pub fn detect_build_cmd(working_dir: &Path) -> Option<String> {
    if let Some(cmd) = crate::config::coding_build_cmd_override() {
        return Some(cmd);
    }
    if working_dir.join("Cargo.toml").is_file() {
        return Some("cargo build".to_string());
    }
    if working_dir.join("package.json").is_file() {
        if package_json_has_script(&working_dir.join("package.json"), "build") {
            return Some("npm run build".to_string());
        }
        if working_dir.join("tsconfig.json").is_file() {
            return Some("npx tsc --noEmit".to_string());
        }
    }
    if working_dir.join("go.mod").is_file() {
        return Some("go build ./...".to_string());
    }
    if working_dir.join("pyproject.toml").is_file() {
        return Some("python -m build".to_string());
    }
    if let Some(wrapper) = detect_gradle(working_dir) {
        return Some(if wrapper {
            "./gradlew build".to_string()
        } else {
            "gradle build".to_string()
        });
    }
    if let Some(wrapper) = detect_maven(working_dir) {
        return Some(if wrapper {
            "./mvnw package".to_string()
        } else {
            "mvn package".to_string()
        });
    }
    if detect_cmake(working_dir).is_some() {
        return Some("cmake --build build".to_string());
    }
    if detect_make(working_dir).is_some() {
        return Some("make".to_string());
    }
    None
}

/// Lightweight check: does `package.json` declare a `"test"` script?
///
/// Avoids depending on `serde_json` for one tiny lookup — we already
/// pull it in transitively, but the check itself is cheap enough that a
/// regex-free string match keeps the dependency story honest.
fn package_json_has_test_script(path: &Path) -> bool {
    package_json_has_script(path, "test")
}

/// Generalized form of [`package_json_has_test_script`] — checks the
/// `"scripts"` block for an entry with the given name. Reused by the
/// build-command detector for `"build"`.
fn package_json_has_script(path: &Path, script: &str) -> bool {
    let Ok(contents) = std::fs::read_to_string(path) else {
        return false;
    };
    let Some(scripts_start) = contents.find("\"scripts\"") else {
        return false;
    };
    let tail = &contents[scripts_start..];
    let Some(open) = tail.find('{') else {
        return false;
    };
    let body = &tail[open..];
    let needle = format!("\"{script}\"");
    body.contains(&needle)
}

/// Snapshot of repo state injected into the system prompt as a
/// `<repo_context>` block at the start of every coding-agent turn.
///
/// Best-effort: any field can be `None` / empty when the working
/// directory isn't a git repo, the relevant tool isn't on `PATH`, or
/// reads error out. The block is purely informational.
#[derive(Debug, Clone, Default)]
pub struct RepoContext {
    pub branch: Option<String>,
    pub last_commits: Vec<String>,
    pub dirty: bool,
    pub dirty_files: Vec<String>,
    pub top_level_tree: Vec<String>,
    pub linter: Option<String>,
    pub test_cmd: Option<String>,
    pub build_cmd: Option<String>,
}

/// Cached repo-context snapshot, keyed by the working directory it was
/// collected for. The first `collect_repo_context()` call populates it
/// and every subsequent call returns the cached value. The CLI's
/// `/coding refresh` slash command (and the host after a `write_file` /
/// `edit_file` / `remove_file` / `create_directory`) call
/// [`invalidate_repo_context`] to bust the cache.
static REPO_CONTEXT_CACHE: OnceLock<Mutex<Option<(PathBuf, RepoContext)>>> = OnceLock::new();

fn cache_slot() -> &'static Mutex<Option<(PathBuf, RepoContext)>> {
    REPO_CONTEXT_CACHE.get_or_init(|| Mutex::new(None))
}

/// Bust the cached `<repo_context>` snapshot. The next call to
/// [`collect_repo_context`] will re-read the working directory from
/// scratch. Safe to call from any thread.
pub fn invalidate_repo_context() {
    if let Ok(mut slot) = cache_slot().lock() {
        *slot = None;
    }
}

/// Workspace paths the agent has modified since the session started.
/// Populated by [`record_workspace_change`] from the agent loop after
/// every successful `write_file` / `edit_file` / `remove_file` /
/// `create_directory` tool call; consumed by the host-driven Review
/// hook in coding-agent mode to know whether a structured review is
/// even worth running.
static CHANGED_PATHS: OnceLock<Mutex<Vec<PathBuf>>> = OnceLock::new();

fn changed_slot() -> &'static Mutex<Vec<PathBuf>> {
    CHANGED_PATHS.get_or_init(|| Mutex::new(Vec::new()))
}

/// Note that `path` has been touched by a workspace-mutating tool.
/// Idempotent — repeated edits of the same file collapse to one entry.
pub fn record_workspace_change(path: &Path) {
    if let Ok(mut slot) = changed_slot().lock() {
        let path = path.to_path_buf();
        if !slot.contains(&path) {
            slot.push(path);
        }
    }
}

/// Drain and return the changed paths set. Called by the Review hook
/// when it decides whether to run; consumers that want a read-only
/// snapshot should call [`changed_paths`] instead.
#[must_use]
pub fn take_changed_paths() -> Vec<PathBuf> {
    changed_slot()
        .lock()
        .map(|mut s| std::mem::take(&mut *s))
        .unwrap_or_default()
}

/// Snapshot of the changed-paths set. Used by the `/coding status`
/// printer; tools that want to consume-and-clear should call
/// [`take_changed_paths`] instead.
#[must_use]
pub fn changed_paths() -> Vec<PathBuf> {
    changed_slot().lock().map(|s| s.clone()).unwrap_or_default()
}

/// Forget every recorded workspace change. Wired to session resets so
/// a fresh session starts the Review hook from "no changes yet".
pub fn clear_changed_paths() {
    if let Ok(mut slot) = changed_slot().lock() {
        slot.clear();
    }
}

// --- Structured Review hook ---

/// Detail returned by one step of the structured Review hook.
#[derive(Debug, Clone, Default)]
pub struct StepResult {
    pub label: String,
    pub command: String,
    pub exit_code: i32,
    pub output_tail: String,
}

/// Outcome of a single structured Review run.
#[derive(Debug, Clone)]
pub enum ReviewOutcome {
    Pass {
        reason: String,
    },
    Fail {
        build: Option<StepResult>,
        lints: Vec<StepResult>,
    },
    Skipped {
        reason: String,
    },
}

const REVIEW_TAIL_BYTES: usize = 2048;

/// Run the structured Review hook against the current working dir.
/// Reads the recorded changed paths (consuming them) and the detected
/// build / lint commands, then runs them in sequence.
///
/// All commands are run with scrubbed env, pinned to the security
/// policy's working dir, capped by `shell_timeout`. No tool dispatch,
/// no audit log entry — this is host machinery, not a model-visible
/// tool call.
pub async fn run_structured_review() -> ReviewOutcome {
    if crate::config::coding_skip_review() {
        clear_changed_paths();
        return ReviewOutcome::Skipped {
            reason: "AICTL_CODING_SKIP_REVIEW=true".to_string(),
        };
    }

    let changed = take_changed_paths();
    if changed.is_empty() {
        return ReviewOutcome::Pass {
            reason: "no workspace changes recorded this session".to_string(),
        };
    }

    let working_dir = crate::security::policy().paths.working_dir.clone();

    let mut build: Option<StepResult> = None;
    let mut failed_build = false;
    if let Some(build_cmd) = detect_build_cmd(&working_dir) {
        let step = run_shell_step("build", &build_cmd, &working_dir).await;
        if step.exit_code != 0 {
            failed_build = true;
        }
        build = Some(step);
    }

    let mut lints: Vec<StepResult> = Vec::new();
    let mut any_lint_fail = false;
    for path in &changed {
        let path_str = path.to_string_lossy().to_string();
        // lint_file picks the linter from the extension. Files without
        // an extension or without a configured linter just return
        // "Error: …" and we treat that as informational, not a Review
        // failure, since the file may be a config or markdown file.
        let body = crate::tools::tool_lint_file(&path_str).await;
        let exit_code = if body.starts_with("Error:") { -2 } else { 0 };
        // A linter that prints output and exits non-zero comes back as
        // "<header>\n<diagnostics>" — we cannot recover the exit code
        // from `tool_lint_file`, so we treat the substring "Error:" as
        // an informational marker (linter not installed / unsupported
        // ext) and anything *else* whose body is non-trivial and not a
        // "clean" sentinel as a failure.
        let trimmed = body.trim();
        let looks_clean = trimmed.contains("no issues")
            || trimmed.contains("clean")
            || trimmed.contains("0 issues")
            || trimmed.is_empty();
        let is_informational = exit_code == -2;
        let final_exit = if is_informational {
            -2
        } else {
            i32::from(!looks_clean)
        };
        if final_exit == 1 {
            any_lint_fail = true;
        }
        lints.push(StepResult {
            label: format!("lint {path_str}"),
            command: format!("lint_file {path_str}"),
            exit_code: final_exit,
            output_tail: tail_bytes(&body, REVIEW_TAIL_BYTES),
        });
    }

    // Re-record the changed paths so a follow-up Review run still sees
    // them — `take_changed_paths` consumed the list, but the workspace
    // is still in the same state and a re-loop should re-check.
    {
        if let Ok(mut slot) = changed_slot().lock() {
            for p in &changed {
                if !slot.contains(p) {
                    slot.push(p.clone());
                }
            }
        }
    }

    if failed_build || any_lint_fail {
        ReviewOutcome::Fail { build, lints }
    } else {
        ReviewOutcome::Pass {
            reason: build.as_ref().map_or_else(
                || "lint passed; no build command configured".to_string(),
                |_| "build + lint passed".to_string(),
            ),
        }
    }
}

async fn run_shell_step(label: &str, cmd: &str, working_dir: &Path) -> StepResult {
    let mut proc = tokio::process::Command::new("sh");
    proc.arg("-c").arg(cmd);
    proc.env_clear();
    for (k, v) in crate::security::scrubbed_env() {
        proc.env(k, v);
    }
    proc.env("NO_COLOR", "1");
    proc.env("CLICOLOR", "0");
    proc.env("CARGO_TERM_COLOR", "never");
    proc.current_dir(working_dir);
    proc.stdin(std::process::Stdio::null());
    proc.stdout(std::process::Stdio::piped());
    proc.stderr(std::process::Stdio::piped());
    proc.kill_on_drop(true);

    let output_future = async {
        let child = proc.spawn()?;
        child.wait_with_output().await
    };

    let output = if let Some(timeout) = crate::security::shell_timeout() {
        match tokio::time::timeout(timeout, output_future).await {
            Ok(r) => r,
            Err(_) => {
                return StepResult {
                    label: label.to_string(),
                    command: cmd.to_string(),
                    exit_code: -1,
                    output_tail: format!("[timed out after {}s]", timeout.as_secs()),
                };
            }
        }
    } else {
        output_future.await
    };

    match output {
        Ok(out) => {
            let mut buf = String::new();
            buf.push_str(&String::from_utf8_lossy(&out.stdout));
            if !out.stderr.is_empty() {
                if !buf.is_empty() && !buf.ends_with('\n') {
                    buf.push('\n');
                }
                buf.push_str("[stderr]\n");
                buf.push_str(&String::from_utf8_lossy(&out.stderr));
            }
            StepResult {
                label: label.to_string(),
                command: cmd.to_string(),
                exit_code: out.status.code().unwrap_or(-1),
                output_tail: tail_bytes(&buf, REVIEW_TAIL_BYTES),
            }
        }
        Err(e) => StepResult {
            label: label.to_string(),
            command: cmd.to_string(),
            exit_code: -1,
            output_tail: format!("failed to spawn: {e}"),
        },
    }
}

fn tail_bytes(s: &str, cap: usize) -> String {
    if s.len() <= cap {
        return s.to_string();
    }
    let mut idx = s.len().saturating_sub(cap);
    while idx < s.len() && !s.is_char_boundary(idx) {
        idx += 1;
    }
    format!("…\n{}", &s[idx..])
}

/// Collect (or return cached) `RepoContext` for `working_dir`.
///
/// The collected fields are all best-effort: errors from git or the
/// filesystem fall back to `None` / empty rather than propagating —
/// the block is informational, not load-bearing.
#[must_use]
pub fn collect_repo_context(working_dir: &Path) -> RepoContext {
    if let Ok(slot) = cache_slot().lock()
        && let Some((cached_dir, ctx)) = slot.as_ref()
        && cached_dir == working_dir
    {
        return ctx.clone();
    }

    let branch = git_one_line(working_dir, &["rev-parse", "--abbrev-ref", "HEAD"]);
    let last_commits = git_multi_line(
        working_dir,
        &["log", "-n", "5", "--pretty=oneline", "--abbrev-commit"],
        5,
    );
    let dirty_files = git_multi_line(working_dir, &["status", "--short"], 40);
    let dirty = !dirty_files.is_empty();
    let depth = crate::config::coding_repo_context_tree_depth();
    let max = crate::config::coding_repo_context_tree_max();
    let top_level_tree = walk_tree(working_dir, depth, max);
    let linter = detect_linter(working_dir);
    let test_cmd = detect_test_cmd(working_dir);
    let build_cmd = detect_build_cmd(working_dir);

    let ctx = RepoContext {
        branch,
        last_commits,
        dirty,
        dirty_files,
        top_level_tree,
        linter,
        test_cmd,
        build_cmd,
    };

    if let Ok(mut slot) = cache_slot().lock() {
        *slot = Some((working_dir.to_path_buf(), ctx.clone()));
    }
    ctx
}

/// Render the cached `RepoContext` as a markdown-friendly block
/// suitable for appending to the system prompt. Returns an empty string
/// when [`crate::config::coding_repo_context_enabled`] is `false` so
/// the caller can unconditionally concatenate the result.
#[must_use]
pub fn format_repo_context(working_dir: &Path) -> String {
    use std::fmt::Write as _;
    if !crate::config::coding_repo_context_enabled() {
        return String::new();
    }
    let ctx = collect_repo_context(working_dir);
    let mut out = String::new();
    out.push_str("\n\n# Repo context\n");

    if let Some(branch) = &ctx.branch {
        let _ = writeln!(out, "\nBranch: {branch}");
    }
    if ctx.dirty {
        let count = ctx.dirty_files.len();
        let _ = writeln!(
            out,
            "Working tree: dirty ({} changed file{})",
            count,
            if count == 1 { "" } else { "s" }
        );
    } else if ctx.branch.is_some() {
        out.push_str("Working tree: clean\n");
    }

    if !ctx.last_commits.is_empty() {
        out.push_str("\nRecent commits:\n");
        for line in &ctx.last_commits {
            let _ = writeln!(out, "  {line}");
        }
    }

    if !ctx.dirty_files.is_empty() {
        out.push_str("\nModified files:\n");
        for line in &ctx.dirty_files {
            let _ = writeln!(out, "  {line}");
        }
    }

    if !ctx.top_level_tree.is_empty() {
        out.push_str("\nTop-level layout:\n");
        for line in &ctx.top_level_tree {
            let _ = writeln!(out, "  {line}");
        }
    }

    let any_cmd = ctx.build_cmd.is_some() || ctx.linter.is_some() || ctx.test_cmd.is_some();
    if any_cmd {
        out.push_str("\nProject commands:\n");
        if let Some(b) = &ctx.build_cmd {
            let _ = writeln!(out, "  build: {b}");
        }
        if let Some(l) = &ctx.linter {
            let _ = writeln!(out, "  lint:  {l}");
        }
        if let Some(t) = &ctx.test_cmd {
            let _ = writeln!(out, "  test:  {t}");
        }
    }

    out
}

/// Run `git <args>` in `working_dir`, return the first non-empty stdout
/// line. Used for one-shot reads like the branch name.
fn git_one_line(working_dir: &Path, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(working_dir)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .find(|l| !l.trim().is_empty())
        .map(|l| l.trim().to_string())
}

/// Run `git <args>` in `working_dir`, return up to `cap` non-empty
/// trimmed stdout lines.
fn git_multi_line(working_dir: &Path, args: &[&str], cap: usize) -> Vec<String> {
    let Ok(output) = std::process::Command::new("git")
        .args(args)
        .current_dir(working_dir)
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .take(cap)
        .map(|l| l.trim_end().to_string())
        .collect()
}

/// Walk `root` to `depth` levels and return up to `max` entries as a
/// shallow tree listing. Directories are suffixed with `/` and shown
/// before files at each level; hidden entries (leading `.`) plus a few
/// common build-output directories (`target`, `node_modules`, `dist`,
/// `build`) are skipped so the listing stays useful.
fn walk_tree(root: &Path, depth: usize, max: usize) -> Vec<String> {
    let mut out = Vec::new();
    walk_tree_inner(root, depth, max, 0, &mut out);
    out
}

fn walk_tree_inner(dir: &Path, depth: usize, max: usize, level: usize, out: &mut Vec<String>) {
    if level >= depth || out.len() >= max {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut items: Vec<(bool, String)> = Vec::new();
    for entry in entries.flatten() {
        let Ok(file_name) = entry.file_name().into_string() else {
            continue;
        };
        if file_name.starts_with('.') {
            continue;
        }
        if matches!(
            file_name.as_str(),
            "target" | "node_modules" | "dist" | "build" | "out" | "__pycache__"
        ) {
            continue;
        }
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        items.push((is_dir, file_name));
    }
    items.sort_by(|a, b| match (a.0, b.0) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.1.cmp(&b.1),
    });
    for (is_dir, name) in items {
        if out.len() >= max {
            out.push("…".to_string());
            return;
        }
        let indent = "  ".repeat(level);
        let suffix = if is_dir { "/" } else { "" };
        out.push(format!("{indent}{name}{suffix}"));
        if is_dir {
            walk_tree_inner(&dir.join(&name), depth, max, level + 1, out);
            if out.len() >= max {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_tag_extracts_phase_and_strips() {
        let (phase, rest) = WorkflowPhase::parse_tag("<phase>plan</phase>\nthe plan…").unwrap();
        assert_eq!(phase, WorkflowPhase::Plan);
        assert_eq!(rest, "the plan…");
    }

    #[test]
    fn parse_tag_handles_leading_whitespace() {
        let (phase, rest) =
            WorkflowPhase::parse_tag("  \n<phase>code</phase>writing edit").unwrap();
        assert_eq!(phase, WorkflowPhase::Code);
        // Leading whitespace before the tag is preserved as-is.
        assert!(rest.starts_with("  \n"));
        assert!(rest.ends_with("writing edit"));
    }

    #[test]
    fn parse_tag_rejects_unknown_label() {
        assert!(WorkflowPhase::parse_tag("<phase>refactor</phase>body").is_none());
    }

    #[test]
    fn parse_tag_returns_none_without_tag() {
        assert!(WorkflowPhase::parse_tag("no tag here").is_none());
    }

    #[test]
    fn from_label_is_case_insensitive() {
        assert_eq!(
            WorkflowPhase::from_label("EXPLORE"),
            Some(WorkflowPhase::Explore)
        );
        assert_eq!(
            WorkflowPhase::from_label("  Plan  "),
            Some(WorkflowPhase::Plan)
        );
        assert_eq!(WorkflowPhase::from_label("bogus"), None);
    }

    #[test]
    fn next_advances_through_phases() {
        assert_eq!(WorkflowPhase::Explore.next(), WorkflowPhase::Plan);
        assert_eq!(WorkflowPhase::Plan.next(), WorkflowPhase::Code);
        assert_eq!(WorkflowPhase::Code.next(), WorkflowPhase::Review);
        assert_eq!(WorkflowPhase::Review.next(), WorkflowPhase::Test);
        // Test is terminal — host decides re-looping.
        assert_eq!(WorkflowPhase::Test.next(), WorkflowPhase::Test);
    }

    #[test]
    fn skip_to_after_honors_skip_flags() {
        // Skip review only.
        assert_eq!(
            WorkflowPhase::Code.skip_to_after(true, false),
            WorkflowPhase::Test
        );
        // Skip neither — same as next().
        assert_eq!(
            WorkflowPhase::Code.skip_to_after(false, false),
            WorkflowPhase::Review
        );
    }

    /// Build a fresh temp directory the test owns. Each test gets a
    /// uniquely named directory so parallel `cargo test` runs don't trip
    /// over one another.
    fn fixture_dir(tag: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "aictl_coding_{}_{}_{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn touch(dir: &std::path::Path, name: &str) {
        let p = dir.join(name);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(p, "").unwrap();
    }

    #[test]
    fn detect_gradle_picks_wrapper_when_present() {
        let dir = fixture_dir("gradle_wrapper");
        touch(&dir, "build.gradle");
        touch(&dir, "gradlew");
        assert_eq!(detect_gradle(&dir), Some(true));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn detect_gradle_picks_system_without_wrapper() {
        let dir = fixture_dir("gradle_system");
        touch(&dir, "build.gradle.kts");
        assert_eq!(detect_gradle(&dir), Some(false));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn detect_gradle_settings_only_still_matches() {
        let dir = fixture_dir("gradle_settings");
        touch(&dir, "settings.gradle.kts");
        assert_eq!(detect_gradle(&dir), Some(false));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn detect_gradle_returns_none_without_markers() {
        let dir = fixture_dir("gradle_empty");
        assert_eq!(detect_gradle(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn detect_maven_picks_wrapper_when_present() {
        let dir = fixture_dir("maven_wrapper");
        touch(&dir, "pom.xml");
        touch(&dir, "mvnw");
        assert_eq!(detect_maven(&dir), Some(true));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn detect_maven_picks_system_without_wrapper() {
        let dir = fixture_dir("maven_system");
        touch(&dir, "pom.xml");
        assert_eq!(detect_maven(&dir), Some(false));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn detect_cmake_with_compile_db() {
        let dir = fixture_dir("cmake_db");
        touch(&dir, "CMakeLists.txt");
        std::fs::create_dir_all(dir.join("build")).unwrap();
        touch(&dir, "build/compile_commands.json");
        let shape = detect_cmake(&dir).unwrap();
        assert!(shape.has_compile_db);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn detect_cmake_without_compile_db() {
        let dir = fixture_dir("cmake_nodb");
        touch(&dir, "CMakeLists.txt");
        let shape = detect_cmake(&dir).unwrap();
        assert!(!shape.has_compile_db);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn detect_make_recognizes_targets() {
        let dir = fixture_dir("make_targets");
        std::fs::write(
            dir.join("Makefile"),
            "all: build\n\nbuild:\n\techo build\n\ntest:\n\techo run\n\ncheck:\n\techo lint\n",
        )
        .unwrap();
        let shape = detect_make(&dir).unwrap();
        assert!(shape.has_test);
        assert!(shape.has_check);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn detect_make_without_relevant_targets() {
        let dir = fixture_dir("make_bare");
        std::fs::write(dir.join("Makefile"), "all:\n\techo hi\n").unwrap();
        let shape = detect_make(&dir).unwrap();
        assert!(!shape.has_test);
        assert!(!shape.has_check);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn makefile_has_target_matches_basic_rule() {
        assert!(makefile_has_target("test:\n\trun\n", "test"));
    }

    #[test]
    fn makefile_has_target_matches_phony_declaration() {
        assert!(makefile_has_target(
            ".PHONY: build test clean\n\nbuild:\n\trun\n",
            "test"
        ));
    }

    #[test]
    fn makefile_has_target_rejects_comment() {
        assert!(!makefile_has_target("# test: not a rule\n", "test"));
    }

    #[test]
    fn makefile_has_target_rejects_prefix_only_match() {
        assert!(!makefile_has_target("test-foo:\n\trun\n", "test"));
    }

    #[test]
    fn makefile_has_target_accepts_double_colon_rule() {
        assert!(makefile_has_target("test::\n\trun\n", "test"));
    }

    /// Skip `detect_linter`/`detect_test_cmd` precedence tests if a dev's
    /// real `~/.aictl/config` has the override set — the override fires
    /// first by design and any test against detection output would be a
    /// false negative.
    fn linter_override_set() -> bool {
        crate::config::coding_linter_override().is_some()
    }

    fn test_cmd_override_set() -> bool {
        crate::config::coding_test_cmd_override().is_some()
    }

    /// `detect_linter` should keep Rust's precedence ahead of Gradle even
    /// when both markers exist (polyglot monorepo, root-level Cargo).
    #[test]
    fn detect_linter_rust_wins_over_gradle() {
        if linter_override_set() {
            return;
        }
        let dir = fixture_dir("precedence_rust_gradle");
        touch(&dir, "Cargo.toml");
        touch(&dir, "build.gradle");
        let cmd = detect_linter(&dir).unwrap();
        assert!(cmd.contains("cargo"), "got: {cmd}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// When only JVM markers are present, Gradle beats Maven (matches
    /// industry default for migration repos).
    #[test]
    fn detect_linter_gradle_wins_over_maven() {
        if linter_override_set() {
            return;
        }
        let dir = fixture_dir("precedence_gradle_maven");
        touch(&dir, "build.gradle");
        touch(&dir, "pom.xml");
        let cmd = detect_linter(&dir).unwrap();
        assert!(cmd.contains("gradle"), "got: {cmd}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn detect_test_cmd_maven_wrapper() {
        if test_cmd_override_set() {
            return;
        }
        let dir = fixture_dir("test_maven_wrapper");
        touch(&dir, "pom.xml");
        touch(&dir, "mvnw");
        assert_eq!(detect_test_cmd(&dir).as_deref(), Some("./mvnw test"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn detect_test_cmd_cmake_uses_ctest() {
        if test_cmd_override_set() {
            return;
        }
        let dir = fixture_dir("test_cmake");
        touch(&dir, "CMakeLists.txt");
        assert_eq!(
            detect_test_cmd(&dir).as_deref(),
            Some("ctest --test-dir build --output-on-failure")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn detect_test_cmd_make_prefers_test_over_check() {
        if test_cmd_override_set() {
            return;
        }
        let dir = fixture_dir("test_make_both");
        std::fs::write(
            dir.join("Makefile"),
            "test:\n\trun-test\n\ncheck:\n\trun-lint\n",
        )
        .unwrap();
        assert_eq!(detect_test_cmd(&dir).as_deref(), Some("make test"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn detect_linter_cmake_with_db_picks_clang_tidy() {
        if linter_override_set() {
            return;
        }
        let dir = fixture_dir("lint_cmake_db");
        touch(&dir, "CMakeLists.txt");
        std::fs::create_dir_all(dir.join("build")).unwrap();
        touch(&dir, "build/compile_commands.json");
        assert_eq!(
            detect_linter(&dir).as_deref(),
            Some("clang-tidy --quiet -p build")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn detect_linter_cmake_without_db_falls_back_to_cppcheck() {
        if linter_override_set() {
            return;
        }
        let dir = fixture_dir("lint_cmake_nodb");
        touch(&dir, "CMakeLists.txt");
        assert_eq!(
            detect_linter(&dir).as_deref(),
            Some("cppcheck --enable=warning --quiet .")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn build_cmd_override_set() -> bool {
        crate::config::coding_build_cmd_override().is_some()
    }

    #[test]
    fn detect_build_cmd_rust() {
        if build_cmd_override_set() {
            return;
        }
        let dir = fixture_dir("build_rust");
        touch(&dir, "Cargo.toml");
        assert_eq!(detect_build_cmd(&dir).as_deref(), Some("cargo build"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn detect_build_cmd_go() {
        if build_cmd_override_set() {
            return;
        }
        let dir = fixture_dir("build_go");
        touch(&dir, "go.mod");
        assert_eq!(detect_build_cmd(&dir).as_deref(), Some("go build ./..."));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn detect_build_cmd_node_with_build_script() {
        if build_cmd_override_set() {
            return;
        }
        let dir = fixture_dir("build_node");
        std::fs::write(
            dir.join("package.json"),
            r#"{"name":"x","scripts":{"build":"webpack"}}"#,
        )
        .unwrap();
        assert_eq!(detect_build_cmd(&dir).as_deref(), Some("npm run build"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn detect_build_cmd_node_falls_back_to_tsc() {
        if build_cmd_override_set() {
            return;
        }
        let dir = fixture_dir("build_node_tsc");
        std::fs::write(dir.join("package.json"), r#"{"name":"x","scripts":{}}"#).unwrap();
        touch(&dir, "tsconfig.json");
        assert_eq!(detect_build_cmd(&dir).as_deref(), Some("npx tsc --noEmit"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn detect_build_cmd_gradle_wrapper() {
        if build_cmd_override_set() {
            return;
        }
        let dir = fixture_dir("build_gradle_wrap");
        touch(&dir, "build.gradle");
        touch(&dir, "gradlew");
        assert_eq!(detect_build_cmd(&dir).as_deref(), Some("./gradlew build"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn detect_build_cmd_cmake() {
        if build_cmd_override_set() {
            return;
        }
        let dir = fixture_dir("build_cmake");
        touch(&dir, "CMakeLists.txt");
        assert_eq!(
            detect_build_cmd(&dir).as_deref(),
            Some("cmake --build build")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn detect_build_cmd_returns_none_for_empty_dir() {
        if build_cmd_override_set() {
            return;
        }
        let dir = fixture_dir("build_empty");
        assert!(detect_build_cmd(&dir).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn walk_tree_skips_hidden_and_build_outputs() {
        let dir = fixture_dir("walk_skip");
        touch(&dir, ".hidden");
        std::fs::create_dir_all(dir.join("node_modules")).unwrap();
        std::fs::create_dir_all(dir.join("src")).unwrap();
        touch(&dir, "src/main.rs");
        touch(&dir, "Cargo.toml");
        let entries = walk_tree(&dir, 2, 60);
        // Hidden entry and node_modules should be skipped.
        assert!(entries.iter().all(|e| !e.contains(".hidden")));
        assert!(entries.iter().all(|e| !e.contains("node_modules")));
        // src dir and its child main.rs should appear.
        assert!(entries.iter().any(|e| e == "src/"));
        assert!(entries.iter().any(|e| e.ends_with("main.rs")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn walk_tree_caps_entries() {
        let dir = fixture_dir("walk_cap");
        for i in 0..20 {
            touch(&dir, &format!("file_{i:02}.txt"));
        }
        let entries = walk_tree(&dir, 1, 5);
        // 5 entries plus the trailing `…` marker.
        assert!(entries.len() <= 6);
        assert!(entries.iter().any(|e| e == "…"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn repo_context_cache_returns_cached_then_refreshes() {
        let dir = fixture_dir("repo_ctx_cache");
        touch(&dir, "Cargo.toml");
        invalidate_repo_context();
        let first = collect_repo_context(&dir);
        let second = collect_repo_context(&dir);
        assert_eq!(first.build_cmd, second.build_cmd);
        // Different directory bypasses the cache.
        let other = fixture_dir("repo_ctx_other");
        touch(&other, "go.mod");
        invalidate_repo_context();
        let third = collect_repo_context(&other);
        assert_ne!(first.build_cmd, third.build_cmd);
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&other);
    }

    #[test]
    fn format_repo_context_includes_project_commands() {
        let dir = fixture_dir("repo_ctx_format");
        touch(&dir, "Cargo.toml");
        invalidate_repo_context();
        let rendered = format_repo_context(&dir);
        assert!(rendered.contains("# Repo context"));
        assert!(rendered.contains("Project commands:"));
        assert!(rendered.contains("build: cargo build"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
