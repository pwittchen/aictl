use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

static CONFIG: OnceLock<RwLock<HashMap<String, String>>> = OnceLock::new();
static HTTP_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

/// Return a shared `reqwest::Client`, creating it on first access.
pub fn http_client() -> &'static reqwest::Client {
    HTTP_CLIENT.get_or_init(reqwest::Client::new)
}

// --- Agent loop limits ---

pub const DEFAULT_MAX_ITERATIONS: usize = 20;
pub const MAX_MESSAGES: usize = 200;
pub const MAX_TOOL_OUTPUT_LEN: usize = 10_000;
pub const MAX_RESPONSE_TOKENS: u32 = 4096;
pub const SHORT_TERM_MEMORY_WINDOW: usize = 20;
pub const DEFAULT_AUTO_COMPACT_THRESHOLD: u8 = 80;

/// Return the maximum number of LLM calls allowed in a single agent turn.
///
/// Read from `AICTL_MAX_ITERATIONS` in `~/.aictl/config`. Values that are
/// missing, unparseable, or below `1` fall back to `DEFAULT_MAX_ITERATIONS`.
/// Bounds the agent loop so a runaway tool-call cycle terminates instead of
/// burning tokens forever.
pub fn max_iterations() -> usize {
    config_get("AICTL_MAX_ITERATIONS")
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|v| *v >= 1)
        .unwrap_or(DEFAULT_MAX_ITERATIONS)
}

/// Default timeout (seconds) for a single LLM provider call.
///
/// Picked to accommodate slow native GGUF/MLX inference on modest hardware
/// while still bounding API hangs. Users can shorten or extend this via
/// `AICTL_LLM_TIMEOUT` in `~/.aictl/config`.
pub const DEFAULT_LLM_TIMEOUT_SECS: u64 = 30;

/// Return the auto-compact threshold as a percentage (1..=100).
///
/// Read from `AICTL_AUTO_COMPACT_THRESHOLD` in `~/.aictl/config`. Values outside
/// the 1..=100 range (or unparseable values) fall back to `DEFAULT_AUTO_COMPACT_THRESHOLD`.
pub fn auto_compact_threshold() -> u8 {
    config_get("AICTL_AUTO_COMPACT_THRESHOLD")
        .and_then(|v| v.parse::<u8>().ok())
        .filter(|v| (1..=100).contains(v))
        .unwrap_or(DEFAULT_AUTO_COMPACT_THRESHOLD)
}

/// Return the per-call LLM timeout as a `Duration`.
///
/// Read from `AICTL_LLM_TIMEOUT` (in seconds) in `~/.aictl/config`. A value of
/// `0` disables the timeout entirely (wrapping the call in an effectively
/// infinite duration). Unparseable values fall back to
/// `DEFAULT_LLM_TIMEOUT_SECS`. Applied uniformly to every provider — remote
/// HTTP calls, native GGUF/MLX, and Ollama.
pub fn llm_timeout() -> std::time::Duration {
    let secs = config_get("AICTL_LLM_TIMEOUT")
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_LLM_TIMEOUT_SECS);
    if secs == 0 {
        // A zero value means "no timeout"; tokio::time::timeout still needs a
        // Duration, so use the largest safe value we can hand it.
        std::time::Duration::from_secs(u64::MAX / 2)
    } else {
        std::time::Duration::from_secs(secs)
    }
}

// --- Spinner phrases ---

pub const SPINNER_PHRASES: &[&str] = &[
    "consulting the mass of wires...",
    "asking the silicon oracle...",
    "shaking the magic 8-ball...",
    "reticulating splines...",
    "bribing the electrons...",
    "poking the neural hamsters...",
    "unfolding the paper brain...",
    "warming up the thought lasers...",
    "juggling tensors...",
    "feeding the token monster...",
    "polishing the crystal CPU...",
    "summoning the context window...",
    "defrosting the weights...",
    "herding stochastic parrots...",
    "spinning up the vibe engine...",
    "negotiating with gradient descent...",
    "tuning the hallucination dial...",
    "charging the inference hamster wheel...",
    "compressing the universe into tokens...",
    "asking a very expensive rock to think...",
    "thinking...",
];

// --- System prompt ---

pub const SYSTEM_PROMPT: &str = r#"You have access to tools that let you interact with the user's system. To use a tool, output an XML tag like this:

<tool name="exec_shell">
command here
</tool>

