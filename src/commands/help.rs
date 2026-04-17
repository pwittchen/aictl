use crossterm::style::{Color, Stylize};

pub(super) fn print_help() {
    let entries: &[(&str, &str)] = &[
        ("/agent", "manage agents"),
        ("/clear", "clear conversation context"),
        ("/compact", "compact context into a summary"),
        ("/context", "show context usage"),
        ("/copy", "copy last response to clipboard"),
        ("/retry", "remove last exchange and retry"),
        ("/help", "show this help message"),
        ("/history", "view conversation (filter by role or keyword)"),
        ("/info", "show setup info"),
        ("/gguf", "manage native local GGUF models [experimental]"),
        (
            "/mlx",
            "manage native MLX models (Apple Silicon) [experimental]",
        ),
        ("/behavior", "switch auto/human-in-the-loop behavior"),
        ("/model", "switch model and provider"),
        ("/security", "show security policy"),
        ("/session", "manage sessions"),
        ("/stats", "view and manage usage statistics"),
        ("/memory", "switch memory mode (long-term/short-term)"),
        ("/tools", "show available tools"),
        ("/keys", "manage API keys (lock, unlock, clear)"),
        ("/config", "re-run the configuration wizard"),
        ("/update", "update to the latest version"),
        (
            "/uninstall",
            "remove the aictl binary (asks for confirmation)",
        ),
        ("/version", "check current version against the latest"),
        ("/exit", "exit the REPL"),
    ];
    let max_len = entries.iter().map(|(c, _)| c.len()).max().unwrap_or(0);
    println!();
    for (cmd, desc) in entries {
        let pad = max_len - cmd.len() + 2;
        println!("  {}{:pad$}{desc}", cmd.with(Color::Cyan), "");
    }
    println!();
}
