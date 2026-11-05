use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use arrow::array::{Array, AsArray, UInt64Array};
use arrow::datatypes::{UInt32Type, UInt64Type};
use futures_util::stream::{self, StreamExt, TryStreamExt};
use object_store::path::Path as ObjectPath;
use object_store::{ObjectStore, ObjectStoreExt};
// parquet 59 deprecates ParquetObjectReader in favour of implementing
// AsyncFileReader by hand; it still works and is the smallest object_store
// bridge, so keep it until the replacement is needed for a real reason.
#[allow(deprecated)]
use parquet::arrow::async_reader::ParquetObjectReader;
use parquet::arrow::async_reader::ParquetRecordBatchStreamBuilder;

use pb_types::event::{
    BookCheckpoint, BookEvent, BookEventKind, DataSource, EventProvenance, IngestEvent,
    IngestEventKind, MarketDataWindow, ReplayMode, ReplayValidation, Side, TradeEvent,
};
use pb_types::{storage::PARQUET_RECOVERY_MANIFEST_PREFIX, ParquetRecoveryManifest};
use pb_types::{AssetId, FixedPrice, FixedSize, PriceLevel, Sequence, TradeFidelity};

use crate::error::ReplayError;

pub trait EventReader: Send + Sync {
    fn read_market_data(
        &self,
        asset_id: &AssetId,
        start_us: u64,
        end_us: u64,
    ) -> impl std::future::Future<Output = Result<MarketDataWindow, ReplayError>> + Send;

    fn read_checkpoints(
        &self,
        asset_id: &AssetId,
        start_us: u64,
        end_us: u64,
    ) -> impl std::future::Future<Output = Result<Vec<BookCheckpoint>, ReplayError>> + Send;

    fn read_latest_checkpoint(
        &self,
        asset_id: &AssetId,
        at_us: u64,
    ) -> impl std::future::Future<Output = Result<Option<BookCheckpoint>, ReplayError>> + Send;

    /// Read the latest checkpoint for many assets. Backends may override this
    /// to share one inventory/query; the default preserves compatibility for
    /// readers where individual lookups are already efficient.
    fn read_latest_checkpoints(
        &self,
        asset_ids: &[AssetId],
        at_us: u64,
    ) -> impl std::future::Future<Output = Result<Vec<Option<BookCheckpoint>>, ReplayError>> + Send
    {
        async move {
            let mut checkpoints = Vec::with_capacity(asset_ids.len());
            for asset_id in asset_ids {
                checkpoints.push(self.read_latest_checkpoint(asset_id, at_us).await?);
            }
            Ok(checkpoints)
        }
    }

    fn read_validations(
        &self,
        asset_id: &AssetId,
        start_us: u64,
        end_us: u64,
    ) -> impl std::future::Future<Output = Result<Vec<ReplayValidation>, ReplayError>> + Send;
}

#[derive(Clone)]
pub struct ParquetReader {
    store: Arc<dyn ObjectStore>,
    base_path: ObjectPath,
}

/// The on-disk Parquet schema version this reader understands. Files written by
/// `pb_store` carry it in their schema metadata (`pb_store::schema::PB_SCHEMA_VERSION`).
const EXPECTED_PARQUET_SCHEMA_VERSION: &str = "2";

/// Validate a Parquet file's `pb_schema_version`. A pre-split (unversioned) or
/// future-versioned file is a typed error rather than a silent empty/mis-mapped
/// read.
fn check_schema_version(version: Option<&str>) -> Result<(), ReplayError> {
    match version {
        Some(v) if v == EXPECTED_PARQUET_SCHEMA_VERSION => Ok(()),
        Some(other) => Err(ReplayError::Other(format!(
            "schema version {other}, expected {EXPECTED_PARQUET_SCHEMA_VERSION}; migrate it"
        ))),
        None => Err(ReplayError::Other(
            "no pb_schema_version (pre-split/legacy layout); migrate, do not read as empty".into(),
        )),
    }
}

const PARQUET_READ_CONCURRENCY: usize = 8;
const MAX_PARQUET_DATASET_ROWS: usize = 500_000;

