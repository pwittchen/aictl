use crossterm::style::{Color, Stylize};

use super::menu::{build_simple_menu_lines, confirm_yn, select_from_menu};

const STATS_MENU_ITEMS: &[(&str, &str)] = &[
    ("view stats", "show today / this month / overall"),
    ("clear stats", "remove all usage statistics"),
];

/// Display usage statistics: today, this month, overall.
pub fn print_stats() {
    let today = crate::stats::today();
    let month = crate::stats::this_month();
    let overall = crate::stats::overall();
    let days = crate::stats::day_count();

    println!();
    print_stats_section("Today", &today);
    println!();
    print_stats_section("This month", &month);
    println!();
    print_stats_section(&format!("Overall ({days} days)"), &overall);
    println!();
}

/// Clear all stats after user confirmation.
pub fn run_clear_stats(_show_error: &dyn Fn(&str)) {
    println!();
    if !confirm_yn("clear ALL usage statistics?") {
        return;
    }
    crate::stats::clear_all();
    println!("  {} statistics cleared", "✓".with(Color::Green));
    println!();
}

/// Open the `/stats` interactive menu (view or clear).
pub fn run_stats_menu(show_error: &dyn Fn(&str)) {
    let Some(sel) = select_from_menu(STATS_MENU_ITEMS.len(), 0, |s| {
        build_simple_menu_lines(STATS_MENU_ITEMS, s)
    }) else {
        return;
    };
    match sel {
        0 => print_stats(),
        1 => run_clear_stats(show_error),
        _ => {}
    }
}

/// Print a single stats section (today / this month / overall).
fn print_stats_section(label: &str, stats: &crate::stats::DayStats) {
    println!(
        "  {}",
        label
            .with(Color::Cyan)
            .attribute(crossterm::style::Attribute::Bold),
    );
    println!(
        "    {} {}",
        format!("{:<15}", "sessions:").with(Color::DarkGrey),
        stats.sessions,
    );
    println!(
        "    {} {}",
        format!("{:<15}", "requests:").with(Color::DarkGrey),
        stats.requests,
    );
    println!(
        "    {} {}",
        format!("{:<15}", "llm calls:").with(Color::DarkGrey),
        stats.llm_calls,
    );
    println!(
        "    {} {}",
        format!("{:<15}", "tool calls:").with(Color::DarkGrey),
        stats.tool_calls,
    );
    println!(
        "    {} {}",
        format!("{:<15}", "input tokens:").with(Color::DarkGrey),
        format_token_count(stats.input_tokens),
    );
    println!(
        "    {} {}",
        format!("{:<15}", "output tokens:").with(Color::DarkGrey),
        format_token_count(stats.output_tokens),
    );
    println!(
        "    {} {}",
        format!("{:<15}", "cost:").with(Color::DarkGrey),
        format_cost(stats.cost_usd),
    );
    if !stats.models.is_empty() {
        let mut models: Vec<_> = stats.models.iter().collect();
        models.sort_by(|a, b| b.1.cmp(a.1));
        println!(
            "    {} {} ({})",
            format!("{:<15}", "models:").with(Color::DarkGrey),
            models[0].0,
            models[0].1,
        );
        for (model, count) in models.iter().skip(1) {
            println!("    {:<15} {} ({})", "", model, count);
        }
    }
}

#[allow(clippy::cast_precision_loss)]
fn format_token_count(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}K", tokens as f64 / 1_000.0)
    } else {
        format!("{tokens}")
    }
}

fn format_cost(cost: f64) -> String {
    if cost < 0.01 {
        format!("${cost:.4}")
    } else {
        format!("${cost:.2}")
    }
}
