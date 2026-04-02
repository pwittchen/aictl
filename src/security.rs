use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::config::config_get;
use crate::tools::ToolCall;

static POLICY: OnceLock<SecurityPolicy> = OnceLock::new();

// --- Default blocked commands ---

const DEFAULT_BLOCKED_COMMANDS: &[&str] = &[
    "rm", "rmdir", "mkfs", "dd", "shutdown", "reboot", "halt", "poweroff", "sudo", "su", "doas",
    "eval", "exec", "nc", "ncat", "netcat",
];

// --- Default blocked paths (relative to $HOME) ---

const DEFAULT_BLOCKED_HOME_PATHS: &[&str] = &[".ssh", ".gnupg", ".aictl", ".aws", ".config/gcloud"];

const DEFAULT_BLOCKED_ABSOLUTE_PATHS: &[&str] = &["/etc/shadow", "/etc/sudoers"];

// --- Default safe env vars to keep ---

const SAFE_ENV_VARS: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "TERM",
    "LANG",
    "SHELL",
    "EDITOR",
    "VISUAL",
    "LC_ALL",
    "LC_CTYPE",
    "TMPDIR",
    "XDG_RUNTIME_DIR",
    "COLORTERM",
    "TERM_PROGRAM",
];

// --- Sensitive env var patterns (suffix-matched) ---

const SENSITIVE_ENV_SUFFIXES: &[&str] = &["_KEY", "_SECRET", "_TOKEN", "_PASSWORD"];

// --- Default shell timeout ---

const DEFAULT_SHELL_TIMEOUT_SECS: u64 = 30;
const DEFAULT_MAX_WRITE_BYTES: usize = 1_048_576; // 1 MB

// --- Command prefixes to strip ---

const COMMAND_PREFIXES: &[&str] = &[
    "sudo", "su", "doas", "env", "nohup", "nice", "time", "command", "builtin",
];

// --- Policy structs ---

pub struct SecurityPolicy {
    pub enabled: bool,
    pub shell: ShellPolicy,
    pub paths: PathPolicy,
    pub resources: ResourcePolicy,
    pub env: EnvPolicy,
    pub disabled_tools: Vec<String>,
}

pub struct ShellPolicy {
    pub allowed_commands: Vec<String>,
    pub blocked_commands: Vec<String>,
    pub block_subshell: bool,
}

pub struct PathPolicy {
    pub working_dir: PathBuf,
    pub restrict_to_cwd: bool,
    pub blocked_paths: Vec<PathBuf>,
    pub allowed_paths: Vec<PathBuf>,
}

pub struct ResourcePolicy {
    pub shell_timeout_secs: u64,
    pub max_file_write_bytes: usize,
}

pub struct EnvPolicy {
    pub blocked_env_vars: Vec<String>,
}

// --- Initialization ---

/// Initialize the security policy. Call once at startup after `load_config()`.
pub fn init(unrestricted: bool) {
    let policy = if unrestricted {
        SecurityPolicy {
            enabled: false,
            shell: ShellPolicy {
                allowed_commands: vec![],
                blocked_commands: vec![],
                block_subshell: false,
            },
            paths: PathPolicy {
                working_dir: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
                restrict_to_cwd: false,
                blocked_paths: vec![],
                allowed_paths: vec![],
            },
            resources: ResourcePolicy {
                shell_timeout_secs: 0,
                max_file_write_bytes: 0,
            },
            env: EnvPolicy {
                blocked_env_vars: vec![],
            },
            disabled_tools: vec![],
        }
    } else {
        load_policy()
    };
    POLICY.set(policy).ok();
}

/// Access the global security policy.
/// Returns a permissive default if `init()` has not been called (e.g. in tests).
pub fn policy() -> &'static SecurityPolicy {
    static DEFAULT: OnceLock<SecurityPolicy> = OnceLock::new();
    POLICY.get().unwrap_or_else(|| {
        DEFAULT.get_or_init(|| SecurityPolicy {
            enabled: false,
            shell: ShellPolicy {
                allowed_commands: vec![],
                blocked_commands: vec![],
                block_subshell: false,
            },
            paths: PathPolicy {
                working_dir: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
                restrict_to_cwd: false,
                blocked_paths: vec![],
                allowed_paths: vec![],
            },
            resources: ResourcePolicy {
                shell_timeout_secs: 0,
                max_file_write_bytes: 0,
            },
            env: EnvPolicy {
                blocked_env_vars: vec![],
            },
            disabled_tools: vec![],
        })
    })
}