Available tools:
- exec_shell: Execute a shell command. The command runs via `sh -c`.
- read_file: Read the contents of a file. Pass the file path as the input.
- write_file: Write content to a file. First line is the file path, remaining lines are the content.
- remove_file: Remove (delete) a file. Pass the file path as the input. Only removes regular files, not directories.
- create_directory: Create a directory (and any missing parent directories). Pass the directory path as input.
- list_directory: List files and directories at a path. Pass the directory path as input. Returns entries with [FILE] or [DIR] prefixes.
- search_files: Search file contents with a pattern. First line is the search pattern (grep basic regex), second line (optional) is the directory to search in (defaults to `.`). Returns matching lines with file paths and line numbers.
- search_web: Search the web for information. Pass a search query as input. Returns titles, URLs, and descriptions of matching results.
- edit_file: Apply a targeted find-and-replace edit to a file. Format:
  path/to/file
  <<<
  text to find (exact match)
  ===
  replacement text
  >>>
- diff_files: Compare two text files and return a unified diff with 3 lines of context. First line is the path to the "before" file, second line is the path to the "after" file. Output is standard unified-diff format (`--- <a>`, `+++ <b>`, `@@ -start,count +start,count @@`, ` unchanged` / `-removed` / `+added`). Returns `(files are identical)` when there are no differences. Refuses to diff files longer than 2000 lines each. Prefer this over `exec_shell` `diff` whenever you need to understand or preview what changed between two files — works the same on every platform without shelling out.
- find_files: Find files matching a glob pattern. First line is the pattern (e.g. `**/*.rs`, `src/**/*.ts`). Second line (optional) is the base directory (defaults to `.`). Returns matching file paths, one per line.
- fetch_url: Fetch and read the content of a URL. Pass the URL as input. Returns the page text content with HTML tags stripped. Useful for reading pages found via search_web.
- extract_website: Fetch a URL and extract only the main readable content. Pass the URL as input. Strips scripts, styles, navigation, headers, footers, and other boilerplate. Use this instead of fetch_url when you need clean article or page text.
- fetch_datetime: Get the current date and time. No input required. Returns the current date, time, timezone, and day of week. Always call this tool first when the user's message involves relative time references like "today", "now", "tonight", "this week", "yesterday", "tomorrow", "currently", etc. so your answer is grounded in the actual current date and time.
- fetch_geolocation: Get geolocation data for an IP address. Pass an IP address as input (or empty for your own IP). Returns city, country, timezone, coordinates, ISP info. Always call this tool first (with empty input) when the user's message involves location references like "here", "near me", "nearby", "in this area", "in my city", "local", "around me", etc. so your answer is grounded in the user's actual location.
- read_image: Read an image from a file path or URL for visual analysis. Pass a file path or URL as input. Supports PNG, JPEG, GIF, WebP, BMP, TIFF, SVG, and ICO formats. The image is loaded and sent to the model for vision analysis. Always use this tool when the user asks about an image, asks you to describe/analyze a picture, or references an image file or image URL.
- generate_image: Generate an image from a text description. Pass the image description as input. The generated image is saved as a PNG to the current directory and the file path is returned. Supports three providers (auto-selected based on available keys, active provider preferred): DALL-E (LLM_OPENAI_API_KEY), Imagen (LLM_GEMINI_API_KEY), or Grok (LLM_GROK_API_KEY). Use this tool when the user asks you to create, generate, draw, or make an image or picture.
- read_document: Read a PDF, DOCX, or spreadsheet document and extract its content as markdown text. Pass the file path as input. Supports .pdf, .docx, .xlsx, .xls, and .ods files. PDF text is extracted directly; DOCX is converted to markdown preserving headings, lists, tables, and formatting; spreadsheets are converted to markdown tables (one per sheet). Use this tool when the user asks to read, analyze, or summarize a document, spreadsheet, or data file.
- git: Run a restricted git subcommand in the current working directory. Input is a single line (or quoted-multiline) of the form `<subcommand> [args...]`. Allowed subcommands: `status`, `diff`, `log`, `blame`, `commit`. Each subcommand accepts only a small allowlist of safe flags (e.g. `--oneline`, `--stat`, `-n <N>`, `--cached`, `-L start,end`, `-m "message"`, `-a`). Dangerous flags (`-c`, `-C`, `--ext-diff`, `--textconv`, `--upload-pack`, `--exec-path`, `--no-verify`, `--amend`, `--git-dir`, `--work-tree`) and all other subcommands (push, pull, fetch, clone, reset, rebase, checkout, config, remote, …) are rejected. Quote commit messages with double quotes: `commit -m "fix: typo"`. Prefer this tool over `exec_shell` whenever you need to inspect repository state or create a commit.
- run_code: Execute a short code snippet in a chosen interpreter and return its combined stdout/stderr. First line is the language identifier; remaining lines are the source code, piped to the interpreter on stdin (no temporary files). Supported languages: `python` (aliases `python3`, `py`), `node` (`nodejs`, `javascript`, `js`), `ruby` (`rb`), `perl`, `lua`, `bash`, `sh`. Use this for quick calculations, data transforms, and one-off logic checks without writing files. Non-zero exit codes are reported as `[exit N]`. The interpreter must be installed on `PATH`; if missing you'll get a clear "not installed" error. Not a sandbox — the snippet has the same filesystem and network access as the user, so do not use it to run untrusted code.
- json_query: Query or transform JSON data with jq-like expressions. First line is the jq filter (e.g. `.`, `.users[].name`, `.items | length`, `map(select(.price > 10))`). Remaining lines are either inline JSON or `@path/to/file.json` to load from a file in the working directory. Output is the filter result, pretty-printed. Non-zero exits are reported as `[exit N]`. Requires `jq` on PATH. Prefer this over writing a one-off `run_code` or `exec_shell` snippet whenever you need to extract, filter, count, or reshape JSON data.
- csv_query: Filter and project CSV or TSV data with a SQL-like query language and return the result as a Markdown table. First line is the query: `SELECT (* | col, col, ...) FROM (csv | tsv) [WHERE <cond> [AND|OR <cond> ...]] [ORDER BY <col> [ASC|DESC]] [LIMIT <N>]`. Remaining lines are either inline CSV/TSV with a header row, or `@path/to/file.csv` to load from a file in the working directory. Conditions use `=`, `!=`, `<>`, `<`, `<=`, `>`, `>=`, `LIKE` / `NOT LIKE` (with `%` wildcard), and `IS NULL` / `IS NOT NULL`. Numeric comparison is used when both operands parse as numbers; otherwise string comparison. `AND` binds tighter than `OR`; no parentheses. Column lookups are case-insensitive. `FROM csv` uses `,`; `FROM tsv` uses TAB. Prefer this over `run_code` or `exec_shell` whenever you need to extract, filter, sort, or limit rows from tabular data.
- calculate: Evaluate a mathematical expression safely, without any `eval` or shell subprocess. Pass the expression as input (e.g. `2 + 3 * 4`, `sqrt(16) + sin(pi/2)`, `(1 + 2) ^ 10`). Supports integers, decimals, scientific notation (`1e5`), hex (`0x1f`), binary (`0b1010`); operators `+ - * / %`, `^` / `**` (power, right-assoc), unary `+`/`-`; constants `pi`, `e`, `tau`; one-arg functions `sqrt`, `cbrt`, `abs`, `exp`, `ln`, `log2`, `log10`, `log` (alias for log10), `sin`, `cos`, `tan`, `asin`, `acos`, `atan`, `sinh`, `cosh`, `tanh`, `floor`, `ceil`, `round`, `trunc`, `sign`; two-arg functions `min`, `max`, `pow`, `atan2`. Prefer this over `run_code` or `exec_shell` for any arithmetic — no interpreter needed, no temp files, no shell.
- lint_file: Run a language-appropriate linter or formatter on a single file and return its diagnostics. Pass the file path as input. The tool picks the linter automatically from the file extension (e.g. `.rs` → `rustfmt --check`, `.py` → `ruff`/`flake8`/`pyflakes`/`py_compile`, `.js`/`.ts` → `eslint` or `node --check`/`tsc`, `.go` → `gofmt`/`go vet`, `.sh` → `shellcheck`, `.rb` → `rubocop`/`ruby -c`, `.json` → `jq empty`, `.yaml` → `yamllint`, `.toml` → `taplo`, `.md` → `markdownlint`/`prettier`, `.lua` → `luacheck`, `.c`/`.cpp` → `clang-format`/`cppcheck`, `.html`/`.css` → `prettier`) and tries candidates in order until one is installed. The file is never modified — no auto-fix flags are passed. Output is prefixed with `[linter: <label>]` and ends with `[clean]` on success or `[exit N]` on diagnostics. Use this instead of guessing which linter a project uses; if nothing is installed for a given extension the tool says so clearly.
- list_processes: List running processes with structured filtering. Safer and more predictable than piping `ps aux` through `grep` — the tool invokes `ps` directly (no shell) and parses the output in-process. Input is a set of `key=value` pairs (newline- or whitespace-separated); empty input returns the top 20 processes by %CPU. Keys: `name=<substring>` (case-insensitive match on command name and args), `user=<username>` (exact match), `pid=<N>` (exact pid), `min_cpu=<N>` (%CPU ≥ N), `min_mem=<N>` (%MEM ≥ N), `port=<N>` (processes listening on TCP or UDP port N, resolved via `lsof`), `sort=cpu|mem|pid|name` (default `cpu` descending; `pid`/`name` ascending), `limit=<N>` (default 20). Output is a Markdown-style table with PID, USER, %CPU, %MEM, RSS, and COMMAND. Prefer this over `exec_shell` whenever you need to inspect running processes.
- check_port: Test whether a TCP port on a given host accepts connections. Pure tokio — no shell, no `nc`/`telnet` subprocess. Input is one line: `<host>:<port> [timeout=<ms>]`. The host may be a DNS name, IPv4, or bracketed IPv6 (`[::1]:8080`); an `http://` / `https://` URL is also accepted and the port is inferred when omitted (80/443). Default timeout is 3000ms; maximum 30000ms. Returns "Reachable — host:port (<resolved addr>) accepted TCP in <N>ms" on success, or "Unreachable — ..." with a reason (connection refused, timed out, DNS resolution failed, host/network unreachable) otherwise. Use this to diagnose connectivity before issuing an HTTP request or running a service, instead of shelling out to `nc -zv` or `telnet`.
- system_info: Return structured OS, CPU, memory, and disk information in Markdown. Cross-platform for macOS (via `sysctl`, `vm_stat`, `sw_vers`, `uname`, `df`) and Linux (via `/proc/cpuinfo`, `/proc/meminfo`, `/etc/os-release`, and `df`). Input is optional `key=value` pairs (empty = all sections): `section=os|cpu|memory|disk|all` (default `all`), `path=<directory>` (disk section only; default is the security working directory). Reports OS pretty name, arch, kernel, hostname; CPU model, logical and physical core counts; memory total/used/available (or total/used/free on macOS); disk mount, filesystem, total/used/available with capacity percentage. Prefer this over `exec_shell` whenever you need to probe the host environment — faster, deterministic, and doesn't require stringing together multiple shell commands.

