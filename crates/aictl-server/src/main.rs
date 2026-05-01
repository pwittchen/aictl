//! `aictl-server` — OpenAI-compatible HTTP LLM proxy.
//!
//! Pure proxy. No agent loop, no tool dispatch, no skills/agents/sessions.
//! See SERVER.md and `.claude/plans/server.md` for the full design.

#![allow(
    clippy::must_use_candidate,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    // OpenAI's request schema accepts fields the server doesn't act on
    // yet (`temperature`, `top_p`, `max_tokens`, …). Keep them so serde
    // round-trips faithfully when callers send them. Also covers the
    // full taxonomy of `ApiError` variants — every status the server
    // documents has a variant even when no current handler emits it.
    dead_code,
    clippy::doc_markdown,
    clippy::cast_possible_truncation,
    clippy::unused_async,
    clippy::double_must_use,
    clippy::struct_field_names,
    clippy::collapsible_if,
    clippy::items_after_statements,
    clippy::match_same_arms
)]

mod auth;
mod config;
mod error;
mod log;
mod master_key;
mod openai;
mod rate_limit;
mod routes;
mod sse;
mod state;
mod uninstall;
mod update;

use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::middleware;
use axum::routing::{get, post};
use clap::Parser;
use tower_http::cors::CorsLayer;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

use crate::config::ServerConfig;
use crate::master_key::KeySource;
use crate::state::AppState;

#[derive(Parser, Debug)]
#[command(
    name = "aictl-server",
    version,
    disable_version_flag = true,
    about = "OpenAI-compatible HTTP LLM proxy for aictl"
)]
#[allow(clippy::struct_excessive_bools)]
struct Cli {
    /// Print version information (with an upstream-latest check).
    #[arg(short = 'v', long = "version")]
    version: bool,
    /// Update to the latest version by re-running the published install
    /// script. Exits when the upgrade finishes.
    #[arg(long = "update")]
    update: bool,
    /// Override the path to the aictl config file (default `~/.aictl/config`).
    #[arg(long)]
    config: Option<PathBuf>,
    /// Override `AICTL_SERVER_BIND` for this launch (e.g. `0.0.0.0:7878`).
    #[arg(long)]
    bind: Option<String>,
    /// Provide the master API key for this launch (not persisted).
    #[arg(long)]
    master_key: Option<String>,
    /// Suppress the startup banner on stderr.
    #[arg(long)]
    quiet: bool,
    /// Override `AICTL_SERVER_LOG_LEVEL` (`trace`/`debug`/`info`/`warn`/`error`).
    #[arg(long)]
    log_level: Option<String>,
    /// Override `AICTL_SERVER_LOG_FILE` (empty disables the file sink).
    #[arg(long)]
    log_file: Option<PathBuf>,
    /// Override `AICTL_SERVER_AUDIT_FILE` (per-process audit log path).
    /// Defaults to `~/.aictl/server-audit.log`. The audit subsystem
    /// itself can still be disabled via
    /// `AICTL_SERVER_SECURITY_AUDIT_LOG=false`; this flag only controls
    /// where the file lands when audit is on.
    #[arg(long = "audit-file")]
    audit_file: Option<PathBuf>,
    /// Remove the `aictl-server` binary from `~/.cargo/bin/`,
    /// `~/.local/bin/`, `/usr/local/bin/` (and `$AICTL_INSTALL_DIR` if
    /// set) and exit. Leaves `~/.aictl/` untouched.
    #[arg(long = "uninstall")]
    uninstall: bool,
}

