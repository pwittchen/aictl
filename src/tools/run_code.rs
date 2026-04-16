//! Execute a short code snippet in a chosen interpreter (Python, Node,
//! Ruby, etc.) and return combined stdout/stderr.
//!
//! The source is piped in on stdin so no temporary file is written. The
//! child process inherits the shared `scrubbed_env` (no secrets), gets
//! killed on the shared shell timeout, and runs in the security policy's
//! working directory. `kill_on_drop` is set so that if the outer timeout
//! future is dropped the interpreter is reaped instead of being orphaned.
//!
//! This is not a true sandbox — the interpreter can read files, make
//! network calls, and spawn subprocesses just like `exec_shell`. Treat it
//! as an ergonomic convenience for one-shot snippets, not as a boundary.
//! Users who want the capability gone entirely can add `run_code` to
//! `AICTL_SECURITY_DISABLED_TOOLS`.

use std::fmt::Write as _;
use std::process::Stdio;

use tokio::io::AsyncWriteExt;

use super::util::truncate_output;

struct Interpreter {
    /// Language names the model may use as the first line of input.
    aliases: &'static [&'static str],
    /// Binary name resolved via `PATH`.
    binary: &'static str,
    /// Args telling the interpreter to read source from stdin.
    args: &'static [&'static str],
}

const INTERPRETERS: &[Interpreter] = &[
    Interpreter {
        aliases: &["python", "python3", "py"],
        binary: "python3",
        args: &["-"],
    },
    Interpreter {
        aliases: &["node", "nodejs", "javascript", "js"],
        binary: "node",
        args: &["-"],
    },
    Interpreter {
        aliases: &["ruby", "rb"],
        binary: "ruby",
        args: &["-"],
    },
    Interpreter {
        aliases: &["perl"],
        binary: "perl",
        args: &[],
    },
    Interpreter {
        aliases: &["lua"],
        binary: "lua",
        args: &["-"],
    },
    Interpreter {
        aliases: &["bash"],
        binary: "bash",
        args: &["-s"],
    },
    Interpreter {
        aliases: &["sh"],
        binary: "sh",
        args: &["-s"],
    },
];

pub(super) async fn tool_run_code(input: &str) -> String {
    let input = input.trim_start();
    if input.is_empty() {
        return "Error: empty input. Expected: <language>\\n<code>".to_string();
    }
    if input.contains('\0') {
        return "Error: input contains null byte".to_string();
    }

    let (lang, code) = match input.split_once('\n') {
        Some((l, c)) => (l.trim(), c),
        None => {
            return "Error: no code provided after language line. Expected: <language>\\n<code>"
                .to_string();
        }
    };
    if lang.is_empty() {
        return "Error: first line must be a language identifier (e.g. 'python', 'node')"
            .to_string();
    }
    let code = code.trim_end_matches('\n');
    if code.is_empty() {
        return "Error: no code provided after language line".to_string();
    }

    let Some(interp) = resolve_interpreter(lang) else {
        let supported: Vec<&str> = INTERPRETERS
            .iter()
            .flat_map(|i| i.aliases.iter().copied())
            .collect();
        return format!(
            "Error: language '{lang}' not supported. Available: {}",
            supported.join(", ")
        );
    };

    run(interp, code).await
}

fn resolve_interpreter(lang: &str) -> Option<&'static Interpreter> {
    let lang = lang.to_ascii_lowercase();
    INTERPRETERS
        .iter()
        .find(|i| i.aliases.iter().any(|a| *a == lang))
}