impl ParquetReader {
    /// Build a reader for a local filesystem path.
    ///
    /// Cloud URLs must be constructed by the runtime and passed to
    /// [`Self::from_store`], so reads and writes share identical credentials and
    /// endpoint configuration.
    pub fn new(base_path: impl AsRef<std::path::Path>) -> Self {
        let path = base_path.as_ref();
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .unwrap_or_else(|_| std::path::PathBuf::from("/"))
                .join(path)
        };
        Self {
            store: Arc::new(object_store::local::LocalFileSystem::new()),
            base_path: ObjectPath::from(absolute.to_string_lossy().as_ref()),
        }
    }

    /// Build a Parquet reader over an arbitrary object-store backend.
    pub fn from_store(store: Arc<dyn ObjectStore>, base_path: impl Into<String>) -> Self {
        let base_path = base_path.into();
        Self {
            store,
            // `parse_url_opts` returns an already percent-encoded object-store
            // prefix. `Path::from` would encode `%` again, and every child path
            // would then point at a different key.
            base_path: ObjectPath::parse(base_path.as_str())
                .expect("object-store prefixes must be valid object paths"),
        }
    }

    fn child_path(&self, suffix: &str) -> ObjectPath {
        let path = if self.base_path.as_ref().is_empty() {
            suffix.to_string()
        } else {
            format!("{}/{suffix}", self.base_path)
        };
        ObjectPath::parse(path).expect("validated base plus internal suffix is a valid object path")
    }

    pub(crate) fn hour_paths(&self, dataset: &str, start_us: u64, end_us: u64) -> Vec<ObjectPath> {
        use chrono::{Datelike, TimeZone, Timelike, Utc};

        let start_dt = Utc
            .timestamp_opt(start_us as i64 / 1_000_000, 0)
            .single()
            .unwrap_or_default();
        let end_dt = Utc
            .timestamp_opt(end_us as i64 / 1_000_000, 0)
            .single()
            .unwrap_or_default();

        let mut paths = Vec::new();
        let mut current = start_dt
            .date_naive()
            .and_hms_opt(start_dt.hour(), 0, 0)
            .unwrap();
        let end_naive = end_dt.naive_utc();

        while current <= end_naive {
            let hour_key = format!(
                "{:04}/{:02}/{:02}/{:02}",
                current.year(),
                current.month(),
                current.day(),
                current.hour(),
            );
            paths.push(self.child_path(&format!("{dataset}/{hour_key}")));
            current += chrono::Duration::hours(1);
        }
        paths
    }

    async fn dataset_files(
        &self,
        dataset: &str,
        asset_prefix: Option<&str>,
        start_us: u64,
        end_us: u64,
    ) -> Result<Vec<ObjectPath>, ReplayError> {
        let mut files = Vec::new();
        for dir in self.hour_paths(dataset, start_us, end_us) {
            let entries = self.store.list(Some(&dir)).collect::<Vec<_>>().await;
            for entry in entries {
                let meta = entry?;
                let path = meta.location;
                if !path
                    .filename()
                    .is_some_and(|name| name.ends_with(".parquet"))
                {
                    continue;
                }
                if let Some(prefix) = asset_prefix {
                    let Some(name) = path.filename() else {
                        continue;
                    };
                    let expected = pb_types::newtype::storage_key_for(prefix);
                    if !pb_types::newtype::storage_object_file_matches_asset(name, &expected) {
                        continue;
                    }
                }
                files.push(path);
            }

            let manifests = self.recovery_manifests(dataset, &dir, asset_prefix).await?;
            if !manifests.is_empty() {
                let superseded: HashSet<&str> = manifests
                    .iter()
                    .flat_map(|manifest| manifest.superseded_objects.iter().map(String::as_str))
                    .collect();
                files.retain(|path| !superseded.contains(path.as_ref()));
                for manifest in manifests {
                    for object in manifest.active_objects {
                        let path = ObjectPath::parse(&object).map_err(|error| {
                            ReplayError::Other(format!(
                                "invalid active object path {object} in recovery manifest: {error}"
                            ))
                        })?;
                        files.push(path);
                    }
                }
            }
        }
        files.sort();
        files.dedup();
        Ok(files)
    }

    async fn recovery_manifests(
        &self,
        dataset: &str,
        hour_path: &ObjectPath,
        asset_prefix: Option<&str>,
    ) -> Result<Vec<ParquetRecoveryManifest>, ReplayError> {
        let hour_suffix = hour_path
            .as_ref()
            .strip_prefix(self.base_path.as_ref())
            .unwrap_or(hour_path.as_ref())
            .trim_start_matches('/');
        let hour_key = hour_suffix
            .strip_prefix(dataset)
            .unwrap_or(hour_suffix)
            .trim_start_matches('/');
        let manifest_prefix = self.child_path(&format!(
            "{PARQUET_RECOVERY_MANIFEST_PREFIX}/{dataset}/{hour_key}"
        ));

        let mut manifests = Vec::new();
        if let Some(asset_id) = asset_prefix {
            let asset_key = pb_types::newtype::storage_key_for(asset_id);
            let manifest_name = format!("{asset_key}.json");
            // Both components are already percent-encoded object paths. `join`
            // accepts a raw segment and would encode `%` a second time.
            let path = ObjectPath::parse(format!("{manifest_prefix}/{manifest_name}"))
                .expect("validated manifest prefix and encoded asset form a valid object path");
            match self.store.get(&path).await {
                Ok(result) => {
                    let bytes = result.bytes().await?;
                    let manifest = self.decode_manifest(&path, bytes.as_ref())?;
                    self.validate_manifest_scope(
                        &path,
                        &manifest,
                        dataset,
                        hour_key,
                        Some(&asset_key),
                    )?;
                    manifests.push(manifest);
                }
                Err(object_store::Error::NotFound { .. }) => {}
                Err(error) => return Err(error.into()),
            }
        } else {
            let entries = self
                .store
                .list(Some(&manifest_prefix))
                .collect::<Vec<_>>()
                .await;
            for entry in entries {
                let meta = entry?;
                if !meta
                    .location
                    .filename()
                    .is_some_and(|name| name.ends_with(".json"))
                {
                    continue;
                }
                let bytes = self.store.get(&meta.location).await?.bytes().await?;
                let manifest = self.decode_manifest(&meta.location, bytes.as_ref())?;
                self.validate_manifest_scope(&meta.location, &manifest, dataset, hour_key, None)?;
                manifests.push(manifest);
            }
        }
        Ok(manifests)
    }

    fn decode_manifest(
        &self,
        path: &ObjectPath,
        bytes: &[u8],
    ) -> Result<ParquetRecoveryManifest, ReplayError> {
        let manifest: ParquetRecoveryManifest = serde_json::from_slice(bytes)?;
        manifest.validate().map_err(|error| {
            ReplayError::Other(format!("invalid recovery manifest {path}: {error}"))
        })?;
        Ok(manifest)
    }

    fn validate_manifest_scope(
        &self,
        path: &ObjectPath,
        manifest: &ParquetRecoveryManifest,
        dataset: &str,
        hour_key: &str,
        expected_asset: Option<&str>,
    ) -> Result<(), ReplayError> {
        let identity_matches = manifest.dataset == dataset
            && manifest.hour_key == hour_key
            && expected_asset.is_none_or(|asset| manifest.asset_key == asset)
            && path
                .filename()
                .and_then(|name| name.strip_suffix(".json"))
                .is_some_and(|stem| {
                    pb_types::newtype::storage_object_component_matches_key(
                        stem,
                        &manifest.asset_key,
                    )
                });
        let recovery_prefix = self
            .child_path(&format!(
                "{}/{dataset}/{hour_key}",
                pb_types::storage::PARQUET_RECOVERY_OBJECT_PREFIX
            ))
            .to_string();
        let normal_prefix = self
            .child_path(&format!("{dataset}/{hour_key}"))
            .to_string();
        let matches_asset = |object: &str| {
            object.rsplit('/').next().is_some_and(|name| {
                pb_types::newtype::storage_object_file_matches_asset(name, &manifest.asset_key)
            })
        };
        let active_objects_are_scoped = manifest.active_objects.iter().all(|object| {
            [&normal_prefix, &recovery_prefix].iter().any(|prefix| {
                object
                    .strip_prefix(prefix.as_str())
                    .is_some_and(|suffix| suffix.starts_with('/') && suffix.ends_with(".parquet"))
            }) && matches_asset(object)
        });
        let superseded_objects_are_scoped = manifest.superseded_objects.iter().all(|object| {
            [&normal_prefix, &recovery_prefix].iter().any(|prefix| {
                object
                    .strip_prefix(prefix.as_str())
                    .is_some_and(|suffix| suffix.starts_with('/') && suffix.ends_with(".parquet"))
            }) && matches_asset(object)
        });
        if !identity_matches || !active_objects_are_scoped || !superseded_objects_are_scoped {
            return Err(ReplayError::Other(format!(
                "recovery manifest {path} does not match its partition scope"
            )));
        }
        Ok(())
    }

    async fn read_parquet_file<T, F>(
        &self,
        path: &ObjectPath,
        extractor: F,
    ) -> Result<Vec<T>, ReplayError>
    where
        F: Fn(&arrow::record_batch::RecordBatch) -> Result<Vec<T>, ReplayError>,
    {
        let meta = self.store.head(path).await?;
        #[allow(deprecated)]
        let object_reader =
            ParquetObjectReader::new(self.store.clone(), path.clone()).with_file_size(meta.size);
        let builder = ParquetRecordBatchStreamBuilder::new(object_reader).await?;

        // Reject an incompatible on-disk layout instead of silently yielding
        // empty or mis-mapped rows.
        check_schema_version(
            builder
                .schema()
                .metadata()
                .get("pb_schema_version")
                .map(String::as_str),
        )
        .map_err(|e| ReplayError::Other(format!("Parquet object {path}: {e}")))?;

        let mut stream = builder.build()?;
        let mut rows = Vec::new();

        use futures_util::StreamExt;
        while let Some(batch_result) = stream.next().await {
            let batch = batch_result?;
            let extracted = extractor(&batch)?;
            if rows.len().saturating_add(extracted.len()) > MAX_PARQUET_DATASET_ROWS {
                return Err(ReplayError::Other(format!(
                    "Parquet read exceeds {MAX_PARQUET_DATASET_ROWS} rows per dataset; narrow the requested time window"
                )));
            }
            rows.extend(extracted);
        }

        Ok(rows)
    }

    async fn read_parquet_files<T, F>(
        &self,
        paths: Vec<ObjectPath>,
        extractor: F,
    ) -> Result<Vec<T>, ReplayError>
    where
        T: Send,
        F: Fn(&arrow::record_batch::RecordBatch) -> Result<Vec<T>, ReplayError>
            + Clone
            + Send
            + Sync,
    {
        // Files are read concurrently and may complete out of order. This is safe
        // for determinism because the replay engine sorts the merged events into a
        // total order (engine::sort_book_events) before applying them, so the
        // unordered read order cannot affect reconstruction output.
        stream::iter(paths.into_iter().map(|path| {
            let extractor = extractor.clone();
            async move { self.read_parquet_file(&path, extractor).await }
        }))
        .buffer_unordered(PARQUET_READ_CONCURRENCY)
        .try_fold(Vec::new(), |mut rows, mut batch| async move {
            if rows.len().saturating_add(batch.len()) > MAX_PARQUET_DATASET_ROWS {
                return Err(ReplayError::Other(format!(
                    "Parquet read exceeds {MAX_PARQUET_DATASET_ROWS} rows per dataset; narrow the requested time window"
                )));
            }
            rows.append(&mut batch);
            Ok(rows)
        })
        .await
    }

    /// Resolve and read one manifest-aware dataset view, retrying the complete
    /// resolution once if post-publication garbage collection removed an object
    /// selected immediately before the manifest changed.
    async fn read_dataset_with_view_retry<T, F>(
        &self,
        dataset: &str,
        asset_prefix: Option<&str>,
        start_us: u64,
        end_us: u64,
        extractor: F,
    ) -> Result<Vec<T>, ReplayError>
    where
        T: Send,
        F: Fn(&arrow::record_batch::RecordBatch) -> Result<Vec<T>, ReplayError>
            + Clone
            + Send
            + Sync,
    {
        for attempt in 0..2 {
            let files = self
                .dataset_files(dataset, asset_prefix, start_us, end_us)
                .await?;
            match self.read_parquet_files(files, extractor.clone()).await {
                Err(error) if attempt == 0 && is_object_not_found(&error) => {
                    tracing::warn!(
                        dataset,
                        "Parquet recovery view changed during read; resolving manifest once more"
                    );
                }
                result => return result,
            }
        }
        unreachable!("the second view-resolution attempt always returns")
    }

    /// List the checkpoint dataset once and group matching objects by their
    /// lexicographically sortable `YYYY/MM/DD/HH` partition. This is used by
    /// startup hydration: probing every hour while exponentially widening to
    /// epoch turns an empty S3 checkpoint lookup into millions of sequential
    /// LIST/GET requests.
    async fn checkpoint_files_by_asset_and_hour(
        &self,
        asset_ids: &[AssetId],
        at_us: u64,
    ) -> Result<Vec<BTreeMap<String, Vec<ObjectPath>>>, ReplayError> {
        use chrono::{Datelike, TimeZone, Timelike, Utc};

        let at = Utc
            .timestamp_opt(at_us as i64 / 1_000_000, 0)
            .single()
            .unwrap_or_default();
        let at_hour = format!(
            "{:04}/{:02}/{:02}/{:02}",
            at.year(),
            at.month(),
            at.day(),
            at.hour()
        );
        let dataset_prefix = self.child_path("book_checkpoints");
        let relative_prefix = format!("{dataset_prefix}/");
        let logical_assets = asset_ids
            .iter()
            .map(|asset_id| pb_types::newtype::storage_key_for(asset_id.as_str()))
            .collect::<Vec<_>>();
        let entries = self
            .store
            .list(Some(&dataset_prefix))
            .collect::<Vec<_>>()
            .await;
        let mut by_asset = vec![BTreeMap::<String, Vec<ObjectPath>>::new(); asset_ids.len()];
        for entry in entries {
            let meta = entry?;
            let Some(file_name) = meta.location.filename() else {
                continue;
            };
            let Some(asset_index) = logical_assets.iter().position(|logical_asset| {
                pb_types::newtype::storage_object_file_matches_asset(file_name, logical_asset)
            }) else {
                continue;
            };
            let Some(relative) = meta.location.as_ref().strip_prefix(&relative_prefix) else {
                continue;
            };
            let Some((hour_key, _)) = relative.rsplit_once('/') else {
                continue;
            };
            if hour_key.len() == "YYYY/MM/DD/HH".len() && hour_key <= at_hour.as_str() {
                by_asset[asset_index]
                    .entry(hour_key.to_string())
                    .or_default()
                    .push(meta.location);
            }
        }
        Ok(by_asset)
    }
}