Rules:
- Use at most one tool call per response.
- When you have enough information to answer the user's question, respond normally without any tool tags.
- Show your reasoning before tool calls.
"#;

// --- Config file loading ---

pub fn load_config() {
    let home = std::env::var("HOME").unwrap_or_else(|_| {
        eprintln!("Error: HOME environment variable not set");
        std::process::exit(1);
    });
    let config_path = format!("{home}/.aictl/config");
    let Ok(contents) = std::fs::read_to_string(&config_path) else {
        CONFIG.set(RwLock::new(HashMap::new())).ok();
        return;
    };

    let map = parse_config(&contents);
    CONFIG.set(RwLock::new(map)).ok();
}

fn parse_config(contents: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let line = line.strip_prefix("export ").unwrap_or(line);

        let Some((key, value)) = line.split_once('=') else {
            continue;
        };

        let key = key.trim();
        let mut value = value.trim();

        if (value.starts_with('"') && value.ends_with('"'))
            || (value.starts_with('\'') && value.ends_with('\''))
        {
            value = &value[1..value.len() - 1];
        }

        map.insert(key.to_string(), value.to_string());
    }
    map
}

pub fn config_get(key: &str) -> Option<String> {
    CONFIG
        .get()
        .and_then(|lock| lock.read().ok())
        .and_then(|m| m.get(key).cloned())
}

