//! Typed error enum for HTTP handlers.
//!
//! Maps to the OpenAI-style error envelope `{"error": {"code": "...",
//! "message": "..."}}` so client SDKs that already handle OpenAI
//! errors keep working unchanged. See plan §9.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

#[derive(Debug)]
pub enum ApiError {
    BadRequest { code: &'static str, message: String },
    Unauthorized,
    Forbidden { reason: &'static str },
    NotFound { what: &'static str },
    PayloadTooLarge { limit: u64 },
    UnprocessableEntity { code: &'static str, message: String },
    TooManyRequests,
    InternalError { trace_id: String },
    ServiceUnavailable { reason: &'static str },
    GatewayTimeout,
}

#[derive(Serialize)]
struct ErrorEnvelope<'a> {
    error: ErrorBody<'a>,
}

#[derive(Serialize)]
struct ErrorBody<'a> {
    code: &'a str,
    message: &'a str,
}

impl ApiError {
    #[must_use]
    pub fn status(&self) -> StatusCode {
        match self {
            Self::BadRequest { .. } => StatusCode::BAD_REQUEST,
            Self::Unauthorized => StatusCode::UNAUTHORIZED,
            Self::Forbidden { .. } => StatusCode::FORBIDDEN,
            Self::NotFound { .. } => StatusCode::NOT_FOUND,
            Self::PayloadTooLarge { .. } => StatusCode::PAYLOAD_TOO_LARGE,
            Self::UnprocessableEntity { .. } => StatusCode::UNPROCESSABLE_ENTITY,
            Self::TooManyRequests => StatusCode::TOO_MANY_REQUESTS,
            Self::InternalError { .. } => StatusCode::INTERNAL_SERVER_ERROR,
            Self::ServiceUnavailable { .. } => StatusCode::SERVICE_UNAVAILABLE,
            Self::GatewayTimeout => StatusCode::GATEWAY_TIMEOUT,
        }
    }

    fn parts(&self) -> (&'static str, String) {
        match self {
            Self::BadRequest { code, message } | Self::UnprocessableEntity { code, message } => {
                (code, message.clone())
            }
            Self::Unauthorized => ("auth_invalid", "authentication required".to_string()),
            Self::Forbidden { reason } => ("forbidden", (*reason).to_string()),
            Self::NotFound { what } => ("not_found", format!("{what} not found")),
            Self::PayloadTooLarge { limit } => (
                "body_too_large",
                format!("request body exceeded the {limit}-byte cap"),
            ),
            Self::TooManyRequests => ("rate_limited", "too many requests".to_string()),
            Self::InternalError { trace_id } => (
                "internal_error",
                format!("internal server error (trace_id={trace_id})"),
            ),
            Self::ServiceUnavailable { reason } => ("service_unavailable", (*reason).to_string()),
            Self::GatewayTimeout => ("gateway_timeout", "upstream provider timed out".to_string()),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = self.status();
        let (code, message) = self.parts();
        let body = ErrorEnvelope {
            error: ErrorBody {
                code,
                message: &message,
            },
        };
        (status, Json(body)).into_response()
    }
}

/// Convert an `aictl_core::AictlError` from a provider call into an
/// `ApiError` carrying the right HTTP status and a stable error code.
#[must_use]
pub fn from_aictl_error(err: aictl_core::AictlError) -> ApiError {
    use aictl_core::AictlError as E;
    match err {
        E::Timeout { .. } => ApiError::GatewayTimeout,
        E::Auth { provider, .. } => ApiError::Forbidden {
            reason: provider_static_for(provider, "provider_auth_failed"),
        },
        E::Provider { .. } => ApiError::ServiceUnavailable {
            reason: "provider_unavailable",
        },
        E::EmptyResponse { .. } => ApiError::ServiceUnavailable {
            reason: "provider_unavailable",
        },
        E::Stream { .. } => ApiError::ServiceUnavailable {
            reason: "provider_unavailable",
        },
        E::Injection(reason) => ApiError::BadRequest {
            code: "prompt_injection",
            message: reason,
        },
        E::Redaction(reason) => ApiError::BadRequest {
            code: "redaction_blocked",
            message: reason,
        },
        E::Interrupted => ApiError::ServiceUnavailable {
            reason: "interrupted",
        },
        E::Json(_) | E::Http(_) | E::Io(_) => ApiError::InternalError {
            trace_id: short_trace(),
        },
        E::MaxIterations { .. } | E::Other(_) => ApiError::InternalError {
            trace_id: short_trace(),
        },
    }
}

fn provider_static_for(_provider: &'static str, fallback: &'static str) -> &'static str {
    // The variant carries a `&'static str`, but `parts()` needs one too;
    // we don't surface the provider name in the public error code.
    fallback
}

fn short_trace() -> String {
    let id = uuid::Uuid::new_v4();
    let s = id.simple().to_string();
    s[..12].to_string()
}
