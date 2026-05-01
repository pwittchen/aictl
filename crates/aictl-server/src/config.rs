//! Server-only configuration knobs.
//!
//! The server reads the same `~/.aictl/config` file the CLI reads, so
//! `aictl_core::config::config_get` is the source of truth. This module
//! adds typed accessors for the `AICTL_SERVER_*` keys that exist only
//! on the server side. CLI flags override these — the resolution order
//! is `CLI flag > env-style config > built-in default`.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use aictl_core::config::config_get;

pub const DEFAULT_BIND: &str = "127.0.0.1:7878";
pub const DEFAULT_REQUEST_TIMEOUT_SECS: u64 = 120;
pub const DEFAULT_BODY_LIMIT_BYTES: usize = 2 * 1024 * 1024;
pub const DEFAULT_MAX_CONCURRENT_REQUESTS: usize = 32;
pub const DEFAULT_SHUTDOWN_TIMEOUT_SECS: u64 = 20;
pub const DEFAULT_SSE_KEEPALIVE_SECS: u64 = 15;
pub const DEFAULT_LOG_LEVEL: &str = "info";
/// Per-IP rate limit in requests-per-minute. `0` disables — the
/// global concurrency cap (`AICTL_SERVER_MAX_CONCURRENT_REQUESTS`)
/// remains in effect either way.
pub const DEFAULT_RATE_LIMIT_RPM: u32 = 0;
/// Token-bucket burst capacity. `0` means "use RPM" so the bucket
/// holds one minute's worth of tokens by default.
pub const DEFAULT_RATE_LIMIT_BURST: u32 = 0;

/// Resolved server configuration. Built once at startup; the rest of
/// the server reads the immutable `Arc<ServerConfig>` from `AppState`.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub bind: SocketAddr,
    pub request_timeout: Duration,
    pub body_limit_bytes: usize,
    pub max_concurrent_requests: usize,
    pub shutdown_timeout: Duration,
    pub sse_keepalive: Option<Duration>,
    pub log_level: String,
    pub log_file: Option<PathBuf>,
    pub log_bodies: bool,
    pub cors_origins: Vec<String>,
    pub rate_limit_rpm: u32,
    pub rate_limit_burst: u32,
}

impl ServerConfig {
    /// Build the runtime config. CLI overrides come in via the
    /// individual `Option` parameters; anything `None` falls back to
    /// the corresponding `AICTL_SERVER_*` config key, then the default.
    #[must_use]
    pub fn load(
        bind_override: Option<String>,
        log_level_override: Option<String>,
        log_file_override: Option<PathBuf>,
    ) -> Self {
        let bind_str = bind_override
            .or_else(|| config_get("AICTL_SERVER_BIND"))
            .unwrap_or_else(|| DEFAULT_BIND.to_string());
        let bind: SocketAddr = bind_str.parse().unwrap_or_else(|_| {
            DEFAULT_BIND
                .parse()
                .expect("DEFAULT_BIND must be a valid SocketAddr")
        });

        let request_timeout_secs = config_get("AICTL_SERVER_REQUEST_TIMEOUT")
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(DEFAULT_REQUEST_TIMEOUT_SECS);
        let request_timeout = if request_timeout_secs == 0 {
            Duration::from_secs(u64::MAX / 2)
        } else {
            Duration::from_secs(request_timeout_secs)
        };

        let body_limit_bytes = config_get("AICTL_SERVER_BODY_LIMIT_BYTES")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(DEFAULT_BODY_LIMIT_BYTES);

        let max_concurrent_requests = config_get("AICTL_SERVER_MAX_CONCURRENT_REQUESTS")
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|v| *v >= 1)
            .unwrap_or(DEFAULT_MAX_CONCURRENT_REQUESTS);

        let shutdown_timeout_secs = config_get("AICTL_SERVER_SHUTDOWN_TIMEOUT")
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(DEFAULT_SHUTDOWN_TIMEOUT_SECS);
        let shutdown_timeout = Duration::from_secs(shutdown_timeout_secs);

        let sse_keepalive_secs = config_get("AICTL_SERVER_SSE_KEEPALIVE")
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(DEFAULT_SSE_KEEPALIVE_SECS);
        let sse_keepalive = if sse_keepalive_secs == 0 {
            None
        } else {
            Some(Duration::from_secs(sse_keepalive_secs))
        };

        let log_level = log_level_override
            .or_else(|| config_get("AICTL_SERVER_LOG_LEVEL"))
            .unwrap_or_else(|| DEFAULT_LOG_LEVEL.to_string());

