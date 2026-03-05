use thiserror::Error;

#[derive(Debug, Error)]
pub enum BookError {
    #[error("invalid price for book operation: {0}")]
    InvalidPrice(String),

    #[error("unknown side: {0}")]
    UnknownSide(String),

    #[error("sequence gap detected: expected {expected}, got {got}")]
    SequenceGap { expected: u64, got: u64 },
}