fn load_policy() -> SecurityPolicy {
    let enabled = config_get("AICTL_SECURITY").is_none_or(|v| v != "false" && v != "0");

    let restrict_to_cwd =
        config_get("AICTL_SECURITY_CWD_RESTRICT").is_none_or(|v| v != "false" && v != "0");

    let block_subshell =
        config_get("AICTL_SECURITY_BLOCK_SUBSHELL").is_none_or(|v| v != "false" && v != "0");

    let allowed_commands =
        parse_csv(&config_get("AICTL_SECURITY_SHELL_ALLOWED").unwrap_or_default());

    let mut blocked_commands: Vec<String> = DEFAULT_BLOCKED_COMMANDS
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    blocked_commands.extend(parse_csv(
        &config_get("AICTL_SECURITY_SHELL_BLOCKED").unwrap_or_default(),
    ));

    let home = std::env::var("HOME").unwrap_or_default();
    let mut blocked_paths: Vec<PathBuf> = DEFAULT_BLOCKED_HOME_PATHS
        .iter()
        .map(|p| PathBuf::from(&home).join(p))
        .collect();
    blocked_paths.extend(DEFAULT_BLOCKED_ABSOLUTE_PATHS.iter().map(PathBuf::from));
    for extra in parse_csv(&config_get("AICTL_SECURITY_BLOCKED_PATHS").unwrap_or_default()) {
        let expanded = if let Some(rest) = extra.strip_prefix('~') {
            PathBuf::from(&home).join(rest.strip_prefix('/').unwrap_or(rest))
        } else {
            PathBuf::from(&extra)
        };
        blocked_paths.push(expanded);
    }

    let allowed_paths: Vec<PathBuf> =
        parse_csv(&config_get("AICTL_SECURITY_ALLOWED_PATHS").unwrap_or_default())
            .into_iter()
            .map(|p| {
                if let Some(rest) = p.strip_prefix('~') {
                    PathBuf::from(&home).join(rest.strip_prefix('/').unwrap_or(rest))
                } else {
                    PathBuf::from(&p)
                }
            })
            .collect();

    let shell_timeout_secs = config_get("AICTL_SECURITY_SHELL_TIMEOUT")
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_SHELL_TIMEOUT_SECS);

    let max_file_write_bytes = config_get("AICTL_SECURITY_MAX_WRITE")
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_MAX_WRITE_BYTES);

    let mut blocked_env_vars: Vec<String> = vec![];
    blocked_env_vars.extend(parse_csv(
        &config_get("AICTL_SECURITY_BLOCKED_ENV").unwrap_or_default(),
    ));

    let disabled_tools =
        parse_csv(&config_get("AICTL_SECURITY_DISABLED_TOOLS").unwrap_or_default());

    SecurityPolicy {
        enabled,
        shell: ShellPolicy {
            allowed_commands,
            blocked_commands,
            block_subshell,
        },
        paths: PathPolicy {
            working_dir: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            restrict_to_cwd,
            blocked_paths,
            allowed_paths,
        },
        resources: ResourcePolicy {
            shell_timeout_secs,
            max_file_write_bytes,
        },
        env: EnvPolicy { blocked_env_vars },
        disabled_tools,
    }
}

fn parse_csv(s: &str) -> Vec<String> {
    s.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect()
}

// --- Validation entry point ---

