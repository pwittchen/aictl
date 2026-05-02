//! `/v1/chat/completions` and `/v1/completions` — the OpenAI gateway.
//!
//! Both routes follow the same skeleton:
//!
//! 1. Parse the request, translate `messages[]` (or legacy `prompt`)
//!    into the engine's `Vec<Message>`.
//! 2. Resolve the provider from the requested model.
//! 3. Run the prompt-injection guard on user messages.
//! 4. Run `aictl_core::run::redact_outbound` right before dispatch.
//! 5. Audit the dispatch as a `gateway:<provider>` tool call.
//! 6. Dispatch to `aictl_core::llm::call_<provider>` — buffered for
//!    `stream: false`, streaming via SSE for `stream: true`.

use std::convert::Infallible;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Instant;

use axum::Json;
use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use tokio::sync::mpsc;
use uuid::Uuid;

use aictl_core::audit;
use aictl_core::keys;
use aictl_core::llm::{TokenSink, TokenUsage};
use aictl_core::message::{Message, Role};
use aictl_core::run::Provider;
use aictl_core::security;
use aictl_core::security::redaction;
use aictl_core::tools::ToolCall;

use crate::error::{ApiError, from_aictl_error};
use crate::openai::{
    self, ChatCompletionRequest, CompletionRequest, key_name_for_provider, resolve_provider,
};
use crate::state::AppState;

/// `POST /v1/chat/completions`.
pub async fn chat_completions(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ChatCompletionRequest>,
) -> Result<Response, ApiError> {
    let permit = acquire_permit(&state)?;
    openai::reject_tool_request(&req)?;

    let request_id = short_id();
    let provider = resolve_provider(&req.model).await?;
    let api_key = resolve_api_key(&provider)?;

    let messages = openai::to_internal(req.messages)?;
    apply_injection_guard(&messages)?;
    let dispatched = redact(&messages, &provider)?;

    let started = Instant::now();
    let stream = req.stream.unwrap_or(false);

    if stream {
        Ok(stream_chat(
            state, request_id, provider, api_key, req.model, dispatched, permit,
        )
        .await)
    } else {
        let resp = buffered_chat(&request_id, &provider, &api_key, &req.model, &dispatched).await?;
        tracing::info!(
            event = "request_completed",
            request_id = %request_id,
            model = %req.model,
            elapsed_ms = started.elapsed().as_millis() as u64,
        );
        drop(permit);
        Ok(Json(resp).into_response())
    }
}

/// `POST /v1/completions` — legacy text completion. Wraps the prompt
/// in a single user message and routes it through the chat path.
pub async fn completions(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CompletionRequest>,
) -> Result<Response, ApiError> {
    let permit = acquire_permit(&state)?;
    let request_id = short_id();
    let provider = resolve_provider(&req.model).await?;
    let api_key = resolve_api_key(&provider)?;

    let prompt = req.prompt.into_text();
    let messages = vec![Message {
        role: Role::User,
        content: prompt,
        images: vec![],
    }];
    apply_injection_guard(&messages)?;
    let dispatched = redact(&messages, &provider)?;

    let started = Instant::now();

    if req.stream.unwrap_or(false) {
        Ok(stream_chat(
            state, request_id, provider, api_key, req.model, dispatched, permit,
        )
        .await)
    } else {
        let (text, usage) = call_provider(&provider, &api_key, &req.model, &dispatched).await?;
        audit_dispatch(&provider, &request_id, &dispatched);
        tracing::info!(
            event = "request_completed",
            request_id = %request_id,
            model = %req.model,
            elapsed_ms = started.elapsed().as_millis() as u64,
        );
        let resp = openai::wrap_completion_response(&request_id, &req.model, text, &usage);
        drop(permit);
        Ok(Json(resp).into_response())
    }
}

// --- Internals ---------------------------------------------------------------

