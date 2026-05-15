//! Run the project's test command and return a structured pass/fail
//! summary.
//!
//! In coding-agent mode the host reads the parsed [`TestSummary`] from
//! [`take_last_summary`] right after the tool call returns; on `failed >
//! 0` it injects a synthetic `<test_failure>` user turn carrying the
//! structured failures so the model can plan a fix. Outside coding-agent
//! mode the tool is still useful as a one-shot "run the tests" — the
//! prose body shows the same human-readable shape; the structured slot
//! just isn't read by anyone.
//!
//! Runs through the same subprocess plumbing as `exec_shell`: env scrubbed,
//! pinned to the security policy's working directory, capped by the shared
//! `shell_timeout`. No new privileges.

use std::fmt::Write as _;
use std::process::Stdio;
use std::sync::OnceLock;
use std::time::Instant;

use tokio::sync::Mutex;

mod parsers;

/// Per-test failure detail extracted from the runner output.
#[derive(Debug, Clone, Default)]
pub struct TestFailure {
    pub name: String,
    pub message: String,
    pub location: Option<String>,
}

/// Structured rendering of a single `test` tool invocation. Populated
/// by [`tool_test`] and consumed by the agent loop in coding-agent mode.
#[derive(Debug, Clone, Default)]
pub struct TestSummary {
    pub command: String,
    pub exit_code: i32,
    pub duration_ms: u128,
    pub passed: u32,
    pub failed: u32,
    pub skipped: u32,
    pub failures: Vec<TestFailure>,
    pub raw_tail: String,
    pub parse_warning: Option<String>,
}

/// Most-recent `TestSummary` produced by this process. Single
/// producer (the `test` tool) / single consumer (the agent loop's
/// post-dispatch hook), per-turn. Stored in a Tokio `Mutex` because both
/// sides are inside `async fn`s.
static LAST_SUMMARY: OnceLock<Mutex<Option<TestSummary>>> = OnceLock::new();

fn slot() -> &'static Mutex<Option<TestSummary>> {
    LAST_SUMMARY.get_or_init(|| Mutex::new(None))
}

/// Drain the structured summary stored by the most recent
/// [`tool_test`] dispatch. Returns `None` when no `test` tool has run
/// (or the slot has already been drained this turn).
pub async fn take_last_summary() -> Option<TestSummary> {
    slot().lock().await.take()
}

/// Maximum number of failures we render in the prose body / store in
/// the structured slot. Failure messages are also capped to keep the
/// prompt budget honest.
const MAX_FAILURES: usize = 25;
const MAX_FAILURE_MESSAGE: usize = 400;
const RAW_TAIL_BYTES: usize = 4096;