/// Object-store range failures made by `ParquetObjectReader` are wrapped in
/// `ParquetError::External`. Walk the error chain so a manifest transition that
/// deletes an object between HEAD and a later range GET is retried just like a
/// direct object-store `NotFound`.
pub(crate) fn is_object_not_found(error: &ReplayError) -> bool {
    let mut current: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(source) = current {
        if source
            .downcast_ref::<object_store::Error>()
            .is_some_and(|error| matches!(error, object_store::Error::NotFound { .. }))
        {
            return true;
        }
        current = source.source();
    }
    false
}

fn parse_source(value: &str) -> Result<DataSource, ReplayError> {
    match value {
        "websocket" => Ok(DataSource::WebSocket),
        "rest_snapshot" => Ok(DataSource::RestSnapshot),
        "replay_validator" => Ok(DataSource::ReplayValidator),
        "system" => Ok(DataSource::System),
        other => Err(ReplayError::Other(format!("invalid source: {other}"))),
    }
}

fn parse_book_kind(value: u8) -> Result<BookEventKind, ReplayError> {
    match value {
        1 => Ok(BookEventKind::Snapshot),
        2 => Ok(BookEventKind::Delta),
        other => Err(ReplayError::InvalidEventType {
            raw: other.to_string(),
        }),
    }
}