        let log_file = log_file_override.or_else(|| {
            let raw = config_get("AICTL_SERVER_LOG_FILE")?;
            if raw.is_empty() {
                None
            } else {
                Some(expand_home(&raw))
            }
        });

        let log_bodies =
            config_get("AICTL_SERVER_LOG_BODIES").is_none_or(|v| v != "false" && v != "0");

        let cors_origins = config_get("AICTL_SERVER_CORS_ORIGINS")
            .map(|s| {
                s.split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();

        let rate_limit_rpm = config_get("AICTL_SERVER_RATE_LIMIT_RPM")
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(DEFAULT_RATE_LIMIT_RPM);
        let rate_limit_burst = config_get("AICTL_SERVER_RATE_LIMIT_BURST")
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(DEFAULT_RATE_LIMIT_BURST);

        Self {
            bind,
            request_timeout,
            body_limit_bytes,
            max_concurrent_requests,
            shutdown_timeout,
            sse_keepalive,
            log_level,
            log_file,
            log_bodies,
            cors_origins,
            rate_limit_rpm,
            rate_limit_burst,
        }
    }
}

fn expand_home(raw: &str) -> PathBuf {
    if let Some(rest) = raw.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(raw)
}

/// Whether the resolved bind address listens on a non-loopback
/// interface. Used to print a startup warning so accidental
/// `0.0.0.0` (e.g. from a container template) is at least visible.
#[must_use]
pub fn is_non_loopback(addr: &SocketAddr) -> bool {
    !addr.ip().is_loopback()
}

/// Format a byte count as `"NNN B"` / `"NNN KiB"` / `"NNN MiB"`.
fn format_bytes(bytes: usize) -> String {
    if bytes >= 1024 * 1024 {
        #[allow(clippy::cast_precision_loss)]
        let mib = bytes as f64 / (1024.0 * 1024.0);
        format!("{mib:.1} MiB")
    } else if bytes >= 1024 {
        #[allow(clippy::cast_precision_loss)]
        let kib = bytes as f64 / 1024.0;
        format!("{kib:.1} KiB")
    } else {
        format!("{bytes} B")
    }
}

/// Format an `Option<Duration>` as `"NNs"` or `"disabled"`.
fn format_duration(d: Duration) -> String {
    if d.as_secs() >= u64::MAX / 4 {
        "disabled".to_string()
    } else {
        format!("{}s", d.as_secs())
    }
}

/// Emit one structured tracing event per knob covering this server's
/// resolved configuration: every `AICTL_SERVER_*` value, plus the
/// engine-side security and redaction posture (`AICTL_SERVER_SECURITY_*`
/// and `AICTL_SERVER_REDACTION_*` keys, with fall-throughs to the shared
/// `AICTL_*` keys already resolved).
///
/// Sensitive fields (master key, provider API keys, NER allowlist
/// content) are intentionally **not** logged — the goal is to make the
/// posture obvious to an operator inspecting `~/.aictl/server.log`,
/// not to dump secrets. The `--log-bodies` setting governs request
/// bodies separately.
///
/// Mirrors a one-liner banner to stderr (gated on `quiet`) so an
/// operator running the server in the foreground sees the same picture
/// without tailing the log file.
#[allow(clippy::too_many_lines)]
pub fn log_startup_config(cfg: &ServerConfig, quiet: bool) {
    use aictl_core::audit;
    use aictl_core::security;
    use aictl_core::security::redaction;

    let log_file_display = cfg
        .log_file
        .as_ref()
        .map_or_else(|| "(stderr only)".to_string(), |p| p.display().to_string());

    let cors_display = if cfg.cors_origins.is_empty() {
        "(off)".to_string()
    } else {
        cfg.cors_origins.join(",")
    };

    let rate_limit_display = if cfg.rate_limit_rpm == 0 {
        "(disabled)".to_string()
    } else {
        let burst = if cfg.rate_limit_burst == 0 {
            cfg.rate_limit_rpm
        } else {
            cfg.rate_limit_burst
        };
        format!("{} rpm, burst {}", cfg.rate_limit_rpm, burst)
    };

    tracing::info!(
        event = "startup_config",
        bind = %cfg.bind,
        request_timeout = %format_duration(cfg.request_timeout),
        body_limit = %format_bytes(cfg.body_limit_bytes),
        max_concurrent_requests = cfg.max_concurrent_requests,
        shutdown_timeout = %format_duration(cfg.shutdown_timeout),
        sse_keepalive = ?cfg.sse_keepalive.map(|d| d.as_secs()),
        log_level = %cfg.log_level,
        log_file = %log_file_display,
        log_bodies = cfg.log_bodies,
        cors = %cors_display,
        rate_limit = %rate_limit_display,
    );

    let sec = security::policy();
    tracing::info!(
        event = "startup_security",
        enabled = sec.enabled,
        injection_guard = sec.injection_guard,
        audit_log = audit::enabled(),
    );

    let red = redaction::policy();
    let detectors_display = if red.enabled_detectors.is_empty() {
        "(all built-ins)".to_string()
    } else {
        red.enabled_detectors.join(",")
    };
    let extras_display = if red.extra_patterns.is_empty() {
        "(none)".to_string()
    } else {
        red.extra_patterns
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>()
            .join(",")
    };
    tracing::info!(
        event = "startup_redaction",
        mode = ?red.mode,
        skip_local_providers = red.skip_local,
        detectors = %detectors_display,
        extra_patterns = %extras_display,
        allowlist_count = red.allowlist.len(),
        ner_requested = red.ner_requested,
    );

    if !quiet {
        eprintln!(
            "[server] startup posture: security={} injection_guard={} audit={} redaction={:?}{} cors={}",
            sec.enabled,
            sec.injection_guard,
            audit::enabled(),
            red.mode,
            if red.ner_requested { " ner=requested" } else { "" },
            cors_display,
        );
        eprintln!(
            "[server] limits: body={} max_concurrent={} request_timeout={} sse_keepalive={} rate_limit={}",
            format_bytes(cfg.body_limit_bytes),
            cfg.max_concurrent_requests,
            format_duration(cfg.request_timeout),
            cfg.sse_keepalive
                .map_or_else(|| "off".to_string(), |d| format!("{}s", d.as_secs())),
            rate_limit_display,
        );
    }
}

/// Probe the model catalogue once at startup so an operator can see
/// from the log how many models the server can serve and which
/// providers are actually wired up.
///
/// This calls into the same `list_models` machinery the
/// `GET /v1/models` route uses, so the report is exactly what a client
/// would receive — including locally-detected Ollama / GGUF / MLX
/// entries and the per-provider `available` flag (which is true when
/// the upstream API key is configured, or for local providers when the
/// model file is on disk).
///
/// Async because Ollama is probed over HTTP.
pub async fn log_startup_models(quiet: bool) {
    use std::collections::BTreeMap;

    let models = crate::openai::list_models().await;
    let total = models.data.len();
    let available_total = models.data.iter().filter(|m| m.available).count();

    // Group available models by their `owned_by` label so the operator
    // sees both *which* providers are wired up and how many models per
    // provider are dispatchable. Providers with zero available models
    // (e.g. catalogued cloud providers whose API key is unset) appear
    // separately in the unconfigured count rather than showing up as
    // "0 models" rows that drown out the useful entries.
    let mut available_by_provider: BTreeMap<String, usize> = BTreeMap::new();
    let mut unconfigured_by_provider: BTreeMap<String, usize> = BTreeMap::new();
    for m in &models.data {
        if m.available {
            *available_by_provider
                .entry(m.owned_by.clone())
                .or_insert(0) += 1;
        } else {
            *unconfigured_by_provider
                .entry(m.owned_by.clone())
                .or_insert(0) += 1;
        }
    }

    // Render `Provider:N` pairs in alphabetical order so the log line
    // is stable across restarts.
    let configured_providers: String = if available_by_provider.is_empty() {
        "(none)".to_string()
    } else {
        available_by_provider
            .iter()
            .map(|(name, n)| format!("{name}:{n}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let unconfigured_providers: String = if unconfigured_by_provider.is_empty() {
        String::new()
    } else {
        unconfigured_by_provider
            .keys()
            .cloned()
            .collect::<Vec<_>>()
            .join(", ")
    };

    tracing::info!(
        event = "startup_models",
        total = total,
        available = available_total,
        unavailable = total - available_total,
        configured_providers = %configured_providers,
        unconfigured_providers = %unconfigured_providers,
    );

    if !quiet {
        eprintln!(
            "[server] models: {available_total} available out of {total} (configured: {configured_providers})"
        );
        if !unconfigured_providers.is_empty() {
            eprintln!(
                "[server] unconfigured providers (no API key): {unconfigured_providers}"
            );
        }
    }
}