fn acquire_permit(state: &Arc<AppState>) -> Result<tokio::sync::OwnedSemaphorePermit, ApiError> {
    state
        .semaphore
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError::ServiceUnavailable {
            reason: "concurrency_cap_reached",
        })
}

fn short_id() -> String {
    let id = Uuid::new_v4();
    id.simple().to_string()[..16].to_string()
}

fn resolve_api_key(provider: &Provider) -> Result<String, ApiError> {
    let Some(key_name) = key_name_for_provider(provider) else {
        return Ok(String::new());
    };
    keys::get_secret(key_name).ok_or(ApiError::ServiceUnavailable {
        reason: "provider_key_not_configured",
    })
}

fn apply_injection_guard(messages: &[Message]) -> Result<(), ApiError> {
    if !security::policy().enabled || !security::policy().injection_guard {
        return Ok(());
    }
    for m in messages {
        if matches!(m.role, Role::User)
            && let Err(reason) = security::detect_prompt_injection(&m.content)
        {
            return Err(ApiError::BadRequest {
                code: "prompt_injection",
                message: reason,
            });
        }
    }
    Ok(())
}

fn redact(messages: &[Message], provider: &Provider) -> Result<Vec<Message>, ApiError> {
    let pol = redaction::policy();
    match aictl_core::run::redact_outbound(messages, &pol, provider) {
        Ok(Some(rewritten)) => Ok(rewritten),
        Ok(None) => Ok(messages.to_vec()),
        Err(err) => Err(from_aictl_error(err)),
    }
}

async fn buffered_chat(
    request_id: &str,
    provider: &Provider,
    api_key: &str,
    model: &str,
    messages: &[Message],
) -> Result<openai::ChatCompletionResponse, ApiError> {
    let (text, usage) = call_provider(provider, api_key, model, messages).await?;
    audit_dispatch(provider, request_id, messages);
    Ok(openai::wrap_chat_response(request_id, model, text, &usage))
}

async fn call_provider(
    provider: &Provider,
    api_key: &str,
    model: &str,
    messages: &[Message],
) -> Result<(String, TokenUsage), ApiError> {
    let res = match provider {
        Provider::Openai => {
            aictl_core::llm::openai::call_openai(api_key, model, messages, None).await
        }
        Provider::Anthropic => {
            aictl_core::llm::anthropic::call_anthropic(api_key, model, messages, None).await
        }
        Provider::Gemini => {
            aictl_core::llm::gemini::call_gemini(api_key, model, messages, None).await
        }
        Provider::Grok => aictl_core::llm::grok::call_grok(api_key, model, messages, None).await,
        Provider::Mistral => {
            aictl_core::llm::mistral::call_mistral(api_key, model, messages, None).await
        }
        Provider::Deepseek => {
            aictl_core::llm::deepseek::call_deepseek(api_key, model, messages, None).await
        }
        Provider::Kimi => aictl_core::llm::kimi::call_kimi(api_key, model, messages, None).await,
        Provider::Zai => aictl_core::llm::zai::call_zai(api_key, model, messages, None).await,
        Provider::Ollama => aictl_core::llm::ollama::call_ollama(model, messages, None).await,
        Provider::Gguf => aictl_core::llm::gguf::call_gguf(model, messages, None).await,
        Provider::Mlx => aictl_core::llm::mlx::call_mlx(model, messages, None).await,
        Provider::Mock => {
            return Err(ApiError::Forbidden {
                reason: "mock_provider_disabled",
            });
        }
        Provider::AictlServer => {
            // The server cannot route a request back to itself — that
            // would be an infinite loop. The CLI's explicit
            // `aictl-server` provider is meaningful only on the client
            // side; if a request arrives here with that provider tag
            // the operator has misconfigured something.
            return Err(ApiError::BadRequest {
                code: "model_not_found",
                message:
                    "model resolves to the aictl-server provider — server cannot proxy to itself"
                        .to_string(),
            });
        }
    };
    res.map_err(from_aictl_error)
}

