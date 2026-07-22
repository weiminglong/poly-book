//! Checkpoint hydration: restore book state from checkpoints + WAL on startup.
//!
//! Flow:
//! 1. Load the latest checkpoint per active asset from Parquet.
//! 2. Apply each checkpoint to the `LiveReadModel` via `hydrate_checkpoint()`.
//! 3. If WAL is available, seek to the checkpoint's `wal_offset` and replay
//!    all subsequent records through the projector.
//! 4. Mark the read model as hydrated (ready to serve).
//!
//! Fallback: if no checkpoints exist, tail WAL from earliest offset. A
//! configured but missing/unreadable WAL keeps the standalone serve runtime
//! unready instead of presenting an empty state as healthy.

use std::collections::HashMap;

use pb_types::AssetId;
use tracing::{info, warn};

use crate::live_state::LiveReadModel;

/// Result of the hydration process.
#[derive(Debug)]
pub struct HydrationResult {
    /// Number of assets hydrated from checkpoints.
    pub checkpoints_loaded: usize,
    /// Number of WAL records replayed.
    pub wal_records_replayed: usize,
    /// The WAL offset we resumed from (if any).
    pub wal_resume_offset: Option<u64>,
    /// The exact WAL position after hydration replay finishes.
    pub wal_end_position: Option<pb_wal::WalPosition>,
    /// Whether every configured recovery source completed without error.
    pub recovery_succeeded: bool,
}

/// Hydrate the read model from the latest checkpoint per asset, then replay
/// any WAL records written after the checkpoint.
///
/// If `wal_config` is `None`, the caller explicitly requested feed-only mode.
/// A configured but missing WAL is a recovery failure and remains unready.
///
/// If no reader is provided or no checkpoints exist, skips checkpoint loading.
pub async fn hydrate<R: pb_replay::EventReader>(
    model: &LiveReadModel,
    reader: Option<&R>,
    wal_config: Option<&pb_wal::WalConfig>,
    active_assets: &[String],
) -> HydrationResult {
    let mut result = HydrationResult {
        checkpoints_loaded: 0,
        wal_records_replayed: 0,
        wal_resume_offset: None,
        wal_end_position: None,
        recovery_succeeded: true,
    };

    if wal_config.is_some() {
        model.mark_unhydrated().await;
    }

    // Phase 1: Load latest checkpoint per asset.
    let mut checkpoint_offsets = HashMap::<String, u64>::new();

    if let Some(reader) = reader {
        let now_us = now_us();
        let asset_ids = active_assets
            .iter()
            .map(|asset_id| AssetId::new(asset_id.as_str()))
            .collect::<Vec<_>>();
        match reader.read_latest_checkpoints(&asset_ids, now_us).await {
            Ok(checkpoints) => {
                for (asset_id_str, checkpoint) in active_assets.iter().zip(checkpoints) {
                    if let Some(checkpoint) = checkpoint {
                        if wal_config.is_some() && checkpoint.wal_offset.is_none() {
                            warn!(
                                asset_id = %asset_id_str,
                                checkpoint_ts = checkpoint.checkpoint_timestamp_us,
                                "checkpoint has no WAL cut; ignoring it and replaying retained WAL"
                            );
                            continue;
                        }
                        if let Some(offset) = checkpoint.wal_offset {
                            checkpoint_offsets.insert(asset_id_str.clone(), offset);
                        }
                        info!(
                            asset_id = %asset_id_str,
                            checkpoint_ts = checkpoint.checkpoint_timestamp_us,
                            wal_offset = ?checkpoint.wal_offset,
                            "hydrating from checkpoint"
                        );
                        model.hydrate_checkpoint(checkpoint).await;
                        result.checkpoints_loaded += 1;
                    } else {
                        info!(
                            asset_id = %asset_id_str,
                            "no checkpoint found, starting empty"
                        );
                    }
                }
            }
            Err(e) => {
                result.recovery_succeeded = false;
                warn!(
                    error = %e,
                    assets = active_assets.len(),
                    "failed to inventory checkpoints, starting from retained WAL"
                );
            }
        }
    }

    // A global fast-forward is safe only when every active asset has a durable
    // checkpoint cut. Otherwise start at the earliest retained WAL record so an
    // asset without a checkpoint is not silently left empty. Per-asset cutoffs
    // below still prevent older records from being applied on top of newer
    // checkpoints whose offsets differ.
    let wal_resume_offset = (!active_assets.is_empty()
        && active_assets
            .iter()
            .all(|asset_id| checkpoint_offsets.contains_key(asset_id)))
    .then(|| checkpoint_offsets.values().copied().min())
    .flatten();

    // Phase 2: Replay WAL tail from the minimum checkpoint offset, using the
    // operator-configured WalConfig so the global-offset math matches the
    // writer's `global_offset()` (a hardcoded default segment size mis-skipped
    // records under a non-default `wal.segment_size_mb`).
    if let Some(cfg) = wal_config {
        if cfg.base_path.exists() {
            let (wal_records_replayed, wal_end_position, replay_succeeded) =
                replay_wal_tail(model, cfg, wal_resume_offset, &checkpoint_offsets).await;
            result.wal_records_replayed = wal_records_replayed;
            result.wal_resume_offset = wal_resume_offset;
            result.wal_end_position = wal_end_position;
            result.recovery_succeeded &= replay_succeeded;
        } else {
            result.recovery_succeeded = false;
            warn!(path = %cfg.base_path.display(), "WAL directory not found; serve remains unready");
        }
    }

    // Phase 3: only trustworthy recovery is ready to serve.
    if result.recovery_succeeded {
        model.mark_hydrated().await;
    }

    info!(
        checkpoints = result.checkpoints_loaded,
        wal_records = result.wal_records_replayed,
        recovery_succeeded = result.recovery_succeeded,
        "hydration complete"
    );

    result
}

