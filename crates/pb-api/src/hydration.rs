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

use std::path::Path;

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
    wal_path: Option<&Path>,
    active_assets: &[String],
) -> HydrationResult {
    let mut result = HydrationResult {
        checkpoints_loaded: 0,
        wal_records_replayed: 0,
        wal_resume_offset: None,
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

    // Phase 2: Replay WAL tail from the minimum checkpoint offset.
    if let Some(wal_dir) = wal_path {
        if wal_dir.exists() {
            result.wal_records_replayed =
                replay_wal_tail(model, wal_dir, min_wal_offset).await;
            result.wal_resume_offset = min_wal_offset;
        } else {
            info!(path = %wal_dir.display(), "WAL directory not found, skipping WAL replay");
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
    wal_dir: &Path,
    from_global_offset: Option<u64>,
) -> usize {
    let config = pb_wal::WalConfig {
        base_path: wal_dir.to_path_buf(),
        ..Default::default()
    };

    let mut reader = match pb_wal::WalReader::open(config.clone(), "serve-hydration") {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "failed to open WAL reader for hydration");
            return 0;
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
                let current_global =
                    seg_id * config.segment_size + seg_offset as u64;

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

    records_replayed
}

fn now_us() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64
}
