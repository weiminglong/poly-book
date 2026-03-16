use std::io::Write;
use std::path::PathBuf;

use tracing::warn;

use crate::error::WalError;
use crate::segment;
use crate::WalConfig;

/// Durable reader position within the WAL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WalPosition {
    pub segment_id: u64,
    pub offset: usize,
}

/// WAL consumer that reads records with independent position tracking.
pub struct WalReader {
    config: WalConfig,
    consumer_name: String,
    /// Current segment being read.
    current_segment_id: u64,
    /// Byte offset within the current segment.
    current_offset: usize,
    /// Cached data of the current segment.
    current_data: Option<Vec<u8>>,
    /// Sorted list of available segment IDs (refreshed on segment advance).
    available_segments: Vec<u64>,
}

impl WalReader {
    /// Open a WAL reader for the given consumer. Resumes from the last
    /// committed position if one exists.
    pub fn open(config: WalConfig, consumer_name: &str) -> Result<Self, WalError> {
        let available = segment::list_segment_ids(&config.base_path)?;

        let (start_seg, start_offset) = Self::load_position(&config.base_path, consumer_name)?
            .unwrap_or_else(|| {
                // Start from the earliest available segment.
                let first = available.first().copied().unwrap_or(0);
                (first, 0)
            });

        let mut reader = Self {
            config,
            consumer_name: consumer_name.to_string(),
            current_segment_id: start_seg,
            current_offset: start_offset,
            current_data: None,
            available_segments: available,
        };

        // Load the starting segment data.
        reader.load_segment(start_seg)?;
        Ok(reader)
    }

    /// Open a WAL reader at an explicit position instead of the last committed
    /// consumer position. This is useful for handing off from checkpoint/WAL
    /// hydration directly into live tailing without replaying already-applied
    /// records.
    pub fn open_at(
        config: WalConfig,
        consumer_name: &str,
        position: WalPosition,
    ) -> Result<Self, WalError> {
        let available = segment::list_segment_ids(&config.base_path)?;
        let mut reader = Self {
            config,
            consumer_name: consumer_name.to_string(),
            current_segment_id: position.segment_id,
            current_offset: position.offset,
            current_data: None,
            available_segments: available,
        };
        reader.load_segment(position.segment_id)?;
        Ok(reader)
    }

