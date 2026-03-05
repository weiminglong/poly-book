use thiserror::Error;

#[derive(Debug, Error)]
pub enum TypesError {
    #[error("invalid price: {0} (must be 0..=10000)")]
    InvalidPrice(u32),

    #[error("failed to parse price from string: {0}")]
    PriceParse(String),

    #[error("failed to parse size from string: {0}")]
    SizeParse(String),

    #[error("invalid side: {0}")]
    InvalidSide(String),

    #[error("deserialization error: {0}")]
    Deserialize(#[from] serde_json::Error),
}
