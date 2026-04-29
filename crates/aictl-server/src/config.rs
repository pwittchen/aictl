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
