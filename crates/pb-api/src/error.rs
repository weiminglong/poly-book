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
        let status = match self {
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::ServiceUnavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        let body = Json(ApiErrorResponse {
            error: self.to_string(),
        });
        (status, body).into_response()
    }
}