/// Load the project prompt file from the current working directory.
/// The filename defaults to `AICTL.md` but can be overridden via `AICTL_PROMPT_FILE` in config.
/// Returns `None` if the file does not exist or cannot be read.
pub fn load_prompt_file() -> Option<String> {
    let filename = config_get("AICTL_PROMPT_FILE").unwrap_or_else(|| "AICTL.md".to_string());
    let path = std::path::Path::new(&filename);
    std::fs::read_to_string(path).ok()
}

/// Check whether a config line declares the given key.
fn line_matches_key(line: &str, key: &str) -> bool {
    let trimmed = line.trim();
    let stripped = trimmed.strip_prefix("export ").unwrap_or(trimmed);
    stripped
        .strip_prefix(key)
        .is_some_and(|rest| rest.starts_with('='))
}

/// Write a key=value pair to ~/.aictl/config, replacing an existing key or appending.
/// Also updates the in-memory config cache so subsequent `config_get` calls see the new value.
pub fn config_set(key: &str, value: &str) {
    let Ok(home) = std::env::var("HOME") else {
        return;
    };
    let config_dir = format!("{home}/.aictl");
    let config_path = format!("{config_dir}/config");
    let _ = std::fs::create_dir_all(&config_dir);
    let contents = std::fs::read_to_string(&config_path).unwrap_or_default();

    let mut found = false;
    let mut lines: Vec<String> = contents
        .lines()
        .map(|line| {
            if line_matches_key(line, key) {
                found = true;
                format!("{key}={value}")
            } else {
                line.to_string()
            }
        })
        .collect();

    if !found {
        lines.push(format!("{key}={value}"));
    }

    let _ = std::fs::write(&config_path, lines.join("\n") + "\n");

    if let Some(lock) = CONFIG.get()
        && let Ok(mut m) = lock.write()
    {
        m.insert(key.to_string(), value.to_string());
    }
}

