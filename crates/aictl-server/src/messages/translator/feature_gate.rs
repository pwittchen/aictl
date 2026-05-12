//! Drop, warn, or reject Anthropic-only features the target provider
//! can't honor. Driven by `AICTL_SERVER_MESSAGES_FEATURE_GATE`:
//!
//! - `strip` (default) — silently drop the feature. Return the list
//!   of dropped names so the translator can log a `feature_dropped`
//!   audit event.
//! - `warn` — same as strip, but the caller should set an
//!   `aictl-warning` response header listing the dropped features.
//! - `reject` — return `400 feature_unsupported_for_provider` so
//!   strict operators surface the gap immediately.

use aictl_core::config::config_get;
use aictl_core::run::Provider;

use crate::error::ApiError;
use crate::messages::provider_tag;
use crate::messages::translator::ir::{
    AnthropicRequest, ContentBlock, ImageSource, ToolResultBlock, ToolResultContent,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateMode {
    Strip,
    Warn,
    Reject,
}

#[must_use]
pub fn mode() -> GateMode {
    match config_get("AICTL_SERVER_MESSAGES_FEATURE_GATE")
        .as_deref()
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("reject") => GateMode::Reject,
        Some("warn") => GateMode::Warn,
        _ => GateMode::Strip,
    }
}

/// Walk the IR, drop Anthropic-only features (or reject the request).
/// Returns the list of features that were stripped — the caller logs
/// them via the audit subsystem and, in `warn` mode, surfaces them on
/// the response.
pub fn apply(
    ir: &mut AnthropicRequest,
    provider: &Provider,
) -> Result<Vec<&'static str>, ApiError> {
    let mode = mode();
    let mut dropped: Vec<&'static str> = Vec::new();

    if ir.cache_control_seen {
        dropped.push("cache_control");
        ir.cache_control_seen = false;
    }
    if ir.thinking_seen {
        dropped.push("thinking");
        ir.thinking_seen = false;
    }
    if ir.document_block_seen {
        // PDF blocks are always rejected on cross-provider routes —
        // there's no clean equivalent on OpenAI/Gemini/Ollama, and
        // silently stripping a document the user expected to be
        // processed would be worse than failing loud.
        return Err(ApiError::BadRequest {
            code: "feature_unsupported_for_provider",
            message: format!(
                "PDF document blocks are not supported on cross-provider routes (target: {})",
                provider_tag(provider)
            ),
        });
    }

    // Image-block validation per provider. Gemini and Ollama only
    // accept base64 inline images; URL images are rejected. OpenAI
    // accepts both (the data URL we build is just a base64 wrapped in
    // a `data:` URI).
    if matches!(provider, Provider::Gemini | Provider::Ollama) {
        for m in &ir.messages {
            for b in &m.content {
                if matches!(
                    b,
                    ContentBlock::Image {
                        source: ImageSource::Url { .. }
                    }
                ) {
                    return Err(ApiError::BadRequest {
                        code: "feature_unsupported_for_provider",
                        message: format!(
                            "URL image sources are not supported on {} (use base64 instead)",
                            provider_tag(provider)
                        ),
                    });
                }
                if let ContentBlock::ToolResult { content, .. } = b
                    && let ToolResultContent::Blocks(blocks) = content
                {
                    for tr in blocks {
                        if matches!(tr, ToolResultBlock::Image(ImageSource::Url { .. })) {
                            return Err(ApiError::BadRequest {
                                code: "feature_unsupported_for_provider",
                                message: format!(
                                    "URL image sources are not supported on {}",
                                    provider_tag(provider)
                                ),
                            });
                        }
                    }
                }
            }
        }
    }

    // OpenAI-family doesn't take top_k; drop it.
    if matches!(
        provider,
        Provider::Openai
            | Provider::Grok
            | Provider::Mistral
            | Provider::Deepseek
            | Provider::Kimi
            | Provider::Zai
    ) && ir.top_k.is_some()
    {
        dropped.push("top_k");
        ir.top_k = None;
    }

    // Only OpenAI-family carries metadata.user_id through (mapped to
    // `user`). Gemini and Ollama have no equivalent — drop it.
    if matches!(provider, Provider::Gemini | Provider::Ollama)
        && ir.metadata.as_ref().is_some_and(|m| m.user_id.is_some())
    {
        dropped.push("metadata.user_id");
        if let Some(m) = ir.metadata.as_mut() {
            m.user_id = None;
        }
    }

    if !dropped.is_empty() && matches!(mode, GateMode::Reject) {
        return Err(ApiError::BadRequest {
            code: "feature_unsupported_for_provider",
            message: format!(
                "the request uses features unsupported on {}: {}",
                provider_tag(provider),
                dropped.join(",")
            ),
        });
    }

    Ok(dropped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::translator::ir::{AnthropicRole, ImageSource};
    use serde_json::json;

    fn base_ir() -> AnthropicRequest {
        let v = json!({
            "model": "gpt-4o-mini",
            "max_tokens": 100,
            "messages": [{"role": "user", "content": "hi"}],
        });
        crate::messages::translator::ir::parse(&v).unwrap()
    }

    #[test]
    fn strip_drops_cache_control() {
        let mut ir = base_ir();
        ir.cache_control_seen = true;
        let dropped = apply(&mut ir, &Provider::Openai).unwrap();
        assert!(dropped.contains(&"cache_control"));
        assert!(!ir.cache_control_seen);
    }

    #[test]
    fn strip_drops_thinking_and_top_k() {
        let mut ir = base_ir();
        ir.thinking_seen = true;
        ir.top_k = Some(40);
        let dropped = apply(&mut ir, &Provider::Openai).unwrap();
        assert!(dropped.contains(&"thinking"));
        assert!(dropped.contains(&"top_k"));
        assert!(ir.top_k.is_none());
    }

    #[test]
    fn pdf_blocks_rejected() {
        let mut ir = base_ir();
        ir.document_block_seen = true;
        let err = apply(&mut ir, &Provider::Openai).unwrap_err();
        assert!(matches!(
            err,
            ApiError::BadRequest {
                code: "feature_unsupported_for_provider",
                ..
            }
        ));
    }

    #[test]
    fn gemini_rejects_url_images() {
        let mut ir = base_ir();
        ir.messages[0].content.push(ContentBlock::Image {
            source: ImageSource::Url {
                url: "https://example.com/cat.png".into(),
            },
        });
        ir.messages[0].role = AnthropicRole::User;
        let err = apply(&mut ir, &Provider::Gemini).unwrap_err();
        assert!(matches!(
            err,
            ApiError::BadRequest {
                code: "feature_unsupported_for_provider",
                ..
            }
        ));
    }

    #[test]
    fn openai_accepts_url_images() {
        let mut ir = base_ir();
        ir.messages[0].content.push(ContentBlock::Image {
            source: ImageSource::Url {
                url: "https://example.com/cat.png".into(),
            },
        });
        assert!(apply(&mut ir, &Provider::Openai).is_ok());
    }
}
