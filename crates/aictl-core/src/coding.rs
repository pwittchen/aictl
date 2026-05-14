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

use std::path::Path;

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
    None
}

/// Lightweight check: does `package.json` declare a `"test"` script?
///
/// Avoids depending on `serde_json` for one tiny lookup — we already
/// pull it in transitively, but the check itself is cheap enough that a
/// regex-free string match keeps the dependency story honest.
fn package_json_has_test_script(path: &Path) -> bool {
    let Ok(contents) = std::fs::read_to_string(path) else {
        return false;
    };
    // Look for `"test"` inside a `"scripts"` block. Cheap-and-correct
    // for the common shapes; nested `"test"` keys elsewhere are
    // extremely rare and the false positive only causes us to try
    // `npm test`, which gives a clear error if the script is missing.
    let Some(scripts_start) = contents.find("\"scripts\"") else {
        return false;
    };
    let tail = &contents[scripts_start..];
    let Some(open) = tail.find('{') else {
        return false;
    };
    let body = &tail[open..];
    body.contains("\"test\"")
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
}
