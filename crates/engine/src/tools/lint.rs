//! Run a language-appropriate linter / formatter on a single file and
//! return its diagnostics. The model passes a file path as input; the
//! tool picks the right invocation based on the file extension, trying a
//! short allowlist of well-known tools in order and stopping at the first
//! one that is actually installed on `PATH`.
//!
//! The value for the agent is "I don't need to know which linter this
//! project uses" — the tool encapsulates the lookup. Each invocation is
//! run as a direct subprocess (no shell), inherits the shared scrubbed
//! env, is pinned to the security policy's working dir, and shares the
//! shell timeout with `exec_shell` / `git` / `run_code`.
//!
//! The tool deliberately only *reports* diagnostics — no `--fix` or
//! `--write` flags are ever passed, so the file on disk is never
//! modified. The model can then propose edits via `edit_file`.

use std::fmt::Write as _;
use std::path::Path;
use std::process::Stdio;

use super::util::truncate_output;

/// A single linter candidate for a group of file extensions. `args` are
/// passed verbatim before the file path, which is always appended as the
/// final positional argument.
struct LinterCmd {
    /// Binary name resolved via `PATH`.
    binary: &'static str,
    /// Literal args passed before the file path.
    args: &'static [&'static str],
    /// Short label used in the output header (e.g. "ruff check").
    label: &'static str,
}

struct LinterGroup {
    /// File extensions this group handles, without the leading dot, all
    /// lowercase. Matched case-insensitively against the file's suffix.
    extensions: &'static [&'static str],
    /// Ordered list of linter candidates. The first binary that resolves
    /// on `PATH` wins; if none resolve the tool returns a clear error
    /// listing the ones it tried.
    candidates: &'static [LinterCmd],
}

const LINTERS: &[LinterGroup] = &[
    LinterGroup {
        extensions: &["rs"],
        candidates: &[LinterCmd {
            binary: "rustfmt",
            args: &["--check", "--color=never"],
            label: "rustfmt --check",
        }],
    },
    LinterGroup {
        extensions: &["py", "pyi"],
        candidates: &[
            LinterCmd {
                binary: "ruff",
                args: &["check", "--no-cache"],
                label: "ruff check",
            },
            LinterCmd {
                binary: "flake8",
                args: &[],
                label: "flake8",
            },
            LinterCmd {
                binary: "pyflakes",
                args: &[],
                label: "pyflakes",
            },
            LinterCmd {
                binary: "python3",
                args: &["-m", "py_compile"],
                label: "python3 -m py_compile",
            },
        ],
    },
    LinterGroup {
        extensions: &["js", "mjs", "cjs", "jsx"],
        candidates: &[
            LinterCmd {
                binary: "eslint",
                args: &["--no-color"],
                label: "eslint",
            },
            LinterCmd {
                binary: "node",
                args: &["--check"],
                label: "node --check",
            },
        ],
    },
    LinterGroup {
        extensions: &["ts", "tsx"],
        candidates: &[
            LinterCmd {
                binary: "eslint",
                args: &["--no-color"],
                label: "eslint",
            },
            LinterCmd {
                binary: "tsc",
                args: &["--noEmit", "--pretty", "false"],
                label: "tsc --noEmit",
            },
        ],
    },
    LinterGroup {
        extensions: &["go"],
        candidates: &[
            LinterCmd {
                binary: "gofmt",
                args: &["-l", "-d"],
                label: "gofmt -l -d",
            },
            LinterCmd {
                binary: "go",
                args: &["vet"],
                label: "go vet",
            },
        ],
    },
    LinterGroup {
        extensions: &["sh", "bash"],
        candidates: &[
            LinterCmd {
                binary: "shellcheck",
                args: &["--color=never"],
                label: "shellcheck",
            },
            LinterCmd {
                binary: "bash",
                args: &["-n"],
                label: "bash -n",
            },
        ],
    },
    LinterGroup {
        extensions: &["rb"],
        candidates: &[
            LinterCmd {
                binary: "rubocop",
                args: &["--no-color"],
                label: "rubocop",
            },
            LinterCmd {
                binary: "ruby",
                args: &["-c"],
                label: "ruby -c",
            },
        ],
    },
    LinterGroup {
        extensions: &["json"],
        candidates: &[
            LinterCmd {
                binary: "jq",
                args: &["empty"],
                label: "jq empty",
            },
            LinterCmd {
                binary: "python3",
                args: &["-m", "json.tool", "--no-ensure-ascii"],
                label: "python3 -m json.tool",
            },
        ],
    },
    LinterGroup {
        extensions: &["yaml", "yml"],
        candidates: &[LinterCmd {
            binary: "yamllint",
            args: &["--no-warnings", "-f", "parsable"],
            label: "yamllint",
        }],
    },
    LinterGroup {
        extensions: &["toml"],
        candidates: &[LinterCmd {
            binary: "taplo",
            args: &["check", "--no-colors"],
            label: "taplo check",
        }],
    },
    LinterGroup {
        extensions: &["md", "markdown"],
        candidates: &[
            LinterCmd {
                binary: "markdownlint",
                args: &[],
                label: "markdownlint",
            },
            LinterCmd {
                binary: "prettier",
                args: &["--check"],
                label: "prettier --check",
            },
        ],
    },
    LinterGroup {
        extensions: &["lua"],
        candidates: &[
            LinterCmd {
                binary: "luacheck",
                args: &["--no-color"],
                label: "luacheck",
            },
            LinterCmd {
                binary: "luac",
                args: &["-p"],
                label: "luac -p",
            },
        ],
    },
    LinterGroup {
        extensions: &["c", "h", "cpp", "cc", "cxx", "hpp", "hh", "hxx"],
        candidates: &[
            LinterCmd {
                binary: "clang-format",
                args: &["--dry-run", "-Werror"],
                label: "clang-format --dry-run",
            },
            LinterCmd {
                binary: "cppcheck",
                args: &["--enable=warning", "--quiet"],
                label: "cppcheck",
            },
        ],
    },
    LinterGroup {
        extensions: &["html", "htm", "css", "scss", "sass"],
        candidates: &[LinterCmd {
            binary: "prettier",
            args: &["--check"],
            label: "prettier --check",
        }],
    },
];

