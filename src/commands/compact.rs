use std::sync::atomic::{AtomicU32, Ordering};

use crossterm::style::{Color, Stylize};

use crate::llm;
use crate::ui::AgentUI;
use crate::{Message, Provider, Role};

static MANUAL_COMPACTIONS: AtomicU32 = AtomicU32::new(0);
static AUTO_COMPACTIONS: AtomicU32 = AtomicU32::new(0);

pub fn compaction_counts() -> (u32, u32) {
    (
        MANUAL_COMPACTIONS.load(Ordering::Relaxed),
        AUTO_COMPACTIONS.load(Ordering::Relaxed),
    )
}

#[allow(clippy::too_many_lines)]
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

    ui.start_spinner("compacting context...");

    let mut summary_msgs = messages.clone();
    summary_msgs.push(Message {
        role: Role::User,
        content: "Summarize our conversation so far in a compact form. \
            Include all key facts, decisions, code changes, file paths, \
            and open tasks so we can continue without losing context. \
            Be concise but thorough."
            .to_string(),
        images: vec![],
    });

    let llm_timeout = crate::config::llm_timeout();
    // Compaction never streams — it produces a one-shot summary the user
    // doesn't see. Pass `None` to every provider so they take the buffered
    // code path.
    let result = match provider {
        Provider::Openai => {
            crate::with_esc_cancel(tokio::time::timeout(
                llm_timeout,
                crate::llm::openai::call_openai(api_key, model, &summary_msgs, None),
            ))
            .await
        }
        Provider::Anthropic => {
            crate::with_esc_cancel(tokio::time::timeout(
                llm_timeout,
                crate::llm::anthropic::call_anthropic(api_key, model, &summary_msgs, None),
            ))
            .await
        }
        Provider::Gemini => {
            crate::with_esc_cancel(tokio::time::timeout(
                llm_timeout,
                crate::llm::gemini::call_gemini(api_key, model, &summary_msgs, None),
            ))
            .await
        }
        Provider::Grok => {
            crate::with_esc_cancel(tokio::time::timeout(
                llm_timeout,
                crate::llm::grok::call_grok(api_key, model, &summary_msgs, None),
            ))
            .await
        }
        Provider::Mistral => {
            crate::with_esc_cancel(tokio::time::timeout(
                llm_timeout,
                crate::llm::mistral::call_mistral(api_key, model, &summary_msgs, None),
            ))
            .await
        }
        Provider::Deepseek => {
            crate::with_esc_cancel(tokio::time::timeout(
                llm_timeout,
                crate::llm::deepseek::call_deepseek(api_key, model, &summary_msgs, None),
            ))
            .await
        }
        Provider::Kimi => {
            crate::with_esc_cancel(tokio::time::timeout(
                llm_timeout,
                crate::llm::kimi::call_kimi(api_key, model, &summary_msgs, None),
            ))
            .await
        }
        Provider::Zai => {
            crate::with_esc_cancel(tokio::time::timeout(
                llm_timeout,
                crate::llm::zai::call_zai(api_key, model, &summary_msgs, None),
            ))
            .await
        }
        Provider::Ollama => {
            crate::with_esc_cancel(tokio::time::timeout(
                llm_timeout,
                crate::llm::ollama::call_ollama(model, &summary_msgs, None),
            ))
            .await
        }
        Provider::Gguf => {
            crate::with_esc_cancel(tokio::time::timeout(
                llm_timeout,
                crate::llm::gguf::call_gguf(model, &summary_msgs, None),
            ))
            .await
        }
        Provider::Mlx => {
            crate::with_esc_cancel(tokio::time::timeout(
                llm_timeout,
                crate::llm::mlx::call_mlx(model, &summary_msgs, None),
            ))
            .await
        }
    };

    ui.stop_spinner();

    let result = match result {
        Ok(inner) => inner,
        Err(_interrupted) => {
            println!("\n  {} interrupted\n", "✗".with(Color::Yellow));
            return;
        }
    };

    let result = match result {
        Ok(inner) => inner,
        Err(_elapsed) => {
            println!(
                "\n  {} compaction timed out after {}s (AICTL_LLM_TIMEOUT)\n",
                "✗".with(Color::Yellow),
                llm_timeout.as_secs()
            );
            return;
        }
    };

    match result {
        Ok((summary, usage)) => {
            let system = messages[0].clone();
            messages.clear();
            messages.push(system);
            messages.push(Message {
                role: Role::User,
                content: format!("Here is a summary of our conversation so far:\n\n{summary}"),
                images: vec![],
            });
            messages.push(Message {
                role: Role::Assistant,
                content: "Understood. I have the context from our previous \
                    conversation. How can I help you next?"
                    .to_string(),
                images: vec![],
            });
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
