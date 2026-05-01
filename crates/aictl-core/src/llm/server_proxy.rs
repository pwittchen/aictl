//! `aictl-server` upstream proxy.
//!
//! When `AICTL_CLIENT_HOST` (and `AICTL_CLIENT_MASTER_KEY`) is configured,
//! the agent loop in [`crate::run`] routes every non-local LLM call through
//! [`call`] instead of the per-provider modules. The server speaks the
//! `OpenAI` shape, so we reuse the request/response types from
//! [`crate::llm::openai`] verbatim — no parallel struct hierarchy.
//!
//! What this module **does not** do:
//!
//!   * Translate model names. The model string is forwarded unchanged; the
//!     server is responsible for picking the upstream provider.
//!   * Handle tool-call XML. The CLI's tool protocol lives inside the
//!     assistant's `content` field as `<tool ...>` markup; the proxy
//!     treats that as opaque chat content.
//!   * Cache, retry, or fall back. One request, one upstream, one response.
//!
//! See `.claude/plans/cli-as-server-client.md` §5 for the full contract.

use std::sync::OnceLock;

use crate::error::AictlError;
use crate::llm::openai::{
    OpenAiRequest, OpenAiResponse, StreamOptions, build_messages, parse_openai_usage,
};
use crate::llm::{TokenSink, TokenUsage};
use crate::message::Message;

/// Bearer-token shaped error returned by the server. Mirrors the
/// `{"error":{"code":"...","message":"..."}}` envelope `aictl-server`
/// emits on every non-streaming error path.
#[derive(serde::Deserialize)]
struct ServerErrorEnvelope {
    error: ServerErrorBody,
}

#[derive(serde::Deserialize)]
struct ServerErrorBody {
    #[serde(default)]
    code: String,
    #[serde(default)]
    message: String,
}

/// Once-per-process cache for the `/healthz` probe. Keyed by URL so a
/// `--client-url` override at run time gets its own probe instead of
/// inheriting the result of an earlier ambient-config probe.
static HEALTH_CHECKED: OnceLock<std::sync::Mutex<Option<String>>> = OnceLock::new();

/// Send a chat completion through `${server_url}/v1/chat/completions`
/// using the master key as a Bearer token. Streams when `on_token` is
/// `Some`, else returns the full assembled body buffered. Mirrors the
/// shape of [`crate::llm::openai::call_openai`] so the agent loop
/// dispatch site is symmetric with the direct-provider path.
pub async fn call(
    server_url: &str,
    master_key: &str,
    model: &str,
    messages: &[Message],
    on_token: Option<TokenSink>,
) -> Result<(String, TokenUsage), AictlError> {
    health_probe_once(server_url).await;

    let client = crate::config::http_client();
    let oai_messages = build_messages(messages);

    let stream = on_token.is_some();
    let body = OpenAiRequest {
        model: model.to_string(),
        messages: oai_messages,
        stream: stream.then_some(true),
        stream_options: stream.then_some(StreamOptions {
            include_usage: true,
        }),
    };

    let url = format!("{}/v1/chat/completions", server_url.trim_end_matches('/'));
    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {master_key}"))
        .json(&body)
        .send()
        .await?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(map_server_error(status, &text));
    }

    if let Some(sink) = on_token {
        let (content, usage) =
            crate::llm::stream::drive_openai_compatible_stream(resp, &sink, parse_openai_usage)
                .await?;
        if content.is_empty() {
            return Err(AictlError::EmptyResponse {
                provider: "aictl-server",
            });
        }
        return Ok((content, usage));
    }

    let text = resp.text().await?;
    let parsed: OpenAiResponse = serde_json::from_str(&text)?;
    let content = parsed
        .choices
        .first()
        .map(|c| c.message.content.clone())
        .unwrap_or_default();
    if content.is_empty() {
        return Err(AictlError::EmptyResponse {
            provider: "aictl-server",
        });
    }
    let usage = parsed
        .usage
        .map(|u| {
            let cached = u.prompt_tokens_details.unwrap_or_default().cached_tokens;
            TokenUsage {
                input_tokens: u.prompt_tokens.saturating_sub(cached),
                output_tokens: u.completion_tokens,
                cache_read_input_tokens: cached,
                ..TokenUsage::default()
            }
        })
        .unwrap_or_default();
    Ok((content, usage))
}

/// Map a non-2xx response from the server into the closest `AictlError`
/// variant. Tries to parse the documented `{"error":{"code":..,"message":..}}`
/// envelope first; if it doesn't parse, falls back to a generic
/// `AictlError::from_http("aictl-server", ...)`.
fn map_server_error(status: reqwest::StatusCode, body: &str) -> AictlError {
    if let Ok(env) = serde_json::from_str::<ServerErrorEnvelope>(body) {
        let code = env.error.code.as_str();
        let msg = if env.error.message.is_empty() {
            body.to_string()
        } else {
            env.error.message.clone()
        };
        return match code {
            "auth_invalid" | "auth_missing" => AictlError::Auth {
                provider: "aictl-server",
                status: status.as_u16(),
                body: msg,
            },
            "prompt_injection" => AictlError::Injection(msg),
            "redaction_blocked" => AictlError::Redaction(msg),
            _ => AictlError::Provider {
                provider: "aictl-server",
                status: status.as_u16(),
                body: msg,
            },
        };
    }
    AictlError::from_http("aictl-server", status, body.to_string())
}

