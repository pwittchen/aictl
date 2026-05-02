use crossterm::style::{Color, Stylize};

use aictl_core::tools::BUILTIN_TOOLS;

pub(super) fn print_tools() {
    let enabled = crate::tools::tools_enabled();
    let max_len = BUILTIN_TOOLS
        .iter()
        .map(|(n, _)| n.len())
        .max()
        .unwrap_or(0);
    println!();
    if !enabled {
        println!(
            "  {}",
            "all tools disabled (AICTL_TOOLS_ENABLED=false)".with(Color::Yellow)
        );
        println!();
    }
    let plugin_list = crate::plugins::list();
    let mcp_servers = crate::mcp::list();
    let mcp_max = mcp_servers
        .iter()
        .filter(|s| matches!(s.state, crate::mcp::ServerState::Ready))
        .flat_map(|s| {
            s.tools
                .iter()
                .map(move |t| crate::mcp::qualify(&s.name, &t.name).len())
        })
        .max()
        .unwrap_or(0);
    let max_len = std::cmp::max(
        max_len,
        plugin_list.iter().map(|p| p.name.len()).max().unwrap_or(0),
    );
    let max_len = std::cmp::max(max_len, mcp_max);
    for (name, desc) in BUILTIN_TOOLS {
        let pad = max_len - name.len() + 2;
        println!("  {}{:pad$}{desc}", name.with(Color::Cyan), "");
    }
    if !plugin_list.is_empty() {
        println!();
        println!("  {}", "plugins:".with(Color::DarkGrey));
        for p in plugin_list {
            let pad = max_len - p.name.len() + 2;
            println!(
                "  {}{:pad$}{} {}",
                p.name.as_str().with(Color::Cyan),
                "",
                p.description.as_str(),
                "(plugin)".with(Color::DarkGrey),
            );
        }
    }
    let any_ready_mcp = mcp_servers
        .iter()
        .any(|s| matches!(s.state, crate::mcp::ServerState::Ready) && !s.tools.is_empty());
    if any_ready_mcp {
        println!();
        println!("  {}", "mcp:".with(Color::DarkGrey));
        for s in &mcp_servers {
            if !matches!(s.state, crate::mcp::ServerState::Ready) {
                continue;
            }
            for t in &s.tools {
                let qualified = crate::mcp::qualify(&s.name, &t.name);
                let pad = max_len - qualified.len() + 2;
                let suffix = format!("(mcp: {})", s.name);
                println!(
                    "  {}{:pad$}{} {}",
                    qualified.as_str().with(Color::Cyan),
                    "",
                    t.description.as_str(),
                    suffix.with(Color::DarkGrey),
                );
            }
        }
    }
    println!();
}

#[cfg(test)]
mod tests {
    use super::BUILTIN_TOOLS;

    #[test]
    fn tools_list_matches_tool_count() {
        assert_eq!(
            BUILTIN_TOOLS.len(),
            crate::tools::TOOL_COUNT,
            "the BUILTIN_TOOLS catalog is out of sync with TOOL_COUNT — add or remove entries in aictl-core/src/tools.rs"
        );
    }

    #[test]
    fn tools_list_has_no_duplicates() {
        let mut names: Vec<&str> = BUILTIN_TOOLS.iter().map(|(n, _)| *n).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(before, names.len(), "duplicate tool name in BUILTIN_TOOLS");
    }
}
