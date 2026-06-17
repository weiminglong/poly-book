use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use thiserror::Error;

use crate::dto::ApiErrorResponse;

#[derive(Debug, Error)]
pub enum ApiError {
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    ServiceUnavailable(String),
    #[error("{0}")]
    Internal(String),
}

impl From<pb_service::ServiceError> for ApiError {
    fn from(err: pb_service::ServiceError) -> Self {
        match err {
            pb_service::ServiceError::NotFound(msg) => Self::NotFound(msg),
            pb_service::ServiceError::InvalidParams(msg) => Self::BadRequest(msg),
            pb_service::ServiceError::Unavailable(msg) => Self::ServiceUnavailable(msg),
            pb_service::ServiceError::Internal(msg) => Self::Internal(msg),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        // Client-facing message. For 4xx the message is a validation hint and is
        // safe to return; for 5xx the internal detail (ClickHouse URLs, storage
        // errors, etc.) must NOT leak to unauthenticated clients, so it is
        // logged server-side and replaced with an opaque message (A.95).
        let (status, client_message) = match &self {
            Self::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg.clone()),
            Self::NotFound(msg) => (StatusCode::NOT_FOUND, msg.clone()),
            Self::ServiceUnavailable(_) => (
                StatusCode::SERVICE_UNAVAILABLE,
                "service temporarily unavailable".to_string(),
            ),
            Self::Internal(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal server error".to_string(),
            ),
        };

        if status.is_server_error() {
            // Log the real detail so operators can diagnose; clients never see it.
            tracing::error!(status = %status.as_u16(), detail = %self, "API request failed");
        }

        let body = Json(ApiErrorResponse {
            error: client_message,
        });
        (status, body).into_response()
    }
}