#[tokio::main]
#[allow(clippy::too_many_lines)]
async fn main() {
    let cli = Cli::parse();

    if cli.version {
        update::run_version().await;
        return;
    }

    if cli.update {
        update::run_update_cli().await;
        return;
    }

    if cli.uninstall {
        uninstall::run();
    }

    if cli.config.is_some() {
        // Plan §3: the engine config loader hard-codes `~/.aictl/config`.
        // The Modular Architecture plan owns extending it to take an
        // override; until that lands, surface a clear error so an
        // operator running multiple servers knows what's missing.
        eprintln!(
            "[server] --config is not yet supported in this build; aictl-server reads ~/.aictl/config. \
             Run separate processes with HOME pointing at distinct config trees as a workaround."
        );
    }

    if let Err(msg) = aictl_core::config::load_config() {
        eprintln!("[server] failed to load config: {msg}");
        std::process::exit(1);
    }

    // Tag this process as the server role *before* the security /
    // redaction / audit subsystems read config. That makes
    // `AICTL_SERVER_SECURITY_*` and `AICTL_SERVER_REDACTION_*` win for
    // their respective lookups; absent overrides fall through to the
    // shared `AICTL_*` keys so single-host setups keep working.
    aictl_core::config::set_role(aictl_core::config::Role::Server);

    let server_config = ServerConfig::load(
        cli.bind.clone(),
        cli.log_level.clone(),
        cli.log_file.clone(),
        cli.audit_file.clone(),
    );

    log::init(&server_config.log_level, server_config.log_file.as_deref());

    // Pin a per-process audit file before any gateway dispatch could
    // run. The CLI's session-keyed audit scheme
    // (`~/.aictl/audit/<session-id>`) does not apply here because the
    // server has no notion of a session — without this override the
    // `audit::log_tool` calls in `routes::gateway` early-return.
    //
    // We only set the override when the audit toggle is on, so an
    // operator who switched audit off via
    // `AICTL_SERVER_SECURITY_AUDIT_LOG=false` continues to get no
    // disk writes (the override would otherwise force-enable the
    // subsystem — its semantics for the CLI's `--audit-file` flag,
    // which is a single-shot opt-in).
    if aictl_core::audit::enabled()
        && let Some(path) = server_config.audit_file.as_deref()
    {
        aictl_core::audit::set_file_override(path);
    }

    // Initialize the engine's security policy and redaction pipeline so
    // the prompt-injection guard and `redact_outbound` use the user's
    // configured rules. `--unrestricted` is not a server flag — the
    // server's job is to apply policy, not bypass it. Redaction
    // warnings are surfaced through tracing.
    let redaction_warnings = aictl_core::security::init(false);
    for w in redaction_warnings {
        tracing::warn!(event = "redaction_init", warning = %w);
    }

    let resolved = master_key::resolve(cli.master_key.clone());
    if let KeySource::Generated = resolved.source {
        // Print the new key exactly once, both to stderr and to the
        // structured log, so operators can grab it.
        eprintln!(
            "[server] generated new master API key — set Authorization: Bearer {}",
            resolved.key
        );
        eprintln!("[server] persisted to ~/.aictl/config (AICTL_SERVER_MASTER_KEY)");
        tracing::info!(
            event = "master_key_generated",
            persisted = true,
            "new master API key generated and persisted"
        );
    }

    if config::is_non_loopback(&server_config.bind) {
        let msg = format!(
            "binding non-loopback address {} — exposed beyond localhost",
            server_config.bind
        );
        if !cli.quiet {
            eprintln!("[server] WARNING: {msg}");
        }
        tracing::warn!(event = "non_loopback_bind", bind = %server_config.bind);
    }

    let state = AppState::new(resolved.key, server_config.clone());

    // Log every resolved knob (server-side `AICTL_SERVER_*` plus the
    // role-scoped security / redaction / audit posture) before
    // accepting connections so an operator inspecting the log file —
    // or watching stderr in the foreground — can see exactly which
    // policies are in force without grepping config or curling
    // `/healthz`. Secrets (master key, provider keys, allowlist
    // content) are not included on purpose.
    config::log_startup_config(&server_config, cli.quiet);
    // Probe the model catalogue too so the operator sees how many
    // models are available and which providers are actually
    // configured — same numbers `GET /v1/models` would return.
    config::log_startup_models(cli.quiet).await;

    let app = build_router(state.clone());

    let addr = server_config.bind;
    if !cli.quiet {
        eprintln!(
            "[server] aictl-server {} listening on http://{}",
            aictl_core::VERSION,
            addr
        );
    }
    tracing::info!(
        event = "server_listening",
        bind = %addr,
        version = aictl_core::VERSION,
    );
    if state.rate_limiter.is_some() {
        tracing::info!(
            event = "rate_limit_enabled",
            rpm = server_config.rate_limit_rpm,
            burst = server_config.rate_limit_burst,
        );
    }

    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!(event = "bind_failed", error = %e);
            eprintln!("[server] failed to bind {addr}: {e}");
            std::process::exit(1);
        }
    };

    let shutdown_timeout = server_config.shutdown_timeout;
    let serve = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal(shutdown_timeout));

    if let Err(e) = serve.await {
        tracing::error!(event = "server_error", error = %e);
        std::process::exit(1);
    }

    tracing::info!(event = "server_shutdown_complete");
}

fn build_router(state: Arc<AppState>) -> Router {
    let server_config = state.config.clone();

    // Authenticated routes — every gateway request must carry the master key.
    // Layer order: outer → inner is auth → rate-limit → handler. We want
    // unauthenticated traffic to hit 401 *before* burning a token bucket
    // entry, so the auth layer wraps the rate limiter.
    let mut authed = Router::new()
        .route(
            "/v1/chat/completions",
            post(routes::gateway::chat_completions),
        )
        .route("/v1/completions", post(routes::gateway::completions))
        .route("/v1/models", get(routes::models::list))
        .route("/v1/stats", get(routes::stats::stats))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth::rate_limit,
        ))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_master_key,
        ));

    if !server_config.cors_origins.is_empty() {
        // CORS off by default — only enabled when origins are configured.
        let cors = build_cors(&server_config.cors_origins);
        authed = authed.layer(cors);
    }

    Router::new()
        .route("/healthz", get(routes::health::healthz))
        .merge(authed)
        .layer(DefaultBodyLimit::max(server_config.body_limit_bytes))
        .layer(RequestBodyLimitLayer::new(server_config.body_limit_bytes))
        .layer(TimeoutLayer::with_status_code(
            axum::http::StatusCode::GATEWAY_TIMEOUT,
            server_config.request_timeout,
        ))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

fn build_cors(origins: &[String]) -> CorsLayer {
    use axum::http::HeaderValue;
    let mut layer = CorsLayer::new()
        .allow_methods([
            axum::http::Method::GET,
            axum::http::Method::POST,
            axum::http::Method::OPTIONS,
        ])
        .allow_headers([
            axum::http::header::CONTENT_TYPE,
            axum::http::header::AUTHORIZATION,
        ])
        .allow_credentials(true);
    let mut parsed: Vec<HeaderValue> = Vec::new();
    for o in origins {
        if let Ok(v) = HeaderValue::from_str(o) {
            parsed.push(v);
        }
    }
    if !parsed.is_empty() {
        layer = layer.allow_origin(parsed);
    }
    layer
}

async fn shutdown_signal(grace: std::time::Duration) {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {}
        () = terminate => {}
    }

    tracing::info!(
        event = "shutdown_started",
        grace_secs = grace.as_secs(),
        "received shutdown signal"
    );
}