    /// Read the next record, or `None` if no more records are available.
    /// Automatically advances across segment boundaries.
    /// Skips corrupt records (CRC mismatch) with a warning log.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Result<Option<Vec<u8>>, WalError> {
        loop {
            if let Some(data) = &self.current_data {
                match segment::read_record_at(data, self.current_offset, self.current_segment_id) {
                    Ok(Some((payload, next_offset))) => {
                        self.current_offset = next_offset;
                        return Ok(Some(payload));
                    }
                    Ok(None) => {
                        // End of this segment — try advancing.
                        if !self.advance_segment()? {
                            return Ok(None);
                        }
                        continue;
                    }
                    Err(WalError::CrcMismatch {
                        segment_id,
                        offset,
                        expected,
                        actual,
                    }) => {
                        warn!(
                            segment_id,
                            offset,
                            expected = format!("{expected:#010x}"),
                            actual = format!("{actual:#010x}"),
                            "CRC mismatch, skipping corrupt record"
                        );
                        // Skip past the corrupt record.
                        let len = u32::from_le_bytes(
                            data[self.current_offset..self.current_offset + 4]
                                .try_into()
                                .unwrap(),
                        ) as usize;
                        self.current_offset += crate::FRAME_HEADER_LEN + len;
                        continue;
                    }
                    Err(WalError::TruncatedRecord { .. }) => {
                        // Truncated record at end of segment — advance.
                        if !self.advance_segment()? {
                            return Ok(None);
                        }
                        continue;
                    }
                    Err(e) => return Err(e),
                }
            } else {
                return Ok(None);
            }
        }
    }

    /// Commit the current read position atomically to disk so it survives
    /// restarts. Uses write-to-temp + rename for crash safety.
    pub fn commit_position(&self) -> Result<(), WalError> {
        let path = self.position_file_path();
        let content = format!("{}:{}", self.current_segment_id, self.current_offset);
        let tmp_path = path.with_extension("pos.tmp");
        let mut file = std::fs::File::create(&tmp_path).map_err(|e| WalError::io(&tmp_path, e))?;
        file.write_all(content.as_bytes())
            .map_err(|e| WalError::io(&tmp_path, e))?;
        file.sync_all().map_err(|e| WalError::io(&tmp_path, e))?;
        std::fs::rename(&tmp_path, &path).map_err(|e| WalError::io(&path, e))?;
        if let Some(parent) = path.parent() {
            std::fs::File::open(parent)
                .and_then(|dir| dir.sync_all())
                .map_err(|e| WalError::io(parent, e))?;
        }
        Ok(())
    }

    /// Returns the current read position as (segment_id, offset).
    pub fn position(&self) -> (u64, usize) {
        (self.current_segment_id, self.current_offset)
    }

    /// Returns the current read position as a typed value.
    pub fn current_position(&self) -> WalPosition {
        WalPosition {
            segment_id: self.current_segment_id,
            offset: self.current_offset,
        }
    }

    /// Check if the reader's committed position references a segment that has
    /// been pruned. Uses the cached segment list from the most recent
    /// `advance_segment` or `open` call to avoid redundant directory scans.
    /// Returns true if the reader needs to re-hydrate from checkpoint.
    pub fn needs_resync(&self) -> bool {
        if let Some(&earliest) = self.available_segments.first() {
            return self.current_segment_id < earliest;
        }
        false
    }

    /// Returns the byte lag between the reader's current position and the
    /// latest data on disk. Uses the cached segment list and only stats the
    /// files for size information.
    pub fn lag_bytes(&self) -> Option<u64> {
        if self.available_segments.is_empty() {
            return Some(0);
        }

        let mut lag: u64 = 0;

        for &seg_id in &self.available_segments {
            if seg_id < self.current_segment_id {
                continue;
            }
            let path = segment::segment_path(&self.config.base_path, seg_id);
            let file_size = std::fs::metadata(&path).ok()?.len();

            if seg_id == self.current_segment_id {
                lag += file_size.saturating_sub(self.current_offset as u64);
            } else {
                lag += file_size;
            }
        }
        Some(lag)
    }

    /// Refresh the cached list of available segments from disk.
    pub fn refresh_segments(&mut self) -> Result<(), WalError> {
        self.available_segments = segment::list_segment_ids(&self.config.base_path)?;
        Ok(())
    }

    fn position_file_path(&self) -> PathBuf {
        self.config
            .base_path
            .join(format!("consumer_{}.pos", self.consumer_name))
    }

    fn load_position(
        base_path: &std::path::Path,
        consumer_name: &str,
    ) -> Result<Option<(u64, usize)>, WalError> {
        let path = base_path.join(format!("consumer_{consumer_name}.pos"));
        if !path.exists() {
            return Ok(None);
        }
        let content = std::fs::read_to_string(&path).map_err(|e| WalError::io(&path, e))?;
        let trimmed = content.trim();
        if let Some((seg_str, off_str)) = trimmed.split_once(':') {
            if let (Ok(seg), Ok(off)) = (seg_str.parse::<u64>(), off_str.parse::<usize>()) {
                return Ok(Some((seg, off)));
            }
        }
        Ok(None)
    }

    fn load_segment(&mut self, segment_id: u64) -> Result<bool, WalError> {
        let path = segment::segment_path(&self.config.base_path, segment_id);
        if !path.exists() {
            self.current_data = None;
            return Ok(false);
        }
        let data = std::fs::read(&path).map_err(|e| WalError::io(&path, e))?;
        self.current_data = Some(data);
        self.current_segment_id = segment_id;
        Ok(true)
    }

    fn advance_segment(&mut self) -> Result<bool, WalError> {
        // Refresh available segments in case new ones appeared.
        self.available_segments = segment::list_segment_ids(&self.config.base_path)?;

        // Find the next segment after current.
        let next = self
            .available_segments
            .iter()
            .find(|&&id| id > self.current_segment_id);

        match next {
            Some(&next_id) => {
                self.current_offset = 0;
                self.load_segment(next_id)
            }
            None => {
                // Reload current segment to check for new data appended since last read.
                let path = segment::segment_path(&self.config.base_path, self.current_segment_id);
                if path.exists() {
                    let data = std::fs::read(&path).map_err(|e| WalError::io(&path, e))?;
                    if data.len() > self.current_offset {
                        self.current_data = Some(data);
                        return Ok(true);
                    }
                }
                Ok(false)
            }
        }
    }
}

impl std::fmt::Debug for WalReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WalReader")
            .field("consumer", &self.consumer_name)
            .field("segment_id", &self.current_segment_id)
            .field("offset", &self.current_offset)
            .finish()
    }
}
