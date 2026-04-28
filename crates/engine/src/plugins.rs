//! User-installed plugin tools.
//!
//! A plugin is an executable (script or binary) plus a `plugin.toml`
//! manifest dropped under `~/.aictl/plugins/<name>/`. At startup
//! [`init`] walks the directory, parses each manifest, validates the
//! entrypoint, and stores the survivors. The agent loop dispatches to
//! a plugin via [`execute_plugin`] when a `<tool>` tag names one — the
//! built-in dispatch table in [`crate::tools::execute_tool`] falls
//! through to [`find`] before returning "Unknown tool".
//!
//! Plugins are gated behind `AICTL_PLUGINS_ENABLED` (default `false`)
//! because they execute third-party code. The standard security gate
//! (`security::validate_tool`) still runs, so per-tool disables and
//! the `--unrestricted` bypass behave identically to built-in tools.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::config::config_get;

static PLUGINS: OnceLock<Vec<Plugin>> = OnceLock::new();

/// One discovered plugin.
#[derive(Debug, Clone)]
pub struct Plugin {
    pub name: String,
    pub dir: PathBuf,
    pub entrypoint: PathBuf,
    pub description: String,
    pub schema_hint: Option<String>,
    pub requires_confirmation: bool,
    pub timeout_secs: Option<u64>,
}

impl Plugin {
    /// Catalogue line as it appears in the system prompt: combines
    /// description and (if present) the schema hint with a blank line.
    pub fn catalog_body(&self) -> String {
        match self.schema_hint.as_deref() {
            Some(hint) if !hint.trim().is_empty() => {
                format!("{}\n\n{}", self.description.trim(), hint.trim())
            }
            _ => self.description.trim().to_string(),
        }
    }
}

/// Returns whether the plugin subsystem is opted in. Default `false`:
/// plugins must not load silently because they're third-party code.
pub fn enabled() -> bool {
    matches!(config_get("AICTL_PLUGINS_ENABLED").as_deref(), Some(v) if v != "false" && v != "0")
}

/// Root directory holding `<name>/plugin.toml`. Override via
/// `AICTL_PLUGINS_DIR` (used by tests).
pub fn plugins_dir() -> PathBuf {
    if let Some(dir) = config_get("AICTL_PLUGINS_DIR") {
        return PathBuf::from(dir);
    }
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(format!("{home}/.aictl/plugins"))
}

