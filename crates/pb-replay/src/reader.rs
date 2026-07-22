use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use arrow::array::{Array, AsArray, UInt64Array};
use arrow::datatypes::{UInt32Type, UInt64Type};
use futures_util::stream::{self, StreamExt, TryStreamExt};
use object_store::path::Path as ObjectPath;
use object_store::{ObjectStore, ObjectStoreExt};
use parquet::arrow::async_reader::{ParquetObjectReader, ParquetRecordBatchStreamBuilder};

use pb_types::event::{
    BookCheckpoint, BookEvent, BookEventKind, DataSource, EventProvenance, ExecutionEvent,
    ExecutionEventKind, IngestEvent, IngestEventKind, LatencyTrace, MarketDataWindow, ReplayMode,
    ReplayValidation, Side, TradeEvent,
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

    fn read_execution_events(
        &self,
        order_id: Option<&str>,
        start_us: u64,
        end_us: u64,
    ) -> impl std::future::Future<Output = Result<Vec<ExecutionEvent>, ReplayError>> + Send;
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

/// Hard backstop on rows returned by a single unbounded ClickHouse read
/// (ingest/execution events). Applied via `max_result_rows` +
/// `result_overflow_mode = 'throw'` so a pathological window ERRORS loudly
/// instead of silently truncating or OOM-ing the serve process. Far above any legitimate window's row count.
const MAX_READ_ROWS: u64 = 5_000_000;

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
        "strategy" => Ok(DataSource::Strategy),
        "exchange" => Ok(DataSource::Exchange),
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

/// ClickHouse stores `event_kind`/`side` as `Enum8`, which RowBinary returns as
/// the i8 discriminant. These mirror the writer's `book_kind_to_i8`/`side_to_i8`.
fn book_kind_from_i8(value: i8) -> Result<BookEventKind, ReplayError> {
    match value {
        1 => Ok(BookEventKind::Snapshot),
        2 => Ok(BookEventKind::Delta),
        other => Err(ReplayError::InvalidEventType {
            raw: other.to_string(),
        }),
    }
}

fn side_from_i8(value: i8) -> Result<Side, ReplayError> {
    match value {
        1 => Ok(Side::Bid),
        2 => Ok(Side::Ask),
        other => Err(ReplayError::InvalidSide {
            raw: other.to_string(),
        }),
    }
}

fn opt_side_from_i8(value: Option<i8>) -> Result<Option<Side>, ReplayError> {
    value.map(side_from_i8).transpose()
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

fn parse_execution_kind(value: &str) -> Result<ExecutionEventKind, ReplayError> {
    match value {
        "submit_intent" => Ok(ExecutionEventKind::SubmitIntent),
        "exchange_ack" => Ok(ExecutionEventKind::ExchangeAck),
        "cancel_request" => Ok(ExecutionEventKind::CancelRequest),
        "cancel_ack" => Ok(ExecutionEventKind::CancelAck),
        "reject" => Ok(ExecutionEventKind::Reject),
        "partial_fill" => Ok(ExecutionEventKind::PartialFill),
        "fill" => Ok(ExecutionEventKind::Fill),
        "terminal" => Ok(ExecutionEventKind::Terminal),
        other => Err(ReplayError::Other(format!(
            "invalid execution event kind: {other}"
        ))),
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

fn extract_execution_events(
    batch: &arrow::record_batch::RecordBatch,
    order_id: Option<&str>,
    start_us: u64,
    end_us: u64,
) -> Result<Vec<ExecutionEvent>, ReplayError> {
    let event_ts_col = batch
        .column_by_name("event_timestamp_us")
        .ok_or_else(|| ReplayError::Other("missing event_timestamp_us column".into()))?
        .as_primitive::<UInt64Type>();
    let asset_id_col = batch
        .column_by_name("asset_id")
        .ok_or_else(|| ReplayError::Other("missing asset_id column".into()))?
        .as_string::<i32>();
    let order_id_col = batch
        .column_by_name("order_id")
        .ok_or_else(|| ReplayError::Other("missing order_id column".into()))?
        .as_string::<i32>();
    let client_order_id_col = batch
        .column_by_name("client_order_id")
        .ok_or_else(|| ReplayError::Other("missing client_order_id column".into()))?
        .as_string::<i32>();
    let venue_order_id_col = batch
        .column_by_name("venue_order_id")
        .ok_or_else(|| ReplayError::Other("missing venue_order_id column".into()))?
        .as_string::<i32>();
    let kind_col = batch
        .column_by_name("event_kind")
        .ok_or_else(|| ReplayError::Other("missing event_kind column".into()))?
        .as_string::<i32>();
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
    let status_col = batch
        .column_by_name("status")
        .ok_or_else(|| ReplayError::Other("missing status column".into()))?
        .as_string::<i32>();
    let reason_col = batch
        .column_by_name("reason")
        .ok_or_else(|| ReplayError::Other("missing reason column".into()))?
        .as_string::<i32>();
    let latency_col = batch
        .column_by_name("latency_json")
        .ok_or_else(|| ReplayError::Other("missing latency_json column".into()))?
        .as_string::<i32>();

    let mut rows = Vec::new();
    for i in 0..batch.num_rows() {
        let event_ts = event_ts_col.value(i);
        if event_ts < start_us || event_ts > end_us {
            continue;
        }
        if let Some(filter_order_id) = order_id {
            if order_id_col.value(i) != filter_order_id {
                continue;
            }
        }
        rows.push(ExecutionEvent {
            event_timestamp_us: event_ts,
            asset_id: if asset_id_col.is_null(i) {
                None
            } else {
                Some(AssetId::new(asset_id_col.value(i)))
            },
            order_id: order_id_col.value(i).to_string(),
            client_order_id: if client_order_id_col.is_null(i) {
                None
            } else {
                Some(client_order_id_col.value(i).to_string())
            },
            venue_order_id: if venue_order_id_col.is_null(i) {
                None
            } else {
                Some(venue_order_id_col.value(i).to_string())
            },
            kind: parse_execution_kind(kind_col.value(i))?,
            side: if side_col.is_null(i) {
                None
            } else {
                Some(parse_side_value(side_col.value(i))?)
            },
            price: if price_col.is_null(i) {
                None
            } else {
                Some(FixedPrice::new(price_col.value(i))?)
            },
            size: if size_col.is_null(i) {
                None
            } else {
                Some(FixedSize::new(size_col.value(i)))
            },
            status: if status_col.is_null(i) {
                None
            } else {
                Some(status_col.value(i).to_string())
            },
            reason: if reason_col.is_null(i) {
                None
            } else {
                Some(reason_col.value(i).to_string())
            },
            latency: serde_json::from_str::<LatencyTrace>(latency_col.value(i))?,
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

    async fn read_execution_events(
        &self,
        order_id: Option<&str>,
        start_us: u64,
        end_us: u64,
    ) -> Result<Vec<ExecutionEvent>, ReplayError> {
        let files = self
            .dataset_files("execution_events", None, start_us, end_us)
            .await?;
        let mut events = self
            .read_parquet_files(files, |batch| {
                extract_execution_events(batch, order_id, start_us, end_us)
            })
            .await?;
        // Deterministic tie-break matching the ClickHouse reader's
        // `ORDER BY event_timestamp_us, order_id, event_kind`, so equal-timestamp
        // events are stable and the two backends agree.
        events.sort_by(|a, b| {
            a.event_timestamp_us
                .cmp(&b.event_timestamp_us)
                .then_with(|| a.order_id.cmp(&b.order_id))
                .then_with(|| a.kind.to_string().cmp(&b.kind.to_string()))
        });
        Ok(events)
    }
}

pub struct ClickHouseReader {
    client: clickhouse::Client,
}

/// Server-side aggregate counts for an integrity summary window.
///
/// These are computed with ClickHouse `count()`/`countIf()` so the summary never
/// transfers full book/trade rows over the wire just to count them
/// (see `query-mv-incremental`).
#[derive(Debug, Clone, Copy, Default)]
pub struct IntegrityAggregates {
    pub book_event_count: u64,
    pub validation_count: u64,
    pub validation_match_count: u64,
}

impl ClickHouseReader {
    pub fn new(url: &str, database: &str) -> Self {
        let client = clickhouse::Client::default()
            .with_url(url)
            .with_database(database);
        Self { client }
    }

    /// A client clone that caps the result set at `MAX_READ_ROWS` and throws on
    /// overflow, so an unbounded read errors loudly instead of materializing
    /// millions of rows and OOM-ing the serve process.
    fn bounded_client(&self) -> clickhouse::Client {
        self.client
            .clone()
            .with_setting("max_result_rows", MAX_READ_ROWS.to_string())
            .with_setting("result_overflow_mode", "throw")
    }

    /// Compute integrity-summary counts entirely server-side.
    ///
    /// Issues two small aggregate queries (book-event `count()`, and validation
    /// `count()`/`countIf(matched)`) instead of materializing every book and
    /// trade row in the window only to call `.len()` on the client. The two
    /// queries run concurrently.
    pub async fn read_integrity_aggregates(
        &self,
        asset_id: &AssetId,
        start_us: u64,
        end_us: u64,
    ) -> Result<IntegrityAggregates, ReplayError> {
        let book_query = "SELECT count() AS c FROM book_events WHERE asset_id = ? AND recv_timestamp_us >= ? AND recv_timestamp_us <= ?";
        let validation_query = "SELECT count() AS total, countIf(matched = 1) AS matched FROM replay_validations WHERE asset_id = ? AND persisted_at_us >= ? AND persisted_at_us <= ?";

        let (book, validation) = tokio::try_join!(
            async {
                self.client
                    .query(book_query)
                    .bind(asset_id.as_str())
                    .bind(start_us)
                    .bind(end_us)
                    .fetch_one::<CountRow>()
                    .await
                    .map_err(ReplayError::from)
            },
            async {
                self.client
                    .query(validation_query)
                    .bind(asset_id.as_str())
                    .bind(start_us)
                    .bind(end_us)
                    .fetch_one::<ValidationAggRow>()
                    .await
                    .map_err(ReplayError::from)
            },
        )?;

        Ok(IntegrityAggregates {
            book_event_count: book.c,
            validation_count: validation.total,
            validation_match_count: validation.matched,
        })
    }

    /// Read only the ingest events for a window (reconnects, gaps, stale-snapshot
    /// skips). These are bounded in practice and are needed both for the
    /// `continuity_events` list and the per-kind counts, so unlike book/trade
    /// rows they are materialized rather than counted server-side.
    pub async fn read_ingest_events(
        &self,
        asset_id: &AssetId,
        start_us: u64,
        end_us: u64,
    ) -> Result<Vec<IngestEvent>, ReplayError> {
        let ingest_query = "SELECT recv_timestamp_us, exchange_timestamp_us, asset_id, event_kind, sequence, expected_sequence, observed_sequence, details, source, source_event_id, source_session_id FROM ingest_events WHERE recv_timestamp_us >= ? AND recv_timestamp_us <= ? AND (asset_id = ? OR asset_id IS NULL) ORDER BY recv_timestamp_us, event_kind, sequence";
        let rows: Vec<IngestEventRow> = self
            .bounded_client()
            .query(ingest_query)
            .bind(start_us)
            .bind(end_us)
            .bind(asset_id.as_str())
            .fetch_all()
            .await?;
        let mut events = rows
            .into_iter()
            .map(ingest_row_to_event)
            .collect::<Result<Vec<_>, ReplayError>>()?;
        events.sort_by_key(|event| {
            (
                event.provenance.recv_timestamp_us,
                event.provenance.sequence.unwrap_or_default().raw(),
            )
        });
        Ok(events)
    }
}

fn ingest_row_to_event(row: IngestEventRow) -> Result<IngestEvent, ReplayError> {
    Ok(IngestEvent {
        asset_id: row.asset_id.map(AssetId::new),
        kind: parse_ingest_kind(&row.event_kind)?,
        provenance: EventProvenance {
            recv_timestamp_us: row.recv_timestamp_us,
            exchange_timestamp_us: row.exchange_timestamp_us,
            source: parse_source(&row.source)?,
            source_event_id: row.source_event_id,
            source_session_id: row.source_session_id,
            sequence: row.sequence.map(Sequence::new),
            ingest_ordinal: None,
        },
        expected_sequence: row.expected_sequence,
        observed_sequence: row.observed_sequence,
        details: row.details,
    })
}

#[derive(Debug, clickhouse::Row, serde::Deserialize)]
struct CountRow {
    c: u64,
}

#[derive(Debug, clickhouse::Row, serde::Deserialize)]
struct ValidationAggRow {
    total: u64,
    matched: u64,
}

#[derive(Debug, clickhouse::Row, serde::Deserialize)]
struct BookEventRow {
    recv_timestamp_us: u64,
    exchange_timestamp_us: u64,
    asset_id: String,
    event_kind: i8,
    side: i8,
    price: u32,
    size: u64,
    sequence: u64,
    source: String,
    source_event_id: Option<String>,
    source_session_id: Option<String>,
    ingest_ordinal: Option<u64>,
}

#[derive(Debug, clickhouse::Row, serde::Deserialize)]
struct TradeEventRow {
    recv_timestamp_us: u64,
    exchange_timestamp_us: u64,
    asset_id: String,
    price: u32,
    size: Option<u64>,
    side: Option<i8>,
    trade_id: Option<String>,
    fidelity: String,
    sequence: Option<u64>,
    source: String,
    source_event_id: Option<String>,
    source_session_id: Option<String>,
}

#[derive(Debug, clickhouse::Row, serde::Deserialize)]
struct IngestEventRow {
    recv_timestamp_us: u64,
    exchange_timestamp_us: u64,
    asset_id: Option<String>,
    event_kind: String,
    sequence: Option<u64>,
    expected_sequence: Option<u64>,
    observed_sequence: Option<u64>,
    details: Option<String>,
    source: String,
    source_event_id: Option<String>,
    source_session_id: Option<String>,
}

#[derive(Debug, clickhouse::Row, serde::Deserialize)]
struct CheckpointRow {
    checkpoint_timestamp_us: u64,
    recv_timestamp_us: u64,
    exchange_timestamp_us: u64,
    asset_id: String,
    source: String,
    source_event_id: Option<String>,
    source_session_id: Option<String>,
    bids_json: String,
    asks_json: String,
    wal_offset: Option<u64>,
}

#[derive(Debug, clickhouse::Row, serde::Deserialize)]
struct ReplayValidationRow {
    asset_id: String,
    mode: String,
    replay_timestamp_us: u64,
    reference_timestamp_us: u64,
    matched: u8,
    mismatch_summary: Option<String>,
    persisted_at_us: u64,
}

#[derive(Debug, clickhouse::Row, serde::Deserialize)]
struct ExecutionEventRow {
    event_timestamp_us: u64,
    asset_id: Option<String>,
    order_id: String,
    client_order_id: Option<String>,
    venue_order_id: Option<String>,
    event_kind: String,
    side: Option<i8>,
    price: Option<u32>,
    size: Option<u64>,
    status: Option<String>,
    reason: Option<String>,
    latency_json: String,
}

impl EventReader for ClickHouseReader {
    async fn read_market_data(
        &self,
        asset_id: &AssetId,
        start_us: u64,
        end_us: u64,
    ) -> Result<MarketDataWindow, ReplayError> {
        let book_query = "SELECT recv_timestamp_us, exchange_timestamp_us, asset_id, event_kind, side, price, size, sequence, source, source_event_id, source_session_id, ingest_ordinal FROM book_events WHERE asset_id = ? AND recv_timestamp_us >= ? AND recv_timestamp_us <= ? ORDER BY recv_timestamp_us, sequence";
        let trade_query = "SELECT recv_timestamp_us, exchange_timestamp_us, asset_id, price, size, side, trade_id, fidelity, sequence, source, source_event_id, source_session_id FROM trade_events WHERE asset_id = ? AND recv_timestamp_us >= ? AND recv_timestamp_us <= ? ORDER BY recv_timestamp_us, trade_id";
        let ingest_query = "SELECT recv_timestamp_us, exchange_timestamp_us, asset_id, event_kind, sequence, expected_sequence, observed_sequence, details, source, source_event_id, source_session_id FROM ingest_events WHERE recv_timestamp_us >= ? AND recv_timestamp_us <= ? AND (asset_id = ? OR asset_id IS NULL) ORDER BY recv_timestamp_us, event_kind, sequence";
        let client = self.bounded_client();

        let (book_rows, trade_rows, ingest_rows) = tokio::try_join!(
            async {
                client
                    .clone()
                    .query(book_query)
                    .bind(asset_id.as_str())
                    .bind(start_us)
                    .bind(end_us)
                    .fetch_all::<BookEventRow>()
                    .await
                    .map_err(ReplayError::from)
            },
            async {
                client
                    .clone()
                    .query(trade_query)
                    .bind(asset_id.as_str())
                    .bind(start_us)
                    .bind(end_us)
                    .fetch_all::<TradeEventRow>()
                    .await
                    .map_err(ReplayError::from)
            },
            async {
                client
                    .clone()
                    .query(ingest_query)
                    .bind(start_us)
                    .bind(end_us)
                    .bind(asset_id.as_str())
                    .fetch_all::<IngestEventRow>()
                    .await
                    .map_err(ReplayError::from)
            },
        )?;

        let mut book_events = book_rows
            .into_iter()
            .map(|row| {
                Ok(BookEvent {
                    asset_id: AssetId::new(row.asset_id),
                    kind: book_kind_from_i8(row.event_kind)?,
                    side: side_from_i8(row.side)?,
                    price: FixedPrice::new(row.price)?,
                    size: FixedSize::new(row.size),
                    provenance: EventProvenance {
                        recv_timestamp_us: row.recv_timestamp_us,
                        exchange_timestamp_us: row.exchange_timestamp_us,
                        source: parse_source(&row.source)?,
                        source_event_id: row.source_event_id,
                        source_session_id: row.source_session_id,
                        sequence: Some(Sequence::new(row.sequence)),
                        ingest_ordinal: row.ingest_ordinal,
                    },
                })
            })
            .collect::<Result<Vec<_>, ReplayError>>()?;
        book_events.sort_by_key(|event| {
            (
                event.provenance.recv_timestamp_us,
                event.provenance.sequence.unwrap_or_default().raw(),
            )
        });

        let mut trade_events = trade_rows
            .into_iter()
            .map(|row| {
                Ok(TradeEvent {
                    asset_id: AssetId::new(row.asset_id),
                    price: FixedPrice::new(row.price)?,
                    size: row.size.map(FixedSize::new),
                    side: opt_side_from_i8(row.side)?,
                    trade_id: row.trade_id,
                    fidelity: parse_trade_fidelity(&row.fidelity)?,
                    provenance: EventProvenance {
                        recv_timestamp_us: row.recv_timestamp_us,
                        exchange_timestamp_us: row.exchange_timestamp_us,
                        source: parse_source(&row.source)?,
                        source_event_id: row.source_event_id,
                        source_session_id: row.source_session_id,
                        sequence: row.sequence.map(Sequence::new),
                        ingest_ordinal: None,
                    },
                })
            })
            .collect::<Result<Vec<_>, ReplayError>>()?;
        trade_events.sort_by_key(|event| {
            (
                event.provenance.recv_timestamp_us,
                event.provenance.sequence.unwrap_or_default().raw(),
            )
        });

        let mut ingest_events = ingest_rows
            .into_iter()
            .map(ingest_row_to_event)
            .collect::<Result<Vec<_>, ReplayError>>()?;
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
        let query = "SELECT checkpoint_timestamp_us, recv_timestamp_us, exchange_timestamp_us, asset_id, source, source_event_id, source_session_id, bids_json, asks_json, wal_offset FROM book_checkpoints WHERE asset_id = ? AND checkpoint_timestamp_us >= ? AND checkpoint_timestamp_us <= ? ORDER BY checkpoint_timestamp_us, wal_offset";
        let rows: Vec<CheckpointRow> = self
            .client
            .query(query)
            .bind(asset_id.as_str())
            .bind(start_us)
            .bind(end_us)
            .fetch_all()
            .await?;
        rows.into_iter()
            .map(|row| {
                Ok(BookCheckpoint {
                    asset_id: AssetId::new(row.asset_id),
                    checkpoint_timestamp_us: row.checkpoint_timestamp_us,
                    provenance: EventProvenance {
                        recv_timestamp_us: row.recv_timestamp_us,
                        exchange_timestamp_us: row.exchange_timestamp_us,
                        source: parse_source(&row.source)?,
                        source_event_id: row.source_event_id,
                        source_session_id: row.source_session_id,
                        sequence: None,
                        ingest_ordinal: None,
                    },
                    bids: serde_json::from_str(&row.bids_json)?,
                    asks: serde_json::from_str(&row.asks_json)?,
                    wal_offset: row.wal_offset,
                })
            })
            .collect()
    }

    async fn read_latest_checkpoint(
        &self,
        asset_id: &AssetId,
        at_us: u64,
    ) -> Result<Option<BookCheckpoint>, ReplayError> {
        let query = "SELECT checkpoint_timestamp_us, recv_timestamp_us, exchange_timestamp_us, asset_id, source, source_event_id, source_session_id, bids_json, asks_json, wal_offset FROM book_checkpoints WHERE asset_id = ? AND checkpoint_timestamp_us <= ? ORDER BY checkpoint_timestamp_us DESC, wal_offset DESC LIMIT 1";
        let row: Option<CheckpointRow> = self
            .client
            .query(query)
            .bind(asset_id.as_str())
            .bind(at_us)
            .fetch_optional()
            .await?;
        row.map(|row| {
            Ok(BookCheckpoint {
                asset_id: AssetId::new(row.asset_id),
                checkpoint_timestamp_us: row.checkpoint_timestamp_us,
                provenance: EventProvenance {
                    recv_timestamp_us: row.recv_timestamp_us,
                    exchange_timestamp_us: row.exchange_timestamp_us,
                    source: parse_source(&row.source)?,
                    source_event_id: row.source_event_id,
                    source_session_id: row.source_session_id,
                    sequence: None,
                    ingest_ordinal: None,
                },
                bids: serde_json::from_str(&row.bids_json)?,
                asks: serde_json::from_str(&row.asks_json)?,
                wal_offset: row.wal_offset,
            })
        })
        .transpose()
    }

    async fn read_validations(
        &self,
        asset_id: &AssetId,
        start_us: u64,
        end_us: u64,
    ) -> Result<Vec<ReplayValidation>, ReplayError> {
        let query = "SELECT asset_id, mode, replay_timestamp_us, reference_timestamp_us, matched, mismatch_summary, persisted_at_us FROM replay_validations WHERE asset_id = ? AND persisted_at_us >= ? AND persisted_at_us <= ? ORDER BY persisted_at_us";
        let rows: Vec<ReplayValidationRow> = self
            .client
            .query(query)
            .bind(asset_id.as_str())
            .bind(start_us)
            .bind(end_us)
            .fetch_all()
            .await?;
        rows.into_iter()
            .map(|row| {
                Ok(ReplayValidation {
                    asset_id: AssetId::new(row.asset_id),
                    mode: parse_replay_mode(&row.mode)?,
                    replay_timestamp_us: row.replay_timestamp_us,
                    reference_timestamp_us: row.reference_timestamp_us,
                    matched: row.matched > 0,
                    mismatch_summary: row.mismatch_summary,
                    persisted_at_us: row.persisted_at_us,
                })
            })
            .collect()
    }

    async fn read_execution_events(
        &self,
        order_id: Option<&str>,
        start_us: u64,
        end_us: u64,
    ) -> Result<Vec<ExecutionEvent>, ReplayError> {
        let base_query = "SELECT event_timestamp_us, asset_id, order_id, client_order_id, venue_order_id, event_kind, side, price, size, status, reason, latency_json FROM execution_events WHERE event_timestamp_us >= ? AND event_timestamp_us <= ?";
        // Deterministic tie-break (event_timestamp_us, order_id, event_kind) so
        // equal-timestamp events are stable and match the Parquet reader.
        let query = if order_id.is_some() {
            format!(
                "{base_query} AND order_id = ? ORDER BY event_timestamp_us, order_id, event_kind"
            )
        } else {
            format!("{base_query} ORDER BY event_timestamp_us, order_id, event_kind")
        };

        let mut request = self
            .bounded_client()
            .query(&query)
            .bind(start_us)
            .bind(end_us);
        if let Some(order_id) = order_id {
            request = request.bind(order_id);
        }
        let rows: Vec<ExecutionEventRow> = request.fetch_all().await?;
        rows.into_iter()
            .map(|row| {
                Ok(ExecutionEvent {
                    event_timestamp_us: row.event_timestamp_us,
                    asset_id: row.asset_id.map(AssetId::new),
                    order_id: row.order_id,
                    client_order_id: row.client_order_id,
                    venue_order_id: row.venue_order_id,
                    kind: parse_execution_kind(&row.event_kind)?,
                    side: opt_side_from_i8(row.side)?,
                    price: row.price.map(FixedPrice::new).transpose()?,
                    size: row.size.map(FixedSize::new),
                    status: row.status,
                    reason: row.reason,
                    latency: serde_json::from_str(&row.latency_json)?,
                })
            })
            .collect()
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
