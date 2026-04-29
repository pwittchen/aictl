//! Read from or write to the system clipboard.
//!
//! Useful for staging results for the user without touching the filesystem —
//! the agent can hand a command, a rewritten block of text, or a generated
//! snippet straight to the user's clipboard so they can paste it into their
//! editor of choice.
//!
//! Input format:
//!
//! ```text
//! read
//! ```
//!
//! reads the clipboard contents. The word `read` on the first line is
//! optional — an empty input is treated as a read.
//!
//! ```text
//! write
//! <content line 1>
//! <content line 2>
//! ...
//! ```
//!
//! writes everything after the first newline to the clipboard.
//!
//! # Platform support
//!
//! The tool shells out to the platform's standard clipboard helper rather
//! than linking a GUI crate: on macOS it uses `pbcopy` / `pbpaste`; on Linux
//! it prefers Wayland (`wl-copy` / `wl-paste`) and falls back to X11
//! (`xclip -selection clipboard` or `xsel --clipboard`). If no helper is
//! installed the tool returns a clear error naming the binaries it tried.
//!
//! Content is piped directly on stdin with no shell interpolation, so
//! arbitrary bytes (including quotes, backticks, and newlines) round-trip
//! safely. The subprocess inherits the shared scrubbed environment and
//! shell timeout used by every other spawning tool.
//!
//! # Caveats
//!
//! On Linux X11 the clipboard ordinarily lives in the process that set it —
//! `xclip` forks itself into the background so the value survives after
//! the tool returns. This works for our purposes but means the user may see
//! a brief background process; `xsel --clipboard --input` behaves the same.
//! On Wayland `wl-copy` forks into the background too. We don't try to
//! persist content beyond the process lifetime on platforms without a
//! native clipboard daemon.

use std::process::Stdio;

use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use super::util::truncate_output;

const MAX_WRITE_BYTES: usize = 1_048_576; // 1 MB — same ceiling as write_file default

#[cfg_attr(test, derive(Debug))]
enum Op<'a> {
    Read,
    Write(&'a str),
}

pub(super) async fn tool_clipboard(input: &str) -> String {
    let op = match parse_input(input) {
        Ok(op) => op,
        Err(e) => return e,
    };
    match op {
        Op::Read => read_clipboard().await,
        Op::Write(content) => write_clipboard(content).await,
    }
}

fn parse_input(input: &str) -> Result<Op<'_>, String> {
    let trimmed = input.trim_start_matches(['\r', '\n']);
    let trimmed = trimmed.trim_end();
    if trimmed.is_empty() {
        return Ok(Op::Read);
    }

    let (first, rest) = match trimmed.split_once('\n') {
        Some((a, b)) => (a.trim(), b),
        None => (trimmed.trim(), ""),
    };

    match first.to_ascii_lowercase().as_str() {
        "read" | "paste" | "get" => {
            if !rest.trim().is_empty() {
                return Err(
                    "Invalid input: `read` takes no content — put content on subsequent lines only for `write`".to_string(),
                );
            }
            Ok(Op::Read)
        }
        "write" | "copy" | "set" => Ok(Op::Write(rest)),
        _ => Err(format!(
            "Invalid input: expected `read` or `write` on the first line, got `{first}`"
        )),
    }
}

// --- Backend probing ---

struct Backend {
    /// Binary name
    bin: &'static str,
    /// Args for the operation
    args: &'static [&'static str],
    /// Human-readable label
    label: &'static str,
}

fn read_backends() -> &'static [Backend] {
    if cfg!(target_os = "macos") {
        &[Backend {
            bin: "pbpaste",
            args: &[],
            label: "pbpaste",
        }]
    } else if cfg!(target_os = "linux") {
        &[
            Backend {
                bin: "wl-paste",
                args: &["--no-newline"],
                label: "wl-paste",
            },
            Backend {
                bin: "xclip",
                args: &["-selection", "clipboard", "-o"],
                label: "xclip",
            },
            Backend {
                bin: "xsel",
                args: &["--clipboard", "--output"],
                label: "xsel",
            },
        ]
    } else {
        &[]
    }
}

