# Storage Pipeline -- Tasks

- [x] Scaffold `pb-store` crate with `Cargo.toml` and module declarations
- [x] Define `StoreError` enum with `thiserror` derives and `From` impls
- [x] Define `orderbook_schema()` returning canonical Arrow schema
- [x] Implement `events_to_record_batch` with column array construction
- [x] Implement `ParquetSink` struct with `object_store` backend and configurable flush interval
- [x] Implement `ParquetSink::flush` with asset grouping, time-partitioned paths, and Zstd compression
- [x] Implement `ParquetSink::run` loop with `tokio::select!` on channel recv and interval tick
- [x] Define ClickHouse `CREATE TABLE` DDL with `ReplacingMergeTree`, date partitioning, and 90-day TTL
- [x] Implement `ClickHouseRow` struct with `From<&OrderbookEvent>` conversion
- [x] Implement `ClickHouseSink::ensure_table` for DDL execution
- [x] Implement `ClickHouseSink::flush` with batch insert
- [x] Implement `ClickHouseSink::run` loop with batch-size and interval flush triggers