/// Replay WAL records from the given global offset through the projector.
///
/// If `from_global_offset` is `None`, replays from the earliest available
/// WAL segment.
async fn replay_wal_tail(
    model: &LiveReadModel,
    config: &pb_wal::WalConfig,
    from_global_offset: Option<u64>,
    checkpoint_offsets: &HashMap<String, u64>,
) -> (usize, Option<pb_wal::WalPosition>, bool) {
    let config = config.clone();

    let mut reader = match pb_wal::WalReader::open(config.clone(), "serve-hydration") {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "failed to open WAL reader for hydration");
            return (0, None, false);
        }
    };

    // If we have a global offset from the checkpoint, we need to seek past it.
    // The WAL reader starts from the earliest position by default; we skip
    // records until we pass the checkpoint's offset.
    let skip_to = from_global_offset.unwrap_or(0);
    let mut records_replayed = 0;
    let mut records_skipped = 0;
    let mut replay_succeeded = true;

    loop {
        match reader.next() {
            Ok(Some(payload)) => {
                // Compute current global offset for skip comparison.
                let (seg_id, seg_offset) = reader.position();
                let current_global = seg_id * config.segment_size + seg_offset as u64;

                // Skip records that are before the checkpoint offset.
                if current_global <= skip_to && from_global_offset.is_some() {
                    records_skipped += 1;
                    continue;
                }

                // Decode the versioned WAL payload.
                match pb_wal::codec::decode(&payload) {
                    Ok(record) => {
                        if checkpoint_offsets
                            .get(record.asset_partition())
                            .is_some_and(|offset| current_global <= *offset)
                        {
                            records_skipped += 1;
                            continue;
                        }
                        model.apply_record(record).await;
                        records_replayed += 1;
                    }
                    Err(e) => {
                        replay_succeeded = false;
                        warn!(error = %e, "failed to decode WAL record during hydration; serve remains unready");
                        break;
                    }
                }
            }
            Ok(None) => break, // Caught up to head.
            Err(e) => {
                replay_succeeded = false;
                warn!(error = %e, "WAL read error during hydration, stopping replay");
                break;
            }
        }
    }

    if records_skipped > 0 {
        info!(
            skipped = records_skipped,
            "skipped WAL records before checkpoint offset"
        );
    }

    (
        records_replayed,
        Some(reader.current_position()),
        replay_succeeded,
    )
}

