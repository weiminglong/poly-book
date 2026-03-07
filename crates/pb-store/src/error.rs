use thiserror::Error;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("Parquet write/read error: {0}")]
    Parquet(#[from] parquet::errors::ParquetError),

    #[error("Arrow schema/conversion error: {0}")]
    Arrow(#[from] arrow::error::ArrowError),

    #[error("ClickHouse query/insert error: {0}")]
    ClickHouse(#[from] clickhouse::error::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("object store error: {0}")]
    ObjectStore(#[from] object_store::Error),

    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("{0}")]
    Other(String),
}
