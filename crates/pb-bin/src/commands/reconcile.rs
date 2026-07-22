use anyhow::Result;
use config::Config;

use super::pipeline;

const HOUR_US: u64 = 3_600_000_000;
// Recovery runs in the same 512 MiB class used by the default ECS task. A
// compressed WAL hour can expand substantially when decoded plus Arrow encoded,
// so reject oversized hours before publishing any partition.
const MAX_RECOVERY_HOUR_ENCODED_BYTES: u64 = 128 * 1024 * 1024;
const MAX_RECOVERY_HOUR_RECORDS: usize = 2_000_000;

#[derive(Default)]
struct HourSummary {
    records: usize,
    encoded_bytes: u64,
}

fn is_receive_time_partitioned(record: &pb_types::PersistedRecord) -> bool {
    matches!(
        record,
        pb_types::PersistedRecord::Book(_)
            | pb_types::PersistedRecord::Trade(_)
            | pb_types::PersistedRecord::Ingest(_)
    )
}

/// Rebuild complete Parquet hour partitions from the durable WAL.
///
/// The Parquet sink buffers up to a flush interval (default 5 minutes) in memory;
/// a crash, OOM, or SIGKILL loses that window from storage. The WAL captures the
/// same records durably (fsynced), but nothing replayed it back into Parquet — so
/// the loss became permanent. This command reads the retained WAL strictly and
/// only publishes hours fully spanned by that retained stream.
///
/// Run this **offline** after a crash or to backfill a storage gap, with every
/// process that can write this Parquet prefix stopped. The command can enforce
/// that the WAL ingest writer is offline by acquiring its lease; standalone
/// backfill/append processes must be stopped operationally. Boundary hours, WAL
/// gaps, CRC errors, and decode failures fail closed. Each recovered object is
/// verified before a manifest atomically publishes it, so re-running is
/// idempotent and a crash never exposes a delete-before-write window.
pub async fn run(settings: Config) -> Result<()> {
    let wal_config = pipeline::wal_config_from_settings(&settings);
    let _maintenance_guard = pb_wal::WalMaintenanceGuard::acquire(&wal_config)
        .map_err(|e| anyhow::anyhow!("reconcile requires an offline WAL: {e}"))?;
    let parquet_base = settings
        .get_string("storage.parquet_base_path")
        .unwrap_or_else(|_| "./data".to_string());
    let (store, base_path) = pipeline::build_object_store(&parquet_base)?;
    let writer = pb_store::ParquetRecordWriter::new(store, base_path);

    // Ignore any stale maintenance cursor and inspect the entire retained WAL.
    // The strict reader refuses corruption and internal segment gaps instead of
    // silently making an incomplete stream authoritative.
    let mut reader =
        pb_wal::WalReader::open_from_start(wal_config.clone(), "reconcile-validate")
            .map_err(|e| anyhow::anyhow!("failed to open WAL reader for reconcile: {e}"))?;

    // Pass 1 validates every retained frame and derives coverage without keeping
    // the decoded WAL in memory. Retention can exceed the ECS task's memory, so
    // materializing the whole stream would make recovery itself an OOM risk.
    let mut wal_records = 0usize;
    let mut supported_hours = std::collections::BTreeMap::<u64, HourSummary>::new();
    let mut unsupported_records_skipped = 0usize;
    let mut coverage_start_us = None;
    let mut coverage_end_us = None;
    loop {
        match reader.next_strict() {
            Ok(Some(payload)) => {
                wal_records += 1;
                let encoded_bytes = payload.len() as u64;
                let record = pb_wal::codec::decode(&payload).map_err(|e| {
                    anyhow::anyhow!("refusing recovery after undecodable WAL record: {e}")
                })?;
                if !is_receive_time_partitioned(&record) {
                    unsupported_records_skipped += 1;
                    continue;
                }
                let timestamp_us = record
                    .recv_timestamp_us()
                    .expect("receive-time-partitioned records carry provenance");
                if coverage_end_us.is_some_and(|previous| timestamp_us < previous) {
                    anyhow::bail!(
                        "unsafe recovery disabled: receive timestamps move backwards at {timestamp_us}; no Parquet data changed"
                    );
                }
                coverage_start_us.get_or_insert(timestamp_us);
                coverage_end_us = Some(timestamp_us);
                let summary = supported_hours
                    .entry(timestamp_us / HOUR_US * HOUR_US)
                    .or_default();
                summary.records = summary.records.saturating_add(1);
                summary.encoded_bytes = summary.encoded_bytes.saturating_add(encoded_bytes);
            }
            Ok(None) => break,
            Err(e) => {
                return Err(anyhow::anyhow!("WAL read error during reconcile: {e}"));
            }
        }
    }

    if wal_records == 0 {
        tracing::info!("WAL is empty; nothing to reconcile");
        return Ok(());
    }

    // Only datasets partitioned exactly by local receive time can use the WAL
    // endpoints as a full-hour proof. Checkpoints are partitioned by exchange
    // snapshot time, while validation/execution timestamps are independently
    // supplied; replacing those partitions from this coverage would recreate the
    // original partial-history bug under a different name.
    let coverage_start_us = coverage_start_us.ok_or_else(|| {
        anyhow::anyhow!("WAL has no receive-time-partitioned records to prove hourly coverage")
    })?;
    let coverage_end_us = coverage_end_us.expect("coverage start and end are set together");
    let coverage = pb_store::RecoveryCoverage::new(coverage_start_us, coverage_end_us)
        .map_err(|e| anyhow::anyhow!("WAL does not prove a recoverable time span: {e}"))?;

    let recoverable_records: usize = supported_hours
        .iter()
        .filter(|(hour_start, _)| coverage.contains_complete_hour(**hour_start))
        .map(|(_, summary)| summary.records)
        .sum();
    let boundary_records_skipped = supported_hours
        .values()
        .map(|summary| summary.records)
        .sum::<usize>()
        - recoverable_records;
    if recoverable_records == 0 {
        anyhow::bail!(
            "unsafe recovery disabled: retained WAL {}..{} does not fully cover any receive-time-partitioned UTC hour; no Parquet data changed",
            coverage.start_us(),
            coverage.end_us()
        );
    }
    if let Some((hour_start, summary)) = supported_hours.iter().find(|(hour_start, summary)| {
        coverage.contains_complete_hour(**hour_start)
            && (summary.records > MAX_RECOVERY_HOUR_RECORDS
                || summary.encoded_bytes > MAX_RECOVERY_HOUR_ENCODED_BYTES)
    }) {
        anyhow::bail!(
            "unsafe recovery disabled: UTC hour starting {hour_start} contains {} records / {} encoded bytes, above the recovery memory ceiling of {} records / {} bytes; no Parquet data changed",
            summary.records,
            summary.encoded_bytes,
            MAX_RECOVERY_HOUR_RECORDS,
            MAX_RECOVERY_HOUR_ENCODED_BYTES
        );
    }

    // Pass 2 retains at most one eligible UTC hour at a time. Supported receive
    // timestamps were proven nondecreasing above and the WAL is static under the
    // maintenance lease, so crossing an hour boundary makes the prior hour
    // complete for publication without buffering the entire retained WAL.
    let mut reader = pb_wal::WalReader::open_from_start(wal_config, "reconcile-publish")
        .map_err(|e| anyhow::anyhow!("failed to reopen WAL for reconcile: {e}"))?;
    let mut hour_records = Vec::new();
    let mut current_hour = None;
    let mut report = pb_store::RecoveryReport::default();
    loop {
        let payload = match reader.next_strict() {
            Ok(Some(payload)) => payload,
            Ok(None) => break,
            Err(e) => return Err(anyhow::anyhow!("WAL read error during reconcile: {e}")),
        };
        let record = pb_wal::codec::decode(&payload)
            .map_err(|e| anyhow::anyhow!("WAL changed after validation: {e}"))?;
        if !is_receive_time_partitioned(&record)
            || !coverage.contains_complete_hour(record.partition_timestamp_us())
        {
            continue;
        }
        let hour = record.partition_timestamp_us() / HOUR_US * HOUR_US;
        if current_hour.is_some_and(|current| current != hour) {
            let hour_report = writer
                .write_batch_replacing(&hour_records, coverage)
                .await
                .map_err(|e| anyhow::anyhow!("reconcile write failed: {e}"))?;
            report.partitions_published += hour_report.partitions_published;
            report.records_published += hour_report.records_published;
            report.cleanup_failures += hour_report.cleanup_failures;
            hour_records.clear();
        }
        current_hour = Some(hour);
        hour_records.push(record);
    }
    if !hour_records.is_empty() {
        let hour_report = writer
            .write_batch_replacing(&hour_records, coverage)
            .await
            .map_err(|e| anyhow::anyhow!("reconcile write failed: {e}"))?;
        report.partitions_published += hour_report.partitions_published;
        report.records_published += hour_report.records_published;
        report.cleanup_failures += hour_report.cleanup_failures;
    }

    tracing::info!(
        records = report.records_published,
        partitions = report.partitions_published,
        boundary_records_skipped,
        unsupported_records_skipped,
        cleanup_failures = report.cleanup_failures,
        coverage_start_us,
        coverage_end_us,
        "reconcile complete: crash-consistent Parquet manifests published from WAL"
    );
    if report.cleanup_failures > 0 {
        anyhow::bail!(
            "reconcile published manifest-aware views, but {} superseded object(s) could not be deleted; direct Parquet scans are unsafe until a rerun completes cleanup",
            report.cleanup_failures
        );
    }
    Ok(())
}