#[allow(clippy::too_many_lines)]
pub(super) async fn tool_test(input: &str) -> String {
    let parsed = match ToolInvocation::parse(input) {
        Ok(p) => p,
        Err(msg) => return msg,
    };

    let cwd = crate::security::policy().paths.working_dir.clone();
    let command = match parsed.resolve_command(&cwd) {
        Ok(cmd) => cmd,
        Err(msg) => return msg,
    };

    let runner = Runner::detect(&command);

    let mut proc = tokio::process::Command::new("sh");
    proc.arg("-c").arg(&command);
    proc.env_clear();
    for (k, v) in crate::security::scrubbed_env() {
        proc.env(k, v);
    }
    proc.env("NO_COLOR", "1");
    proc.env("CLICOLOR", "0");
    proc.env("CARGO_TERM_COLOR", "never");
    proc.env("FORCE_COLOR", "0");
    proc.current_dir(&cwd);
    proc.stdin(Stdio::null());
    proc.stdout(Stdio::piped());
    proc.stderr(Stdio::piped());
    proc.kill_on_drop(true);

    let started = Instant::now();
    let output_future = async {
        let child = proc.spawn()?;
        child.wait_with_output().await
    };
    let output = if let Some(timeout) = crate::security::shell_timeout() {
        if let Ok(r) = tokio::time::timeout(timeout, output_future).await {
            r
        } else {
            let summary = TestSummary {
                command: command.clone(),
                exit_code: -1,
                duration_ms: started.elapsed().as_millis(),
                parse_warning: Some(format!(
                    "test command timed out after {}s",
                    timeout.as_secs()
                )),
                raw_tail: String::new(),
                failed: 1,
                failures: vec![TestFailure {
                    name: "<timeout>".to_string(),
                    message: format!(
                        "the test command did not finish within {}s; consider raising AICTL_SECURITY_SHELL_TIMEOUT.",
                        timeout.as_secs()
                    ),
                    location: None,
                }],
                ..Default::default()
            };
            let body = render_body(&summary);
            store_summary(summary).await;
            return body;
        }
    } else {
        output_future.await
    };

    let duration_ms = started.elapsed().as_millis();

    let (stdout, stderr, exit_code) = match output {
        Ok(out) => (
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
            out.status.code().unwrap_or(-1),
        ),
        Err(e) => {
            let summary = TestSummary {
                command: command.clone(),
                exit_code: -1,
                duration_ms,
                parse_warning: Some(format!("failed to spawn test command: {e}")),
                ..Default::default()
            };
            let body = render_body(&summary);
            store_summary(summary).await;
            return body;
        }
    };

    let mut parsed_summary = match runner {
        Runner::Cargo => parsers::parse_cargo(&stdout, &stderr),
        Runner::Npm => parsers::parse_npm(&stdout, &stderr),
        Runner::Pytest => parsers::parse_pytest(&stdout, &stderr),
        Runner::Go => parsers::parse_go(&stdout, &stderr),
        Runner::Generic => parsers::parse_generic(&stdout, &stderr),
    };

    // Truncate failure-message tails to keep the prompt budget bounded.
    for f in &mut parsed_summary.failures {
        if f.message.len() > MAX_FAILURE_MESSAGE {
            f.message.truncate(MAX_FAILURE_MESSAGE);
            f.message.push('…');
        }
    }
    if parsed_summary.failures.len() > MAX_FAILURES {
        parsed_summary.failures.truncate(MAX_FAILURES);
    }

    // If the parser couldn't find any structured failures but the
    // process exited non-zero, treat it as a failure with the raw tail
    // so the model sees something actionable.
    if exit_code != 0 && parsed_summary.failed == 0 && parsed_summary.failures.is_empty() {
        parsed_summary.failed = 1;
        parsed_summary.failures.push(TestFailure {
            name: format!("<runner exit {exit_code}>"),
            message: "test runner exited non-zero but no failures parsed; see raw tail."
                .to_string(),
            location: None,
        });
        if parsed_summary.parse_warning.is_none() {
            parsed_summary.parse_warning = Some(
                "no structured failures parsed; counts unavailable. Treat raw tail as the failure detail."
                    .to_string(),
            );
        }
    }

    parsed_summary.command = command;
    parsed_summary.exit_code = exit_code;
    parsed_summary.duration_ms = duration_ms;
    parsed_summary.raw_tail = build_raw_tail(&stdout, &stderr);

    let body = render_body(&parsed_summary);
    store_summary(parsed_summary).await;
    body
}

async fn store_summary(summary: TestSummary) {
    let mut slot = slot().lock().await;
    *slot = Some(summary);
}

/// Last `RAW_TAIL_BYTES` of the combined stdout/stderr stream, on UTF-8
/// boundaries so the tail is safe to inject back into the conversation.
fn build_raw_tail(stdout: &str, stderr: &str) -> String {
    let mut combined = String::with_capacity(stdout.len() + stderr.len() + 16);
    if !stdout.is_empty() {
        combined.push_str(stdout);
    }
    if !stderr.is_empty() {
        if !combined.is_empty() && !combined.ends_with('\n') {
            combined.push('\n');
        }
        combined.push_str("[stderr]\n");
        combined.push_str(stderr);
    }
    if combined.len() <= RAW_TAIL_BYTES {
        return combined;
    }
    let mut idx = combined.len().saturating_sub(RAW_TAIL_BYTES);
    while idx < combined.len() && !combined.is_char_boundary(idx) {
        idx += 1;
    }
    format!("…\n{}", &combined[idx..])
}

#[allow(clippy::cast_precision_loss)]
fn render_body(s: &TestSummary) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "Command: {}", s.command);
    let _ = writeln!(out, "Exit:    {}", s.exit_code);
    let _ = writeln!(out, "Time:    {:.1}s", s.duration_ms as f64 / 1000.0);
    out.push('\n');
    let _ = writeln!(out, "Passed:  {}", s.passed);
    let _ = writeln!(out, "Failed:  {}", s.failed);
    let _ = writeln!(out, "Skipped: {}", s.skipped);

    if let Some(warn) = &s.parse_warning {
        let _ = writeln!(out, "\nParser note: {warn}");
    }

    if s.failures.is_empty() {
        if s.failed == 0 {
            out.push_str("\n(no failures)\n");
        }
    } else {
        out.push_str("\nFailures:\n");
        for f in &s.failures {
            let _ = writeln!(out, "  {}", f.name);
            for line in f.message.lines() {
                let _ = writeln!(out, "    {line}");
            }
            if let Some(loc) = &f.location {
                let _ = writeln!(out, "    at {loc}");
            }
            out.push('\n');
        }
        if s.failed as usize > s.failures.len() {
            let _ = writeln!(
                out,
                "(showing {} of {} failures; full output truncated)",
                s.failures.len(),
                s.failed
            );
        }
    }

    if !s.raw_tail.is_empty() && (s.failed > 0 || s.parse_warning.is_some()) {
        out.push_str("\nRaw tail:\n");
        out.push_str(&s.raw_tail);
        if !out.ends_with('\n') {
            out.push('\n');
        }
    }

    out
}

