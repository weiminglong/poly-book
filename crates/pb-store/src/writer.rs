use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use chrono::{Datelike, Timelike};
use futures_util::StreamExt;
use object_store::path::Path as ObjectPath;
use object_store::ObjectStore;
use object_store::ObjectStoreExt;
use object_store::PutPayload;
use parquet::arrow::ArrowWriter;
use parquet::basic::{Compression, Encoding, ZstdLevel};
use parquet::file::properties::WriterProperties;

use pb_types::event::PersistedRecord;
use pb_types::storage::{
    PARQUET_RECOVERY_MANIFEST_PREFIX, PARQUET_RECOVERY_MANIFEST_VERSION,
    PARQUET_RECOVERY_OBJECT_PREFIX,
};
use pb_types::ParquetRecoveryManifest;

use crate::error::StoreError;
use crate::schema::{records_to_record_batch, schema_for_record};

const ROW_GROUP_SIZE: usize = 65_536;
const HOUR_US: u64 = 3_600_000_000;

#[derive(Clone)]
pub struct ParquetRecordWriter {
    store: Arc<dyn ObjectStore>,
    base_path: String,
}

/// Inclusive observed WAL timestamp span used to prove complete hourly coverage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecoveryCoverage {
    start_us: u64,
    end_us: u64,
}

impl RecoveryCoverage {
    pub fn new(start_us: u64, end_us: u64) -> Result<Self, StoreError> {
        if start_us >= end_us {
            return Err(StoreError::UnsafeRecovery(format!(
                "WAL coverage must increase, got {start_us}..{end_us}"
            )));
        }
        if start_us < MIN_PLAUSIBLE_PARTITION_US || end_us > MAX_PLAUSIBLE_PARTITION_US {
            return Err(StoreError::UnsafeRecovery(format!(
                "WAL coverage {start_us}..{end_us} is outside the plausible timestamp range"
            )));
        }
        Ok(Self { start_us, end_us })
    }

    pub const fn start_us(self) -> u64 {
        self.start_us
    }

    pub const fn end_us(self) -> u64 {
        self.end_us
    }

