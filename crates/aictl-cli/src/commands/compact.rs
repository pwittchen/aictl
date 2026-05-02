use std::sync::atomic::{AtomicU32, Ordering};

use crossterm::style::{Color, Stylize};

use aictl_core::error::AictlError;
use aictl_core::run::compact_messages;

use crate::llm;
use crate::ui::AgentUI;
use crate::{Message, Provider};

static MANUAL_COMPACTIONS: AtomicU32 = AtomicU32::new(0);
static AUTO_COMPACTIONS: AtomicU32 = AtomicU32::new(0);

pub fn compaction_counts() -> (u32, u32) {
    (
        MANUAL_COMPACTIONS.load(Ordering::Relaxed),
        AUTO_COMPACTIONS.load(Ordering::Relaxed),
    )
}

pub async fn compact(
    provider: &Provider,
    api_key: &str,
    model: &str,
    messages: &mut Vec<Message>,
    ui: &dyn AgentUI,
    memory: &str,
    is_auto: bool,
) {
    if messages.len() <= 1 {
        ui.show_error("Nothing to compact.");
        return;
    }

    // PreCompact hook fires before the summary call so a hook can capture
    // the about-to-be-discarded transcript or veto the compaction.
    let trigger = if is_auto { "auto" } else { "manual" };
    let pre = crate::hooks::run_hooks(
        crate::hooks::HookEvent::PreCompact,
        "",
        crate::hooks::HookContext {
            session_id: crate::session::current_id(),
            cwd: std::env::current_dir().ok(),
            trigger: Some(trigger),
            ..Default::default()
        },
    )
    .await;
    if let Some(reason) = pre.blocked {
        ui.show_error(&format!("compaction blocked by hook: {reason}"));
        return;
    }

    ui.start_spinner("compacting context...");

    let cancellable =
        crate::with_esc_cancel(ui, compact_messages(provider, api_key, model, messages)).await;

    ui.stop_spinner();

    let result = match cancellable {
        Ok(inner) => inner,
        Err(_interrupted) => {
            println!("\n  {} interrupted\n", "✗".with(Color::Yellow));
            return;
        }
    };

    match result {
        Ok(usage) => {
            println!();
            ui.show_token_usage(
                &usage,
                model,
                false,
                0,
                std::time::Duration::ZERO,
                0,
                memory,
            );
            if is_auto {
                AUTO_COMPACTIONS.fetch_add(1, Ordering::Relaxed);
            } else {
                MANUAL_COMPACTIONS.fetch_add(1, Ordering::Relaxed);
            }
            println!("  {} context compacted", "✓".with(Color::Green));
            println!();
        }
        Err(AictlError::Timeout { secs }) => {
            println!(
                "\n  {} compaction timed out after {secs}s (AICTL_LLM_TIMEOUT)\n",
                "✗".with(Color::Yellow),
            );
        }
        Err(e) => ui.show_error(&format!("Compact failed: {e}")),
    }
}

pub fn print_context(
    model: &str,
    messages_len: usize,
    last_input_tokens: u64,
    max_messages: usize,
) {
    let limit = llm::context_limit(model);
    let token_pct = llm::pct(last_input_tokens, limit);
    let message_pct = llm::pct_usize(messages_len, max_messages);
    let context_pct = token_pct.max(message_pct).min(100);

    let bar_width = 30;
    let filled = (context_pct as usize * bar_width / 100).min(bar_width);
    let empty = bar_width - filled;
    let bar_color = if context_pct >= 80 {
        Color::Red
    } else if context_pct >= 50 {
        Color::Yellow
    } else {
        Color::Green
    };

    println!();
    println!(
        "  {} {}{} {context_pct}%",
        format!("{:<13}", "context:").with(Color::Cyan),
        "█".repeat(filled).with(bar_color),
        "░".repeat(empty).with(Color::DarkGrey),
    );
    println!(
        "  {} {last_input_tokens} / {limit}",
        format!("{:<13}", "tokens:").with(Color::DarkGrey),
    );
    println!(
        "  {} {messages_len} / {max_messages}",
        format!("{:<13}", "messages:").with(Color::DarkGrey),
    );
    let (manual, auto) = compaction_counts();
    println!(
        "  {} manual: {manual}, auto: {auto}",
        format!("{:<13}", "compactions:").with(Color::DarkGrey),
    );
    let threshold = crate::config::auto_compact_threshold();
    let source = if crate::config::config_get("AICTL_AUTO_COMPACT_THRESHOLD")
        .and_then(|v| v.parse::<u8>().ok())
        .filter(|v| (1..=100).contains(v))
        .is_some()
    {
        "config"
    } else {
        "default"
    };
    println!(
        "  {} {threshold}% ({source})",
        format!("{:<13}", "auto-compact:").with(Color::DarkGrey),
    );
    println!();
}
