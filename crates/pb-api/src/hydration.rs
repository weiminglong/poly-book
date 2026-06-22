//! Checkpoint hydration: restore book state from checkpoints + WAL on startup.
//!
//! Flow:
//! 1. Load the latest checkpoint per active asset from Parquet.
//! 2. Apply each checkpoint to the `LiveReadModel` via `hydrate_checkpoint()`.
//! 3. If WAL is available, seek to the checkpoint's `wal_offset` and replay
//!    all subsequent records through the projector.
//! 4. Mark the read model as hydrated (ready to serve).
//!
//! Fallback: if no checkpoints exist, tail WAL from earliest offset.
//! If no WAL exists, fall back to feed-only mode (mark hydrated immediately).

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
}

/// Hydrate the read model from the latest checkpoint per asset, then replay
/// any WAL records written after the checkpoint.
///
/// If `wal_path` is `None` or the WAL directory doesn't exist, skips WAL
/// replay and marks hydrated immediately.
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
    };

    // Phase 1: Load latest checkpoint per asset.
    let mut min_wal_offset: Option<u64> = None;

    if let Some(reader) = reader {
        let now_us = now_us();
        for asset_id_str in active_assets {
            let asset_id = AssetId::new(asset_id_str.as_str());
            match reader.read_latest_checkpoint(&asset_id, now_us).await {
                Ok(Some(checkpoint)) => {
                    if let Some(offset) = checkpoint.wal_offset {
                        min_wal_offset = Some(match min_wal_offset {
                            Some(current) => current.min(offset),
                            None => offset,
                        });
                    }
                    info!(
                        asset_id = %asset_id_str,
                        checkpoint_ts = checkpoint.checkpoint_timestamp_us,
                        wal_offset = ?checkpoint.wal_offset,
                        "hydrating from checkpoint"
                    );
                    model.hydrate_checkpoint(checkpoint).await;
                    result.checkpoints_loaded += 1;
                }
                Ok(None) => {
                    info!(
                        asset_id = %asset_id_str,
                        "no checkpoint found, starting empty"
                    );
                }
                Err(e) => {
                    warn!(
                        asset_id = %asset_id_str,
                        error = %e,
                        "failed to read checkpoint, starting empty"
                    );
                }
            }
        }
    }

    // Phase 2: Replay WAL tail from the minimum checkpoint offset, using the
    // operator-configured WalConfig so the global-offset math matches the
    // writer's `global_offset()` (a hardcoded default segment size mis-skipped
    // records under a non-default `wal.segment_size_mb`).
    if let Some(cfg) = wal_config {
        if cfg.base_path.exists() {
            let (wal_records_replayed, wal_end_position) =
                replay_wal_tail(model, cfg, min_wal_offset).await;
            result.wal_records_replayed = wal_records_replayed;
            result.wal_resume_offset = min_wal_offset;
            result.wal_end_position = wal_end_position;
        } else {
            info!(path = %cfg.base_path.display(), "WAL directory not found, skipping WAL replay");
        }
    }

    // Phase 3: Mark hydrated.
    model.mark_hydrated().await;

    info!(
        checkpoints = result.checkpoints_loaded,
        wal_records = result.wal_records_replayed,
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
) -> (usize, Option<pb_wal::WalPosition>) {
    let config = config.clone();

    let mut reader = match pb_wal::WalReader::open(config.clone(), "serve-hydration") {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "failed to open WAL reader for hydration");
            return (0, None);
        }
    };

    // If we have a global offset from the checkpoint, we need to seek past it.
    // The WAL reader starts from the earliest position by default; we skip
    // records until we pass the checkpoint's offset.
    let skip_to = from_global_offset.unwrap_or(0);
    let mut records_replayed = 0;
    let mut records_skipped = 0;

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
                        model.apply_record(record).await;
                        records_replayed += 1;
                    }
                    Err(e) => {
                        warn!(error = %e, "failed to decode WAL record during hydration, skipping");
                    }
                }
            }
            Ok(None) => break, // Caught up to head.
            Err(e) => {
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

    (records_replayed, Some(reader.current_position()))
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

        let (replayed, _pos) = replay_wal_tail(&live, &config, Some(cutoff)).await;
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

        let (replayed, _pos) = replay_wal_tail(&live, &config, Some(cutoff)).await;
        assert_eq!(
            replayed, 1,
            "record at the exact checkpoint offset must be skipped, only the later record replayed"
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
}