    /// True only when the observed WAL spans the entire UTC hour containing
    /// `timestamp_us`. Boundary hours are intentionally excluded.
    pub fn contains_complete_hour(self, timestamp_us: u64) -> bool {
        let hour_start = timestamp_us / HOUR_US * HOUR_US;
        // A snapshot is persisted as many records with the same receive
        // timestamp, and WAL retention may cut inside that equal-timestamp
        // run. Seeing an earliest record exactly at the hour boundary therefore
        // does not prove that all records at that boundary were retained.
        self.start_us < hour_start
            && hour_start
                .checked_add(HOUR_US)
                .is_some_and(|hour_end| self.end_us >= hour_end)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RecoveryReport {
    pub partitions_published: usize,
    pub records_published: usize,
    pub cleanup_failures: usize,
}

/// Lower bound for a plausible event timestamp (~2001-09-09 in µs). Anything
/// below this is treated as corrupt/unstamped rather than a real 1970s event.
const MIN_PLAUSIBLE_PARTITION_US: u64 = 1_000_000_000_000_000;
/// Upper bound for a plausible event timestamp (~2286 in µs). Guards against an
/// absurd far-future value (e.g. a u64 that overflows i64) misfiling records.
const MAX_PLAUSIBLE_PARTITION_US: u64 = 10_000_000_000_000_000;

/// Build the `YYYY/MM/DD/HH` partition key for a record timestamp.
///
/// Timestamps outside a wide plausible band — or not representable as a datetime
/// — are routed to a dedicated `invalid_timestamp` partition with a warning,
/// instead of being silently misfiled into the 1970-01-01 partition by
/// `unwrap_or_default()`. This keeps corrupt/unstamped
/// records visible and quarantined rather than corrupting a real date partition.
pub(crate) fn partition_hour_key(partition_ts_us: u64) -> String {
    if !(MIN_PLAUSIBLE_PARTITION_US..=MAX_PLAUSIBLE_PARTITION_US).contains(&partition_ts_us) {
        tracing::warn!(
            partition_ts_us,
            "record timestamp outside plausible range; routing to invalid_timestamp partition"
        );
        return "invalid_timestamp".to_string();
    }
    match chrono::DateTime::from_timestamp_micros(partition_ts_us as i64) {
        Some(dt) => format!(
            "{:04}/{:02}/{:02}/{:02}",
            dt.date_naive().year(),
            dt.date_naive().month(),
            dt.date_naive().day(),
            dt.time().hour(),
        ),
        None => {
            tracing::warn!(
                partition_ts_us,
                "record timestamp not representable; routing to invalid_timestamp partition"
            );
            "invalid_timestamp".to_string()
        }
    }
}

impl ParquetRecordWriter {
    pub fn new(store: Arc<dyn ObjectStore>, base_path: impl Into<String>) -> Self {
        Self {
            store,
            base_path: base_path.into(),
        }
    }

    fn child_path(&self, suffix: &str) -> ObjectPath {
        let path = if self.base_path.is_empty() {
            suffix.to_string()
        } else {
            format!("{}/{suffix}", self.base_path)
        };
        // Runtime constructors provide either a canonical local path or the
        // already-encoded prefix returned by `parse_url_opts`. Parsing preserves
        // those encoded components; `Path::from` would encode `%` again.
        ObjectPath::parse(path).expect("validated base plus internal suffix is a valid object path")
    }

    pub async fn write_record(&self, record: PersistedRecord) -> Result<(), StoreError> {
        self.write_batch(std::slice::from_ref(&record)).await
    }

    pub async fn write_batch(&self, records: &[PersistedRecord]) -> Result<(), StoreError> {
        if records.is_empty() {
            return Ok(());
        }

        let flush_start = std::time::Instant::now();
        let mut groups: BTreeMap<(String, String, String), Vec<&PersistedRecord>> = BTreeMap::new();
        for record in records {
            let hour_key = partition_hour_key(record.partition_timestamp_us());
            groups
                .entry((
                    record.dataset_name().to_string(),
                    pb_types::newtype::storage_key_for(record.asset_partition()),
                    hour_key,
                ))
                .or_default()
                .push(record);
        }

        for ((dataset, asset, hour_key), records) in &groups {
            self.write_group(dataset, asset, hour_key, records).await?;
        }

        pb_metrics::record_storage_flush("parquet");
        pb_metrics::record_flush_duration_ms(flush_start.elapsed().as_millis() as f64);
        Ok(())
    }

    /// Crash-consistently replace complete hourly partitions from a strict WAL
    /// replay.
    ///
    /// Every record must be a book, trade, or ingest record partitioned exactly
    /// by receive time, and must belong to an hour fully contained in `coverage`.
    /// Recovery writes a new immutable object first, verifies it, then atomically
    /// publishes a small manifest that makes it authoritative. Old files are
    /// deleted only after publication, so a crash always leaves either the old or
    /// new complete view readable. Boundary hours and independently timestamped
    /// datasets are rejected rather than risking destructive partial replacement.
    pub async fn write_batch_replacing(
        &self,
        records: &[PersistedRecord],
        coverage: RecoveryCoverage,
    ) -> Result<RecoveryReport, StoreError> {
        if records.is_empty() {
            return Ok(RecoveryReport::default());
        }
        let mut groups: BTreeMap<(String, String, String), Vec<&PersistedRecord>> = BTreeMap::new();
        for record in records {
            if !matches!(
                record,
                PersistedRecord::Book(_) | PersistedRecord::Trade(_) | PersistedRecord::Ingest(_)
            ) {
                return Err(StoreError::UnsafeRecovery(format!(
                    "dataset {} is not partitioned by receive time and has no complete-hour WAL coverage proof",
                    record.dataset_name()
                )));
            }
            let timestamp_us = record.partition_timestamp_us();
            if !coverage.contains_complete_hour(timestamp_us) {
                return Err(StoreError::UnsafeRecovery(format!(
                    "record at {timestamp_us} belongs to a boundary hour not fully covered by WAL {}..{}",
                    coverage.start_us, coverage.end_us
                )));
            }
            let hour_key = partition_hour_key(record.partition_timestamp_us());
            if hour_key == "invalid_timestamp" {
                return Err(StoreError::UnsafeRecovery(format!(
                    "record at {timestamp_us} has no recoverable UTC hour"
                )));
            }
            groups
                .entry((
                    record.dataset_name().to_string(),
                    pb_types::newtype::storage_key_for(record.asset_partition()),
                    hour_key,
                ))
                .or_default()
                .push(record);
        }

        let mut report = RecoveryReport::default();
        for ((dataset, asset, hour_key), group_records) in &groups {
            report.cleanup_failures += self
                .replace_group(dataset, asset, hour_key, group_records, coverage)
                .await?;
            report.partitions_published += 1;
            report.records_published += group_records.len();
        }
        Ok(report)
    }

    async fn replace_group(
        &self,
        dataset: &str,
        asset: &str,
        hour_key: &str,
        records: &[&PersistedRecord],
        coverage: RecoveryCoverage,
    ) -> Result<usize, StoreError> {
        let existing_objects = self.group_objects(dataset, asset, hour_key).await?;
        let abandoned_staged_objects = self
            .recovery_group_objects(dataset, asset, hour_key)
            .await?;
        let manifest_path = self.recovery_manifest_path(dataset, asset, hour_key);
        let previous_manifest = self
            .read_recovery_manifest(&manifest_path, dataset, asset, hour_key)
            .await?;

        let (buf, file_name) = self.encode_group(records)?;
        let staged_path = self.child_path(&format!(
            "{PARQUET_RECOVERY_OBJECT_PREFIX}/{dataset}/{hour_key}/{file_name}"
        ));
        // The final active object stays in the normal dataset partition so
        // documented direct Parquet consumers continue to work after cleanup.
        // It is first staged under a hidden prefix: putting it into the normal
        // listing before a manifest exists would expose old+new rows and leave a
        // duplicate orphan if the process crashed in that window.
        let recovered_path = self.child_path(&format!("{dataset}/{hour_key}/{file_name}"));
        let expected_size = buf.len() as u64;
        self.store
            .put(&staged_path, PutPayload::from(buf.clone()))
            .await?;
        let written = self.store.head(&staged_path).await?;
        if written.size != expected_size {
            return Err(StoreError::UnsafeRecovery(format!(
                "staged recovery object {staged_path} has size {}, expected {expected_size}",
                written.size
            )));
        }

        let mut superseded: BTreeSet<String> =
            existing_objects.into_iter().map(String::from).collect();
        superseded.extend(abandoned_staged_objects.into_iter().map(String::from));
        if let Some(previous) = previous_manifest {
            superseded.extend(previous.active_objects);
            // Keep retrying any prior best-effort cleanup, including immutable
            // recovery objects that are not discoverable from the normal
            // partition listing. Dropping this set on the next manifest would
            // leak an interrupted cleanup permanently.
            superseded.extend(previous.superseded_objects);
        }
        // Publication phase 1: switch manifest-aware readers to the hidden
        // staged object. Predeclare the future normal path as superseded so a
        // reader remains on the staged view if it lists during the promotion PUT.
        let mut staged_superseded = superseded.clone();
        staged_superseded.insert(recovered_path.to_string());
        staged_superseded.remove(staged_path.as_ref());
        let staged_manifest = ParquetRecoveryManifest {
            version: PARQUET_RECOVERY_MANIFEST_VERSION,
            dataset: dataset.to_string(),
            asset_key: asset.to_string(),
            hour_key: hour_key.to_string(),
            covered_start_us: coverage.start_us,
            covered_end_us: coverage.end_us,
            active_objects: vec![staged_path.to_string()],
            superseded_objects: staged_superseded.into_iter().collect(),
        };
        self.publish_recovery_manifest(&manifest_path, &staged_manifest, dataset, asset, hour_key)
            .await?;

        // Promotion phase: materialize the same verified bytes in the normal
        // partition while phase 1 keeps that path invisible to application
        // readers, then atomically point the manifest at it.
        self.store
            .put(&recovered_path, PutPayload::from(buf))
            .await?;
        let promoted = self.store.head(&recovered_path).await?;
        if promoted.size != expected_size {
            return Err(StoreError::UnsafeRecovery(format!(
                "promoted recovery object {recovered_path} has size {}, expected {expected_size}",
                promoted.size
            )));
        }

        let mut final_superseded = superseded;
        final_superseded.insert(staged_path.to_string());
        final_superseded.remove(recovered_path.as_ref());
        let final_manifest = ParquetRecoveryManifest {
            version: PARQUET_RECOVERY_MANIFEST_VERSION,
            dataset: dataset.to_string(),
            asset_key: asset.to_string(),
            hour_key: hour_key.to_string(),
            covered_start_us: coverage.start_us,
            covered_end_us: coverage.end_us,
            active_objects: vec![recovered_path.to_string()],
            superseded_objects: final_superseded.iter().cloned().collect(),
        };
        self.publish_recovery_manifest(&manifest_path, &final_manifest, dataset, asset, hour_key)
            .await?;

        let mut cleanup_failures = 0;
        for stale in final_superseded {
            // Manifest paths are already object-store encoded. Re-parsing with
            // `ObjectPath::from` would encode `%` again and delete a different
            // key for assets that required percent encoding.
            let stale_path = ObjectPath::parse(&stale).map_err(|error| {
                StoreError::UnsafeRecovery(format!(
                    "invalid superseded object path {stale}: {error}"
                ))
            })?;
            match self.store.delete(&stale_path).await {
                Ok(()) | Err(object_store::Error::NotFound { .. }) => {}
                Err(error) => {
                    cleanup_failures += 1;
                    tracing::warn!(path = %stale_path, error = %error, "recovery cleanup deferred");
                }
            }
        }

        tracing::info!(
            dataset,
            asset,
            hour_key,
            rows = records.len(),
            manifest = %manifest_path,
            cleanup_failures,
            "published crash-consistent parquet recovery partition"
        );
        Ok(cleanup_failures)
    }

    async fn publish_recovery_manifest(
        &self,
        path: &ObjectPath,
        manifest: &ParquetRecoveryManifest,
        dataset: &str,
        asset: &str,
        hour_key: &str,
    ) -> Result<(), StoreError> {
        manifest.validate().map_err(StoreError::UnsafeRecovery)?;
        self.validate_recovery_manifest_scope(path, manifest, dataset, asset, hour_key)?;
        let manifest_bytes = serde_json::to_vec(manifest)?;
        self.store
            .put(path, PutPayload::from(manifest_bytes))
            .await?;
        let published = self
            .read_recovery_manifest(path, dataset, asset, hour_key)
            .await?
            .ok_or_else(|| {
                StoreError::UnsafeRecovery(format!(
                    "recovery manifest {path} disappeared after publication"
                ))
            })?;
        if published != *manifest {
            return Err(StoreError::UnsafeRecovery(format!(
                "recovery manifest {path} failed read-after-write verification"
            )));
        }
        Ok(())
    }

    fn recovery_manifest_path(&self, dataset: &str, asset: &str, hour_key: &str) -> ObjectPath {
        let manifest_name = format!("{asset}.json");
        self.child_path(&format!(
            "{PARQUET_RECOVERY_MANIFEST_PREFIX}/{dataset}/{hour_key}/{manifest_name}"
        ))
    }

    async fn group_objects(
        &self,
        dataset: &str,
        asset: &str,
        hour_key: &str,
    ) -> Result<Vec<ObjectPath>, StoreError> {
        let dir_path = self.child_path(&format!("{dataset}/{hour_key}"));
        let existing = self.store.list(Some(&dir_path)).collect::<Vec<_>>().await;
        let mut matches = Vec::new();
        for meta in existing {
            let meta = meta?;
            if meta.location.filename().is_some_and(|name| {
                pb_types::newtype::storage_object_file_matches_asset(name, asset)
            }) {
                matches.push(meta.location);
            }
        }
        Ok(matches)
    }

    async fn recovery_group_objects(
        &self,
        dataset: &str,
        asset: &str,
        hour_key: &str,
    ) -> Result<Vec<ObjectPath>, StoreError> {
        let dir_path = self.child_path(&format!(
            "{PARQUET_RECOVERY_OBJECT_PREFIX}/{dataset}/{hour_key}"
        ));
        let existing = self.store.list(Some(&dir_path)).collect::<Vec<_>>().await;
        let mut matches = Vec::new();
        for meta in existing {
            let meta = meta?;
            if meta.location.filename().is_some_and(|name| {
                pb_types::newtype::storage_object_file_matches_asset(name, asset)
            }) {
                matches.push(meta.location);
            }
        }
        Ok(matches)
    }

    async fn read_recovery_manifest(
        &self,
        path: &ObjectPath,
        dataset: &str,
        asset: &str,
        hour_key: &str,
    ) -> Result<Option<ParquetRecoveryManifest>, StoreError> {
        let result = match self.store.get(path).await {
            Ok(result) => result,
            Err(object_store::Error::NotFound { .. }) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let bytes = result.bytes().await?;
        let manifest: ParquetRecoveryManifest = serde_json::from_slice(bytes.as_ref())?;
        manifest.validate().map_err(StoreError::UnsafeRecovery)?;
        self.validate_recovery_manifest_scope(path, &manifest, dataset, asset, hour_key)?;
        Ok(Some(manifest))
    }

    /// Reject a manifest whose identity or object list escapes the partition it
    /// claims to describe. This check runs before any previous active object is
    /// added to the cleanup set, so corrupt object-store metadata cannot widen a
    /// recovery deletion.
    fn validate_recovery_manifest_scope(
        &self,
        path: &ObjectPath,
        manifest: &ParquetRecoveryManifest,
        dataset: &str,
        asset: &str,
        hour_key: &str,
    ) -> Result<(), StoreError> {
        let identity_matches = manifest.dataset == dataset
            && manifest.asset_key == asset
            && manifest.hour_key == hour_key
            && path
                .filename()
                .and_then(|name| name.strip_suffix(".json"))
                .is_some_and(|stem| {
                    pb_types::newtype::storage_object_component_matches_key(stem, asset)
                });
        let recovery_prefix = self
            .child_path(&format!(
                "{PARQUET_RECOVERY_OBJECT_PREFIX}/{dataset}/{hour_key}"
            ))
            .to_string();
        let normal_prefix = self
            .child_path(&format!("{dataset}/{hour_key}"))
            .to_string();
        let matches_asset = |object: &str| {
            object.rsplit('/').next().is_some_and(|name| {
                pb_types::newtype::storage_object_file_matches_asset(name, asset)
            })
        };
        let under_prefix = |object: &str, prefix: &str| {
            object
                .strip_prefix(prefix)
                .is_some_and(|suffix| suffix.starts_with('/') && suffix.ends_with(".parquet"))
        };
        let active_objects_are_scoped = manifest.active_objects.iter().all(|object| {
            (under_prefix(object, &normal_prefix) || under_prefix(object, &recovery_prefix))
                && matches_asset(object)
        });
        let superseded_objects_are_scoped = manifest.superseded_objects.iter().all(|object| {
            (under_prefix(object, &normal_prefix) || under_prefix(object, &recovery_prefix))
                && matches_asset(object)
        });
        if !identity_matches || !active_objects_are_scoped || !superseded_objects_are_scoped {
            return Err(StoreError::UnsafeRecovery(format!(
                "recovery manifest {path} does not match its partition scope"
            )));
        }
        Ok(())
    }

    /// Write one `(dataset, asset, hour)` group as a single content-hashed Parquet
    /// file (shared by the live flush path and reconciliation).
    async fn write_group(
        &self,
        dataset: &str,
        asset: &str,
        hour_key: &str,
        records: &[&PersistedRecord],
    ) -> Result<(), StoreError> {
        let (buf, file_name) = self.encode_group(records)?;
        let object_path = self.child_path(&format!("{dataset}/{hour_key}/{file_name}"));
        self.store.put(&object_path, PutPayload::from(buf)).await?;

        tracing::debug!(
            dataset = %dataset,
            asset = %asset,
            rows = records.len(),
            path = %object_path,
            "flushed parquet file"
        );
        Ok(())
    }

    fn encode_group(&self, records: &[&PersistedRecord]) -> Result<(Vec<u8>, String), StoreError> {
        let first_ts_us = records[0].partition_timestamp_us();
        let asset = pb_types::newtype::storage_key_for(records[0].asset_partition());
        let batch = records_to_record_batch(records)?;
        let schema = Arc::new(schema_for_record(records[0]));
        let props = WriterProperties::builder()
            .set_compression(Compression::ZSTD(
                ZstdLevel::try_new(3).expect("valid zstd level"),
            ))
            .set_max_row_group_row_count(Some(ROW_GROUP_SIZE))
            .set_column_encoding("recv_timestamp_us".into(), Encoding::DELTA_BINARY_PACKED)
            .set_column_encoding(
                "exchange_timestamp_us".into(),
                Encoding::DELTA_BINARY_PACKED,
            )
            .set_column_encoding(
                "checkpoint_timestamp_us".into(),
                Encoding::DELTA_BINARY_PACKED,
            )
            .set_column_encoding("event_timestamp_us".into(), Encoding::DELTA_BINARY_PACKED)
            .set_column_encoding("sequence".into(), Encoding::DELTA_BINARY_PACKED)
            .set_column_encoding("price".into(), Encoding::DELTA_BINARY_PACKED)
            .set_column_encoding("size".into(), Encoding::DELTA_BINARY_PACKED)
            .build();

        let mut buf = Vec::with_capacity(256 * 1024);
        let mut writer = ArrowWriter::try_new(&mut buf, schema, Some(props))?;
        writer.write(&batch)?;
        writer.close()?;

        // Append a content-derived suffix so two batches that land in the same
        // (asset, hour) bucket with the same first-record timestamp (quiet books,
        // checkpoints, execution-append re-runs) do not silently overwrite each
        // other. Identical content hashes to the same name, making a true
        // retry idempotent. The 64-bit DefaultHasher gives a ~2^-64 collision per
        // pair; including the byte length as well means a silent overwrite would
        // additionally require two distinct batches of the *same length* — which is
        // free and preserves idempotency (identical content has identical length).
        let content_hash = {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            buf.hash(&mut hasher);
            hasher.finish()
        };
        let file_name = format!(
            "{}_{}_{:016x}_{}.parquet",
            asset,
            first_ts_us,
            content_hash,
            buf.len()
        );
        Ok((buf, file_name))
    }
}