fn parse_side_value(value: u8) -> Result<Side, ReplayError> {
    match value {
        1 => Ok(Side::Bid),
        2 => Ok(Side::Ask),
        other => Err(ReplayError::InvalidSide {
            raw: other.to_string(),
        }),
    }
}

fn parse_trade_fidelity(value: &str) -> Result<TradeFidelity, ReplayError> {
    match value {
        "partial" => Ok(TradeFidelity::Partial),
        "full" => Ok(TradeFidelity::Full),
        other => Err(ReplayError::Other(format!(
            "invalid trade fidelity: {other}"
        ))),
    }
}

fn parse_ingest_kind(value: &str) -> Result<IngestEventKind, ReplayError> {
    match value {
        "reconnect_start" => Ok(IngestEventKind::ReconnectStart),
        "reconnect_success" => Ok(IngestEventKind::ReconnectSuccess),
        "sequence_gap" => Ok(IngestEventKind::SequenceGap),
        "stale_snapshot_skip" => Ok(IngestEventKind::StaleSnapshotSkip),
        "source_reset" => Ok(IngestEventKind::SourceReset),
        "book_mismatch" => Ok(IngestEventKind::BookMismatch),
        other => Err(ReplayError::Other(format!(
            "invalid ingest event kind: {other}"
        ))),
    }
}

fn parse_replay_mode(value: &str) -> Result<ReplayMode, ReplayError> {
    match value {
        "recv_time" => Ok(ReplayMode::RecvTime),
        "exchange_time" => Ok(ReplayMode::ExchangeTime),
        other => Err(ReplayError::Other(format!("invalid replay mode: {other}"))),
    }
}

