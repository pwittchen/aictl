//! Provider credit / quota balance (`/balance`).
//!
//! Probes every cloud provider for which we know how to read the remaining
//! credit and prints a per-provider line. Providers without a documented
//! balance API are reported as "unknown" with a hint pointing at the
//! provider's billing dashboard. Providers without a configured key are
//! reported as "no API key" and skipped.

use std::fmt::Write as _;

use crossterm::style::{Color, Stylize};

use crate::llm::balance::{self, Balance, BalanceStatus};

/// Run the `/balance` (and `--balance`) sweep and print results to stdout.
pub async fn run_balance() {
    println!();
    println!("  {} fetching provider balances...", "↻".with(Color::Cyan),);
    println!();

    let results = balance::fetch_all().await;

    let max_name = results.iter().map(|r| r.provider.len()).max().unwrap_or(0);
    let rows: Vec<(&Balance, String)> = results.iter().map(|r| (r, format_detail(r))).collect();
    let max_detail = rows.iter().map(|(_, d)| d.len()).max().unwrap_or(0);

    for (r, detail) in &rows {
        print_row(r, detail, max_name, max_detail);
    }

    let known = results
        .iter()
        .filter(|r| matches!(r.status, BalanceStatus::Amount { .. }))
        .count();
    let unknown = results
        .iter()
        .filter(|r| matches!(r.status, BalanceStatus::Unknown(_)))
        .count();
    let no_key = results
        .iter()
        .filter(|r| matches!(r.status, BalanceStatus::NoKey))
        .count();
    let errors = results
        .iter()
        .filter(|r| matches!(r.status, BalanceStatus::Error(_)))
        .count();
    println!();
    println!(
        "  {} {known} reported · {unknown} unknown · {no_key} no key · {errors} error",
        "summary:".with(Color::Cyan),
    );
    println!();
}

fn format_detail(r: &Balance) -> String {
    match &r.status {
        BalanceStatus::Amount {
            currency,
            total,
            granted,
            topped_up,
            ..
        } => {
            let mut s = format_amount(*total, currency);
            // Show breakdown only when both halves are present and non-zero —
            // otherwise the detail line is just clutter.
            match (granted, topped_up) {
                (Some(g), Some(t)) if *g > 0.0 || *t > 0.0 => {
                    let _ = write!(
                        s,
                        " ({} granted + {} topped up)",
                        format_amount(*g, currency),
                        format_amount(*t, currency),
                    );
                }
                _ => {}
            }
            s
        }
        BalanceStatus::NoKey => "no API key".to_string(),
        BalanceStatus::Unknown(hint) => format!("unknown — {hint}"),
        BalanceStatus::Error(msg) => format!("error: {msg}"),
    }
}

fn format_amount(amount: f64, currency: &str) -> String {
    let symbol = match currency {
        "USD" => "$",
        "CNY" => "¥",
        "EUR" => "€",
        _ => "",
    };
    if symbol.is_empty() {
        format!("{amount:.2} {currency}")
    } else {
        format!("{symbol}{amount:.2}")
    }
}

fn print_row(r: &Balance, detail: &str, max_name: usize, max_detail: usize) {
    let (icon, color) = match &r.status {
        BalanceStatus::Amount { .. } => ("✓", Color::Green),
        BalanceStatus::NoKey | BalanceStatus::Unknown(_) => ("-", Color::DarkGrey),
        BalanceStatus::Error(_) => ("✗", Color::Red),
    };
    let elapsed = r
        .elapsed
        .map(|d| format!("{}ms", d.as_millis()))
        .unwrap_or_default();
    let name_pad = max_name - r.provider.len() + 2;
    let detail_pad = max_detail - detail.len() + 2;
    println!(
        "  {} {}{:name_pad$}{}{:detail_pad$}{}",
        icon.with(color),
        r.provider.with(Color::Cyan),
        "",
        detail.with(color),
        "",
        elapsed.with(Color::DarkGrey),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn b(provider: &'static str, status: BalanceStatus) -> Balance {
        Balance {
            provider,
            status,
            elapsed: Some(Duration::from_millis(42)),
        }
    }

    #[test]
    fn amount_in_usd_uses_dollar_sign() {
        let detail = format_detail(&b(
            "deepseek",
            BalanceStatus::Amount {
                currency: "USD".to_string(),
                total: 12.5,
                granted: None,
                topped_up: None,
            },
        ));
        assert_eq!(detail, "$12.50");
    }

    #[test]
    fn amount_with_breakdown_shows_granted_and_topped_up() {
        let detail = format_detail(&b(
            "deepseek",
            BalanceStatus::Amount {
                currency: "USD".to_string(),
                total: 110.0,
                granted: Some(10.0),
                topped_up: Some(100.0),
            },
        ));
        assert_eq!(detail, "$110.00 ($10.00 granted + $100.00 topped up)");
    }

    #[test]
    fn amount_in_cny_uses_yuan_sign() {
        let detail = format_detail(&b(
            "kimi",
            BalanceStatus::Amount {
                currency: "CNY".to_string(),
                total: 50.0,
                granted: None,
                topped_up: None,
            },
        ));
        assert_eq!(detail, "¥50.00");
    }

    #[test]
    fn unknown_currency_falls_back_to_iso_code() {
        let detail = format_detail(&b(
            "future",
            BalanceStatus::Amount {
                currency: "XYZ".to_string(),
                total: 1.0,
                granted: None,
                topped_up: None,
            },
        ));
        assert_eq!(detail, "1.00 XYZ");
    }

    #[test]
    fn no_key_status_renders_clearly() {
        let detail = format_detail(&b("openai", BalanceStatus::NoKey));
        assert_eq!(detail, "no API key");
    }

    #[test]
    fn unknown_status_includes_hint() {
        let detail = format_detail(&b(
            "anthropic",
            BalanceStatus::Unknown("check console.anthropic.com".to_string()),
        ));
        assert_eq!(detail, "unknown — check console.anthropic.com");
    }

    #[test]
    fn error_status_includes_message() {
        let detail = format_detail(&b("deepseek", BalanceStatus::Error("HTTP 500".to_string())));
        assert_eq!(detail, "error: HTTP 500");
    }

    #[test]
    fn breakdown_skipped_when_both_components_zero() {
        let detail = format_detail(&b(
            "deepseek",
            BalanceStatus::Amount {
                currency: "USD".to_string(),
                total: 0.0,
                granted: Some(0.0),
                topped_up: Some(0.0),
            },
        ));
        assert_eq!(detail, "$0.00");
    }
}