fn write_backends() -> &'static [Backend] {
    if cfg!(target_os = "macos") {
        &[Backend {
            bin: "pbcopy",
            args: &[],
            label: "pbcopy",
        }]
    } else if cfg!(target_os = "linux") {
        &[
            Backend {
                bin: "wl-copy",
                args: &[],
                label: "wl-copy",
            },
            Backend {
                bin: "xclip",
                args: &["-selection", "clipboard"],
                label: "xclip",
            },
            Backend {
                bin: "xsel",
                args: &["--clipboard", "--input"],
                label: "xsel",
            },
        ]
    } else {
        &[]
    }
}

async fn resolve_backend(backends: &'static [Backend]) -> Option<&'static Backend> {
    for b in backends {
        if which(b.bin).await {
            return Some(b);
        }
    }
    None
}

async fn which(bin: &str) -> bool {
    // `command -v` is POSIX-portable; avoid linking a `which` crate for this.
    let out = Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {bin}"))
        .env_clear()
        .envs(crate::security::scrubbed_env())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await;
    matches!(out, Ok(s) if s.success())
}

fn no_backend_error(kind: &str, backends: &'static [Backend]) -> String {
    if backends.is_empty() {
        format!(
            "Error: no clipboard {kind} backend available for this platform (supports macOS and Linux)"
        )
    } else {
        let names: Vec<&str> = backends.iter().map(|b| b.bin).collect();
        format!(
            "Error: no clipboard {kind} backend found on PATH — install one of: {}",
            names.join(", ")
        )
    }
}

// --- Read ---

async fn read_clipboard() -> String {
    let backends = read_backends();
    let Some(backend) = resolve_backend(backends).await else {
        return no_backend_error("read", backends);
    };

    let mut cmd = Command::new(backend.bin);
    cmd.args(backend.args);
    cmd.env_clear();
    for (k, v) in crate::security::scrubbed_env() {
        cmd.env(k, v);
    }
    cmd.stdin(Stdio::null());
    cmd.kill_on_drop(true);

    let future = cmd.output();
    let output = if let Some(timeout) = crate::security::shell_timeout() {
        match tokio::time::timeout(timeout, future).await {
            Ok(r) => r,
            Err(_) => {
                return format!(
                    "Error: clipboard read ({label}) timed out after {secs}s",
                    label = backend.label,
                    secs = timeout.as_secs()
                );
            }
        }
    } else {
        future.await
    };

    match output {
        Ok(out) => {
            if !out.status.success() {
                let stderr = String::from_utf8_lossy(&out.stderr);
                // xclip/xsel return non-zero when the clipboard is empty on
                // some systems — surface an empty-clipboard note instead of
                // an opaque "[exit N]" in that common case.
                if out.stdout.is_empty() && stderr.trim().is_empty() {
                    return "(clipboard is empty)".to_string();
                }
                let code = out.status.code().unwrap_or(-1);
                return format!(
                    "Error: clipboard read ({label}) exited [exit {code}]: {stderr}",
                    label = backend.label
                );
            }
            let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
            if text.is_empty() {
                return "(clipboard is empty)".to_string();
            }
            truncate_output(&mut text);
            text
        }
        Err(e) => format!(
            "Error: failed to spawn clipboard helper `{label}`: {e}",
            label = backend.label
        ),
    }
}

// --- Write ---

async fn write_clipboard(content: &str) -> String {
    if content.len() > MAX_WRITE_BYTES {
        return format!(
            "Error: content size {} bytes exceeds clipboard write limit of {MAX_WRITE_BYTES} bytes",
            content.len()
        );
    }

    let backends = write_backends();
    let Some(backend) = resolve_backend(backends).await else {
        return no_backend_error("write", backends);
    };

    let mut cmd = Command::new(backend.bin);
    cmd.args(backend.args);
    cmd.env_clear();
    for (k, v) in crate::security::scrubbed_env() {
        cmd.env(k, v);
    }
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::piped());
    cmd.kill_on_drop(true);

    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return format!(
                "Error: failed to spawn clipboard helper `{label}`: {e}",
                label = backend.label
            );
        }
    };

    let label = backend.label;
    let wrote_bytes = content.len();
    let future = async move {
        let mut child = child;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(content.as_bytes()).await?;
            stdin.shutdown().await?;
        }
        child.wait_with_output().await
    };

    let result = if let Some(timeout) = crate::security::shell_timeout() {
        match tokio::time::timeout(timeout, future).await {
            Ok(r) => r,
            Err(_) => {
                return format!(
                    "Error: clipboard write ({label}) timed out after {secs}s",
                    secs = timeout.as_secs()
                );
            }
        }
    } else {
        future.await
    };

    match result {
        Ok(out) => {
            if !out.status.success() {
                let stderr = String::from_utf8_lossy(&out.stderr);
                let code = out.status.code().unwrap_or(-1);
                return format!("Error: clipboard write ({label}) exited [exit {code}]: {stderr}");
            }
            format!("Copied {wrote_bytes} bytes to clipboard ({label})")
        }
        Err(e) => format!("Error: clipboard write ({label}) failed: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- parsing ---

    #[test]
    fn parse_empty_is_read() {
        assert!(matches!(parse_input("").unwrap(), Op::Read));
        assert!(matches!(parse_input("   ").unwrap(), Op::Read));
        assert!(matches!(parse_input("\n\n").unwrap(), Op::Read));
    }

    #[test]
    fn parse_read_keyword() {
        assert!(matches!(parse_input("read").unwrap(), Op::Read));
        assert!(matches!(parse_input("READ").unwrap(), Op::Read));
        assert!(matches!(parse_input("paste").unwrap(), Op::Read));
        assert!(matches!(parse_input("get").unwrap(), Op::Read));
    }

    #[test]
    fn parse_read_rejects_content() {
        let err = parse_input("read\nextra data").unwrap_err();
        assert!(err.contains("Invalid input"));
    }

    #[test]
    fn parse_write_with_content() {
        match parse_input("write\nhello world").unwrap() {
            Op::Write(c) => assert_eq!(c, "hello world"),
            Op::Read => panic!("expected write"),
        }
    }

    #[test]
    fn parse_write_preserves_multiline() {
        // Trailing whitespace is stripped for parity with `write_file`;
        // interior newlines are preserved so multi-line content round-trips.
        match parse_input("write\nline one\nline two").unwrap() {
            Op::Write(c) => assert_eq!(c, "line one\nline two"),
            Op::Read => panic!("expected write"),
        }
    }

    #[test]
    fn parse_write_empty_content() {
        // `write` with no content → empty string, not an error. The
        // platform helper is free to clear the clipboard.
        match parse_input("write").unwrap() {
            Op::Write(c) => assert_eq!(c, ""),
            Op::Read => panic!("expected write"),
        }
    }

    #[test]
    fn parse_copy_alias() {
        match parse_input("copy\ndata").unwrap() {
            Op::Write(c) => assert_eq!(c, "data"),
            Op::Read => panic!("expected write"),
        }
    }

    #[test]
    fn parse_unknown_first_line_rejected() {
        let err = parse_input("bogus\nhello").unwrap_err();
        assert!(err.contains("expected `read` or `write`"));
    }

    // --- round-trip (only runs when a clipboard backend is available) ---
    //
    // These tests are opt-in via AICTL_TEST_CLIPBOARD=1 because CI and
    // headless runs typically have no clipboard daemon — running them
    // blindly would show flaky failures that don't reflect a code defect.
    // Local runs with `AICTL_TEST_CLIPBOARD=1 cargo test clipboard` exercise
    // the full subprocess path on a real system clipboard.

    fn should_run_live() -> bool {
        std::env::var("AICTL_TEST_CLIPBOARD").ok().as_deref() == Some("1")
    }

    #[tokio::test]
    async fn live_write_then_read_roundtrip() {
        if !should_run_live() {
            return;
        }
        // Only one live test — multiple would race on the shared system
        // clipboard since cargo runs async tests on a shared runtime. The
        // parse-layer cases (`parse_empty_is_read`, alias handling) are
        // already covered by the pure-function tests above.
        let payload = format!("aictl-clipboard-test-{}", std::process::id());
        let write_out = tool_clipboard(&format!("write\n{payload}")).await;
        assert!(write_out.starts_with("Copied"), "write result: {write_out}");
        let read_out = tool_clipboard("read").await;
        assert!(read_out.contains(&payload), "read result: {read_out}");
        // Verify empty input also triggers a read that sees the same content.
        let empty_read = tool_clipboard("").await;
        assert!(empty_read.contains(&payload), "empty-read: {empty_read}");
    }

    // --- oversized write is rejected without a subprocess spawn ---

    #[tokio::test]
    async fn write_rejects_over_limit_content() {
        let big = "x".repeat(MAX_WRITE_BYTES + 1);
        let out = tool_clipboard(&format!("write\n{big}")).await;
        assert!(out.contains("exceeds clipboard write limit"), "got: {out}");
    }
}
