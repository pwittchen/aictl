use crossterm::style::{Color, Stylize};

const TOOLS: &[(&str, &str)] = &[
    ("exec_shell", "execute a shell command via sh -c"),
    ("read_file", "read the contents of a file"),
    ("write_file", "write content to a file"),
    ("remove_file", "remove (delete) a file"),
    ("edit_file", "edit a file with find-and-replace"),
    (
        "diff_files",
        "compute a unified diff between two text files",
    ),
    (
        "create_directory",
        "create a directory and any missing parents",
    ),
    ("list_directory", "list files and directories at a path"),
    ("search_files", "search file contents by pattern"),
    ("find_files", "find files matching a glob pattern"),
    ("search_web", "search the web via Firecrawl API"),
    ("fetch_url", "fetch a URL and return text content"),
    ("extract_website", "extract readable content from a URL"),
    ("fetch_datetime", "get current date, time, and timezone"),
    (
        "fetch_geolocation",
        "get geolocation data for an IP address",
    ),
    ("read_image", "read an image from file or URL for analysis"),
    (
        "generate_image",
        "generate an image from text (DALL-E/Imagen/Grok)",
    ),
    (
        "read_document",
        "read a PDF, DOCX, or spreadsheet as markdown",
    ),
    (
        "git",
        "run a restricted git subcommand (status, diff, log, blame, commit)",
    ),
    (
        "run_code",
        "execute a code snippet (python, node, ruby, perl, lua, bash, sh)",
    ),
    (
        "lint_file",
        "run a language-appropriate linter/formatter on a file",
    ),
    (
        "json_query",
        "query/transform JSON with jq-like expressions",
    ),
    (
        "csv_query",
        "filter CSV/TSV with SQL-like expressions (table output)",
    ),
    (
        "calculate",
        "evaluate a math expression safely (no eval, no shell)",
    ),
    (
        "list_processes",
        "list running processes with structured filtering",
    ),
    (
        "check_port",
        "test TCP reachability of a host:port (no shell, no nc)",
    ),
    (
        "system_info",
        "OS/CPU/memory/disk info as markdown (cross-platform)",
    ),
    (
        "archive",
        "create, extract, or list tar.gz/tgz/tar/zip archives",
    ),
    (
        "checksum",
        "compute SHA-256 and/or MD5 of a file (streaming)",
    ),
    (
        "clipboard",
        "read from or write to the system clipboard (pbcopy/wl-copy/xclip)",
    ),
    (
        "notify",
        "send a desktop notification (osascript on macOS, notify-send on Linux)",
    ),
];

pub(super) fn print_tools() {
    let enabled = crate::tools::tools_enabled();
    let max_len = TOOLS.iter().map(|(n, _)| n.len()).max().unwrap_or(0);
    println!();
    if !enabled {
        println!(
            "  {}",
            "all tools disabled (AICTL_TOOLS_ENABLED=false)".with(Color::Yellow)
        );
        println!();
    }
    for (name, desc) in TOOLS {
        let pad = max_len - name.len() + 2;
        println!("  {}{:pad$}{desc}", name.with(Color::Cyan), "");
    }
    println!();
}

#[cfg(test)]
mod tests {
    use super::TOOLS;

    #[test]
    fn tools_list_matches_tool_count() {
        assert_eq!(
            TOOLS.len(),
            crate::tools::TOOL_COUNT,
            "the /tools command list is out of sync with TOOL_COUNT — add or remove entries in src/commands/tools.rs"
        );
    }

    #[test]
    fn tools_list_has_no_duplicates() {
        let mut names: Vec<&str> = TOOLS.iter().map(|(n, _)| *n).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(before, names.len(), "duplicate tool name in /tools list");
    }
}
