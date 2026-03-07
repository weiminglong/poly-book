use thiserror::Error;

#[derive(Debug, Error)]
pub enum ReplayError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Parquet error: {0}")]
    Parquet(#[from] parquet::errors::ParquetError),

    #[error("Arrow error: {0}")]
    Arrow(#[from] arrow::error::ArrowError),

    #[error("ClickHouse error: {0}")]
    ClickHouse(#[from] clickhouse::error::Error),

    #[error("book reconstruction error: {0}")]
    BookError(#[from] pb_book::BookError),

    #[error("type conversion error: {0}")]
    TypesError(#[from] pb_types::TypesError),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("replay {asset_id}: no snapshot found before {timestamp_us}us — lookback window may be too narrow")]
    NoSnapshotFound { asset_id: String, timestamp_us: u64 },

    #[error("replay: stored event has invalid type '{raw}' (expected book/trade/ingest)")]
    InvalidEventType { raw: String },

    #[error("replay: stored event has invalid side '{raw}' (expected Bid/Ask)")]
    InvalidSide { raw: String },

    #[error("replay: HTTP fetch failed — {url}: {reason}")]
    Http { url: String, reason: String },

    #[error("{0}")]
    Other(String),
}
