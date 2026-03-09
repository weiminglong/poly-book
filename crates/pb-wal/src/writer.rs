use std::path::Path;

use tracing::info;

use crate::error::WalError;
use crate::segment::{self, Segment};
use crate::WalConfig;

/// Append-only WAL writer that manages segment rotation.
pub struct WalWriter {
    config: WalConfig,
    active: Segment,
    next_segment_id: u64,
}

impl WalWriter {
    /// Open or create a WAL at the configured path.
    pub fn open(config: WalConfig) -> Result<Self, WalError> {
        std::fs::create_dir_all(&config.base_path)
            .map_err(|e| WalError::io(&config.base_path, e))?;

        let ids = segment::list_segment_ids(&config.base_path)?;

        let (active, next_id) = if let Some(&last_id) = ids.last() {
            // Resume appending to the last segment.
            let seg = Segment::open_append(last_id, &config.base_path)?;
            (seg, last_id + 1)
        } else {
            // No segments exist — create the first one.
            let seg = Segment::create(0, &config.base_path)?;
            (seg, 1)
        };

        Ok(Self {
            config,
            active,
            next_segment_id: next_id,
        })
    }

    /// Append a payload to the WAL. Rotates the segment if needed.
    pub fn append(&mut self, payload: &[u8]) -> Result<u64, WalError> {
        let frame_size = crate::FRAME_HEADER_LEN as u64 + payload.len() as u64;
        if self.active.remaining(self.config.segment_size) < frame_size {
            self.rotate()?;
        }
        self.active.append(payload)
    }

    /// Flush the active segment's buffered writes.
    pub fn flush(&self) -> Result<(), WalError> {
        self.active.flush()
    }

    /// Returns the current write position as (segment_id, offset).
    pub fn position(&self) -> (u64, u64) {
        (self.active.id, self.active.write_offset)
    }

    /// Returns a global byte offset combining segment_id and write_offset.
    /// This is monotonically increasing and suitable for checkpoint coordination.
    pub fn global_offset(&self) -> u64 {
        // Encode as segment_id * segment_size + write_offset.
        // This is monotonic because segment IDs increase and write_offset
        // resets to 0 on rotation (but segment_id * segment_size jumps).
        self.active.id * self.config.segment_size + self.active.write_offset
    }

    /// Prune sealed segments that all consumers have advanced past.
    /// `consumer_position_files` is the list of position file paths for all
    /// registered consumers.
    pub fn prune(&self, consumer_position_files: &[std::path::PathBuf]) -> Result<(), WalError> {
        let min_consumed = self.min_consumer_segment(consumer_position_files)?;
        let ids = segment::list_segment_ids(&self.config.base_path)?;

        for &id in &ids {
            // Never prune the active segment.
            if id >= self.active.id {
                continue;
            }
            // Only prune segments fully consumed by all consumers.
            if id < min_consumed {
                let path = segment::segment_path(&self.config.base_path, id);
                if path.exists() {
                    std::fs::remove_file(&path)
                        .map_err(|e| WalError::io(&path, e))?;
                    info!(segment_id = id, "pruned WAL segment");
                }
            }
        }
        Ok(())
    }

    fn rotate(&mut self) -> Result<(), WalError> {
        self.active.flush()?;
        let id = self.next_segment_id;
        self.next_segment_id += 1;
        self.active = Segment::create(id, &self.config.base_path)?;
        info!(segment_id = id, "rotated to new WAL segment");
        Ok(())
    }

    /// Find the minimum segment ID that any consumer is still reading from.
    fn min_consumer_segment(
        &self,
        consumer_position_files: &[std::path::PathBuf],
    ) -> Result<u64, WalError> {
        if consumer_position_files.is_empty() {
            return Ok(self.active.id);
        }

        let mut min_seg = u64::MAX;
        for pos_file in consumer_position_files {
            if !pos_file.exists() {
                // Consumer hasn't committed yet — treat as position 0.
                return Ok(0);
            }
            let content = std::fs::read_to_string(pos_file)
                .map_err(|e| WalError::io(pos_file, e))?;
            // Format: "segment_id:offset"
            if let Some((seg_str, _)) = content.trim().split_once(':') {
                if let Ok(seg_id) = seg_str.parse::<u64>() {
                    min_seg = min_seg.min(seg_id);
                }
            }
        }
        if min_seg == u64::MAX {
            Ok(self.active.id)
        } else {
            Ok(min_seg)
        }
    }
}

impl std::fmt::Debug for WalWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WalWriter")
            .field("base_path", &self.config.base_path)
            .field("active_segment", &self.active.id)
            .field("write_offset", &self.active.write_offset)
            .finish()
    }
}

/// Helper to find all consumer position files in a WAL directory.
pub fn consumer_position_files(dir: &Path) -> Result<Vec<std::path::PathBuf>, WalError> {
    let mut files = Vec::new();
    let entries = std::fs::read_dir(dir).map_err(|e| WalError::io(dir, e))?;
    for entry in entries {
        let entry = entry.map_err(|e| WalError::io(dir, e))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("consumer_") && name.ends_with(".pos") {
            files.push(entry.path());
        }
    }
    Ok(files)
}