fn extract_book_events(
    batch: &arrow::record_batch::RecordBatch,
    asset_id: &AssetId,
    start_us: u64,
    end_us: u64,
) -> Result<Vec<BookEvent>, ReplayError> {
    let recv_ts_col = batch
        .column_by_name("recv_timestamp_us")
        .ok_or_else(|| ReplayError::Other("missing recv_timestamp_us column".into()))?
        .as_primitive::<UInt64Type>();
    let exchange_ts_col = batch
        .column_by_name("exchange_timestamp_us")
        .ok_or_else(|| ReplayError::Other("missing exchange_timestamp_us column".into()))?
        .as_primitive::<UInt64Type>();
    let asset_id_col = batch
        .column_by_name("asset_id")
        .ok_or_else(|| ReplayError::Other("missing asset_id column".into()))?
        .as_string::<i32>();
    let kind_col = batch
        .column_by_name("event_kind")
        .ok_or_else(|| ReplayError::Other("missing event_kind column".into()))?
        .as_primitive::<arrow::datatypes::UInt8Type>();
    let side_col = batch
        .column_by_name("side")
        .ok_or_else(|| ReplayError::Other("missing side column".into()))?
        .as_primitive::<arrow::datatypes::UInt8Type>();
    let price_col = batch
        .column_by_name("price")
        .ok_or_else(|| ReplayError::Other("missing price column".into()))?
        .as_primitive::<UInt32Type>();
    let size_col = batch
        .column_by_name("size")
        .ok_or_else(|| ReplayError::Other("missing size column".into()))?
        .as_primitive::<UInt64Type>();
    let sequence_col = batch
        .column_by_name("sequence")
        .ok_or_else(|| ReplayError::Other("missing sequence column".into()))?
        .as_primitive::<UInt64Type>();
    let source_col = batch
        .column_by_name("source")
        .ok_or_else(|| ReplayError::Other("missing source column".into()))?
        .as_string::<i32>();
    let source_event_id_col = batch
        .column_by_name("source_event_id")
        .ok_or_else(|| ReplayError::Other("missing source_event_id column".into()))?
        .as_string::<i32>();
    let source_session_id_col = batch
        .column_by_name("source_session_id")
        .ok_or_else(|| ReplayError::Other("missing source_session_id column".into()))?
        .as_string::<i32>();
    // Optional for backward compatibility: Parquet files written before the column existed do
    // not have this column. When absent, ingest_ordinal stays None and replay
    // falls back to the sequence/content tiebreakers.
    let ingest_ordinal_col = batch
        .column_by_name("ingest_ordinal")
        .map(|c| c.as_primitive::<UInt64Type>());

    let mut rows = Vec::new();
    for i in 0..batch.num_rows() {
        let recv_ts = recv_ts_col.value(i);
        if recv_ts < start_us || recv_ts > end_us {
            continue;
        }
        if asset_id_col.value(i) != asset_id.as_str() {
            continue;
        }
        rows.push(BookEvent {
            asset_id: AssetId::new(asset_id_col.value(i)),
            kind: parse_book_kind(kind_col.value(i))?,
            side: parse_side_value(side_col.value(i))?,
            price: FixedPrice::new(price_col.value(i))?,
            size: FixedSize::new(size_col.value(i)),
            provenance: EventProvenance {
                recv_timestamp_us: recv_ts,
                exchange_timestamp_us: exchange_ts_col.value(i),
                source: parse_source(source_col.value(i))?,
                source_event_id: if source_event_id_col.is_null(i) {
                    None
                } else {
                    Some(source_event_id_col.value(i).to_string())
                },
                source_session_id: if source_session_id_col.is_null(i) {
                    None
                } else {
                    Some(source_session_id_col.value(i).to_string())
                },
                sequence: if sequence_col.is_null(i) {
                    None
                } else {
                    Some(Sequence::new(sequence_col.value(i)))
                },
                ingest_ordinal: ingest_ordinal_col
                    .filter(|c| !c.is_null(i))
                    .map(|c| c.value(i)),
            },
        });
    }
    Ok(rows)
}

fn extract_trade_events(
    batch: &arrow::record_batch::RecordBatch,
    asset_id: &AssetId,
    start_us: u64,
    end_us: u64,
) -> Result<Vec<TradeEvent>, ReplayError> {
    let recv_ts_col = batch
        .column_by_name("recv_timestamp_us")
        .ok_or_else(|| ReplayError::Other("missing recv_timestamp_us column".into()))?
        .as_primitive::<UInt64Type>();
    let exchange_ts_col = batch
        .column_by_name("exchange_timestamp_us")
        .ok_or_else(|| ReplayError::Other("missing exchange_timestamp_us column".into()))?
        .as_primitive::<UInt64Type>();
    let asset_id_col = batch
        .column_by_name("asset_id")
        .ok_or_else(|| ReplayError::Other("missing asset_id column".into()))?
        .as_string::<i32>();
    let price_col = batch
        .column_by_name("price")
        .ok_or_else(|| ReplayError::Other("missing price column".into()))?
        .as_primitive::<UInt32Type>();
    let size_col = batch
        .column_by_name("size")
        .ok_or_else(|| ReplayError::Other("missing size column".into()))?
        .as_primitive::<UInt64Type>();
    let side_col = batch
        .column_by_name("side")
        .ok_or_else(|| ReplayError::Other("missing side column".into()))?
        .as_primitive::<arrow::datatypes::UInt8Type>();
    let trade_id_col = batch
        .column_by_name("trade_id")
        .ok_or_else(|| ReplayError::Other("missing trade_id column".into()))?
        .as_string::<i32>();
    let fidelity_col = batch
        .column_by_name("fidelity")
        .ok_or_else(|| ReplayError::Other("missing fidelity column".into()))?
        .as_string::<i32>();
    let sequence_col = batch
        .column_by_name("sequence")
        .ok_or_else(|| ReplayError::Other("missing sequence column".into()))?
        .as_primitive::<UInt64Type>();
    let source_col = batch
        .column_by_name("source")
        .ok_or_else(|| ReplayError::Other("missing source column".into()))?
        .as_string::<i32>();
    let source_event_id_col = batch
        .column_by_name("source_event_id")
        .ok_or_else(|| ReplayError::Other("missing source_event_id column".into()))?
        .as_string::<i32>();
    let source_session_id_col = batch
        .column_by_name("source_session_id")
        .ok_or_else(|| ReplayError::Other("missing source_session_id column".into()))?
        .as_string::<i32>();

    let mut rows = Vec::new();
    for i in 0..batch.num_rows() {
        let recv_ts = recv_ts_col.value(i);
        if recv_ts < start_us || recv_ts > end_us {
            continue;
        }
        if asset_id_col.value(i) != asset_id.as_str() {
            continue;
        }
        let side = if side_col.is_null(i) {
            None
        } else {
            Some(parse_side_value(side_col.value(i))?)
        };
        rows.push(TradeEvent {
            asset_id: AssetId::new(asset_id_col.value(i)),
            price: FixedPrice::new(price_col.value(i))?,
            size: if size_col.is_null(i) {
                None
            } else {
                Some(FixedSize::new(size_col.value(i)))
            },
            side,
            trade_id: if trade_id_col.is_null(i) {
                None
            } else {
                Some(trade_id_col.value(i).to_string())
            },
            fidelity: parse_trade_fidelity(fidelity_col.value(i))?,
            provenance: EventProvenance {
                recv_timestamp_us: recv_ts,
                exchange_timestamp_us: exchange_ts_col.value(i),
                source: parse_source(source_col.value(i))?,
                source_event_id: if source_event_id_col.is_null(i) {
                    None
                } else {
                    Some(source_event_id_col.value(i).to_string())
                },
                source_session_id: if source_session_id_col.is_null(i) {
                    None
                } else {
                    Some(source_session_id_col.value(i).to_string())
                },
                sequence: if sequence_col.is_null(i) {
                    None
                } else {
                    Some(Sequence::new(sequence_col.value(i)))
                },
                ingest_ordinal: None,
            },
        });
    }
    Ok(rows)
}