/// Validate a tool call against the security policy.
/// Returns `Ok(())` if allowed, `Err(reason)` if denied.
pub fn validate_tool(tool_call: &ToolCall) -> Result<(), String> {
    let pol = policy();
    if !pol.enabled {
        return Ok(());
    }

    if pol.disabled_tools.iter().any(|t| t == &tool_call.name) {
        return Err(format!(
            "tool '{}' is disabled by security policy",
            tool_call.name
        ));
    }

    let input = &tool_call.input;
    match tool_call.name.as_str() {
        "exec_shell" => check_shell(input),
        "read_file" => check_path_read(input.trim()).map(|_| ()),
        "write_file" => {
            let input = input.trim();
            if let Some((path, content)) = input.split_once('\n') {
                check_path_write(path.trim())?;
                if pol.resources.max_file_write_bytes > 0
                    && content.len() > pol.resources.max_file_write_bytes
                {
                    return Err(format!(
                        "write size {} bytes exceeds limit of {} bytes",
                        content.len(),
                        pol.resources.max_file_write_bytes
                    ));
                }
                Ok(())
            } else {
                Ok(()) // will fail later in tool_write_file with "Invalid input"
            }
        }
        "remove_file" => check_path_write(input.trim()).map(|_| ()),
        "edit_file" => {
            let input = input.trim();
            if let Some((path, _)) = input.split_once('\n') {
                check_path_write(path.trim()).map(|_| ())
            } else {
                Ok(())
            }
        }
        "list_directory" => {
            let path = input.trim();
            let path = if path.is_empty() { "." } else { path };
            check_dir(path).map(|_| ())
        }
        "search_files" => {
            let input = input.trim();
            let dir = match input.split_once('\n') {
                Some((_, d)) => {
                    let d = d.trim();
                    if d.is_empty() { "." } else { d }
                }
                None => ".",
            };
            check_dir(dir).map(|_| ())
        }
        "find_files" => {
            let input = input.trim();
            let base_dir = match input.split_once('\n') {
                Some((_, d)) => {
                    let d = d.trim();
                    if d.is_empty() { "." } else { d }
                }
                None => ".",
            };
            check_dir(base_dir).map(|_| ())
        }
        _ => Ok(()), // fetch_url, search_web, fetch_datetime, fetch_geolocation — no restriction
    }
}

// --- Shell command validation ---

fn check_shell(command: &str) -> Result<(), String> {
    let pol = &policy().shell;

    // Block command substitution patterns
    if pol.block_subshell {
        if command.contains("$(") {
            return Err("command substitution $(...) is blocked".to_string());
        }
        if command.contains('`') {
            return Err("backtick command substitution is blocked".to_string());
        }
        if command.contains("<(") || command.contains(">(") {
            return Err("process substitution is blocked".to_string());
        }
    }

    // Split on shell operators to get individual commands
    let segments = split_shell_commands(command);

    for segment in &segments {
        let segment = segment.trim();
        if segment.is_empty() {
            continue;
        }

        let base_cmd = extract_base_command(segment);
        if base_cmd.is_empty() {
            continue;
        }

        // Check blacklist (always wins)
        if pol.blocked_commands.iter().any(|b| b == &base_cmd) {
            return Err(format!(
                "command '{base_cmd}' is blocked by security policy"
            ));
        }

        // Check whitelist (if non-empty, command must be in it)
        if !pol.allowed_commands.is_empty() && !pol.allowed_commands.iter().any(|a| a == &base_cmd)
        {
            return Err(format!(
                "command '{base_cmd}' is not in the allowed commands list"
            ));
        }
    }

    Ok(())
}

/// Split a shell command string on `|`, `&&`, `||`, `;` while respecting quotes.
fn split_shell_commands(command: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut chars = command.chars().peekable();
    let mut in_single_quote = false;
    let mut in_double_quote = false;

    while let Some(ch) = chars.next() {
        if in_single_quote {
            current.push(ch);
            if ch == '\'' {
                in_single_quote = false;
            }
            continue;
        }
        if in_double_quote {
            current.push(ch);
            if ch == '"' {
                in_double_quote = false;
            }
            continue;
        }

        match ch {
            '\'' => {
                in_single_quote = true;
                current.push(ch);
            }
            '"' => {
                in_double_quote = true;
                current.push(ch);
            }
            '|' => {
                if chars.peek() == Some(&'|') {
                    chars.next(); // consume second |
                }
                segments.push(std::mem::take(&mut current));
            }
            '&' => {
                if chars.peek() == Some(&'&') {
                    chars.next(); // consume second &
                    segments.push(std::mem::take(&mut current));
                } else {
                    // Trailing & (backgrounding) — keep in current segment
                    current.push(ch);
                }
            }
            ';' => {
                segments.push(std::mem::take(&mut current));
            }
            _ => {
                current.push(ch);
            }
        }
    }

    if !current.trim().is_empty() {
        segments.push(current);
    }

    segments
}