async fn dispatch_provider(
    provider: Provider,
    api_key: String,
    model: String,
    messages: Vec<Message>,
    sink: TokenSink,
) -> Result<(String, TokenUsage), aictl_core::AictlError> {
    match provider {
        Provider::Openai => {
            aictl_core::llm::openai::call_openai(&api_key, &model, &messages, Some(sink)).await
        }
        Provider::Anthropic => {
            aictl_core::llm::anthropic::call_anthropic(&api_key, &model, &messages, Some(sink))
                .await
        }
        Provider::Gemini => {
            aictl_core::llm::gemini::call_gemini(&api_key, &model, &messages, Some(sink)).await
        }
        Provider::Grok => {
            aictl_core::llm::grok::call_grok(&api_key, &model, &messages, Some(sink)).await
        }
        Provider::Mistral => {
            aictl_core::llm::mistral::call_mistral(&api_key, &model, &messages, Some(sink)).await
        }
        Provider::Deepseek => {
            aictl_core::llm::deepseek::call_deepseek(&api_key, &model, &messages, Some(sink)).await
        }
        Provider::Kimi => {
            aictl_core::llm::kimi::call_kimi(&api_key, &model, &messages, Some(sink)).await
        }
        Provider::Zai => {
            aictl_core::llm::zai::call_zai(&api_key, &model, &messages, Some(sink)).await
        }
        Provider::Ollama => {
            aictl_core::llm::ollama::call_ollama(&model, &messages, Some(sink)).await
        }
        Provider::Gguf => aictl_core::llm::gguf::call_gguf(&model, &messages, Some(sink)).await,
        Provider::Mlx => aictl_core::llm::mlx::call_mlx(&model, &messages, Some(sink)).await,
        Provider::Mock => Err(aictl_core::AictlError::Other(
            "mock provider disabled".to_string(),
        )),
        Provider::AictlServer => Err(aictl_core::AictlError::Other(
            "aictl-server cannot proxy back to itself".to_string(),
        )),
    }
}

async fn stream_chat(
    state: Arc<AppState>,
    request_id: String,
    provider: Provider,
    api_key: String,
    model: String,
    messages: Vec<Message>,
    permit: tokio::sync::OwnedSemaphorePermit,
) -> Response {
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let collected = Arc::new(Mutex::new(String::new()));
    let collected_for_sink = collected.clone();
    let sink: TokenSink = Arc::new(move |delta: &str| {
        if let Ok(mut buf) = collected_for_sink.lock() {
            buf.push_str(delta);
        }
        let _ = tx.send(delta.to_string());
    });

    let provider_for_task = provider.clone();
    let model_for_task = model.clone();
    let messages_for_task = messages.clone();
    let api_key_for_task = api_key.clone();
    let dispatch = tokio::spawn(async move {
        dispatch_provider(
            provider_for_task,
            api_key_for_task,
            model_for_task,
            messages_for_task,
            sink,
        )
        .await
    });

    let request_id_for_stream = request_id.clone();
    let model_for_stream = model.clone();
    let provider_for_stream = provider.clone();
    let messages_for_audit = messages;
    let collected_for_stream = collected.clone();

    let stream = async_stream::stream! {
        let role_frame = openai::chunk(&request_id_for_stream, &model_for_stream, None, true);
        yield Ok::<_, Infallible>(make_event(&role_frame));

        let mut dispatch_handle = Some(dispatch);
        loop {
            tokio::select! {
                biased;
                Some(delta) = rx.recv() => {
                    let frame = openai::chunk(&request_id_for_stream, &model_for_stream, Some(delta), false);
                    yield Ok(make_event(&frame));
                }
                join = async {
                    match dispatch_handle.as_mut() {
                        Some(h) => h.await,
                        None => std::future::pending().await,
                    }
                } => {
                    let _ = dispatch_handle.take();
                    while let Ok(delta) = rx.try_recv() {
                        let frame = openai::chunk(&request_id_for_stream, &model_for_stream, Some(delta), false);
                        yield Ok(make_event(&frame));
                    }
                    match join {
                        Ok(Ok((_text, usage))) => {
                            let _final_text = collected_for_stream
                                .lock()
                                .map(|g| g.clone())
                                .unwrap_or_default();
                            audit_dispatch(&provider_for_stream, &request_id_for_stream, &messages_for_audit);
                            let final_frame = openai::final_chunk(&request_id_for_stream, &model_for_stream);
                            yield Ok(make_event(&final_frame));
                            // Trailing usage frame so SDK clients (and
                            // the CLI proxy path) can compute costs the
                            // same way they would on the buffered path.
                            // OpenAI itself emits this shape when
                            // `stream_options.include_usage = true`;
                            // we always send it because the cost is
                            // already known and dropping it would force
                            // every stream client to re-tokenize
                            // locally.
                            let usage_frame = openai::usage_chunk(&request_id_for_stream, &model_for_stream, &usage);
                            yield Ok(make_event(&usage_frame));
                            yield Ok(crate::sse::done_event());
                            break;
                        }
                        Ok(Err(err)) => {
                            let api = from_aictl_error(err);
                            let (code, _) = api_error_code(&api);
                            yield Ok(crate::sse::error_event(code, "upstream provider error"));
                            yield Ok(crate::sse::done_event());
                            break;
                        }
                        Err(_join_err) => {
                            yield Ok(crate::sse::error_event("internal_error", "stream task panicked"));
                            yield Ok(crate::sse::done_event());
                            break;
                        }
                    }
                }
            }
        }
        drop(permit);
    };

    let sse = Sse::new(stream);
    if let Some(interval) = state.config.sse_keepalive {
        sse.keep_alive(KeepAlive::new().interval(interval))
            .into_response()
    } else {
        sse.into_response()
    }
}

