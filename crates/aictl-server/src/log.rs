//! Structured server log.
//!
//! Initializes a `tracing` subscriber that fans out to two sinks:
//!
//! 1. **File sink** — JSON-Lines via `tracing-subscriber`'s `json`
//!    formatter, append-only. Disabled when no log file is configured.
//! 2. **Terminal sink** — human-readable, colored (when stderr is a
//!    TTY and `NO_COLOR` is unset), written to stderr.
//!
//! Both sinks share the same level filter (`AICTL_SERVER_LOG_LEVEL` /
//! `--log-level`). Body lines (request/response previews) are emitted
//! at INFO; metadata lines stay at INFO/WARN/ERROR depending on the
//! event. Redaction must happen at the call-site before fields are
//! attached — the logger never sees raw payloads.

use std::path::Path;

use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::writer::MakeWriterExt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Initialize the global tracing subscriber. Idempotent: a second call
/// is a no-op (tracing's `init` returns an error which we swallow).
pub fn init(level: &str, log_file: Option<&Path>) {
    let filter = EnvFilter::try_new(level).unwrap_or_else(|_| EnvFilter::new("info"));

    let stderr_layer = tracing_subscriber::fmt::layer()
        .with_target(false)
        .with_ansi(use_ansi())
        .with_writer(std::io::stderr);

    let registry = tracing_subscriber::registry()
        .with(filter)
        .with(stderr_layer);

    if let Some(path) = log_file {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            let _ = std::fs::create_dir_all(parent);
        }
        match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            Ok(file) => {
                let json_layer = tracing_subscriber::fmt::layer()
                    .json()
                    .with_writer(file.with_max_level(tracing::Level::TRACE));
                let _ = registry.with(json_layer).try_init();
                return;
            }
            Err(e) => {
                eprintln!("[server] failed to open log file {}: {e}", path.display());
            }
        }
    }

    let _ = registry.try_init();
}

fn use_ansi() -> bool {
    if std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    use std::io::IsTerminal;
    std::io::stderr().is_terminal()
}
