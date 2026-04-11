use std::collections::HashMap;
use std::sync::OnceLock;

static CONFIG: OnceLock<HashMap<String, String>> = OnceLock::new();
static HTTP_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

/// Return a shared `reqwest::Client`, creating it on first access.
pub fn http_client() -> &'static reqwest::Client {
    HTTP_CLIENT.get_or_init(reqwest::Client::new)
}

// --- Agent loop limits ---

pub const MAX_ITERATIONS: usize = 20;
pub const MAX_MESSAGES: usize = 200;
pub const MAX_TOOL_OUTPUT_LEN: usize = 10_000;
pub const MAX_RESPONSE_TOKENS: u32 = 4096;
pub const FAST_MODE_WINDOW: usize = 20;
pub const DEFAULT_AUTO_COMPACT_THRESHOLD: u8 = 80;

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
- find_files: Find files matching a glob pattern. First line is the pattern (e.g. `**/*.rs`, `src/**/*.ts`). Second line (optional) is the base directory (defaults to `.`). Returns matching file paths, one per line.
- fetch_url: Fetch and read the content of a URL. Pass the URL as input. Returns the page text content with HTML tags stripped. Useful for reading pages found via search_web.
- extract_website: Fetch a URL and extract only the main readable content. Pass the URL as input. Strips scripts, styles, navigation, headers, footers, and other boilerplate. Use this instead of fetch_url when you need clean article or page text.
- fetch_datetime: Get the current date and time. No input required. Returns the current date, time, timezone, and day of week. Always call this tool first when the user's message involves relative time references like "today", "now", "tonight", "this week", "yesterday", "tomorrow", "currently", etc. so your answer is grounded in the actual current date and time.
- fetch_geolocation: Get geolocation data for an IP address. Pass an IP address as input (or empty for your own IP). Returns city, country, timezone, coordinates, ISP info. Always call this tool first (with empty input) when the user's message involves location references like "here", "near me", "nearby", "in this area", "in my city", "local", "around me", etc. so your answer is grounded in the user's actual location.

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
        CONFIG.set(HashMap::new()).ok();
        return;
    };

    let map = parse_config(&contents);
    CONFIG.set(map).ok();
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
    CONFIG.get().and_then(|m| m.get(key).cloned())
}

/// Load the project prompt file from the current working directory.
/// The filename defaults to `AICTL.md` but can be overridden via `AICTL_PROMPT_FILE` in config.
/// Returns `None` if the file does not exist or cannot be read.
pub fn load_prompt_file() -> Option<String> {
    let filename = config_get("AICTL_PROMPT_FILE").unwrap_or_else(|| "AICTL.md".to_string());
    let path = std::path::Path::new(&filename);
    std::fs::read_to_string(path).ok()
}

/// Write a key=value pair to ~/.aictl/config, replacing an existing key or appending.
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
            let trimmed = line.trim();
            let stripped = trimmed.strip_prefix("export ").unwrap_or(trimmed);
            if stripped.starts_with(key) && stripped[key.len()..].starts_with('=') {
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