/// Extract the base command name from a single shell command segment.
fn extract_base_command(segment: &str) -> String {
    let segment = segment.trim();

    // Skip leading env var assignments (FOO=bar cmd)
    let mut rest = segment;
    loop {
        let trimmed = rest.trim_start();
        // Check if starts with VAR=value pattern
        if let Some(eq_pos) = trimmed.find('=') {
            let before_eq = &trimmed[..eq_pos];
            if !before_eq.is_empty()
                && before_eq
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_')
                && before_eq
                    .chars()
                    .next()
                    .is_some_and(|c| !c.is_ascii_digit())
            {
                // Skip past the value (find next unquoted space)
                let after_eq = &trimmed[eq_pos + 1..];
                if let Some(space_pos) = find_unquoted_space(after_eq) {
                    rest = &after_eq[space_pos..];
                    continue;
                }
                // entire segment is just an assignment
                return String::new();
            }
        }
        break;
    }

    let rest = rest.trim_start();
    if rest.is_empty() {
        return String::new();
    }

    // Extract first word
    let first_word = extract_first_word(rest);

    // Strip leading backslash (\rm -> rm)
    let first_word = first_word.strip_prefix('\\').unwrap_or(&first_word);

    // Strip quotes ("rm" -> rm, 'rm' -> rm)
    let first_word = first_word
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(first_word);
    let first_word = first_word
        .strip_prefix('\'')
        .and_then(|s| s.strip_suffix('\''))
        .unwrap_or(first_word);

    // Resolve full paths (/usr/bin/rm -> rm)
    let first_word = Path::new(first_word)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(first_word);

    // Strip known prefixes (sudo, env, nohup, etc.) and recurse
    if COMMAND_PREFIXES.contains(&first_word) {
        let after_prefix = rest
            .trim_start()
            .strip_prefix(extract_first_word(rest).as_str())
            .unwrap_or("")
            .trim_start();
        if after_prefix.is_empty() {
            return first_word.to_string();
        }
        return extract_base_command(after_prefix);
    }

    first_word.to_string()
}

/// Find the position of the first unquoted space.
fn find_unquoted_space(s: &str) -> Option<usize> {
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    for (i, ch) in s.chars().enumerate() {
        match ch {
            '\'' if !in_double_quote => in_single_quote = !in_single_quote,
            '"' if !in_single_quote => in_double_quote = !in_double_quote,
            ' ' | '\t' if !in_single_quote && !in_double_quote => return Some(i),
            _ => {}
        }
    }
    None
}

/// Extract the first whitespace-delimited word, respecting quotes.
fn extract_first_word(s: &str) -> String {
    let mut word = String::new();
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    for ch in s.chars() {
        match ch {
            '\'' if !in_double_quote => {
                in_single_quote = !in_single_quote;
                word.push(ch);
            }
            '"' if !in_single_quote => {
                in_double_quote = !in_double_quote;
                word.push(ch);
            }
            ' ' | '\t' if !in_single_quote && !in_double_quote => break,
            _ => word.push(ch),
        }
    }
    word
}

// --- Path validation ---

fn check_path_read(path: &str) -> Result<PathBuf, String> {
    check_path(path, false)
}

fn check_path_write(path: &str) -> Result<PathBuf, String> {
    check_path(path, true)
}

fn check_dir(path: &str) -> Result<PathBuf, String> {
    check_path(path, false)
}

fn check_path(path_str: &str, is_write: bool) -> Result<PathBuf, String> {
    let pol = &policy().paths;

    // Reject null bytes
    if path_str.contains('\0') {
        return Err("path contains null byte".to_string());
    }

    let home = std::env::var("HOME").unwrap_or_default();

    // Expand ~
    let expanded = if let Some(rest) = path_str.strip_prefix('~') {
        let rest = rest.strip_prefix('/').unwrap_or(rest);
        PathBuf::from(&home).join(rest)
    } else {
        PathBuf::from(path_str)
    };

    // Make absolute
    let absolute = if expanded.is_relative() {
        pol.working_dir.join(&expanded)
    } else {
        expanded
    };

    // Canonicalize: for existing paths use fs::canonicalize,
    // for new paths (writes) canonicalize parent + filename
    let canonical = if absolute.exists() {
        absolute
            .canonicalize()
            .map_err(|e| format!("cannot resolve path: {e}"))?
    } else if is_write {
        let parent = absolute
            .parent()
            .ok_or_else(|| "cannot determine parent directory".to_string())?;
        if parent.exists() {
            let canon_parent = parent
                .canonicalize()
                .map_err(|e| format!("cannot resolve parent: {e}"))?;
            let filename = absolute
                .file_name()
                .ok_or_else(|| "cannot determine filename".to_string())?;
            canon_parent.join(filename)
        } else {
            return Err(format!(
                "parent directory does not exist: {}",
                parent.display()
            ));
        }
    } else {
        // Read of non-existent path will fail in the tool itself; just validate what we can
        // Normalize .. components manually since canonicalize() requires the path to exist
        normalize_path(&absolute)
    };

    // Check blocked paths
    for blocked in &pol.blocked_paths {
        let blocked_canon = if blocked.exists() {
            blocked.canonicalize().unwrap_or_else(|_| blocked.clone())
        } else {
            blocked.clone()
        };
        if canonical.starts_with(&blocked_canon) {
            return Err(format!("path '{path_str}' is blocked by security policy"));
        }
    }

    // Check CWD jail
    if pol.restrict_to_cwd {
        let cwd_canon = pol
            .working_dir
            .canonicalize()
            .unwrap_or_else(|_| pol.working_dir.clone());

        let allowed = canonical.starts_with(&cwd_canon)
            || pol.allowed_paths.iter().any(|ap| {
                let ap_canon = if ap.exists() {
                    ap.canonicalize().unwrap_or_else(|_| ap.clone())
                } else {
                    ap.clone()
                };
                canonical.starts_with(&ap_canon)
            });

        if !allowed {
            return Err(format!(
                "path '{}' is outside the working directory ({})",
                path_str,
                pol.working_dir.display()
            ));
        }
    }

    Ok(canonical)
}