fn make_event<T: serde::Serialize>(value: &T) -> Event {
    crate::sse::data_event(value).unwrap_or_else(|_| crate::sse::done_event())
}

fn audit_dispatch(provider: &Provider, request_id: &str, messages: &[Message]) {
    let preview: String = messages
        .iter()
        .filter(|m| matches!(m.role, Role::User))
        .map(|m| m.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let tool = ToolCall {
        name: format!("gateway:{}", provider_tag(provider)),
        input: preview,
    };
    audit::log_tool(&tool, audit::Outcome::Executed { result: request_id });
}

fn provider_tag(p: &Provider) -> &'static str {
    match p {
        Provider::Openai => "openai",
        Provider::Anthropic => "anthropic",
        Provider::Gemini => "gemini",
        Provider::Grok => "grok",
        Provider::Mistral => "mistral",
        Provider::Deepseek => "deepseek",
        Provider::Kimi => "kimi",
        Provider::Zai => "zai",
        Provider::Ollama => "ollama",
        Provider::Gguf => "gguf",
        Provider::Mlx => "mlx",
        Provider::Mock => "mock",
        Provider::AictlServer => "aictl-server",
    }
}

fn api_error_code(err: &ApiError) -> (&'static str, &'static str) {
    match err {
        ApiError::BadRequest { code, .. } | ApiError::UnprocessableEntity { code, .. } => {
            (code, "bad_request")
        }
        ApiError::Unauthorized => ("auth_invalid", "unauthorized"),
        ApiError::Forbidden { reason } => (reason, "forbidden"),
        ApiError::NotFound { .. } => ("not_found", "not_found"),
        ApiError::PayloadTooLarge { .. } => ("body_too_large", "payload_too_large"),
        ApiError::TooManyRequests => ("rate_limited", "too_many_requests"),
        ApiError::InternalError { .. } => ("internal_error", "internal_error"),
        ApiError::ServiceUnavailable { reason } => (reason, "service_unavailable"),
        ApiError::GatewayTimeout => ("gateway_timeout", "gateway_timeout"),
    }
}