/// Directory names matching the same character set as agent / skill names.
fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn disabled_set() -> Vec<String> {
    config_get("AICTL_PLUGINS_DISABLED")
        .map(|raw| {
            raw.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

/// Initialize the global plugin catalogue. Idempotent: subsequent calls
/// are no-ops once `PLUGINS` is set. Walk failures (missing dir, bad
/// manifests) leave the catalogue empty rather than aborting startup —
/// plugins are always best-effort.
pub fn init() {
    let plugins = if enabled() {
        discover(&plugins_dir(), &disabled_set(), &builtin_tool_names())
    } else {
        Vec::new()
    };
    let _ = PLUGINS.set(plugins);
}

/// All loaded plugins.
pub fn list() -> &'static [Plugin] {
    PLUGINS.get().map_or(&[], Vec::as_slice)
}

/// Lookup by tool name.
pub fn find(name: &str) -> Option<&'static Plugin> {
    list().iter().find(|p| p.name == name)
}

/// Built-in tool names — used to reject manifest files that try to
/// shadow a real tool. Kept aligned with the dispatch table in
/// `tools.rs`. A drift here would mean a plugin silently lost (still
/// the safe outcome) — never the other direction.
fn builtin_tool_names() -> Vec<String> {
    vec![
        "exec_shell".to_string(),
        "read_file".to_string(),
        "write_file".to_string(),
        "remove_file".to_string(),
        "create_directory".to_string(),
        "list_directory".to_string(),
        "search_files".to_string(),
        "edit_file".to_string(),
        "diff_files".to_string(),
        "search_web".to_string(),
        "find_files".to_string(),
        "fetch_url".to_string(),
        "extract_website".to_string(),
        "fetch_datetime".to_string(),
        "fetch_geolocation".to_string(),
        "read_image".to_string(),
        "generate_image".to_string(),
        "read_document".to_string(),
        "git".to_string(),
        "run_code".to_string(),
        "lint_file".to_string(),
        "json_query".to_string(),
        "csv_query".to_string(),
        "calculate".to_string(),
        "list_processes".to_string(),
        "check_port".to_string(),
        "system_info".to_string(),
        "archive".to_string(),
        "checksum".to_string(),
        "clipboard".to_string(),
        "notify".to_string(),
    ]
}

/// Walk `root`, parse every `<name>/plugin.toml`, return the survivors.
/// Entries are sorted alphabetically by name so `/plugins` output and
/// the catalog injection are deterministic.
fn discover(root: &Path, disabled: &[String], reserved: &[String]) -> Vec<Plugin> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut out: Vec<Plugin> = entries
        .filter_map(|e| {
            let entry = e.ok()?;
            let ft = entry.file_type().ok()?;
            if !ft.is_dir() {
                return None;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if !is_valid_name(&name) {
                eprintln!("plugin '{name}': skipped (invalid directory name)");
                return None;
            }
            if disabled.iter().any(|d| d == &name) {
                return None;
            }
            if reserved.iter().any(|r| r == &name) {
                eprintln!(
                    "plugin '{name}': skipped (name collides with a built-in tool — rename the directory)"
                );
                return None;
            }
            match load_manifest(&entry.path(), &name) {
                Ok(plugin) => Some(plugin),
                Err(reason) => {
                    eprintln!("plugin '{name}': skipped ({reason})");
                    None
                }
            }
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn load_manifest(plugin_dir: &Path, expected_name: &str) -> Result<Plugin, String> {
    let manifest_path = plugin_dir.join("plugin.toml");
    let raw = std::fs::read_to_string(&manifest_path)
        .map_err(|e| format!("cannot read {}: {e}", manifest_path.display()))?;
    let manifest = parse_manifest(&raw)?;

    let name = manifest.name.unwrap_or_default();
    if name.is_empty() {
        return Err("manifest missing required field 'name'".to_string());
    }
    if name != expected_name {
        return Err(format!(
            "manifest name '{name}' does not match directory name '{expected_name}'"
        ));
    }
    let description = manifest.description.unwrap_or_default();
    if description.trim().is_empty() {
        return Err("manifest missing required field 'description'".to_string());
    }

    let entrypoint_rel = manifest.entrypoint.unwrap_or_else(|| "run".to_string());
    let entrypoint = resolve_entrypoint(plugin_dir, &entrypoint_rel)?;

    Ok(Plugin {
        name,
        dir: plugin_dir.to_path_buf(),
        entrypoint,
        description,
        schema_hint: manifest.schema_hint,
        requires_confirmation: manifest.requires_confirmation.unwrap_or(true),
        timeout_secs: manifest.timeout_secs,
    })
}

/// Resolve the entrypoint relative to the plugin directory and confirm
/// it stays inside (rejects symlink escapes), exists, and on Unix is
/// marked executable.
fn resolve_entrypoint(plugin_dir: &Path, rel: &str) -> Result<PathBuf, String> {
    if rel.contains('\0') {
        return Err("entrypoint contains null byte".to_string());
    }
    let candidate = plugin_dir.join(rel);
    if !candidate.exists() {
        return Err(format!(
            "entrypoint '{rel}' not found in {}",
            plugin_dir.display()
        ));
    }
    let canonical = candidate
        .canonicalize()
        .map_err(|e| format!("cannot resolve entrypoint '{rel}': {e}"))?;
    let plugin_canonical = plugin_dir
        .canonicalize()
        .map_err(|e| format!("cannot resolve plugin dir: {e}"))?;
    if !canonical.starts_with(&plugin_canonical) {
        return Err(format!(
            "entrypoint '{rel}' resolves outside the plugin directory"
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let meta =
            std::fs::metadata(&canonical).map_err(|e| format!("cannot stat entrypoint: {e}"))?;
        if meta.permissions().mode() & 0o111 == 0 {
            return Err(format!("entrypoint '{rel}' is not executable (chmod +x)"));
        }
    }
    Ok(canonical)
}

#[derive(Default)]
struct Manifest {
    name: Option<String>,
    description: Option<String>,
    entrypoint: Option<String>,
    requires_confirmation: Option<bool>,
    timeout_secs: Option<u64>,
    schema_hint: Option<String>,
}

/// Parse the limited subset of TOML the manifest needs:
///
/// * `key = "string"` (single line, double-quoted)
/// * `key = bool` / `key = number`
/// * `key = """multi-line string"""`
/// * `# comment` lines / blank lines
///
/// We avoid pulling in the `toml` crate to keep the dep set small —
/// every existing parser in this codebase (config, frontmatter) is
/// hand-rolled in the same spirit.
fn parse_manifest(raw: &str) -> Result<Manifest, String> {
    let mut m = Manifest::default();
    let mut lines = raw.lines().peekable();
    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((key, rest)) = trimmed.split_once('=') else {
            return Err(format!("malformed line (no '='): {line}"));
        };
        let key = key.trim();
        let rest = rest.trim();
        // Multi-line triple-quoted string: collect until closing fence.
        let value: Value = if let Some(after_open) = rest.strip_prefix("\"\"\"") {
            // Single-line triple-quoted: `key = """text"""`
            if let Some(end) = after_open.strip_suffix("\"\"\"") {
                Value::String(end.trim_start_matches('\n').to_string())
            } else {
                let mut buf = String::new();
                if !after_open.is_empty() {
                    buf.push_str(after_open);
                    buf.push('\n');
                }
                let mut closed = false;
                for tail in lines.by_ref() {
                    if let Some(before) = tail.strip_suffix("\"\"\"") {
                        buf.push_str(before);
                        closed = true;
                        break;
                    }
                    buf.push_str(tail);
                    buf.push('\n');
                }
                if !closed {
                    return Err(format!("unterminated triple-quoted string for '{key}'"));
                }
                Value::String(buf)
            }
        } else if let Some(stripped) = rest.strip_prefix('"') {
            let body = stripped
                .strip_suffix('"')
                .ok_or_else(|| format!("unterminated string for '{key}'"))?;
            Value::String(unescape_basic(body))
        } else if rest.eq_ignore_ascii_case("true") {
            Value::Bool(true)
        } else if rest.eq_ignore_ascii_case("false") {
            Value::Bool(false)
        } else if let Ok(n) = rest.parse::<u64>() {
            Value::Int(n)
        } else {
            return Err(format!("unsupported value for '{key}': {rest}"));
        };

        match (key, value) {
            ("name", Value::String(s)) => m.name = Some(s),
            ("version", _) => {} // accepted but unused
            ("description", Value::String(s)) => m.description = Some(s),
            ("entrypoint", Value::String(s)) => m.entrypoint = Some(s),
            ("requires_confirmation", Value::Bool(b)) => m.requires_confirmation = Some(b),
            ("timeout_secs", Value::Int(n)) => m.timeout_secs = Some(n),
            ("schema_hint", Value::String(s)) => m.schema_hint = Some(s),
            (other, _) => return Err(format!("unknown or mistyped field: {other}")),
        }
    }
    Ok(m)
}

enum Value {
    String(String),
    Bool(bool),
    Int(u64),
}

fn unescape_basic(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('"') => out.push('"'),
                Some('\\') | None => out.push('\\'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Spawn `plugin.entrypoint` with `body` on stdin, return stdout (or
/// `[exit N] <stderr>` on failure). Honors the plugin's manifest
/// timeout, falling back to the global `security::shell_timeout` when
/// unset. Pinned to the security CWD with a scrubbed env.
pub async fn execute_plugin(plugin: &Plugin, body: &str) -> String {
    use tokio::io::AsyncWriteExt;
    use tokio::process::Command;

    let mut cmd = Command::new(&plugin.entrypoint);
    cmd.env_clear();
    for (key, value) in crate::security::scrubbed_env() {
        cmd.env(key, value);
    }
    cmd.current_dir(crate::security::policy().paths.working_dir.clone());
    cmd.stdin(std::process::Stdio::piped());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    cmd.kill_on_drop(true);

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return format!("Error spawning plugin '{}': {e}", plugin.name),
    };

    if let Some(mut stdin) = child.stdin.take() {
        let body_bytes = body.as_bytes().to_vec();
        if let Err(e) = stdin.write_all(&body_bytes).await {
            return format!("Error writing plugin stdin: {e}");
        }
        // Dropping `stdin` closes the pipe so the child sees EOF.
        drop(stdin);
    }

    let timeout = plugin
        .timeout_secs
        .map(std::time::Duration::from_secs)
        .or_else(crate::security::shell_timeout);

    let output_future = child.wait_with_output();
    let output = if let Some(t) = timeout {
        match tokio::time::timeout(t, output_future).await {
            Ok(r) => r,
            Err(_) => {
                return format!(
                    "Error: plugin '{}' timed out after {}s",
                    plugin.name,
                    t.as_secs()
                );
            }
        }
    } else {
        output_future.await
    };

    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout).to_string();
            if out.status.success() {
                if stdout.is_empty() {
                    "(no output)".to_string()
                } else {
                    stdout
                }
            } else {
                let code = out.status.code().unwrap_or(-1);
                let stderr = String::from_utf8_lossy(&out.stderr);
                let stderr = stderr.trim();
                if stderr.is_empty() {
                    format!("[exit {code}]")
                } else {
                    format!("[exit {code}] {stderr}")
                }
            }
        }
        Err(e) => format!("Error waiting on plugin '{}': {e}", plugin.name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_temp_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let pid = std::process::id();
        let p = std::env::temp_dir().join(format!("aictl-plugins-test-{tag}-{pid}-{nanos}"));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn parse_manifest_accepts_known_fields() {
        let raw = r#"
name = "demo"
version = "0.1.0"
description = "echo input"
entrypoint = "run"
requires_confirmation = false
timeout_secs = 10
schema_hint = """
line one
line two
"""
"#;
        let m = parse_manifest(raw).unwrap();
        assert_eq!(m.name.as_deref(), Some("demo"));
        assert_eq!(m.description.as_deref(), Some("echo input"));
        assert_eq!(m.entrypoint.as_deref(), Some("run"));
        assert_eq!(m.requires_confirmation, Some(false));
        assert_eq!(m.timeout_secs, Some(10));
        assert!(m.schema_hint.unwrap().contains("line one"));
    }

    #[test]
    fn parse_manifest_rejects_unknown_field() {
        let raw = r#"
name = "demo"
description = "x"
weird_field = "no"
"#;
        assert!(parse_manifest(raw).is_err());
    }

    #[test]
    fn parse_manifest_rejects_unterminated_string() {
        let raw = "name = \"demo";
        assert!(parse_manifest(raw).is_err());
    }

    #[test]
    fn parse_manifest_handles_comments_and_blank_lines() {
        let raw = "# top comment\n\nname = \"demo\"\n# trailing\ndescription = \"x\"\n";
        let m = parse_manifest(raw).unwrap();
        assert_eq!(m.name.as_deref(), Some("demo"));
        assert_eq!(m.description.as_deref(), Some("x"));
    }

    #[test]
    fn parse_manifest_single_line_triple_quoted() {
        let raw = r#"
name = "x"
description = "x"
schema_hint = """one shot"""
"#;
        let m = parse_manifest(raw).unwrap();
        assert_eq!(m.schema_hint.as_deref(), Some("one shot"));
    }

    #[test]
    fn discover_skips_invalid_and_collisions() {
        let root = unique_temp_dir("discover");
        // Valid plugin.
        let good = root.join("good_plugin");
        std::fs::create_dir_all(&good).unwrap();
        let entry_path = good.join("run");
        std::fs::write(&entry_path, "#!/bin/sh\necho hi\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&entry_path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&entry_path, perms).unwrap();
        }
        std::fs::write(
            good.join("plugin.toml"),
            "name = \"good_plugin\"\ndescription = \"x\"\n",
        )
        .unwrap();

        // Collision with built-in tool name.
        let collision = root.join("read_file");
        std::fs::create_dir_all(&collision).unwrap();
        std::fs::write(
            collision.join("plugin.toml"),
            "name = \"read_file\"\ndescription = \"x\"\n",
        )
        .unwrap();

        // Bad-name directory (dot).
        let bad = root.join("bad.name");
        std::fs::create_dir_all(&bad).unwrap();

        let plugins = discover(&root, &[], &builtin_tool_names());
        let names: Vec<_> = plugins.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["good_plugin"]);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn discover_respects_disabled_list() {
        let root = unique_temp_dir("disabled");
        let dir = root.join("muted");
        std::fs::create_dir_all(&dir).unwrap();
        let entry_path = dir.join("run");
        std::fs::write(&entry_path, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&entry_path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&entry_path, perms).unwrap();
        }
        std::fs::write(
            dir.join("plugin.toml"),
            "name = \"muted\"\ndescription = \"x\"\n",
        )
        .unwrap();
        let plugins = discover(&root, &["muted".to_string()], &builtin_tool_names());
        assert!(plugins.is_empty());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn resolve_entrypoint_blocks_symlink_escape() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let root = unique_temp_dir("escape");
            let outside = unique_temp_dir("escape_target");
            std::fs::write(outside.join("evil"), "#!/bin/sh\n").unwrap();
            symlink(outside.join("evil"), root.join("link")).unwrap();
            let err = resolve_entrypoint(&root, "link").unwrap_err();
            assert!(err.contains("outside"));
            std::fs::remove_dir_all(&root).ok();
            std::fs::remove_dir_all(&outside).ok();
        }
    }

    #[test]
    fn catalog_body_combines_description_and_hint() {
        let p = Plugin {
            name: "x".into(),
            dir: PathBuf::from("/tmp"),
            entrypoint: PathBuf::from("/tmp/run"),
            description: "Run the X.".into(),
            schema_hint: Some("first line: foo\nsecond line: bar".into()),
            requires_confirmation: true,
            timeout_secs: None,
        };
        let body = p.catalog_body();
        assert!(body.contains("Run the X."));
        assert!(body.contains("first line: foo"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn execute_plugin_round_trip_stdout() {
        let root = unique_temp_dir("exec_ok");
        let dir = root.join("echo_back");
        std::fs::create_dir_all(&dir).unwrap();
        let entry = dir.join("run");
        std::fs::write(&entry, "#!/bin/sh\ncat\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&entry).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&entry, perms).unwrap();
        std::fs::write(
            dir.join("plugin.toml"),
            "name = \"echo_back\"\ndescription = \"x\"\n",
        )
        .unwrap();
        let plugin = load_manifest(&dir, "echo_back").unwrap();
        let out = execute_plugin(&plugin, "hello there").await;
        assert_eq!(out.trim(), "hello there");
        std::fs::remove_dir_all(&root).ok();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn execute_plugin_surfaces_nonzero_exit() {
        let root = unique_temp_dir("exec_fail");
        let dir = root.join("boom");
        std::fs::create_dir_all(&dir).unwrap();
        let entry = dir.join("run");
        std::fs::write(&entry, "#!/bin/sh\necho nope >&2\nexit 7\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&entry).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&entry, perms).unwrap();
        std::fs::write(
            dir.join("plugin.toml"),
            "name = \"boom\"\ndescription = \"x\"\n",
        )
        .unwrap();
        let plugin = load_manifest(&dir, "boom").unwrap();
        let out = execute_plugin(&plugin, "").await;
        assert!(out.starts_with("[exit 7]"));
        assert!(out.contains("nope"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn execute_plugin_times_out() {
        let root = unique_temp_dir("exec_timeout");
        let dir = root.join("slow");
        std::fs::create_dir_all(&dir).unwrap();
        let entry = dir.join("run");
        std::fs::write(&entry, "#!/bin/sh\nsleep 5\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&entry).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&entry, perms).unwrap();
        std::fs::write(
            dir.join("plugin.toml"),
            "name = \"slow\"\ndescription = \"x\"\ntimeout_secs = 1\n",
        )
        .unwrap();
        let plugin = load_manifest(&dir, "slow").unwrap();
        let out = execute_plugin(&plugin, "").await;
        assert!(out.contains("timed out"));
        std::fs::remove_dir_all(&root).ok();
    }
}
