//! Chat template renderer. Prefers the jinja template embedded in
//! `tokenizer_config.json` (rendered via `minijinja`); falls back to a
//! ChatML-like format if the template is missing or fails to render. Gemma 2
//! uses a dedicated `gemma_prompt` builder because its jinja template rejects
//! system role and enforces strict user/assistant alternation.

#![cfg(all(feature = "mlx", target_os = "macos", target_arch = "aarch64"))]

use crate::{Message, Role};
use minijinja::value::Value;

/// Render a chat template loaded from tokenizer_config.json into a
/// ready-to-tokenize string. Returns `Ok(text)` on success; `Err(reason)`
/// so the caller can fall back to a generic ChatML envelope.
pub fn render(template: &str, messages: &[Message]) -> Result<String, String> {
    let mut env = minijinja::Environment::new();
    env.add_template("chat", template)
        .map_err(|e| format!("chat template compile failed: {e}"))?;
    let tmpl = env.get_template("chat").map_err(|e| e.to_string())?;
    let msgs: Vec<Value> = messages
        .iter()
        .map(|m| {
            let role = match m.role {
                Role::System => "system",
                Role::User => "user",
                Role::Assistant => "assistant",
            };
            Value::from_serialize(&serde_json::json!({
                "role": role,
                "content": m.content,
            }))
        })
        .collect();
    tmpl.render(minijinja::context! {
        messages => Value::from(msgs),
        add_generation_prompt => true,
        bos_token => "",
        eos_token => "",
    })
    .map_err(|e| format!("chat template render failed: {e}"))
}

/// Gemma 2-specific prompt builder. Gemma's official chat template
/// `raise_exception`s on system role and enforces strict user/assistant
/// alternation — both violated by aictl's normal message flow (system
/// prompt, then system reinforcement appended as a tail). We skip the
/// jinja template entirely and build Gemma's `<start_of_turn>role\n...
/// <end_of_turn>` envelope manually, merging adjacent system content
/// into the next user turn. The trailing reinforcement is glued onto
/// the last user message so the whole prompt ends in a single `model`
/// turn start, which is what the tokenizer's BOS + generation prompt
/// contract expects.
pub fn gemma_prompt(messages: &[Message], reinforcement: &str) -> String {
    let mut items: Vec<(&'static str, String)> = Vec::new();
    let mut pending_system = String::new();
    for m in messages {
        match m.role {
            Role::System => {
                if !pending_system.is_empty() {
                    pending_system.push_str("\n\n");
                }
                pending_system.push_str(&m.content);
            }
            Role::User => {
                let content = if pending_system.is_empty() {
                    m.content.clone()
                } else {
                    let merged = format!("{}\n\n{}", pending_system, m.content);
                    pending_system.clear();
                    merged
                };
                items.push(("user", content));
            }
            Role::Assistant => {
                items.push(("model", m.content.clone()));
            }
        }
    }
    if !pending_system.is_empty() {
        // System-only prompt with no user turn: make it the user turn.
        items.push(("user", std::mem::take(&mut pending_system)));
    }

    // Glue the reinforcement onto the final user turn so Gemma's
    // alternation invariant holds (no trailing system/model message).
    if let Some(last) = items.last_mut()
        && last.0 == "user"
    {
        last.1.push_str("\n\n");
        last.1.push_str(reinforcement);
    } else {
        items.push(("user", reinforcement.to_string()));
    }

    let mut out = String::new();
    for (role, content) in &items {
        out.push_str("<start_of_turn>");
        out.push_str(role);
        out.push('\n');
        out.push_str(content.trim());
        out.push_str("<end_of_turn>\n");
    }
    out.push_str("<start_of_turn>model\n");
    out
}

/// ChatML-ish fallback used when the repo's template can't be rendered.
/// Not canonical for any specific model family, but produces coherent
/// output for most instruction-tuned Llama/Qwen/Mistral checkpoints.
pub fn chatml_fallback(messages: &[Message], reinforcement: &str) -> String {
    let mut out = String::new();
    for m in messages {
        let role = match m.role {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
        };
        out.push_str("<|im_start|>");
        out.push_str(role);
        out.push('\n');
        out.push_str(&m.content);
        out.push_str("<|im_end|>\n");
    }
    out.push_str("<|im_start|>system\n");
    out.push_str(reinforcement);
    out.push_str("<|im_end|>\n");
    out.push_str("<|im_start|>assistant\n");
    out
}