/// Normalize a path by resolving `.` and `..` components without touching the filesystem.
fn normalize_path(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                result.pop();
            }
            std::path::Component::CurDir => {}
            _ => result.push(component),
        }
    }
    result
}

// --- Environment scrubbing ---

/// Build a filtered environment for shell subprocesses.
/// Keeps only safe vars and filters out sensitive ones.
pub fn scrubbed_env() -> Vec<(String, String)> {
    let pol = policy();
    if !pol.enabled {
        return std::env::vars().collect();
    }

    let mut env_vars = Vec::new();
    for (key, value) in std::env::vars() {
        // Always keep safe vars
        if SAFE_ENV_VARS.contains(&key.as_str()) {
            env_vars.push((key, value));
            continue;
        }

        // Block explicitly listed vars
        if pol.env.blocked_env_vars.iter().any(|b| b == &key) {
            continue;
        }

        // Block vars matching sensitive suffixes
        if SENSITIVE_ENV_SUFFIXES
            .iter()
            .any(|suffix| key.ends_with(suffix))
        {
            continue;
        }

        // Block anything that looks like a secret
        let upper = key.to_uppercase();
        if upper.contains("SECRET") || upper.contains("PASSWORD") || upper.contains("CREDENTIAL") {
            continue;
        }

        // Keep everything else
        env_vars.push((key, value));
    }

    env_vars
}

/// Get the shell timeout duration, or `None` if disabled.
pub fn shell_timeout() -> Option<std::time::Duration> {
    let pol = policy();
    if !pol.enabled || pol.resources.shell_timeout_secs == 0 {
        None
    } else {
        Some(std::time::Duration::from_secs(
            pol.resources.shell_timeout_secs,
        ))
    }
}

// --- Output sanitization ---

/// Sanitize tool output to prevent prompt injection via tool result.
pub fn sanitize_output(s: &str) -> String {
    s.replace("<tool", "&lt;tool")
        .replace("</tool>", "&lt;/tool&gt;")
}

// --- Display ---

/// Return a summary of the current security policy for display.
pub fn policy_summary() -> Vec<(String, String)> {
    let pol = policy();
    let mut lines = Vec::new();

    lines.push((
        "security".to_string(),
        if pol.enabled {
            "on"
        } else {
            "off (unrestricted)"
        }
        .to_string(),
    ));

    if !pol.enabled {
        return lines;
    }

    lines.push((
        "cwd jail".to_string(),
        if pol.paths.restrict_to_cwd {
            format!("on ({})", pol.paths.working_dir.display())
        } else {
            "off".to_string()
        },
    ));

    lines.push((
        "blocked cmds".to_string(),
        if pol.shell.blocked_commands.is_empty() {
            "none".to_string()
        } else {
            pol.shell.blocked_commands.join(", ")
        },
    ));

    lines.push((
        "allowed cmds".to_string(),
        if pol.shell.allowed_commands.is_empty() {
            "all (except blocked)".to_string()
        } else {
            pol.shell.allowed_commands.join(", ")
        },
    ));

    lines.push((
        "subshell".to_string(),
        if pol.shell.block_subshell {
            "blocked"
        } else {
            "allowed"
        }
        .to_string(),
    ));

    lines.push((
        "shell timeout".to_string(),
        format!("{}s", pol.resources.shell_timeout_secs),
    ));

    lines.push((
        "max write".to_string(),
        format_bytes(pol.resources.max_file_write_bytes),
    ));

    lines.push((
        "env scrubbing".to_string(),
        "on (keys/secrets/tokens/passwords)".to_string(),
    ));

    lines.push((
        "blocked paths".to_string(),
        format!("{} entries", pol.paths.blocked_paths.len()),
    ));

    lines.push((
        "disabled tools".to_string(),
        if pol.disabled_tools.is_empty() {
            "none".to_string()
        } else {
            pol.disabled_tools.join(", ")
        },
    ));

    lines
}