fn now_us() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use pb_types::event::{
        BookEvent, BookEventKind, DataSource, EventProvenance, PersistedRecord, Side,
    };
    use pb_types::{FixedPrice, FixedSize, Sequence};

    fn snapshot_record(side: Side, price: u32, size: f64, seq: u64) -> PersistedRecord {
        PersistedRecord::Book(BookEvent {
            asset_id: AssetId::new("tok1"),
            kind: BookEventKind::Snapshot,
            side,
            price: FixedPrice::new(price).unwrap(),
            size: FixedSize::from_f64(size).unwrap(),
            provenance: EventProvenance {
                recv_timestamp_us: 1_700_000_000_000_000,
                exchange_timestamp_us: 1_700_000_000_000_000,
                source: DataSource::WebSocket,
                source_event_id: Some("snap-1".to_string()),
                source_session_id: Some("ws-session-1".to_string()),
                sequence: Some(Sequence::new(seq)),
                ingest_ordinal: None,
            },
        })
    }

    fn snapshot_record_for_asset(asset_id: &str, price: u32, seq: u64) -> PersistedRecord {
        let mut record = snapshot_record(Side::Bid, price, 10.0, seq);
        if let PersistedRecord::Book(event) = &mut record {
            event.asset_id = AssetId::new(asset_id);
        }
        record
    }

    fn delta_record() -> PersistedRecord {
        PersistedRecord::Book(BookEvent {
            asset_id: AssetId::new("tok1"),
            kind: BookEventKind::Delta,
            side: Side::Bid,
            price: FixedPrice::new(4900).unwrap(),
            size: FixedSize::from_f64(5.0).unwrap(),
            provenance: EventProvenance {
                recv_timestamp_us: 1_700_000_000_100_000,
                exchange_timestamp_us: 1_700_000_000_100_000,
                source: DataSource::WebSocket,
                source_event_id: Some("delta-1".to_string()),
                source_session_id: Some("ws-session-1".to_string()),
                sequence: Some(Sequence::new(2)),
                ingest_ordinal: None,
            },
        })
    }

    #[tokio::test]
    async fn replay_wal_tail_skips_using_configured_segment_size() {
        // A non-default (tiny) segment size forces rotation. The skip math must
        // use the configured segment size, not the 64 MB default — otherwise the
        // global-offset comparison is wrong and records are mis-skipped or
        // double-applied.
        let dir = tempfile::tempdir().unwrap();
        let config = pb_wal::WalConfig {
            base_path: dir.path().to_path_buf(),
            segment_size: 256,
            ..pb_wal::WalConfig::default()
        };
        let mut writer = pb_wal::WalWriter::open(config.clone()).unwrap();
        for i in 0..3u32 {
            writer
                .append(
                    &pb_wal::codec::encode(&snapshot_record(Side::Bid, 5000 + i, 10.0, i as u64))
                        .unwrap(),
                )
                .unwrap();
        }
        // Resume point after the first three records.
        let cutoff = writer.global_offset();
        for i in 3..7u32 {
            writer
                .append(
                    &pb_wal::codec::encode(&snapshot_record(Side::Bid, 5000 + i, 10.0, i as u64))
                        .unwrap(),
                )
                .unwrap();
        }
        writer.flush().unwrap();

        let live = LiveReadModel::new(crate::dto::FeedMode::FixedTokens);
        live.set_active_assets(vec!["tok1".to_string()]).await;

        let (replayed, _pos, succeeded) =
            replay_wal_tail(&live, &config, Some(cutoff), &HashMap::new()).await;
        assert!(succeeded);
        // Only the four records appended after the cutoff are replayed. With the
        // old hardcoded 64 MB segment size, nothing would be skipped and all
        // seven would replay — this asserts the configured size is used.
        assert_eq!(replayed, 4);
    }

    #[tokio::test]
    async fn replay_wal_tail_skips_record_at_exact_checkpoint_offset() {
        // The skip predicate is `current_global <= skip_to`, where current_global
        // is the offset *past* the record just read. A record whose end offset
        // equals the checkpoint offset is the last record already captured in the
        // checkpoint and must be skipped, not double-applied (off-by-one boundary).
        let dir = tempfile::tempdir().unwrap();
        let config = pb_wal::WalConfig {
            base_path: dir.path().to_path_buf(),
            segment_size: 4096,
            ..pb_wal::WalConfig::default()
        };
        let mut writer = pb_wal::WalWriter::open(config.clone()).unwrap();
        // One record fully covered by the checkpoint; cutoff == its exact end.
        writer
            .append(&pb_wal::codec::encode(&snapshot_record(Side::Bid, 5000, 10.0, 0)).unwrap())
            .unwrap();
        let cutoff = writer.global_offset();
        // One record strictly after the checkpoint.
        writer
            .append(&pb_wal::codec::encode(&snapshot_record(Side::Bid, 5001, 10.0, 1)).unwrap())
            .unwrap();
        writer.flush().unwrap();

        let live = LiveReadModel::new(crate::dto::FeedMode::FixedTokens);
        live.set_active_assets(vec!["tok1".to_string()]).await;

        let (replayed, _pos, succeeded) =
            replay_wal_tail(&live, &config, Some(cutoff), &HashMap::new()).await;
        assert!(succeeded);
        assert_eq!(
            replayed, 1,
            "record at the exact checkpoint offset must be skipped, only the later record replayed"
        );
    }

    #[tokio::test]
    async fn replay_wal_tail_honors_each_assets_checkpoint_offset() {
        let dir = tempfile::tempdir().unwrap();
        let config = pb_wal::WalConfig {
            base_path: dir.path().to_path_buf(),
            segment_size: 4096,
            ..pb_wal::WalConfig::default()
        };
        let mut writer = pb_wal::WalWriter::open(config.clone()).unwrap();
        writer
            .append(&pb_wal::codec::encode(&snapshot_record_for_asset("tok1", 5000, 0)).unwrap())
            .unwrap();
        let tok1_cutoff = writer.global_offset();
        writer
            .append(&pb_wal::codec::encode(&snapshot_record_for_asset("tok1", 5001, 1)).unwrap())
            .unwrap();
        writer
            .append(&pb_wal::codec::encode(&snapshot_record_for_asset("tok2", 6000, 0)).unwrap())
            .unwrap();
        let tok2_cutoff = writer.global_offset();
        writer
            .append(&pb_wal::codec::encode(&snapshot_record_for_asset("tok2", 6001, 1)).unwrap())
            .unwrap();
        writer.flush().unwrap();

        let live = LiveReadModel::new(crate::dto::FeedMode::FixedTokens);
        live.set_active_assets(vec!["tok1".to_string(), "tok2".to_string()])
            .await;
        let checkpoint_offsets = HashMap::from([
            ("tok1".to_string(), tok1_cutoff),
            ("tok2".to_string(), tok2_cutoff),
        ]);

        let (replayed, _position, succeeded) =
            replay_wal_tail(&live, &config, Some(tok1_cutoff), &checkpoint_offsets).await;
        assert!(succeeded);
        assert_eq!(
            replayed, 2,
            "tok2 records already represented by its newer checkpoint must not be applied over it"
        );
    }

    #[tokio::test]
    async fn hydration_returns_end_position_for_live_handoff() {
        let dir = tempfile::tempdir().unwrap();
        let config = pb_wal::WalConfig {
            base_path: dir.path().to_path_buf(),
            ..pb_wal::WalConfig::default()
        };
        let mut writer = pb_wal::WalWriter::open(config.clone()).unwrap();
        for record in [
            snapshot_record(Side::Bid, 5000, 10.0, 0),
            snapshot_record(Side::Ask, 6000, 20.0, 1),
        ] {
            writer
                .append(&pb_wal::codec::encode(&record).unwrap())
                .unwrap();
        }
        writer.flush().unwrap();

        let live = LiveReadModel::new(crate::dto::FeedMode::FixedTokens);
        live.set_active_assets(vec!["tok1".to_string()]).await;

        let result =
            hydrate::<pb_replay::ParquetReader>(&live, None, Some(&config), &["tok1".to_string()])
                .await;

        assert_eq!(result.wal_records_replayed, 2);
        let position = result
            .wal_end_position
            .expect("hydration should return an end position");

        let mut reader =
            pb_wal::WalReader::open_at(config.clone(), "handoff-test", position).unwrap();
        assert!(reader.next().unwrap().is_none());

        let delta = delta_record();
        writer
            .append(&pb_wal::codec::encode(&delta).unwrap())
            .unwrap();
        writer.flush().unwrap();

        let payload = reader
            .next()
            .unwrap()
            .expect("expected only the new post-hydration record");
        let decoded = pb_wal::codec::decode(&payload).unwrap();
        assert_eq!(decoded, delta);
        assert!(reader.next().unwrap().is_none());
    }

    #[tokio::test]
    async fn configured_missing_wal_keeps_model_unready() {
        let dir = tempfile::tempdir().unwrap();
        let config = pb_wal::WalConfig {
            base_path: dir.path().join("missing-wal"),
            ..pb_wal::WalConfig::default()
        };
        let live = LiveReadModel::new(crate::dto::FeedMode::FixedTokens);
        live.set_active_assets(vec!["tok1".to_string()]).await;

        let result =
            hydrate::<pb_replay::ParquetReader>(&live, None, Some(&config), &["tok1".to_string()])
                .await;

        assert!(!result.recovery_succeeded);
        assert!(!live.is_hydrated());
    }
}
