//! Send a desktop notification.
//!
//! Useful for long-running tasks in `--auto` mode — the agent can poke the
//! user when a build finishes, a deploy completes, or a multi-step plan
//! reaches a milestone, instead of relying on the user watching the terminal.
//!
//! Input format:
//!
//! ```text
//! <title>
//! <optional body line 1>
//! <optional body line 2>
//! ...
//! ```
//!
//! The first line is the notification title (required). Any remaining lines
//! are joined with `\n` to form the body. A title-only notification is
//! perfectly valid.
//!
//! # Platform support
//!
//! - macOS: `osascript` (bundled with the OS) running a small
//!   `display notification` `AppleScript`. The script source is piped on
//!   stdin, not embedded in `-e` args, so title and body are assigned to
//!   `AppleScript` variables via a templated preamble that escapes only
//!   backslashes and double quotes — no shell interpolation is possible.
//! - Linux: `notify-send` from libnotify. Title and body are passed as
//!   separate argv entries, so newlines and special characters round-trip
//!   safely without any shell involvement.
//!
//! If the required backend is missing the tool returns a clear error
//! naming the binary that was expected.
//!
//! # Caveats
//!
//! - macOS: the first notification from a given terminal emulator may
//!   require the user to grant notification permission in System Settings.
//!   This is a one-time prompt and is a Finder-level control — the tool
//!   can't bypass it, and neither can any other program.
//! - On headless Linux with no notification daemon running, `notify-send`
//!   exits non-zero; that error is surfaced verbatim.

use std::process::Stdio;

use tokio::io::AsyncWriteExt;
use tokio::process::Command;

const MAX_TITLE_BYTES: usize = 256;
const MAX_BODY_BYTES: usize = 4096;

#[cfg_attr(test, derive(Debug))]
struct Parsed<'a> {
    title: &'a str,
    body: String,
}

pub(super) async fn tool_notify(input: &str) -> String {
    let parsed = match parse_input(input) {
        Ok(p) => p,
        Err(e) => return e,
    };

    if cfg!(target_os = "macos") {
        notify_macos(parsed.title, &parsed.body).await
    } else if cfg!(target_os = "linux") {
        notify_linux(parsed.title, &parsed.body).await
    } else {
        "Error: desktop notifications are only supported on macOS and Linux".to_string()
    }
}

fn parse_input(input: &str) -> Result<Parsed<'_>, String> {
    let trimmed = input.trim_start_matches(['\r', '\n']).trim_end();
    if trimmed.is_empty() {
        return Err(
            "Invalid input: expected a title on the first line (optional body on subsequent lines)"
                .to_string(),
        );
    }

    let (title, rest) = match trimmed.split_once('\n') {
        Some((a, b)) => (a.trim_end_matches('\r'), b),
        None => (trimmed, ""),
    };
    let title = title.trim();
    if title.is_empty() {
        return Err("Invalid input: title cannot be empty".to_string());
    }
    if title.len() > MAX_TITLE_BYTES {
        return Err(format!(
            "Invalid input: title exceeds {MAX_TITLE_BYTES}-byte limit"
        ));
    }
    // Join body lines with literal `\n`, matching what both backends expect.
    let body = rest
        .lines()
        .collect::<Vec<_>>()
        .join("\n")
        .trim_end()
        .to_string();
    if body.len() > MAX_BODY_BYTES {
        return Err(format!(
            "Invalid input: body exceeds {MAX_BODY_BYTES}-byte limit"
        ));
    }

    Ok(Parsed { title, body })
}

// --- macOS backend ---

/// Escape a string so it can appear inside an `AppleScript` string literal
/// (the `"..."` on the right of a `set x to "..."` assignment). `AppleScript`
/// only recognizes `\\` and `\"` inside quoted strings; everything else is
/// literal, including control characters.
fn escape_applescript(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            // `AppleScript` quoted strings accept embedded newlines, but the
            // `display notification` action renders them as a single space.
            // Keep the newline as-is; if macOS chooses to collapse, that's
            // fine — still more helpful than escaping it to visible `\n`.
            _ => out.push(ch),
        }
    }
    out
}

async fn notify_macos(title: &str, body: &str) -> String {
    // Build a tiny `AppleScript` source that pulls the strings from literals
    // we control. We escape backslashes and double quotes; no other
    // `AppleScript` metacharacters are interpretted inside quoted literals.
    let script = format!(
        "display notification \"{}\" with title \"{}\"",
        escape_applescript(body),
        escape_applescript(title)
    );

    let mut cmd = Command::new("osascript");
    // `-` tells osascript to read the script from stdin. Args are fixed —
    // no user-controlled flag surface, so there's no flag-smuggling vector.
    cmd.arg("-");
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
                "Error: failed to spawn osascript: {e} (macOS notifications require osascript on PATH)"
            );
        }
    };

    let future = async move {
        let mut child = child;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(script.as_bytes()).await?;
            stdin.shutdown().await?;
        }
        child.wait_with_output().await
    };

    let result = if let Some(timeout) = crate::security::shell_timeout() {
        match tokio::time::timeout(timeout, future).await {
            Ok(r) => r,
            Err(_) => {
                return format!("Error: osascript timed out after {}s", timeout.as_secs());
            }
        }
    } else {
        future.await
    };

    match result {
        Ok(out) => {
            if out.status.success() {
                "Notification sent (osascript)".to_string()
            } else {
                let stderr = String::from_utf8_lossy(&out.stderr);
                let code = out.status.code().unwrap_or(-1);
                format!("Error: osascript exited [exit {code}]: {stderr}")
            }
        }
        Err(e) => format!("Error: osascript failed: {e}"),
    }
}