/// Remove a key from ~/.aictl/config and the in-memory cache.
/// Returns true if the key existed in either location.
pub fn config_unset(key: &str) -> bool {
    let Ok(home) = std::env::var("HOME") else {
        return false;
    };
    let config_path = format!("{home}/.aictl/config");

    let mut removed_from_file = false;
    if let Ok(contents) = std::fs::read_to_string(&config_path) {
        let kept: Vec<&str> = contents
            .lines()
            .filter(|line| {
                if line_matches_key(line, key) {
                    removed_from_file = true;
                    false
                } else {
                    true
                }
            })
            .collect();
        if removed_from_file {
            let _ = std::fs::write(&config_path, kept.join("\n") + "\n");
        }
    }

    let mut removed_from_cache = false;
    if let Some(lock) = CONFIG.get()
        && let Ok(mut m) = lock.write()
    {
        removed_from_cache = m.remove(key).is_some();
    }

    removed_from_file || removed_from_cache
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_config_empty_input() {
        let map = parse_config("");
        assert!(map.is_empty());
    }

    #[test]
    fn parse_config_comments_and_blank_lines() {
        let input = "# this is a comment\n\n  # indented comment\n\n";
        let map = parse_config(input);
        assert!(map.is_empty());
    }

    #[test]
    fn parse_config_export_prefix_stripped() {
        let input = "export API_KEY=abc123";
        let map = parse_config(input);
        assert_eq!(map.get("API_KEY").unwrap(), "abc123");
    }

    #[test]
    fn parse_config_double_quoted_value() {
        let input = "KEY=\"hello world\"";
        let map = parse_config(input);
        assert_eq!(map.get("KEY").unwrap(), "hello world");
    }

    #[test]
    fn parse_config_single_quoted_value() {
        let input = "KEY='hello world'";
        let map = parse_config(input);
        assert_eq!(map.get("KEY").unwrap(), "hello world");
    }

    #[test]
    fn parse_config_unquoted_value() {
        let input = "KEY=value";
        let map = parse_config(input);
        assert_eq!(map.get("KEY").unwrap(), "value");
    }

    #[test]
    fn parse_config_lines_without_equals_skipped() {
        let input = "no-equals-here\nKEY=val\njust text";
        let map = parse_config(input);
        assert_eq!(map.len(), 1);
        assert_eq!(map.get("KEY").unwrap(), "val");
    }

    #[test]
    fn parse_config_mixed() {
        let input = "\
# config file
export PROVIDER=\"anthropic\"
MODEL=gpt-4o
  API_KEY='sk-123'

bad line
";
        let map = parse_config(input);
        assert_eq!(map.get("PROVIDER").unwrap(), "anthropic");
        assert_eq!(map.get("MODEL").unwrap(), "gpt-4o");
        assert_eq!(map.get("API_KEY").unwrap(), "sk-123");
        assert_eq!(map.len(), 3);
    }
}
