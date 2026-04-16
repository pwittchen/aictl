//! Dedicated git subprocess with a strict subcommand and flag allowlist.
//!
//! Invoked directly via `tokio::process::Command` (no shell) so shell
//! metacharacters cannot be smuggled in. Only `status`, `diff`, `log`,
//! `blame`, and `commit` are permitted, each with a small allowlist of
//! flags. Dangerous git mechanisms (e.g. `-c key=val`, `-C <dir>`,
//! `--ext-diff`, `--textconv`, `--upload-pack`, `--exec-path`,
//! `--no-verify`, `--amend`, `--git-dir`, `--work-tree`) are rejected
//! implicitly by the per-subcommand allowlist, and because the first
//! token is required to be a bare subcommand name.
//!
//! Git-specific environment variables that can redirect the process to a
//! different repo, inject config, or replace the ssh / editor / askpass
//! helper are stripped in addition to the shared scrubbed-env pass.

use std::fmt::Write as _;

use super::util::truncate_output;

struct FlagPolicy {
    /// Flags accepted verbatim with no attached value.
    exact: &'static [&'static str],
    /// Flag prefixes accepted with an attached value (`--foo=<val>`).
    with_value: &'static [&'static str],
    /// Flags whose value is the next arg token (`-n 10`, `--max-count 10`).
    with_arg: &'static [&'static str],
    /// Whether positional (non-flag) arguments are allowed.
    allow_positional: bool,
}

const ALLOWED_SUBCOMMANDS: &[&str] = &["status", "diff", "log", "blame", "commit"];

const STATUS_POLICY: FlagPolicy = FlagPolicy {
    exact: &[
        "-s",
        "--short",
        "-b",
        "--branch",
        "--porcelain",
        "--long",
        "-u",
        "--untracked-files",
        "--ignored",
        "--renames",
        "--no-renames",
        "--ahead-behind",
        "--no-ahead-behind",
        "--column",
        "--no-column",
        "-z",
        "--",
    ],
    with_value: &[
        "--porcelain=",
        "--untracked-files=",
        "--ignored=",
        "--find-renames=",
    ],
    with_arg: &[],
    allow_positional: true,
};

const DIFF_POLICY: FlagPolicy = FlagPolicy {
    exact: &[
        "--cached",
        "--staged",
        "--stat",
        "--numstat",
        "--shortstat",
        "--summary",
        "--name-only",
        "--name-status",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "--patch",
        "-p",
        "--no-patch",
        "-s",
        "-w",
        "--ignore-all-space",
        "--ignore-space-change",
        "--ignore-blank-lines",
        "-R",
        "--check",
        "--",
    ],
    with_value: &["--unified=", "--color=", "--stat-width="],
    with_arg: &["-U"],
    allow_positional: true,
};

const LOG_POLICY: FlagPolicy = FlagPolicy {
    exact: &[
        "--oneline",
        "--stat",
        "--numstat",
        "--shortstat",
        "--graph",
        "--decorate",
        "--no-decorate",
        "--all",
        "--branches",
        "--tags",
        "--no-color",
        "--merges",
        "--no-merges",
        "--follow",
        "--reverse",
        "--patch",
        "-p",
        "--no-patch",
        "--name-only",
        "--name-status",
        "--abbrev-commit",
        "--no-abbrev-commit",
        "--",
    ],
    with_value: &[
        "--max-count=",
        "--skip=",
        "--author=",
        "--committer=",
        "--since=",
        "--until=",
        "--after=",
        "--before=",
        "--grep=",
        "--format=",
        "--pretty=",
        "--color=",
        "--decorate=",
    ],
    with_arg: &["-n"],
    allow_positional: true,
};

const BLAME_POLICY: FlagPolicy = FlagPolicy {
    exact: &[
        "-w",
        "-M",
        "-C",
        "-f",
        "--show-name",
        "-n",
        "--show-number",
        "-e",
        "--show-email",
        "--abbrev",
        "--no-color",
        "--",
    ],
    with_value: &["--since=", "--abbrev="],
    with_arg: &["-L"],
    allow_positional: true,
};

const COMMIT_POLICY: FlagPolicy = FlagPolicy {
    exact: &[
        "-a",
        "--all",
        "-s",
        "--signoff",
        "-q",
        "--quiet",
        "--allow-empty",
        "--allow-empty-message",
        "--",
    ],
    with_value: &["--message=", "--author=", "--date="],
    with_arg: &["-m", "--message", "--author", "--date"],
    allow_positional: true,
};