pub(super) async fn tool_lint_file(input: &str) -> String {
    let path = input.trim();
    if path.is_empty() {
        return "Error: empty input. Expected: <file path>".to_string();
    }
    if path.contains('\0') {
        return "Error: input contains null byte".to_string();
    }

    let Some(ext) = extract_extension(path) else {
        return format!(
            "Error: cannot lint '{path}' — the file has no extension to detect the language from"
        );
    };

    let Some(group) = resolve_group(&ext) else {
        return format!(
            "Error: no linter configured for '.{ext}' files. Supported extensions: {}",
            supported_extensions().join(", ")
        );
    };

    match tokio::fs::metadata(path).await {
        Ok(md) if md.is_dir() => {
            return format!("Error: '{path}' is a directory; lint_file expects a regular file");
        }
        Ok(_) => {}
        Err(e) => return format!("Error reading '{path}': {e}"),
    }

    let Some(cmd) = first_available(group) else {
        let tried: Vec<&str> = group.candidates.iter().map(|c| c.binary).collect();
        return format!(
            "Error: no linter for '.{ext}' files is installed. Tried: {}. Install one of them and retry.",
            tried.join(", ")
        );
    };

    run_linter(cmd, path).await
}

fn extract_extension(path: &str) -> Option<String> {
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
}

fn resolve_group(ext: &str) -> Option<&'static LinterGroup> {
    LINTERS.iter().find(|g| g.extensions.contains(&ext))
}

fn supported_extensions() -> Vec<&'static str> {
    let mut out: Vec<&str> = LINTERS
        .iter()
        .flat_map(|g| g.extensions.iter().copied())
        .collect();
    out.sort_unstable();
    out
}

