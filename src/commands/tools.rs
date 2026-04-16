use crossterm::style::{Color, Stylize};

pub(super) fn print_tools() {
    let tools: &[(&str, &str)] = &[
        ("exec_shell", "execute a shell command via sh -c"),
        ("read_file", "read the contents of a file"),
        ("write_file", "write content to a file"),
        ("remove_file", "remove (delete) a file"),
        ("edit_file", "edit a file with find-and-replace"),
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
    ];
    let enabled = crate::tools::tools_enabled();
    let max_len = tools.iter().map(|(n, _)| n.len()).max().unwrap_or(0);
    println!();
    if !enabled {
        println!(
            "  {}",
            "all tools disabled (AICTL_TOOLS_ENABLED=false)".with(Color::Yellow)
        );
        println!();
    }
    for (name, desc) in tools {
        let pad = max_len - name.len() + 2;
        println!("  {}{:pad$}{desc}", name.with(Color::Cyan), "");
    }
    println!();
}