/// Entry point wired into `execute_tool` in `tools.rs`.
pub(super) async fn tool_git(input: &str) -> String {
    let input = input.trim();
    if input.is_empty() {
        return "Error: empty git command. Expected: <subcommand> [args...] (one of: status, diff, log, blame, commit)".to_string();
    }
    if input.contains('\0') {
        return "Error: input contains null byte".to_string();
    }

    let tokens = match tokenize(input) {
        Ok(t) => t,
        Err(e) => return format!("Error parsing git command: {e}"),
    };
    if tokens.is_empty() {
        return "Error: empty git command".to_string();
    }

    let subcommand = &tokens[0];
    let policy = match subcommand.as_str() {
        "status" => &STATUS_POLICY,
        "diff" => &DIFF_POLICY,
        "log" => &LOG_POLICY,
        "blame" => &BLAME_POLICY,
        "commit" => &COMMIT_POLICY,
        other => {
            return format!(
                "Error: git subcommand '{other}' is not allowed. Allowed: {}",
                ALLOWED_SUBCOMMANDS.join(", ")
            );
        }
    };

    if let Err(e) = validate_args(&tokens[1..], policy, subcommand) {
        return format!("Error: {e}");
    }

    run_git(&tokens).await
}

/// Quote-aware tokenizer. Preserves whitespace inside `'...'` and `"..."`
/// and supports `\"` / `\\` escape sequences inside double quotes so commit
/// messages with embedded quotes can be passed through.
fn tokenize(s: &str) -> Result<Vec<String>, String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;
    let mut has_content = false;

    for ch in s.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            has_content = true;
            continue;
        }
        if ch == '\\' && in_double {
            escaped = true;
            continue;
        }
        if ch == '\'' && !in_double {
            in_single = !in_single;
            has_content = true;
            continue;
        }
        if ch == '"' && !in_single {
            in_double = !in_double;
            has_content = true;
            continue;
        }
        if ch.is_whitespace() && !in_single && !in_double {
            if has_content {
                tokens.push(std::mem::take(&mut current));
                has_content = false;
            }
            continue;
        }
        current.push(ch);
        has_content = true;
    }
    if in_single || in_double {
        return Err("unterminated quoted string".to_string());
    }
    if escaped {
        return Err("dangling escape character".to_string());
    }
    if has_content {
        tokens.push(current);
    }
    Ok(tokens)
}

fn validate_args(args: &[String], policy: &FlagPolicy, subcommand: &str) -> Result<(), String> {
    let mut i = 0;
    let mut past_separator = false;

    while i < args.len() {
        let arg = &args[i];
        if arg.contains('\0') {
            return Err("argument contains null byte".to_string());
        }

        if past_separator {
            // After `--`, everything is positional.
            i += 1;
            continue;
        }

        if !arg.starts_with('-') {
            if !policy.allow_positional {
                return Err(format!(
                    "positional arguments not allowed for 'git {subcommand}'"
                ));
            }
            i += 1;
            continue;
        }

        if arg == "--" {
            if !policy.exact.contains(&"--") {
                return Err(format!("'--' separator not allowed for 'git {subcommand}'"));
            }
            past_separator = true;
            i += 1;
            continue;
        }

        if policy.with_arg.contains(&arg.as_str()) {
            if i + 1 >= args.len() {
                return Err(format!("flag '{arg}' requires a value"));
            }
            // The value is accepted as-is. Subprocess args are never shell-
            // interpreted (we invoke `git` directly), so metacharacters are
            // not a vector here; git validates the value itself.
            i += 2;
            continue;
        }

        if policy.exact.contains(&arg.as_str()) {
            i += 1;
            continue;
        }

        if policy
            .with_value
            .iter()
            .any(|prefix| arg.starts_with(prefix))
        {
            i += 1;
            continue;
        }

        return Err(format!("flag '{arg}' not allowed for 'git {subcommand}'"));
    }
    Ok(())
}