/// Best-effort `GET /healthz` probe that runs once per process per URL.
///
/// The probe never blocks the request — it warns on non-2xx, prints a
/// hard error and returns on network failure. Subsequent calls for the
/// same URL skip the probe entirely.
async fn health_probe_once(server_url: &str) {
    let cache = HEALTH_CHECKED.get_or_init(|| std::sync::Mutex::new(None));
    {
        let guard = cache.lock();
        if let Ok(g) = guard
            && g.as_deref() == Some(server_url)
        {
            return;
        }
    }

    let client = crate::config::http_client();
    let url = format!("{}/healthz", server_url.trim_end_matches('/'));
    let result = client
        .get(&url)
        .timeout(std::time::Duration::from_secs(3))
        .send()
        .await;

    match result {
        Ok(resp) => {
            let status = resp.status();
            if !status.is_success() {
                crate::ui::warn_global(&format!(
                    "aictl-server reachable but unhealthy ({}) at {} — proceeding anyway",
                    status.as_u16(),
                    server_url,
                ));
            }
        }
        Err(e) => {
            crate::ui::warn_global(&format!("aictl-server unreachable at {server_url}: {e}"));
        }
    }

    if let Ok(mut g) = cache.lock() {
        *g = Some(server_url.to_string());
    }
}

/// Fetch the server's catalogue at `/v1/models` and return just the
/// available model IDs (in catalogue order).
///
/// The server's response shape mirrors `OpenAI`'s `{"object":"list",
/// "data":[{"id":"...", ...}, ...]}`. Models marked `available: false`
/// (no upstream key configured for that provider) are filtered out so
/// the CLI's `/model` menu only offers models that will actually work.
///
/// Returns an empty `Vec` on connection / auth errors so the caller can
/// fall back to the static catalogue without aborting.
pub async fn fetch_models(server_url: &str, master_key: &str) -> Vec<String> {
    let client = crate::config::http_client();
    let url = format!("{}/v1/models", server_url.trim_end_matches('/'));
    let resp = match client
        .get(&url)
        .header("Authorization", format!("Bearer {master_key}"))
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => r,
        _ => return Vec::new(),
    };
    let value: serde_json::Value = match resp.json().await {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let Some(data) = value.get("data").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    data.iter()
        .filter(|entry| {
            entry
                .get("available")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(true)
        })
        .filter_map(|entry| entry.get("id").and_then(|v| v.as_str()).map(str::to_string))
        .collect()
}

/// Fetch the server's aggregated stats for `/balance`. Returns the raw
/// JSON the server emitted at `/v1/stats` so `llm::balance` can shape it
/// into its own [`crate::llm::balance::Balance`] rows.
pub async fn fetch_stats(
    server_url: &str,
    master_key: &str,
) -> Result<serde_json::Value, AictlError> {
    let client = crate::config::http_client();
    let url = format!("{}/v1/stats", server_url.trim_end_matches('/'));
    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {master_key}"))
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(map_server_error(status, &body));
    }
    let value: serde_json::Value = resp.json().await?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::StatusCode;

    #[test]
    fn map_server_error_recognizes_auth_envelope() {
        let body = r#"{"error":{"code":"auth_invalid","message":"missing key"}}"#;
        let err = map_server_error(StatusCode::UNAUTHORIZED, body);
        match err {
            AictlError::Auth { provider, .. } => assert_eq!(provider, "aictl-server"),
            other => panic!("expected Auth, got {other:?}"),
        }
    }

    #[test]
    fn map_server_error_recognizes_injection_envelope() {
        let body = r#"{"error":{"code":"prompt_injection","message":"blocked"}}"#;
        let err = map_server_error(StatusCode::BAD_REQUEST, body);
        assert!(matches!(err, AictlError::Injection(_)));
    }

    #[test]
    fn map_server_error_recognizes_redaction_envelope() {
        let body = r#"{"error":{"code":"redaction_blocked","message":"sensitive"}}"#;
        let err = map_server_error(StatusCode::BAD_REQUEST, body);
        assert!(matches!(err, AictlError::Redaction(_)));
    }

    #[test]
    fn map_server_error_falls_back_to_generic_provider() {
        let body = r#"{"error":{"code":"weird_unknown_code","message":"x"}}"#;
        let err = map_server_error(StatusCode::INTERNAL_SERVER_ERROR, body);
        assert!(matches!(err, AictlError::Provider { .. }));
    }

    #[test]
    fn map_server_error_handles_non_envelope_body() {
        let err = map_server_error(StatusCode::BAD_GATEWAY, "not json");
        assert!(matches!(err, AictlError::Provider { .. }));
    }
}
