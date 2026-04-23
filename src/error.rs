//! Unified error type for the crate.
//!
//! [`AictlError`] replaces the scattered `Box<dyn std::error::Error>` returns
//! across provider calls (`src/llm/*.rs`), the agent loop (`src/run.rs`), and
//! surrounding call sites. Variants carry enough structure for callers to
//! branch on them — the agent loop distinguishes `Timeout` (retryable) from
//! `Auth` (fatal, prompt re-key) from `Interrupted` (user cancel, rewind
//! history) without string-matching error messages.
//!
//! `From` impls cover the common `?` propagation sources (`reqwest::Error`,
//! `serde_json::Error`, `std::io::Error`, `rustyline::error::ReadlineError`)
//! plus [`crate::run::Interrupted`] and bare `String` / `&str` for ergonomic
//! `into()` conversion at the edges.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum AictlError {
    #[error(
        "LLM call exceeded the {secs}s timeout. Increase AICTL_LLM_TIMEOUT in ~/.aictl/config (seconds, 0 disables) if this is expected on your hardware."
    )]
    Timeout { secs: u64 },

    #[error("{provider} authentication failed ({status}): {body}")]
    Auth {
        provider: &'static str,
        status: u16,
        body: String,
    },

    #[error("{provider} API error ({status}): {body}")]
    Provider {
        provider: &'static str,
        status: u16,
        body: String,
    },

    #[error("No response from {provider}")]
    EmptyResponse { provider: &'static str },

    #[error("{provider} stream error: {message}")]
    Stream {
        provider: &'static str,
        message: String,
    },

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Http(#[from] reqwest::Error),

    #[error(transparent)]
    Json(#[from] serde_json::Error),

    #[error("readline: {0}")]
    Readline(#[from] rustyline::error::ReadlineError),

    #[error("blocked: possible prompt injection ({0})")]
    Injection(String),

    #[error("blocked: outbound message contains sensitive data ({0})")]
    Redaction(String),

    #[error("interrupted")]
    Interrupted,

    #[error("Agent loop reached maximum iterations ({iters}) after {elapsed_secs:.1}s")]
    MaxIterations { iters: u32, elapsed_secs: f64 },

    #[error("{0}")]
    Other(String),
}

impl AictlError {
    /// Classify a non-success HTTP response as either `Auth` (401/403) or
    /// `Provider` (anything else). Keeps provider code terse: a single
    /// `AictlError::from_http(provider, status, body)` replaces the
    /// `format!("{provider} API error ({status}): {body}").into()` chain.
    #[must_use]
    pub fn from_http(provider: &'static str, status: reqwest::StatusCode, body: String) -> Self {
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            AictlError::Auth {
                provider,
                status: status.as_u16(),
                body,
            }
        } else {
            AictlError::Provider {
                provider,
                status: status.as_u16(),
                body,
            }
        }
    }
}

impl From<String> for AictlError {
    fn from(value: String) -> Self {
        AictlError::Other(value)
    }
}

impl From<&str> for AictlError {
    fn from(value: &str) -> Self {
        AictlError::Other(value.to_string())
    }
}

impl From<crate::run::Interrupted> for AictlError {
    fn from(_: crate::run::Interrupted) -> Self {
        AictlError::Interrupted
    }
}