fn first_available(group: &'static LinterGroup) -> Option<&'static LinterCmd> {
    group.candidates.iter().find(|c| binary_on_path(c.binary))
}

/// Probe `PATH` by running `<binary> --version` with stdout/stderr
/// discarded. This is the same technique used in the `run_code` tests
/// and is robust across systems where `which` is unavailable.
fn binary_on_path(binary: &str) -> bool {
    // Some binaries (e.g. `node --check`) also accept `--version`; rustfmt,
    // ruff, go, python3, etc. all do too. For any holdout we still get a
    // deterministic success/failure: a missing binary returns an Err from
    // spawn, which maps to `false`.
    std::process::Command::new(binary)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

async fn run_linter(cmd: &LinterCmd, path: &str) -> String {
    let mut proc = tokio::process::Command::new(cmd.binary);
    proc.args(cmd.args);
    proc.arg(path);
    proc.stdin(Stdio::null());
    proc.stdout(Stdio::piped());
    proc.stderr(Stdio::piped());
    proc.kill_on_drop(true);

    proc.env_clear();
    let env_pairs: Vec<(String, String)> = if crate::security::policy().enabled {
        crate::security::scrubbed_env()
    } else {
        std::env::vars().collect()
    };
    for (k, v) in env_pairs {
        proc.env(k, v);
    }
    proc.env("NO_COLOR", "1");
    proc.env("CLICOLOR", "0");
    proc.env("PYTHONDONTWRITEBYTECODE", "1");

    proc.current_dir(&crate::security::policy().paths.working_dir);

    let spawned = match proc.spawn() {
        Ok(c) => c,
        Err(e) => {
            return format!(
                "Error launching {}: {e}. Is the linter installed and on PATH?",
                cmd.binary
            );
        }
    };

    let output_future = spawned.wait_with_output();
    let output = if let Some(timeout) = crate::security::shell_timeout() {
        match tokio::time::timeout(timeout, output_future).await {
            Ok(r) => r,
            Err(_) => {
                return format!(
                    "Error: {} timed out after {}s",
                    cmd.label,
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
            let mut result = format!("[linter: {}]\n", cmd.label);
            if !stdout.is_empty() {
                result.push_str(&stdout);
            }
            if !stderr.is_empty() {
                if !result.ends_with('\n') {
                    result.push('\n');
                }
                result.push_str("[stderr]\n");
                result.push_str(&stderr);
            }
            if out.status.success() {
                if !result.ends_with('\n') {
                    result.push('\n');
                }
                result.push_str("[clean]");
            } else {
                if !result.ends_with('\n') {
                    result.push('\n');
                }
                let _ = write!(result, "[exit {}]", out.status.code().unwrap_or(-1));
            }
            truncate_output(&mut result);
            result
        }
        Err(e) => format!("Error waiting for {}: {e}", cmd.binary),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_extension_simple() {
        assert_eq!(extract_extension("file.rs").as_deref(), Some("rs"));
        assert_eq!(extract_extension("FILE.PY").as_deref(), Some("py"));
        assert_eq!(extract_extension("src/main.rs").as_deref(), Some("rs"));
    }

    #[test]
    fn extract_extension_missing() {
        assert_eq!(extract_extension("Makefile"), None);
        assert_eq!(extract_extension(""), None);
    }

    #[test]
    fn extract_extension_dotfile() {
        // `.bashrc` has no extension from `Path::extension`'s perspective — it's
        // the whole filename. We surface "no extension" rather than guessing.
        assert_eq!(extract_extension(".bashrc"), None);
    }

    #[test]
    fn resolve_group_known() {
        assert!(resolve_group("rs").is_some());
        assert!(resolve_group("py").is_some());
        assert!(resolve_group("ts").is_some());
        assert!(resolve_group("yml").is_some());
        assert!(resolve_group("yaml").is_some());
    }

    #[test]
    fn resolve_group_unknown() {
        assert!(resolve_group("zzz").is_none());
        assert!(resolve_group("").is_none());
    }

    #[test]
    fn supported_extensions_includes_common_languages() {
        let exts = supported_extensions();
        for e in ["rs", "py", "js", "ts", "go", "sh", "rb", "json", "md"] {
            assert!(exts.contains(&e), "expected {e} in {exts:?}");
        }
    }

    #[tokio::test]
    async fn empty_input_rejected() {
        let r = tool_lint_file("").await;
        assert!(r.contains("empty input"), "got: {r}");
    }

    #[tokio::test]
    async fn null_byte_rejected() {
        let r = tool_lint_file("foo.py\0").await;
        assert!(r.contains("null byte"), "got: {r}");
    }

    #[tokio::test]
    async fn missing_extension_rejected() {
        let r = tool_lint_file("Makefile").await;
        assert!(r.contains("no extension"), "got: {r}");
    }

    #[tokio::test]
    async fn unsupported_extension_rejected() {
        let r = tool_lint_file("notes.brainfuck").await;
        assert!(r.contains("no linter configured"), "got: {r}");
    }

    #[tokio::test]
    async fn directory_rejected() {
        let dir =
            std::env::temp_dir().join(format!("aictl_lint_dirtest_{}.py", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let r = tool_lint_file(dir.to_str().unwrap()).await;
        assert!(r.contains("is a directory"), "got: {r}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn nonexistent_file_rejected() {
        let r = tool_lint_file("/tmp/aictl_lint_nonexistent_xyz.py").await;
        assert!(r.starts_with("Error reading"), "got: {r}");
    }

    #[tokio::test]
    async fn python_clean_file_reports_clean() {
        if !binary_on_path("python3") {
            return;
        }
        let dir = std::env::temp_dir().join(format!("aictl_lint_pyok_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("ok.py");
        std::fs::write(&path, "x = 1\nprint(x)\n").unwrap();
        let r = tool_lint_file(path.to_str().unwrap()).await;
        // At least one of the configured python linters should run and
        // accept this trivial file.
        assert!(r.contains("[linter:"), "got: {r}");
        assert!(r.contains("[clean]") || r.contains("[exit 0]"), "got: {r}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn python_syntax_error_surfaces_diagnostic() {
        if !binary_on_path("python3") {
            return;
        }
        let dir = std::env::temp_dir().join(format!("aictl_lint_pybad_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("bad.py");
        // Unclosed paren: every candidate (ruff/flake8/pyflakes/py_compile)
        // reports this as an error, so the assertion is stable regardless
        // of which one happens to be installed.
        std::fs::write(&path, "print(\n").unwrap();
        let r = tool_lint_file(path.to_str().unwrap()).await;
        assert!(r.contains("[linter:"), "got: {r}");
        assert!(r.contains("[exit "), "expected non-zero exit, got: {r}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn rustfmt_runs_on_rust_file_when_installed() {
        if !binary_on_path("rustfmt") {
            return;
        }
        let dir = std::env::temp_dir().join(format!("aictl_lint_rs_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("ok.rs");
        std::fs::write(&path, "fn main() {}\n").unwrap();
        let r = tool_lint_file(path.to_str().unwrap()).await;
        assert!(r.contains("rustfmt --check"), "got: {r}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
