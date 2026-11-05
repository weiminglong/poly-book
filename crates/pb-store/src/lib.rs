//! Parquet storage sink and writer for persisted event records. See [README](../README.md).

pub mod error;
pub mod parquet_sink;
pub mod schema;
pub mod writer;

pub use error::StoreError;
pub use parquet_sink::ParquetSink;
pub use schema::{
    book_event_refs_to_record_batch, book_event_schema, checkpoint_refs_to_record_batch,
    checkpoint_schema, ingest_event_refs_to_record_batch, ingest_event_schema,
    records_to_record_batch, replay_validation_refs_to_record_batch, replay_validation_schema,
    schema_for_record, trade_event_refs_to_record_batch, trade_event_schema,
};
pub use writer::{ParquetRecordWriter, RecoveryCoverage, RecoveryReport};

#[cfg(test)]
mod tests;