// --- Linux backend ---

async fn notify_linux(title: &str, body: &str) -> String {
    let mut cmd = Command::new("notify-send");
    cmd.arg("--app-name=aictl");
    cmd.arg("--");
    cmd.arg(title);
    if !body.is_empty() {
        cmd.arg(body);
    }
    cmd.env_clear();
    for (k, v) in crate::security::scrubbed_env() {
        cmd.env(k, v);
    }
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::piped());
    cmd.kill_on_drop(true);

    let future = cmd.output();
    let result = if let Some(timeout) = crate::security::shell_timeout() {
        match tokio::time::timeout(timeout, future).await {
            Ok(r) => r,
            Err(_) => {
                return format!("Error: notify-send timed out after {}s", timeout.as_secs());
            }
        }
    } else {
        future.await
    };

    match result {
        Ok(out) => {
            if out.status.success() {
                "Notification sent (notify-send)".to_string()
            } else {
                let stderr = String::from_utf8_lossy(&out.stderr);
                let code = out.status.code().unwrap_or(-1);
                format!(
                    "Error: notify-send exited [exit {code}]: {stderr} (install libnotify / notify-send and run a notification daemon)"
                )
            }
        }
        Err(e) => {
            format!("Error: failed to spawn notify-send: {e} (install libnotify / notify-send)")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- parsing ---

    #[test]
    fn parse_title_only() {
        let p = parse_input("Build done").unwrap();
        assert_eq!(p.title, "Build done");
        assert!(p.body.is_empty());
    }

    #[test]
    fn parse_title_and_body() {
        let p = parse_input("Build done\nall 42 tests passed").unwrap();
        assert_eq!(p.title, "Build done");
        assert_eq!(p.body, "all 42 tests passed");
    }

    #[test]
    fn parse_multiline_body() {
        let p = parse_input("Deploy complete\nstaging: ok\nprod: ok").unwrap();
        assert_eq!(p.title, "Deploy complete");
        assert_eq!(p.body, "staging: ok\nprod: ok");
    }

    #[test]
    fn parse_strips_trailing_whitespace() {
        let p = parse_input("Title\nbody\n\n").unwrap();
        assert_eq!(p.title, "Title");
        assert_eq!(p.body, "body");
    }

    #[test]
    fn parse_empty_rejected() {
        assert!(parse_input("").is_err());
        assert!(parse_input("   ").is_err());
        assert!(parse_input("\n\n").is_err());
    }

    #[test]
    fn parse_title_too_long_rejected() {
        let big = "x".repeat(MAX_TITLE_BYTES + 1);
        let err = parse_input(&big).unwrap_err();
        assert!(err.contains("title exceeds"));
    }

    #[test]
    fn parse_body_too_long_rejected() {
        let big_body = "x".repeat(MAX_BODY_BYTES + 1);
        let err = parse_input(&format!("Title\n{big_body}")).unwrap_err();
        assert!(err.contains("body exceeds"));
    }

    // --- `AppleScript` escaping ---

    #[test]
    fn escape_plain_string_unchanged() {
        assert_eq!(escape_applescript("hello world"), "hello world");
    }

    #[test]
    fn escape_double_quote() {
        assert_eq!(escape_applescript(r#"say "hi""#), r#"say \"hi\""#);
    }

    #[test]
    fn escape_backslash() {
        assert_eq!(escape_applescript(r"a\b"), r"a\\b");
    }

    #[test]
    fn escape_backslash_then_quote() {
        // Backslash must be escaped before the quote is considered, so the
        // sequence `\"` in the input becomes `\\\"` in the output, not `\\"`.
        assert_eq!(escape_applescript(r#"\""#), r#"\\\""#);
    }

    // --- Live tests (opt-in via AICTL_TEST_NOTIFY=1) ---
    //
    // These actually pop a desktop notification, which is disruptive during
    // normal test runs. Gate them behind an env var so CI and routine
    // `cargo test` don't spam the user.

    fn should_run_live() -> bool {
        std::env::var("AICTL_TEST_NOTIFY").ok().as_deref() == Some("1")
    }

    #[tokio::test]
    async fn live_notification_sent() {
        if !should_run_live() {
            return;
        }
        let out = tool_notify(&format!(
            "aictl test\nnotification body (pid {})",
            std::process::id()
        ))
        .await;
        assert!(out.starts_with("Notification sent"), "got: {out}");
    }
}