fn extract_ingest_events(
    batch: &arrow::record_batch::RecordBatch,
    asset_id: &AssetId,
    start_us: u64,
    end_us: u64,
) -> Result<Vec<IngestEvent>, ReplayError> {
    let recv_ts_col = batch
        .column_by_name("recv_timestamp_us")
        .ok_or_else(|| ReplayError::Other("missing recv_timestamp_us column".into()))?
        .as_primitive::<UInt64Type>();
    let exchange_ts_col = batch
        .column_by_name("exchange_timestamp_us")
        .ok_or_else(|| ReplayError::Other("missing exchange_timestamp_us column".into()))?
        .as_primitive::<UInt64Type>();
    let asset_id_col = batch
        .column_by_name("asset_id")
        .ok_or_else(|| ReplayError::Other("missing asset_id column".into()))?
        .as_string::<i32>();
    let kind_col = batch
        .column_by_name("event_kind")
        .ok_or_else(|| ReplayError::Other("missing event_kind column".into()))?
        .as_string::<i32>();
    let sequence_col = batch
        .column_by_name("sequence")
        .ok_or_else(|| ReplayError::Other("missing sequence column".into()))?
        .as_primitive::<UInt64Type>();
    let expected_col = batch
        .column_by_name("expected_sequence")
        .ok_or_else(|| ReplayError::Other("missing expected_sequence column".into()))?
        .as_primitive::<UInt64Type>();
    let observed_col = batch
        .column_by_name("observed_sequence")
        .ok_or_else(|| ReplayError::Other("missing observed_sequence column".into()))?
        .as_primitive::<UInt64Type>();
    let details_col = batch
        .column_by_name("details")
        .ok_or_else(|| ReplayError::Other("missing details column".into()))?
        .as_string::<i32>();
    let source_col = batch
        .column_by_name("source")
        .ok_or_else(|| ReplayError::Other("missing source column".into()))?
        .as_string::<i32>();
    let source_event_id_col = batch
        .column_by_name("source_event_id")
        .ok_or_else(|| ReplayError::Other("missing source_event_id column".into()))?
        .as_string::<i32>();
    let source_session_id_col = batch
        .column_by_name("source_session_id")
        .ok_or_else(|| ReplayError::Other("missing source_session_id column".into()))?
        .as_string::<i32>();

    let mut rows = Vec::new();
    for i in 0..batch.num_rows() {
        let recv_ts = recv_ts_col.value(i);
        if recv_ts < start_us || recv_ts > end_us {
            continue;
        }
        let row_asset = if asset_id_col.is_null(i) {
            None
        } else {
            Some(AssetId::new(asset_id_col.value(i)))
        };
        if let Some(row_asset_id) = row_asset.as_ref() {
            if row_asset_id.as_str() != asset_id.as_str() {
                continue;
            }
        }
        rows.push(IngestEvent {
            asset_id: row_asset,
            kind: parse_ingest_kind(kind_col.value(i))?,
            provenance: EventProvenance {
                recv_timestamp_us: recv_ts,
                exchange_timestamp_us: exchange_ts_col.value(i),
                source: parse_source(source_col.value(i))?,
                source_event_id: if source_event_id_col.is_null(i) {
                    None
                } else {
                    Some(source_event_id_col.value(i).to_string())
                },
                source_session_id: if source_session_id_col.is_null(i) {
                    None
                } else {
                    Some(source_session_id_col.value(i).to_string())
                },
                sequence: if sequence_col.is_null(i) {
                    None
                } else {
                    Some(Sequence::new(sequence_col.value(i)))
                },
                ingest_ordinal: None,
            },
            expected_sequence: if expected_col.is_null(i) {
                None
            } else {
                Some(expected_col.value(i))
            },
            observed_sequence: if observed_col.is_null(i) {
                None
            } else {
                Some(observed_col.value(i))
            },
            details: if details_col.is_null(i) {
                None
            } else {
                Some(details_col.value(i).to_string())
            },
        });
    }
    Ok(rows)
}

