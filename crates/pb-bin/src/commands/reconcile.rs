use anyhow::Result;
use config::Config;

use super::pipeline;

/// Rebuild Parquet storage partitions from the durable WAL.
///
/// The Parquet sink buffers up to a flush interval (default 5 minutes) in memory;
/// a crash, OOM, or SIGKILL loses that window from storage. The WAL captures the
/// same records durably (fsynced), but nothing replayed it back into Parquet — so
/// the loss became permanent. This command reads the retained WAL and rewrites
/// every `(dataset, asset, hour)` partition it covers, making the WAL the
/// authoritative recovery source.
///
/// Run this **offline** (with ingest stopped) after a crash or to backfill a
/// storage gap: a concurrent live sink writing the same partitions would race the
/// per-group replace. Re-running is idempotent (each touched partition is deleted
/// and rewritten from the WAL), so it is safe to run more than once.
pub async fn run(settings: Config) -> Result<()> {
    let wal_config = pipeline::wal_config_from_settings(&settings);
    let parquet_base = settings
        .get_string("storage.parquet_base_path")
        .unwrap_or_else(|_| "./data".to_string());
    let (store, base_path) = pipeline::build_object_store(&parquet_base)?;
    let writer = pb_store::ParquetRecordWriter::new(store, base_path);

    // A dedicated consumer name so reconcile never disturbs the serve-live
    // tailer's position. We deliberately do NOT commit this consumer's position:
    // reconcile rebuilds the entire retained WAL each run, and the replace
    // semantics make that idempotent.
    let mut reader = pb_wal::WalReader::open(wal_config, "reconcile")
        .map_err(|e| anyhow::anyhow!("failed to open WAL reader for reconcile: {e}"))?;

    if reader.needs_resync() {
        tracing::warn!(
            "WAL reconcile consumer is behind the earliest retained segment; \
             reconciling only what remains in the WAL"
        );
    }

    let mut records: Vec<pb_types::PersistedRecord> = Vec::new();
    let mut decode_failures = 0u64;
    loop {
        match reader.next() {
            Ok(Some(payload)) => match pb_wal::codec::decode(&payload) {
                Ok(record) => records.push(record),
                Err(e) => {
                    decode_failures += 1;
                    tracing::warn!(error = %e, "skipping undecodable WAL record during reconcile");
                }
            },
            Ok(None) => break,
            Err(e) => {
                return Err(anyhow::anyhow!("WAL read error during reconcile: {e}"));
            }
        }
    }

    if records.is_empty() {
        tracing::info!("WAL is empty; nothing to reconcile");
        return Ok(());
    }

    let record_count = records.len();
    writer
        .write_batch_replacing(&records)
        .await
        .map_err(|e| anyhow::anyhow!("reconcile write failed: {e}"))?;

    tracing::info!(
        records = record_count,
        decode_failures,
        "reconcile complete: Parquet partitions rebuilt from WAL"
    );
    Ok(())
}