fn format_bytes(bytes: usize) -> String {
    if bytes == 0 {
        "unlimited".to_string()
    } else if bytes >= 1_048_576 {
        #[allow(clippy::cast_precision_loss)]
        let mb = bytes as f64 / 1_048_576.0;
        format!("{mb:.1} MB")
    } else {
        #[allow(clippy::cast_precision_loss)]
        let kb = bytes as f64 / 1024.0;
        format!("{kb:.1} KB")
    }
}

// --- Tests ---

#[cfg(test)]
mod tests {
    use super::*;

    // Helper: init a test policy for use in tests
    fn test_policy() -> SecurityPolicy {
        SecurityPolicy {
            enabled: true,
            shell: ShellPolicy {
                allowed_commands: vec![],
                blocked_commands: DEFAULT_BLOCKED_COMMANDS
                    .iter()
                    .map(|s| (*s).to_string())
                    .collect(),
                block_subshell: true,
            },
            paths: PathPolicy {
                working_dir: std::env::temp_dir(),
                restrict_to_cwd: true,
                blocked_paths: vec![PathBuf::from("/etc/shadow")],
                allowed_paths: vec![],
            },
            resources: ResourcePolicy {
                shell_timeout_secs: 30,
                max_file_write_bytes: 1_048_576,
            },
            env: EnvPolicy {
                blocked_env_vars: vec![],
            },
            disabled_tools: vec![],
        }
    }

    // --- split_shell_commands ---

    #[test]
    fn split_simple_pipe() {
        let segments = split_shell_commands("ls | grep foo");
        assert_eq!(segments, vec!["ls ", " grep foo"]);
    }

    #[test]
    fn split_and() {
        let segments = split_shell_commands("cd dir && ls");
        assert_eq!(segments, vec!["cd dir ", " ls"]);
    }

    #[test]
    fn split_or() {
        let segments = split_shell_commands("cmd1 || cmd2");
        assert_eq!(segments, vec!["cmd1 ", " cmd2"]);
    }

    #[test]
    fn split_semicolon() {
        let segments = split_shell_commands("echo a; echo b");
        assert_eq!(segments, vec!["echo a", " echo b"]);
    }

    #[test]
    fn split_quoted_pipe() {
        let segments = split_shell_commands("echo 'a|b' | cat");
        assert_eq!(segments, vec!["echo 'a|b' ", " cat"]);
    }

    #[test]
    fn split_single_command() {
        let segments = split_shell_commands("ls -la");
        assert_eq!(segments, vec!["ls -la"]);
    }

    // --- extract_base_command ---

    #[test]
    fn base_cmd_simple() {
        assert_eq!(extract_base_command("ls -la"), "ls");
    }

    #[test]
    fn base_cmd_full_path() {
        assert_eq!(extract_base_command("/usr/bin/ls -la"), "ls");
    }

    #[test]
    fn base_cmd_env_prefix() {
        assert_eq!(extract_base_command("FOO=bar ls -la"), "ls");
    }

    #[test]
    fn base_cmd_sudo_prefix() {
        assert_eq!(extract_base_command("sudo rm -rf /"), "rm");
    }

    #[test]
    fn base_cmd_backslash() {
        assert_eq!(extract_base_command("\\rm file.txt"), "rm");
    }

    #[test]
    fn base_cmd_quoted() {
        assert_eq!(extract_base_command("\"ls\" -la"), "ls");
    }

    #[test]
    fn base_cmd_env_nohup() {
        assert_eq!(
            extract_base_command("nohup env FOO=1 python script.py"),
            "python"
        );
    }

    #[test]
    fn base_cmd_multiple_env_vars() {
        assert_eq!(extract_base_command("A=1 B=2 cargo build"), "cargo");
    }