async fn run_git(args: &[String]) -> String {
    let mut cmd = tokio::process::Command::new("git");
    cmd.args(args);

    // Build a clean environment: apply the shared secret scrubber, then
    // drop git-specific vars that could redirect the subprocess (alternate
    // repo, injected config, replacement ssh/editor/askpass helper).
    let security_enabled = crate::security::policy().enabled;
    cmd.env_clear();
    let env_pairs: Vec<(String, String)> = if security_enabled {
        crate::security::scrubbed_env()
            .into_iter()
            .filter(|(k, _)| !is_dangerous_git_env(k))
            .collect()
    } else {
        std::env::vars().collect()
    };
    for (k, v) in env_pairs {
        cmd.env(k, v);
    }
    // Force non-interactive: no terminal prompt, no editor, no color.
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    cmd.env("GIT_EDITOR", ":");
    cmd.env("GIT_SEQUENCE_EDITOR", ":");
    cmd.env("NO_COLOR", "1");
    cmd.env("CLICOLOR", "0");

    // Pin CWD to the security policy's working dir so the subprocess can't
    // drift to a different repo via a stale parent cwd.
    cmd.current_dir(&crate::security::policy().paths.working_dir);

    let future = cmd.output();
    let output = if let Some(timeout) = crate::security::shell_timeout() {
        match tokio::time::timeout(timeout, future).await {
            Ok(r) => r,
            Err(_) => {
                return format!("Error: git command timed out after {}s", timeout.as_secs());
            }
        }
    } else {
        future.await
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
                if !result.is_empty() {
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
        Err(e) => format!("Error running git: {e}"),
    }
}

fn is_dangerous_git_env(key: &str) -> bool {
    matches!(
        key,
        "GIT_DIR"
            | "GIT_WORK_TREE"
            | "GIT_COMMON_DIR"
            | "GIT_NAMESPACE"
            | "GIT_INDEX_FILE"
            | "GIT_OBJECT_DIRECTORY"
            | "GIT_ALTERNATE_OBJECT_DIRECTORIES"
            | "GIT_EXEC_PATH"
            | "GIT_SSH"
            | "GIT_SSH_COMMAND"
            | "GIT_PROXY_COMMAND"
            | "GIT_EDITOR"
            | "GIT_SEQUENCE_EDITOR"
            | "EDITOR"
            | "VISUAL"
            | "GIT_ASKPASS"
            | "SSH_ASKPASS"
            | "GIT_TERMINAL_PROMPT"
            | "GIT_EXTERNAL_DIFF"
            | "GIT_PAGER"
            | "PAGER"
    ) || key.starts_with("GIT_CONFIG_")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenize_simple() {
        let t = tokenize("status").unwrap();
        assert_eq!(t, vec!["status"]);
    }

    #[test]
    fn tokenize_flags() {
        let t = tokenize("log --oneline -n 10").unwrap();
        assert_eq!(t, vec!["log", "--oneline", "-n", "10"]);
    }

    #[test]
    fn tokenize_double_quoted_message() {
        let t = tokenize(r#"commit -m "fix: typo in docs""#).unwrap();
        assert_eq!(t, vec!["commit", "-m", "fix: typo in docs"]);
    }

    #[test]
    fn tokenize_single_quoted_message() {
        let t = tokenize("commit -m 'hello world'").unwrap();
        assert_eq!(t, vec!["commit", "-m", "hello world"]);
    }

    #[test]
    fn tokenize_escaped_quote_in_double() {
        let t = tokenize(r#"commit -m "say \"hi\"""#).unwrap();
        assert_eq!(t, vec!["commit", "-m", r#"say "hi""#]);
    }

    #[test]
    fn tokenize_multiline_in_quotes() {
        let t = tokenize("commit -m \"line 1\nline 2\"").unwrap();
        assert_eq!(t, vec!["commit", "-m", "line 1\nline 2"]);
    }

    #[test]
    fn tokenize_unterminated_double_quote() {
        assert!(tokenize(r#"commit -m "oops"#).is_err());
    }

    #[test]
    fn tokenize_unterminated_single_quote() {
        assert!(tokenize("commit -m 'oops").is_err());
    }

    #[test]
    fn tokenize_empty() {
        let t = tokenize("").unwrap();
        assert!(t.is_empty());
    }

    #[test]
    fn validate_allows_known_status_flags() {
        let args = vec!["--short".into(), "-b".into()];
        assert!(validate_args(&args, &STATUS_POLICY, "status").is_ok());
    }

    #[test]
    fn validate_rejects_unknown_flag_for_status() {
        let args = vec!["--evil".into()];
        let err = validate_args(&args, &STATUS_POLICY, "status").unwrap_err();
        assert!(err.contains("--evil"));
    }

    #[test]
    fn validate_blocks_c_config_override() {
        // `-c key=val` is not in any per-subcommand allowlist. Even if the
        // LLM tries to sneak it in after the subcommand (where git ignores
        // it as a global option anyway), we reject the flag outright.
        let args = vec!["-c".into(), "core.sshCommand=malicious".into()];
        assert!(validate_args(&args, &LOG_POLICY, "log").is_err());
    }

    #[test]
    fn validate_blocks_ext_diff() {
        let args = vec!["--ext-diff".into()];
        assert!(validate_args(&args, &DIFF_POLICY, "diff").is_err());
    }

    #[test]
    fn validate_blocks_no_verify_on_commit() {
        let args = vec!["--no-verify".into()];
        let err = validate_args(&args, &COMMIT_POLICY, "commit").unwrap_err();
        assert!(err.contains("--no-verify"));
    }

    #[test]
    fn validate_blocks_amend() {
        let args = vec!["--amend".into()];
        let err = validate_args(&args, &COMMIT_POLICY, "commit").unwrap_err();
        assert!(err.contains("--amend"));
    }

    #[test]
    fn validate_allows_commit_message_short() {
        let args = vec!["-m".into(), "fix: typo".into()];
        assert!(validate_args(&args, &COMMIT_POLICY, "commit").is_ok());
    }

    #[test]
    fn validate_allows_commit_message_long_equals() {
        let args = vec!["--message=fix: typo".into()];
        assert!(validate_args(&args, &COMMIT_POLICY, "commit").is_ok());
    }

    #[test]
    fn validate_allows_commit_message_long_spaced() {
        let args = vec!["--message".into(), "fix: typo".into()];
        assert!(validate_args(&args, &COMMIT_POLICY, "commit").is_ok());
    }

    #[test]
    fn validate_with_arg_missing_value() {
        let args = vec!["-m".into()];
        let err = validate_args(&args, &COMMIT_POLICY, "commit").unwrap_err();
        assert!(err.contains("requires a value"));
    }

    #[test]
    fn validate_allows_log_with_value_prefix() {
        let args = vec!["--author=Alice".into(), "--since=yesterday".into()];
        assert!(validate_args(&args, &LOG_POLICY, "log").is_ok());
    }

    #[test]
    fn validate_allows_short_with_arg_separate() {
        let args = vec!["-n".into(), "5".into(), "--oneline".into()];
        assert!(validate_args(&args, &LOG_POLICY, "log").is_ok());
    }

    #[test]
    fn validate_allows_blame_line_range() {
        let args = vec!["-L".into(), "10,20".into(), "src/main.rs".into()];
        assert!(validate_args(&args, &BLAME_POLICY, "blame").is_ok());
    }

    #[test]
    fn validate_allows_double_dash_separator() {
        let args = vec!["--stat".into(), "--".into(), "path/to/file".into()];
        assert!(validate_args(&args, &DIFF_POLICY, "diff").is_ok());
    }

    #[test]
    fn validate_allows_positional_ref() {
        let args = vec!["HEAD~3..HEAD".into()];
        assert!(validate_args(&args, &LOG_POLICY, "log").is_ok());
    }

    #[test]
    fn validate_after_separator_anything_goes() {
        // After `--`, even arg-looking tokens are positional paths (no flag
        // validation). Ensure the separator short-circuits correctly.
        let args = vec!["--".into(), "--weird-path-name".into()];
        assert!(validate_args(&args, &DIFF_POLICY, "diff").is_ok());
    }

    #[test]
    fn tool_git_rejects_empty_input() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let r = rt.block_on(tool_git(""));
        assert!(r.starts_with("Error:"));
    }

    #[test]
    fn tool_git_rejects_unknown_subcommand() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let r = rt.block_on(tool_git("push origin main"));
        assert!(r.contains("not allowed"));
        assert!(r.contains("push"));
    }

    #[test]
    fn tool_git_rejects_flag_as_first_token() {
        // Prevents `git -c core.sshCommand=evil status` from slipping
        // through — the first token must be a bare subcommand.
        let rt = tokio::runtime::Runtime::new().unwrap();
        let r = rt.block_on(tool_git("-c core.sshCommand=evil status"));
        assert!(r.contains("not allowed"));
    }

    #[test]
    fn tool_git_rejects_null_byte() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let r = rt.block_on(tool_git("status\0"));
        assert!(r.contains("null byte"));
    }

    #[test]
    fn tool_git_rejects_unterminated_quote() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let r = rt.block_on(tool_git(r#"commit -m "oops"#));
        assert!(r.contains("unterminated"));
    }

    #[test]
    fn dangerous_git_env_vars_detected() {
        assert!(is_dangerous_git_env("GIT_DIR"));
        assert!(is_dangerous_git_env("GIT_SSH_COMMAND"));
        assert!(is_dangerous_git_env("GIT_CONFIG_COUNT"));
        assert!(is_dangerous_git_env("GIT_CONFIG_KEY_0"));
        assert!(is_dangerous_git_env("EDITOR"));
        assert!(!is_dangerous_git_env("PATH"));
        assert!(!is_dangerous_git_env("HOME"));
        assert!(!is_dangerous_git_env("GIT_AUTHOR_NAME"));
    }
}
