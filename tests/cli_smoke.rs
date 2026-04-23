//! End-to-end CLI smoke tests that drive the real `aictl` binary.
//!
//! These complement the in-crate unit / integration tests under `src/`. The
//! unit tests exercise individual functions; these spawn the compiled binary
//! (via `env!("CARGO_BIN_EXE_aictl")`) and assert on stdout / stderr / exit
//! codes so regressions in the CLI surface itself (argument parsing,
//! single-shot vs REPL dispatch, exit codes, stdout framing) are caught.
//!
//! LLM traffic is routed to the scripted `Provider::Mock` via the hidden
//! `--mock` flag. Responses are supplied through a temp file pointed at by
//! `AICTL_MOCK_RESPONSES_FILE` (see `src/llm/mock.rs`). Each test also
//! overrides `HOME` to an isolated tempdir so the user's real
//! `~/.aictl/config`, sessions, stats, and audit log are never touched.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Path to the compiled binary, injected by Cargo for integration tests.
fn bin_path() -> &'static str {
    env!("CARGO_BIN_EXE_aictl")
}

/// Fresh HOME under a tempdir, pre-seeded with a valid `~/.aictl/version`
/// cache so startup never reaches the network. Sessions, audit logs, and
/// config writes all land inside the tempdir and are cleaned up on drop.
struct TestHome {
    dir: tempfile::TempDir,
}