fn extract_checkpoints(
    batch: &arrow::record_batch::RecordBatch,
    asset_id: &AssetId,
    start_us: u64,
    end_us: u64,
) -> Result<Vec<BookCheckpoint>, ReplayError> {
    let checkpoint_ts_col = batch
        .column_by_name("checkpoint_timestamp_us")
        .ok_or_else(|| ReplayError::Other("missing checkpoint_timestamp_us column".into()))?
        .as_primitive::<UInt64Type>();
    let recv_ts_col = batch
        .column_by_name("recv_timestamp_us")
        .ok_or_else(|| ReplayError::Other("missing recv_timestamp_us column".into()))?
        .as_primitive::<UInt64Type>();
    let exchange_ts_col = batch
        .column_by_name("exchange_timestamp_us")
        .ok_or_else(|| ReplayError::Other("missing exchange_timestamp_us column".into()))?
        .as_primitive::<UInt64Type>();
    let asset_id_col = batch
        .column_by_name("asset_id")
        .ok_or_else(|| ReplayError::Other("missing asset_id column".into()))?
        .as_string::<i32>();
    let source_col = batch
        .column_by_name("source")
        .ok_or_else(|| ReplayError::Other("missing source column".into()))?
        .as_string::<i32>();
    let source_event_id_col = batch
        .column_by_name("source_event_id")
        .ok_or_else(|| ReplayError::Other("missing source_event_id column".into()))?
        .as_string::<i32>();
    let source_session_id_col = batch
        .column_by_name("source_session_id")
        .ok_or_else(|| ReplayError::Other("missing source_session_id column".into()))?
        .as_string::<i32>();
    let bids_col = batch
        .column_by_name("bids_json")
        .ok_or_else(|| ReplayError::Other("missing bids_json column".into()))?
        .as_string::<i32>();
    let asks_col = batch
        .column_by_name("asks_json")
        .ok_or_else(|| ReplayError::Other("missing asks_json column".into()))?
        .as_string::<i32>();

    let mut rows = Vec::new();
    for i in 0..batch.num_rows() {
        let checkpoint_ts = checkpoint_ts_col.value(i);
        if checkpoint_ts < start_us || checkpoint_ts > end_us {
            continue;
        }
        if asset_id_col.value(i) != asset_id.as_str() {
            continue;
        }
        rows.push(BookCheckpoint {
            asset_id: AssetId::new(asset_id_col.value(i)),
            checkpoint_timestamp_us: checkpoint_ts,
            provenance: EventProvenance {
                recv_timestamp_us: recv_ts_col.value(i),
                exchange_timestamp_us: exchange_ts_col.value(i),
                source: parse_source(source_col.value(i))?,
                source_event_id: if source_event_id_col.is_null(i) {
                    None
                } else {
                    Some(source_event_id_col.value(i).to_string())
                },
                source_session_id: if source_session_id_col.is_null(i) {
                    None
                } else {
                    Some(source_session_id_col.value(i).to_string())
                },
                sequence: None,
                ingest_ordinal: None,
            },
            bids: serde_json::from_str::<Vec<PriceLevel>>(bids_col.value(i))?,
            asks: serde_json::from_str::<Vec<PriceLevel>>(asks_col.value(i))?,
            wal_offset: batch.column_by_name("wal_offset").and_then(|col| {
                let arr = col.as_any().downcast_ref::<UInt64Array>()?;
                if arr.is_null(i) {
                    None
                } else {
                    Some(arr.value(i))
                }
            }),
        });
    }
    Ok(rows)
}

fn extract_validations(
    batch: &arrow::record_batch::RecordBatch,
    asset_id: &AssetId,
    start_us: u64,
    end_us: u64,
) -> Result<Vec<ReplayValidation>, ReplayError> {
    let asset_id_col = batch
        .column_by_name("asset_id")
        .ok_or_else(|| ReplayError::Other("missing asset_id column".into()))?
        .as_string::<i32>();
    let mode_col = batch
        .column_by_name("mode")
        .ok_or_else(|| ReplayError::Other("missing mode column".into()))?
        .as_string::<i32>();
    let replay_ts_col = batch
        .column_by_name("replay_timestamp_us")
        .ok_or_else(|| ReplayError::Other("missing replay_timestamp_us column".into()))?
        .as_primitive::<UInt64Type>();
    let reference_ts_col = batch
        .column_by_name("reference_timestamp_us")
        .ok_or_else(|| ReplayError::Other("missing reference_timestamp_us column".into()))?
        .as_primitive::<UInt64Type>();
    let matched_col = batch
        .column_by_name("matched")
        .ok_or_else(|| ReplayError::Other("missing matched column".into()))?
        .as_boolean();
    let mismatch_col = batch
        .column_by_name("mismatch_summary")
        .ok_or_else(|| ReplayError::Other("missing mismatch_summary column".into()))?
        .as_string::<i32>();
    let persisted_col = batch
        .column_by_name("persisted_at_us")
        .ok_or_else(|| ReplayError::Other("missing persisted_at_us column".into()))?
        .as_primitive::<UInt64Type>();

    let mut rows = Vec::new();
    for i in 0..batch.num_rows() {
        let persisted_at = persisted_col.value(i);
        if persisted_at < start_us || persisted_at > end_us {
            continue;
        }
        if asset_id_col.value(i) != asset_id.as_str() {
            continue;
        }
        rows.push(ReplayValidation {
            asset_id: AssetId::new(asset_id_col.value(i)),
            mode: parse_replay_mode(mode_col.value(i))?,
            replay_timestamp_us: replay_ts_col.value(i),
            reference_timestamp_us: reference_ts_col.value(i),
            matched: matched_col.value(i),
            mismatch_summary: if mismatch_col.is_null(i) {
                None
            } else {
                Some(mismatch_col.value(i).to_string())
            },
            persisted_at_us: persisted_at,
        });
    }
    Ok(rows)
}

