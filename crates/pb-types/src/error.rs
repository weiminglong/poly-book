use thiserror::Error;

#[derive(Debug, Error)]
pub enum TypesError {
    #[error("invalid price: raw={raw} exceeds max 10000")]
    InvalidPrice { raw: u32 },

    #[error("invalid price: {value} is not a finite non-negative number")]
    InvalidPriceValue { value: String },

    #[error("price parse failed: '{input}' is not a valid decimal in [0.0, 1.0]")]
    PriceParse { input: String },

    #[error("size parse failed: '{input}' is not a valid non-negative decimal")]
    SizeParse { input: String },

    #[error("invalid side: '{raw}' (expected Bid, Ask, BUY, or SELL)")]
    InvalidSide { raw: String },

    #[error("deserialization error: {0}")]
    Deserialize(#[from] serde_json::Error),
}
