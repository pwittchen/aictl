//! Master-key bearer-auth middleware.
//!
//! Every request except `GET /healthz` must carry
//! `Authorization: Bearer <master-key>`. Comparison is constant-time
//! to avoid timing oracles. Missing header → 401; wrong key → 401
//! with an identical body so the distinction can't be enumerated.

use std::sync::Arc;

use axum::extract::State;
use axum::http::{Request, StatusCode, header::AUTHORIZATION};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::error::ApiError;
use crate::state::AppState;

/// Constant-time compare of two byte slices. Returns `false` when the
/// lengths differ — the length itself is not a secret in this protocol.
#[must_use]
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Axum middleware. Rejects 401 unless the request carries a matching
/// `Authorization: Bearer <master-key>` header.
pub async fn require_master_key(
    State(state): State<Arc<AppState>>,
    req: Request<axum::body::Body>,
    next: Next,
) -> Result<Response, ApiError> {
    let Some(header) = req.headers().get(AUTHORIZATION) else {
        tracing::warn!(
            event = "auth_failed",
            reason = "missing_header",
            client_ip = %client_ip(&req),
        );
        return Err(ApiError::Unauthorized);
    };
    let Ok(value) = header.to_str() else {
        tracing::warn!(
            event = "auth_failed",
            reason = "non_utf8_header",
            client_ip = %client_ip(&req),
        );
        return Err(ApiError::Unauthorized);
    };
    let Some(token) = value.strip_prefix("Bearer ").map(str::trim) else {
        tracing::warn!(
            event = "auth_failed",
            reason = "missing_bearer_prefix",
            client_ip = %client_ip(&req),
        );
        return Err(ApiError::Unauthorized);
    };
    if !constant_time_eq(token.as_bytes(), state.master_key.as_bytes()) {
        tracing::warn!(
            event = "auth_failed",
            reason = "invalid_token",
            client_ip = %client_ip(&req),
        );
        return Err(ApiError::Unauthorized);
    }

    Ok(next.run(req).await)
}

fn client_ip(req: &Request<axum::body::Body>) -> String {
    req.extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .map_or_else(|| "unknown".to_string(), |ci| ci.0.ip().to_string())
}

/// Helper used by tests and the `/healthz` route — converts a raw
/// status into a permissible 401-quiet response. Kept separate from
/// the middleware so handler-level paths share the response shape.
#[must_use]
pub fn unauthorized_response() -> (StatusCode, &'static str) {
    (StatusCode::UNAUTHORIZED, "")
}

/// Token-bucket rate limit, keyed on the client IP. No-op when the
/// limiter is disabled (`AICTL_SERVER_RATE_LIMIT_RPM=0`).
pub async fn rate_limit(
    State(state): State<Arc<AppState>>,
    req: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let Some(limiter) = state.rate_limiter.as_ref() else {
        return next.run(req).await;
    };
    let key = client_ip(&req);
    match limiter.check(&key) {
        Ok(()) => next.run(req).await,
        Err(retry_after) => {
            let secs = retry_after.as_secs().max(1);
            tracing::warn!(
                event = "rate_limited",
                client_ip = %key,
                retry_after_secs = secs,
            );
            let mut resp = crate::error::ApiError::TooManyRequests.into_response();
            if let Ok(value) = axum::http::HeaderValue::from_str(&secs.to_string()) {
                resp.headers_mut()
                    .insert(axum::http::header::RETRY_AFTER, value);
            }
            resp
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_time_eq_equal_strings() {
        assert!(constant_time_eq(b"hello", b"hello"));
    }

    #[test]
    fn constant_time_eq_different_strings() {
        assert!(!constant_time_eq(b"hello", b"world"));
    }

    #[test]
    fn constant_time_eq_different_lengths() {
        assert!(!constant_time_eq(b"hello", b"hello!"));
    }

    #[test]
    fn constant_time_eq_empty() {
        assert!(constant_time_eq(b"", b""));
    }
}