impl TestHome {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("create tempdir");
        let aictl_dir = dir.path().join(".aictl");
        std::fs::create_dir_all(&aictl_dir).expect("create .aictl");
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time")
            .as_secs();
        let cache = format!(r#"{{"version":"0.0.0-test","checked_at":{now}}}"#);
        std::fs::write(aictl_dir.join("version"), cache).expect("seed version cache");
        Self { dir }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn sessions_dir(&self) -> PathBuf {
        self.dir.path().join(".aictl").join("sessions")
    }
}

/// Write scripted mock responses to a temp file in the given home's tempdir.
/// Responses are separated by a `---` line per the mock file format.
fn write_mock_responses(home: &TestHome, responses: &[&str]) -> PathBuf {
    let path = home.path().join("mock-responses.txt");
    let mut f = std::fs::File::create(&path).expect("create mock file");
    for (i, r) in responses.iter().enumerate() {
        if i > 0 {
            writeln!(f, "---").expect("write separator");
        }
        writeln!(f, "{r}").expect("write response");
    }
    path
}

/// Base `Command` with the isolated HOME, mock response file, and color
/// suppression. Caller appends CLI args and stdio as needed.
fn base_cmd(home: &TestHome, responses_file: &Path) -> Command {
    let mut cmd = Command::new(bin_path());
    cmd.env("HOME", home.path())
        .env("AICTL_MOCK_RESPONSES_FILE", responses_file)
        // Suppress crossterm/termimad ANSI codes so substring assertions are
        // resilient across platforms. Both libraries honor NO_COLOR.
        .env("NO_COLOR", "1")
        // Disable the blanket `$PROJECT/AICTL.md` / `CLAUDE.md` fallback so
        // the test isn't sensitive to whichever CWD cargo launches from.
        .env("AICTL_PROMPT_FALLBACK", "false");
    cmd
}

fn run(mut cmd: Command) -> Output {
    let out = cmd.output().expect("spawn aictl");
    Output {
        status: out.status.code(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

struct Output {
    status: Option<i32>,
    stdout: String,
    stderr: String,
}

// --- Flag parsing surface ---

#[test]
fn help_flag_prints_usage_and_exits_zero() {
    let home = TestHome::new();
    let responses = write_mock_responses(&home, &[]);
    let mut cmd = base_cmd(&home, &responses);
    cmd.arg("--help");
    let out = run(cmd);
    assert_eq!(out.status, Some(0), "stderr: {}", out.stderr);
    assert!(
        out.stdout.contains("AI agent"),
        "help text missing description: stdout={}",
        out.stdout
    );
    // Hidden flags should not appear in --help output.
    assert!(
        !out.stdout.contains("--mock"),
        "--mock must stay hidden from user-facing help"
    );
}

#[test]
fn version_flag_prints_version_and_exits_zero() {
    let home = TestHome::new();
    let responses = write_mock_responses(&home, &[]);
    let mut cmd = base_cmd(&home, &responses);
    cmd.arg("--version");
    let out = run(cmd);
    assert_eq!(out.status, Some(0), "stderr: {}", out.stderr);
    // The `aictl <version>` prefix is the stable contract.
    assert!(
        out.stdout.starts_with("aictl "),
        "unexpected version line: {:?}",
        out.stdout
    );
}

#[test]
fn missing_provider_surfaces_error_and_exits_nonzero() {
    // No --mock, no config, no --provider → resolve_provider aborts.
    let home = TestHome::new();
    let responses = write_mock_responses(&home, &[]);
    let mut cmd = base_cmd(&home, &responses);
    cmd.args(["--message", "hi"]);
    let out = run(cmd);
    assert_eq!(out.status, Some(1), "stdout: {}", out.stdout);
    assert!(
        out.stderr.contains("no provider specified"),
        "stderr missing provider hint: {}",
        out.stderr
    );
}

// --- Single-shot mode (main.rs) ---

#[test]
fn single_shot_mock_emits_final_answer_on_stdout() {
    let home = TestHome::new();
    let responses = write_mock_responses(&home, &["The answer is 42."]);
    let mut cmd = base_cmd(&home, &responses);
    cmd.args(["--mock", "--message", "what is the answer?"]);
    let out = run(cmd);
    assert_eq!(out.status, Some(0), "stderr: {}", out.stderr);
    assert!(
        out.stdout.contains("The answer is 42."),
        "answer missing from stdout: {:?}",
        out.stdout
    );
}

#[test]
fn single_shot_tool_call_dispatch_produces_final_answer() {
    // First scripted response triggers `calculate`; second is the final answer.
    // `--auto` skips tool confirmation so the loop can run non-interactively.
    let home = TestHome::new();
    let responses = write_mock_responses(
        &home,
        &[
            r#"<tool name="calculate">111 + 222</tool>"#,
            "Done: the sum is 333.",
        ],
    );
    let mut cmd = base_cmd(&home, &responses);
    cmd.args(["--mock", "--auto", "--message", "add 111 + 222"]);
    let out = run(cmd);
    assert_eq!(out.status, Some(0), "stderr: {}", out.stderr);
    assert!(
        out.stdout.contains("Done: the sum is 333."),
        "final answer missing: stdout={:?} stderr={:?}",
        out.stdout,
        out.stderr
    );
}

#[test]
fn quiet_flag_requires_auto_and_keeps_final_answer() {
    // `--quiet` implies `--auto` per clap wiring. It suppresses tool/reasoning
    // chatter but must still print the final answer to stdout.
    let home = TestHome::new();
    let responses = write_mock_responses(&home, &["quiet final answer"]);
    let mut cmd = base_cmd(&home, &responses);
    cmd.args(["--mock", "--auto", "--quiet", "--message", "anything"]);
    let out = run(cmd);
    assert_eq!(out.status, Some(0), "stderr: {}", out.stderr);
    assert!(
        out.stdout.contains("quiet final answer"),
        "quiet mode dropped the final answer: {:?}",
        out.stdout
    );
}

#[test]
fn quiet_without_auto_rejects_at_clap_level() {
    // `#[arg(long, requires = "auto")]` on `quiet` means clap must refuse
    // `--quiet` without `--auto` before our code ever runs. That's the
    // structural guarantee the rest of the single-shot path relies on.
    let home = TestHome::new();
    let responses = write_mock_responses(&home, &["unused"]);
    let mut cmd = base_cmd(&home, &responses);
    cmd.args(["--mock", "--quiet", "--message", "hi"]);
    let out = run(cmd);
    assert_ne!(out.status, Some(0));
    let combined = format!("{}{}", out.stdout, out.stderr);
    assert!(
        combined.contains("--auto") || combined.to_lowercase().contains("required"),
        "clap should complain about missing --auto: stderr={} stdout={}",
        out.stderr,
        out.stdout
    );
}

#[test]
fn unrestricted_flag_emits_warning_on_stderr() {
    let home = TestHome::new();
    let responses = write_mock_responses(&home, &["ok"]);
    let mut cmd = base_cmd(&home, &responses);
    cmd.args(["--mock", "--unrestricted", "--auto", "--message", "hi"]);
    let out = run(cmd);
    assert_eq!(out.status, Some(0), "stderr: {}", out.stderr);
    assert!(
        out.stderr.contains("security restrictions disabled"),
        "unrestricted warning missing: stderr={:?}",
        out.stderr
    );
    // And the normal final answer still flows through.
    assert!(out.stdout.contains("ok"), "stdout={:?}", out.stdout);
}

// --- Session persistence (session.rs wiring) ---

#[test]
fn single_shot_does_not_persist_sessions() {
    // Single-shot mode intentionally skips session bookkeeping — only the REPL
    // path wires `session::set_current` + `save_current`. A single `--message`
    // run therefore leaves no artifacts under `~/.aictl/sessions/`.
    let home = TestHome::new();
    let responses = write_mock_responses(&home, &["single-shot answer"]);
    let mut cmd = base_cmd(&home, &responses);
    cmd.args(["--mock", "--message", "one-off"]);
    let out = run(cmd);
    assert_eq!(out.status, Some(0), "stderr: {}", out.stderr);
    let had_session = std::fs::read_dir(home.sessions_dir())
        .map(|rd| {
            rd.flatten()
                .any(|e| !e.file_name().to_string_lossy().starts_with('.'))
        })
        .unwrap_or(false);
    assert!(
        !had_session,
        "single-shot mode must not create session files"
    );
}

#[test]
fn repl_persists_session_across_invocations() {
    // First REPL invocation drives one turn through the mock, then /exit.
    // The on-disk session file must survive and be listable from a second
    // invocation via --list-sessions.
    let home = TestHome::new();
    let responses = write_mock_responses(&home, &["reply one"]);
    let first = run_repl_with_input(&home, &responses, "first turn\n/exit\n");
    assert_eq!(first.status, Some(0), "stderr: {}", first.stderr);

    let entries: Vec<_> = std::fs::read_dir(home.sessions_dir())
        .expect("sessions dir must exist after REPL run")
        .flatten()
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .collect();
    assert_eq!(
        entries.len(),
        1,
        "expected exactly one session file, found {}",
        entries.len()
    );
    let uuid = entries[0].file_name().to_string_lossy().into_owned();

    // Second invocation: --list-sessions must surface the saved UUID.
    let responses2 = write_mock_responses(&home, &[]);
    let mut cmd = base_cmd(&home, &responses2);
    cmd.args(["--mock", "--list-sessions"]);
    let out = run(cmd);
    assert_eq!(out.status, Some(0), "stderr: {}", out.stderr);
    assert!(
        out.stdout.contains(&uuid),
        "session UUID {uuid} missing from --list-sessions: {:?}",
        out.stdout
    );
}

#[test]
fn incognito_flag_suppresses_session_file_creation() {
    // Incognito guarantees no persistence even through the REPL path.
    let home = TestHome::new();
    let responses = write_mock_responses(&home, &["hello"]);
    let mut cmd = base_cmd(&home, &responses);
    cmd.arg("--mock")
        .arg("--incognito")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("spawn repl");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(b"hi\n/exit\n")
        .expect("write stdin");
    let out = child.wait_with_output().expect("wait");
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let has_session = std::fs::read_dir(home.sessions_dir())
        .map(|rd| {
            rd.flatten()
                .any(|e| !e.file_name().to_string_lossy().starts_with('.'))
        })
        .unwrap_or(false);
    assert!(
        !has_session,
        "--incognito must not persist session files under {}",
        home.sessions_dir().display()
    );
}

// --- REPL dispatch (repl.rs) via piped stdin ---
//
// rustyline falls back to line-oriented stdin reads when the terminal is not
// a TTY, so `Command::stdin(Stdio::piped())` + writing lines drives the REPL
// loop end-to-end. Each of these tests feeds a minimal script and asserts on
// the deterministic stdout emitted by the corresponding command handler.

fn run_repl_with_input(home: &TestHome, responses_file: &Path, stdin_script: &str) -> Output {
    let mut cmd = base_cmd(home, responses_file);
    cmd.arg("--mock")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("spawn repl");
    {
        let stdin = child.stdin.as_mut().expect("stdin pipe");
        stdin
            .write_all(stdin_script.as_bytes())
            .expect("write stdin");
    }
    let out = child.wait_with_output().expect("wait repl");
    Output {
        status: out.status.code(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

#[test]
fn repl_help_slash_command_prints_commands_table() {
    let home = TestHome::new();
    let responses = write_mock_responses(&home, &[]);
    let out = run_repl_with_input(&home, &responses, "/help\n/exit\n");
    assert_eq!(out.status, Some(0), "stderr: {}", out.stderr);
    // /help prints the commands table to stdout via println!.
    // The description text for `/help` itself is the stable anchor.
    assert!(
        out.stdout.contains("show this help message"),
        "help table missing from stdout: {:?}",
        out.stdout
    );
}

#[test]
fn repl_exits_cleanly_on_slash_exit() {
    let home = TestHome::new();
    let responses = write_mock_responses(&home, &[]);
    let out = run_repl_with_input(&home, &responses, "/exit\n");
    assert_eq!(out.status, Some(0), "stderr: {}", out.stderr);
    // Exit path prints the session-saved banner + resume hint.
    assert!(
        out.stdout.contains("session saved") || out.stdout.contains("resume with"),
        "exit banner missing: stdout={:?}",
        out.stdout
    );
}

#[test]
fn repl_exits_cleanly_on_eof() {
    // Closing stdin without sending `/exit` must still exit 0 (Ctrl+D path).
    let home = TestHome::new();
    let responses = write_mock_responses(&home, &[]);
    let out = run_repl_with_input(&home, &responses, "");
    assert_eq!(out.status, Some(0), "stderr: {}", out.stderr);
}

#[test]
fn repl_clear_command_confirms_reset() {
    let home = TestHome::new();
    let responses = write_mock_responses(&home, &[]);
    let out = run_repl_with_input(&home, &responses, "/clear\n/exit\n");
    assert_eq!(out.status, Some(0), "stderr: {}", out.stderr);
    assert!(
        out.stdout.contains("context cleared"),
        "/clear confirmation missing: stdout={:?}",
        out.stdout
    );
}

#[test]
fn repl_drives_agent_turn_through_mock_provider() {
    // REPL path: one user turn runs run_agent_turn which dispatches to
    // Provider::Mock. `InteractiveUI::show_answer` writes to stderr (so the
    // prompt + welcome + answer all share the same stream and the stdout pipe
    // stays reserved for commands that intentionally surface structured
    // output to consumers).
    let home = TestHome::new();
    let responses = write_mock_responses(&home, &["repl-mock-answer"]);
    let out = run_repl_with_input(&home, &responses, "hello\n/exit\n");
    assert_eq!(out.status, Some(0), "stderr: {}", out.stderr);
    assert!(
        out.stderr.contains("repl-mock-answer"),
        "mock answer missing from REPL stderr: {:?}",
        out.stderr
    );
}