impl EventReader for ParquetReader {
    async fn read_market_data(
        &self,
        asset_id: &AssetId,
        start_us: u64,
        end_us: u64,
    ) -> Result<MarketDataWindow, ReplayError> {
        let asset_id = asset_id.clone();
        let asset_filter = asset_id.as_str().to_string();
        let ingest_asset_id = asset_id.clone();
        let (book_events, trade_events, ingest_events) = tokio::try_join!(
            self.read_dataset_with_view_retry(
                "book_events",
                Some(&asset_filter),
                start_us,
                end_us,
                {
                    let asset_id = asset_id.clone();
                    move |batch| extract_book_events(batch, &asset_id, start_us, end_us)
                },
            ),
            self.read_dataset_with_view_retry(
                "trade_events",
                Some(&asset_filter),
                start_us,
                end_us,
                {
                    let asset_id = asset_id.clone();
                    move |batch| extract_trade_events(batch, &asset_id, start_us, end_us)
                },
            ),
            self.read_dataset_with_view_retry(
                "ingest_events",
                None,
                start_us,
                end_us,
                move |batch| extract_ingest_events(batch, &ingest_asset_id, start_us, end_us),
            ),
        )?;

        let mut book_events = book_events;
        book_events.sort_by_key(|event| {
            (
                event.provenance.recv_timestamp_us,
                event.provenance.sequence.unwrap_or_default().raw(),
            )
        });
        let mut trade_events = trade_events;
        trade_events.sort_by_key(|event| {
            (
                event.provenance.recv_timestamp_us,
                event.provenance.sequence.unwrap_or_default().raw(),
            )
        });
        let mut ingest_events = ingest_events;
        ingest_events.sort_by_key(|event| {
            (
                event.provenance.recv_timestamp_us,
                event.provenance.sequence.unwrap_or_default().raw(),
            )
        });

        Ok(MarketDataWindow {
            book_events,
            trade_events,
            ingest_events,
        })
    }

    async fn read_checkpoints(
        &self,
        asset_id: &AssetId,
        start_us: u64,
        end_us: u64,
    ) -> Result<Vec<BookCheckpoint>, ReplayError> {
        let files = self
            .dataset_files(
                "book_checkpoints",
                Some(asset_id.as_str()),
                start_us,
                end_us,
            )
            .await?;
        let mut checkpoints = self
            .read_parquet_files(files, |batch| {
                extract_checkpoints(batch, asset_id, start_us, end_us)
            })
            .await?;
        checkpoints.sort_by_key(|checkpoint| checkpoint.checkpoint_timestamp_us);
        Ok(checkpoints)
    }

    async fn read_latest_checkpoint(
        &self,
        asset_id: &AssetId,
        at_us: u64,
    ) -> Result<Option<BookCheckpoint>, ReplayError> {
        Ok(self
            .read_latest_checkpoints(std::slice::from_ref(asset_id), at_us)
            .await?
            .pop()
            .flatten())
    }

    async fn read_latest_checkpoints(
        &self,
        asset_ids: &[AssetId],
        at_us: u64,
    ) -> Result<Vec<Option<BookCheckpoint>>, ReplayError> {
        // Checkpoints are intentionally not eligible for manifest recovery
        // because their exchange-time partition cannot be proven complete from
        // receive-time WAL endpoints. Inventory the dataset once for the entire
        // startup asset set instead of recursively listing all objects once per
        // asset.
        let inventories = self
            .checkpoint_files_by_asset_and_hour(asset_ids, at_us)
            .await?;
        let mut latest = Vec::with_capacity(asset_ids.len());
        for (asset_id, by_hour) in asset_ids.iter().zip(inventories) {
            let mut found = None;
            for (_, files) in by_hour.into_iter().rev() {
                let mut checkpoints = self
                    .read_parquet_files(files, |batch| {
                        extract_checkpoints(batch, asset_id, 0, at_us)
                    })
                    .await?;
                checkpoints.sort_by_key(|checkpoint| checkpoint.checkpoint_timestamp_us);
                if let Some(checkpoint) = checkpoints.pop() {
                    found = Some(checkpoint);
                    break;
                }
            }
            latest.push(found);
        }
        Ok(latest)
    }

    async fn read_validations(
        &self,
        asset_id: &AssetId,
        start_us: u64,
        end_us: u64,
    ) -> Result<Vec<ReplayValidation>, ReplayError> {
        let files = self
            .dataset_files(
                "replay_validations",
                Some(asset_id.as_str()),
                start_us,
                end_us,
            )
            .await?;
        let mut validations = self
            .read_parquet_files(files, |batch| {
                extract_validations(batch, asset_id, start_us, end_us)
            })
            .await?;
        validations.sort_by_key(|validation| validation.persisted_at_us);
        Ok(validations)
    }
}

#[cfg(test)]
mod schema_version_tests {
    use super::*;

    #[test]
    fn accepts_current_version() {
        assert!(check_schema_version(Some("2")).is_ok());
    }

    #[test]
    fn rejects_old_version() {
        let err = check_schema_version(Some("1")).unwrap_err();
        assert!(err.to_string().contains("migrate"), "{err}");
    }

    #[test]
    fn rejects_missing_version() {
        let err = check_schema_version(None).unwrap_err();
        assert!(err.to_string().contains("legacy"), "{err}");
    }
}