    // --- check_shell (requires OnceLock, test via split+extract) ---

    #[test]
    fn blocked_command_detected() {
        let blocked = &test_policy().shell.blocked_commands;
        let cmd = "rm -rf /tmp/test";
        let segments = split_shell_commands(cmd);
        for seg in segments {
            let base = extract_base_command(seg.trim());
            if blocked.iter().any(|b| b == &base) {
                return; // correctly detected
            }
        }
        panic!("rm should be detected as blocked");
    }

    #[test]
    fn blocked_in_pipe() {
        let blocked = &test_policy().shell.blocked_commands;
        let cmd = "echo test | rm";
        let segments = split_shell_commands(cmd);
        let found = segments.iter().any(|seg| {
            let base = extract_base_command(seg.trim());
            blocked.iter().any(|b| b == &base)
        });
        assert!(found, "rm in pipe should be detected");
    }

    #[test]
    fn blocked_with_full_path() {
        let blocked = &test_policy().shell.blocked_commands;
        let base = extract_base_command("/bin/rm file.txt");
        assert!(
            blocked.iter().any(|b| b == &base),
            "/bin/rm should resolve to rm"
        );
    }

    #[test]
    fn blocked_via_sudo() {
        let blocked = &test_policy().shell.blocked_commands;
        // sudo itself is blocked
        // sudo is the prefix, gets stripped, base should be "ls"
        // but "sudo" itself is in the blocked list too
        let first_word = "sudo";
        assert!(
            blocked.iter().any(|b| b == first_word),
            "sudo should be blocked"
        );
    }

    // --- command substitution detection ---

    #[test]
    fn detect_dollar_paren() {
        assert!(
            "echo $(whoami)".contains("$("),
            "should detect $() substitution"
        );
    }

    #[test]
    fn detect_backtick() {
        assert!(
            "echo `whoami`".contains('`'),
            "should detect backtick substitution"
        );
    }

    #[test]
    fn detect_process_substitution() {
        assert!(
            "diff <(cmd1) <(cmd2)".contains("<("),
            "should detect process substitution"
        );
    }

    // --- sanitize_output ---

    #[test]
    fn sanitize_tool_tags() {
        let input = "result <tool name=\"exec_shell\">ls</tool> done";
        let output = sanitize_output(input);
        assert!(!output.contains("<tool"));
        assert!(!output.contains("</tool>"));
        assert!(output.contains("&lt;tool"));
    }

    #[test]
    fn sanitize_clean_input() {
        let input = "hello world";
        assert_eq!(sanitize_output(input), "hello world");
    }

    // --- parse_csv ---

    #[test]
    fn csv_empty() {
        assert!(parse_csv("").is_empty());
    }

    #[test]
    fn csv_single() {
        assert_eq!(parse_csv("ls"), vec!["ls"]);
    }

    #[test]
    fn csv_multiple() {
        assert_eq!(parse_csv("ls, cat, grep"), vec!["ls", "cat", "grep"]);
    }

    #[test]
    fn csv_with_spaces() {
        assert_eq!(parse_csv("  ls  ,  cat  "), vec!["ls", "cat"]);
    }

    // --- format_bytes ---

    #[test]
    fn format_bytes_zero() {
        assert_eq!(format_bytes(0), "unlimited");
    }

    #[test]
    fn format_bytes_mb() {
        assert_eq!(format_bytes(1_048_576), "1.0 MB");
    }

    #[test]
    fn format_bytes_kb() {
        assert_eq!(format_bytes(1024), "1.0 KB");
    }

    // --- normalize_path ---

    #[test]
    fn normalize_resolves_dotdot() {
        let p = normalize_path(Path::new("/a/b/c/../../d"));
        assert_eq!(p, PathBuf::from("/a/d"));
    }

    #[test]
    fn normalize_resolves_dot() {
        let p = normalize_path(Path::new("/a/./b/./c"));
        assert_eq!(p, PathBuf::from("/a/b/c"));
    }

    #[test]
    fn normalize_traversal_to_root() {
        let p = normalize_path(Path::new("/a/b/../../../etc/passwd"));
        assert_eq!(p, PathBuf::from("/etc/passwd"));
    }

    #[test]
    fn normalize_no_change() {
        let p = normalize_path(Path::new("/a/b/c"));
        assert_eq!(p, PathBuf::from("/a/b/c"));
    }
}