/// Detected runner — picks the parser to use.
enum Runner {
    Cargo,
    Npm,
    Pytest,
    Go,
    Generic,
}

impl Runner {
    fn detect(command: &str) -> Self {
        let lower = command.to_ascii_lowercase();
        if lower.contains("cargo test") {
            Self::Cargo
        } else if lower.contains("npm test")
            || lower.contains("yarn test")
            || lower.contains("pnpm test")
            || lower.contains("npm run test")
            || lower.contains(" jest")
            || lower.contains(" vitest")
            || lower.contains(" mocha")
        {
            Self::Npm
        } else if lower.contains("pytest") {
            Self::Pytest
        } else if lower.contains("go test") {
            Self::Go
        } else {
            Self::Generic
        }
    }
}

/// Parsed `test` tool input.
struct ToolInvocation {
    /// User-supplied filter (positional first line that isn't a flag) —
    /// passed through the runner's documented filter flag.
    filter: Option<String>,
    /// `--cmd <command>` override — runs verbatim.
    cmd_override: Option<String>,
    /// `--watch` flag (not implemented in v1).
    watch: bool,
}

impl ToolInvocation {
    fn parse(input: &str) -> Result<Self, String> {
        let mut filter: Option<String> = None;
        let mut cmd_override: Option<String> = None;
        let mut watch = false;

        let body = input.trim();
        if body.is_empty() {
            return Ok(Self {
                filter: None,
                cmd_override: None,
                watch: false,
            });
        }

        // `--cmd <command>` consumes the rest of the body so the model
        // can pass a multi-word command without quoting concerns.
        if let Some(rest) = body.strip_prefix("--cmd ") {
            return Ok(Self {
                filter: None,
                cmd_override: Some(rest.trim().to_string()),
                watch: false,
            });
        }

        for raw in body.lines() {
            let line = raw.trim();
            if line.is_empty() {
                continue;
            }
            if let Some(rest) = line.strip_prefix("--cmd ") {
                cmd_override = Some(rest.trim().to_string());
                continue;
            }
            if line == "--watch" {
                watch = true;
                continue;
            }
            if filter.is_none() {
                filter = Some(line.to_string());
            } else {
                return Err(format!(
                    "Error: unexpected extra argument '{line}'. The `test` tool accepts at most one filter line; use `--cmd <command>` to pass a multi-token command."
                ));
            }
        }

        Ok(Self {
            filter,
            cmd_override,
            watch,
        })
    }

    fn resolve_command(&self, working_dir: &std::path::Path) -> Result<String, String> {
        if self.watch {
            return Err(
                "Error: --watch is not implemented in this version of the `test` tool.".to_string(),
            );
        }
        if let Some(cmd) = &self.cmd_override {
            return Ok(cmd.clone());
        }

        let filter = self
            .filter
            .clone()
            .or_else(crate::config::coding_test_filter_default);

        let detected = crate::coding::detect_test_cmd(working_dir).ok_or_else(|| {
            "Error: no test command detected for this project. Set `AICTL_CODING_TEST_CMD` in `~/.aictl/config` or pass `--cmd <command>` to override. Detected language markers checked: Cargo.toml, package.json, pyproject.toml/pytest.ini, go.mod, build.gradle, pom.xml, CMakeLists.txt, Makefile."
                .to_string()
        })?;

        let Some(filter) = filter else {
            return Ok(detected);
        };

        Ok(apply_filter(&detected, &filter))
    }
}

