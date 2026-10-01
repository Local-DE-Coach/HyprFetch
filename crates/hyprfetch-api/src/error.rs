//! Error type for the API layer.
//!
//! Maps cleanly to HTTP status codes and the JSON error shape described in
//! `docs/api.md`.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;

/// Stable error codes used in the JSON `error.code` field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiErrorCode {
    InvalidUrl,
    SsrfBlocked,
    TaskNotFound,
    InvalidStateTransition,
    InvalidRequest,
    ServiceUnavailable,
    InternalError,
}

impl ApiErrorCode {
    fn as_str(&self) -> &'static str {
        match self {
            Self::InvalidUrl => "invalid_url",
            Self::SsrfBlocked => "ssrf_blocked",
            Self::TaskNotFound => "task_not_found",
            Self::InvalidStateTransition => "invalid_state_transition",
            Self::InvalidRequest => "invalid_request",
            Self::ServiceUnavailable => "service_unavailable",
            Self::InternalError => "internal_error",
        }
    }

    fn status(&self) -> StatusCode {
        match self {
            Self::InvalidUrl | Self::InvalidRequest => StatusCode::BAD_REQUEST,
            Self::SsrfBlocked => StatusCode::FORBIDDEN,
            Self::TaskNotFound => StatusCode::NOT_FOUND,
            Self::InvalidStateTransition => StatusCode::CONFLICT,
            Self::ServiceUnavailable => StatusCode::SERVICE_UNAVAILABLE,
            Self::InternalError => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

/// Error returned by API handlers.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("invalid URL: {0}")]
    InvalidUrl(String),
    #[error("URL blocked by SSRF protection: {0}")]
    SsrfBlocked(String),
    #[error("task not found: {0}")]
    TaskNotFound(String),
    #[error("invalid state transition: {0}")]
    InvalidStateTransition(String),
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    #[error("service unavailable: {0}")]
    ServiceUnavailable(String),
    #[error("internal error: {0}")]
    InternalError(String),
}

impl ApiError {
    fn code(&self) -> ApiErrorCode {
        match self {
            Self::InvalidUrl(_) => ApiErrorCode::InvalidUrl,
            Self::SsrfBlocked(_) => ApiErrorCode::SsrfBlocked,
            Self::TaskNotFound(_) => ApiErrorCode::TaskNotFound,
            Self::InvalidStateTransition(_) => ApiErrorCode::InvalidStateTransition,
            Self::InvalidRequest(_) => ApiErrorCode::InvalidRequest,
            Self::ServiceUnavailable(_) => ApiErrorCode::ServiceUnavailable,
            Self::InternalError(_) => ApiErrorCode::InternalError,
        }
    }
}

#[derive(Serialize)]
struct ErrorBody {
    error: ErrorBodyInner,
}

#[derive(Serialize)]
struct ErrorBodyInner {
    code: &'static str,
    message: String,
}

impl From<rusqlite::Error> for ApiError {
    fn from(e: rusqlite::Error) -> Self {
        Self::InternalError(format!("db: {e}"))
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let code = self.code();
        let status = code.status();
        let body = ErrorBody {
            error: ErrorBodyInner {
                code: code.as_str(),
                message: self.to_string(),
            },
        };
        (status, Json(body)).into_response()
    }
}
