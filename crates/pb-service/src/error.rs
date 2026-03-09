//! Domain-level service errors, independent of HTTP transport.

/// Service-level error with domain-specific variants.
#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    /// The requested resource was not found.
    #[error("not found: {0}")]
    NotFound(String),

    /// Invalid parameters were provided.
    #[error("invalid params: {0}")]
    InvalidParams(String),

    /// The service is temporarily unavailable (e.g., during hydration).
    #[error("unavailable: {0}")]
    Unavailable(String),

    /// An internal error occurred.
    #[error("internal: {0}")]
    Internal(String),
}