fn apply_filter(base: &str, filter: &str) -> String {
    let trimmed = base.trim();
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("cargo test") {
        return format!("{trimmed} {filter}");
    }
    if lower.starts_with("npm test")
        || lower.starts_with("yarn test")
        || lower.starts_with("pnpm test")
        || lower.starts_with("npm run test")
    {
        // npm convention: --filter args go after `--`.
        if trimmed.contains(" -- ") {
            return format!("{trimmed} {filter}");
        }
        return format!("{trimmed} -- {filter}");
    }
    if lower.starts_with("pytest") {
        return format!("{trimmed} -k {filter}");
    }
    if lower.starts_with("go test") {
        return format!("{trimmed} -run {filter}");
    }
    if lower.starts_with("./gradlew") || lower.starts_with("gradle ") {
        return format!("{trimmed} --tests {filter}");
    }
    if lower.starts_with("./mvnw") || lower.starts_with("mvn ") {
        return format!("{trimmed} -Dtest={filter}");
    }
    if lower.starts_with("ctest") {
        return format!("{trimmed} -R {filter}");
    }
    if lower.starts_with("make ") || lower == "make test" || lower == "make check" {
        // No standard filter mechanism for `make` test targets — append
        // as a positional and let the Makefile decide.
        return format!("{trimmed} {filter}");
    }
    // Unknown runner — append the filter verbatim; the user can use
    // --cmd to be explicit if this guess is wrong.
    format!("{trimmed} {filter}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_empty_body() {
        let inv = ToolInvocation::parse("").unwrap();
        assert!(inv.filter.is_none());
        assert!(inv.cmd_override.is_none());
        assert!(!inv.watch);
    }

    #[test]
    fn parse_filter_only() {
        let inv = ToolInvocation::parse("auth::login").unwrap();
        assert_eq!(inv.filter.as_deref(), Some("auth::login"));
    }

    #[test]
    fn parse_cmd_override() {
        let inv = ToolInvocation::parse("--cmd cargo test -p aictl-core").unwrap();
        assert_eq!(
            inv.cmd_override.as_deref(),
            Some("cargo test -p aictl-core")
        );
    }

    #[test]
    fn parse_watch_rejected_at_resolve() {
        let inv = ToolInvocation {
            filter: None,
            cmd_override: None,
            watch: true,
        };
        let err = inv.resolve_command(std::path::Path::new("/")).unwrap_err();
        assert!(err.contains("--watch"));
    }

    #[test]
    fn apply_filter_cargo() {
        assert_eq!(apply_filter("cargo test", "auth"), "cargo test auth");
    }

    #[test]
    fn apply_filter_npm_adds_double_dash() {
        assert_eq!(apply_filter("npm test", "Login"), "npm test -- Login");
    }

    #[test]
    fn apply_filter_npm_keeps_existing_double_dash() {
        assert_eq!(
            apply_filter("npm test -- --reporter=spec", "Login"),
            "npm test -- --reporter=spec Login"
        );
    }

    #[test]
    fn apply_filter_pytest() {
        assert_eq!(apply_filter("pytest", "test_login"), "pytest -k test_login");
    }

    #[test]
    fn apply_filter_go_test() {
        assert_eq!(
            apply_filter("go test ./...", "TestLogin"),
            "go test ./... -run TestLogin"
        );
    }

    #[test]
    fn apply_filter_gradle() {
        assert_eq!(
            apply_filter("./gradlew test", "com.foo.LoginTest"),
            "./gradlew test --tests com.foo.LoginTest"
        );
    }

    #[test]
    fn apply_filter_maven() {
        assert_eq!(
            apply_filter("./mvnw test", "LoginTest"),
            "./mvnw test -Dtest=LoginTest"
        );
    }

    #[test]
    fn runner_detect_cargo() {
        assert!(matches!(
            Runner::detect("cargo test --color=never"),
            Runner::Cargo
        ));
    }

    #[test]
    fn runner_detect_npm() {
        assert!(matches!(Runner::detect("npm test -- --watch"), Runner::Npm));
    }

    #[test]
    fn runner_detect_pytest() {
        assert!(matches!(Runner::detect("pytest -k login"), Runner::Pytest));
    }

    #[test]
    fn runner_detect_go() {
        assert!(matches!(Runner::detect("go test ./..."), Runner::Go));
    }

    #[test]
    fn runner_detect_generic() {
        assert!(matches!(Runner::detect("./gradlew test"), Runner::Generic));
    }

    #[test]
    fn build_raw_tail_under_cap_returns_full() {
        let tail = build_raw_tail("hello", "");
        assert_eq!(tail, "hello");
    }

    #[test]
    fn build_raw_tail_appends_stderr() {
        let tail = build_raw_tail("ok\n", "err\n");
        assert!(tail.contains("ok"));
        assert!(tail.contains("[stderr]"));
        assert!(tail.contains("err"));
    }

    #[test]
    fn build_raw_tail_truncates_to_cap() {
        let huge = "a".repeat(RAW_TAIL_BYTES * 2);
        let tail = build_raw_tail(&huge, "");
        assert!(tail.starts_with("…"));
        assert!(tail.len() <= RAW_TAIL_BYTES + 16);
    }

    #[test]
    fn render_body_shape_no_failures() {
        let s = TestSummary {
            command: "cargo test".to_string(),
            exit_code: 0,
            duration_ms: 1500,
            passed: 5,
            failed: 0,
            skipped: 0,
            ..Default::default()
        };
        let body = render_body(&s);
        assert!(body.contains("Command: cargo test"));
        assert!(body.contains("Passed:  5"));
        assert!(body.contains("(no failures)"));
    }
}