async fn run(interp: &Interpreter, code: &str) -> String {
    let mut cmd = tokio::process::Command::new(interp.binary);
    cmd.args(interp.args);
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    cmd.kill_on_drop(true);

    // Clean env: shared secret scrubber, same as exec_shell / git.
    cmd.env_clear();
    let env_pairs: Vec<(String, String)> = if crate::security::policy().enabled {
        crate::security::scrubbed_env()
    } else {
        std::env::vars().collect()
    };
    for (k, v) in env_pairs {
        cmd.env(k, v);
    }
    // Python prints immediately instead of buffering through a pipe.
    cmd.env("PYTHONUNBUFFERED", "1");
    cmd.env("PYTHONDONTWRITEBYTECODE", "1");
    cmd.env("NODE_NO_WARNINGS", "1");
    cmd.env("NO_COLOR", "1");

    cmd.current_dir(&crate::security::policy().paths.working_dir);

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return format!(
                "Error launching {}: {e}. Is the interpreter installed and on PATH?",
                interp.binary
            );
        }
    };

    if let Some(mut stdin) = child.stdin.take()
        && let Err(e) = stdin.write_all(code.as_bytes()).await
    {
        return format!("Error piping code to {}: {e}", interp.binary);
    }
    // Stdin drops here (end of the `if` scope), sending EOF so the
    // interpreter actually proceeds to execute the buffered source.

    let output_future = child.wait_with_output();
    let output = if let Some(timeout) = crate::security::shell_timeout() {
        match tokio::time::timeout(timeout, output_future).await {
            Ok(r) => r,
            Err(_) => {
                // Future drop triggers kill_on_drop — no zombie left behind.
                return format!(
                    "Error: code execution timed out after {}s",
                    timeout.as_secs()
                );
            }
        }
    } else {
        output_future.await
    };

    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            let mut result = String::new();
            if !stdout.is_empty() {
                result.push_str(&stdout);
            }
            if !stderr.is_empty() {
                if !result.is_empty() && !result.ends_with('\n') {
                    result.push('\n');
                }
                result.push_str("[stderr]\n");
                result.push_str(&stderr);
            }
            if !out.status.success() {
                if !result.is_empty() && !result.ends_with('\n') {
                    result.push('\n');
                }
                let _ = write!(result, "[exit {}]", out.status.code().unwrap_or(-1));
            }
            if result.is_empty() {
                result.push_str("(no output)");
            }
            truncate_output(&mut result);
            result
        }
        Err(e) => format!("Error waiting for {}: {e}", interp.binary),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn has_interpreter(bin: &str) -> bool {
        std::process::Command::new(bin)
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    #[test]
    fn resolve_python_aliases() {
        assert!(resolve_interpreter("python").is_some());
        assert!(resolve_interpreter("python3").is_some());
        assert!(resolve_interpreter("py").is_some());
        assert_eq!(resolve_interpreter("python").unwrap().binary, "python3");
    }

    #[test]
    fn resolve_node_aliases() {
        assert!(resolve_interpreter("node").is_some());
        assert!(resolve_interpreter("nodejs").is_some());
        assert!(resolve_interpreter("javascript").is_some());
        assert!(resolve_interpreter("js").is_some());
    }

    #[test]
    fn resolve_case_insensitive() {
        assert!(resolve_interpreter("Python").is_some());
        assert!(resolve_interpreter("NODE").is_some());
    }

    #[test]
    fn resolve_unknown_language() {
        assert!(resolve_interpreter("brainfuck").is_none());
        assert!(resolve_interpreter("").is_none());
    }

    #[tokio::test]
    async fn empty_input_rejected() {
        let r = tool_run_code("").await;
        assert!(r.contains("empty input"));
    }

    #[tokio::test]
    async fn null_byte_rejected() {
        let r = tool_run_code("python\nprint(1)\0").await;
        assert!(r.contains("null byte"));
    }

    #[tokio::test]
    async fn missing_code_rejected() {
        let r = tool_run_code("python\n").await;
        assert!(r.contains("no code"));
    }

    #[tokio::test]
    async fn missing_newline_rejected() {
        let r = tool_run_code("python").await;
        assert!(r.contains("no code"));
    }

    #[tokio::test]
    async fn unknown_language_rejected() {
        let r = tool_run_code("cobol\nDISPLAY 'hi'.").await;
        assert!(r.contains("not supported"));
        assert!(r.contains("python"));
    }

    #[tokio::test]
    async fn python_runs_hello() {
        if !has_interpreter("python3") {
            return;
        }
        let r = tool_run_code("python\nprint('hello from python')").await;
        assert!(r.contains("hello from python"), "got: {r}");
    }

    #[tokio::test]
    async fn python_stderr_captured() {
        if !has_interpreter("python3") {
            return;
        }
        let r = tool_run_code("python\nimport sys; sys.stderr.write('oops')").await;
        assert!(r.contains("[stderr]"), "got: {r}");
        assert!(r.contains("oops"), "got: {r}");
    }

    #[tokio::test]
    async fn python_nonzero_exit_reported() {
        if !has_interpreter("python3") {
            return;
        }
        let r = tool_run_code("python\nimport sys; sys.exit(7)").await;
        assert!(r.contains("[exit 7]"), "got: {r}");
    }

    #[tokio::test]
    async fn node_runs_hello() {
        if !has_interpreter("node") {
            return;
        }
        let r = tool_run_code("node\nconsole.log('hello from node')").await;
        assert!(r.contains("hello from node"), "got: {r}");
    }

    #[tokio::test]
    async fn bash_runs_echo() {
        if !has_interpreter("bash") {
            return;
        }
        let r = tool_run_code("bash\necho hello from bash").await;
        assert!(r.contains("hello from bash"), "got: {r}");
    }
}
