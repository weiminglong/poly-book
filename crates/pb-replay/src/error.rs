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

    #[error("Book error: {0}")]
    BookError(#[from] pb_book::BookError),

    #[error("Types error: {0}")]
    TypesError(#[from] pb_types::TypesError),

    #[error("no snapshot found for asset {asset_id} before timestamp {timestamp_us}")]
    NoSnapshotFound { asset_id: String, timestamp_us: u64 },

    #[error("invalid event type in stored data: {0}")]
    InvalidEventType(String),

    #[error("invalid side in stored data: {0}")]
    InvalidSide(String),

    #[error("HTTP error: {0}")]
    Http(String),

    #[error("{0}")]
    Other(String),
}
