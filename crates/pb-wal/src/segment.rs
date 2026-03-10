use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

use crate::error::WalError;
use crate::FRAME_HEADER_LEN;

/// Size of the frame header: 4 bytes length + 4 bytes CRC32C.
pub const HEADER_SIZE: usize = FRAME_HEADER_LEN;

/// Maximum payload size per record (256 MB).
pub const MAX_RECORD_SIZE: usize = 256 * 1024 * 1024;

/// A single WAL segment file.
///
/// Segments are append-only files with a fixed maximum size. Records are
/// framed with a 4-byte little-endian length prefix followed by a 4-byte
/// CRC32C checksum, then the payload bytes.
#[derive(Debug)]
pub struct Segment {
    pub id: u64,
    pub path: PathBuf,
    pub write_offset: u64,
    file: File,
}

impl Segment {
    /// Create a new segment file at the given path.
    pub fn create(id: u64, dir: &Path) -> Result<Self, WalError> {
        let path = segment_path(dir, id);
        let file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .read(true)
            .open(&path)
            .map_err(|e| WalError::io(&path, e))?;
        Ok(Self {
            id,
            path,
            write_offset: 0,
            file,
        })
    }

    /// Open an existing segment file for appending.
    pub fn open_append(id: u64, dir: &Path) -> Result<Self, WalError> {
        let path = segment_path(dir, id);
        let file = OpenOptions::new()
            .write(true)
            .read(true)
            .open(&path)
            .map_err(|e| WalError::io(&path, e))?;
        let write_offset = file.metadata().map_err(|e| WalError::io(&path, e))?.len();
        Ok(Self {
            id,
            path,
            write_offset,
            file,
        })
    }

    /// Append a framed record (header + payload) to this segment.
    pub fn append(&mut self, payload: &[u8]) -> Result<u64, WalError> {
        if payload.len() > MAX_RECORD_SIZE {
            return Err(WalError::RecordTooLarge {
                size: payload.len(),
                max: MAX_RECORD_SIZE,
            });
        }

        let offset = self.write_offset;
        let len = payload.len() as u32;
        let crc = crc32c::crc32c(payload);

        use std::io::Write;
        self.file
            .write_all(&len.to_le_bytes())
            .map_err(|e| WalError::io(&self.path, e))?;
        self.file
            .write_all(&crc.to_le_bytes())
            .map_err(|e| WalError::io(&self.path, e))?;
        self.file
            .write_all(payload)
            .map_err(|e| WalError::io(&self.path, e))?;

        self.write_offset += HEADER_SIZE as u64 + payload.len() as u64;
        Ok(offset)
    }

    /// Flush buffered writes to the OS.
    pub fn flush(&self) -> Result<(), WalError> {
        use std::io::Write;
        (&self.file)
            .flush()
            .map_err(|e| WalError::io(&self.path, e))
    }

    /// Returns the remaining capacity in this segment.
    pub fn remaining(&self, segment_size: u64) -> u64 {
        segment_size.saturating_sub(self.write_offset)
    }
}

/// Read a single framed record from a byte slice at the given offset.
/// Returns `(payload, next_offset)` or `None` if at end of data.
pub fn read_record_at(
    data: &[u8],
    offset: usize,
    segment_id: u64,
) -> Result<Option<(Vec<u8>, usize)>, WalError> {
    if offset >= data.len() {
        return Ok(None);
    }
    if offset + HEADER_SIZE > data.len() {
        // Truncated header — treat as end of segment.
        return Ok(None);
    }

    let len = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
    let stored_crc = u32::from_le_bytes(data[offset + 4..offset + 8].try_into().unwrap());

    if len == 0 {
        // Zero-length record marks end of written data (unused space in segment).
        return Ok(None);
    }

    let payload_start = offset + HEADER_SIZE;
    let payload_end = payload_start + len;

    if payload_end > data.len() {
        return Err(WalError::TruncatedRecord {
            segment_id,
            offset: offset as u64,
        });
    }

    let payload = &data[payload_start..payload_end];
    let computed_crc = crc32c::crc32c(payload);

    if stored_crc != computed_crc {
        return Err(WalError::CrcMismatch {
            segment_id,
            offset: offset as u64,
            expected: stored_crc,
            actual: computed_crc,
        });
    }

    Ok(Some((payload.to_vec(), payload_end)))
}

/// Generate the file path for a segment with the given ID.
pub fn segment_path(dir: &Path, id: u64) -> PathBuf {
    dir.join(format!("segment_{id:020}.wal"))
}

/// List all segment IDs in a directory, sorted ascending.
pub fn list_segment_ids(dir: &Path) -> Result<Vec<u64>, WalError> {
    let mut ids = Vec::new();
    let entries = std::fs::read_dir(dir).map_err(|e| WalError::io(dir, e))?;
    for entry in entries {
        let entry = entry.map_err(|e| WalError::io(dir, e))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if let Some(rest) = name.strip_prefix("segment_") {
            if let Some(id_str) = rest.strip_suffix(".wal") {
                if let Ok(id) = id_str.parse::<u64>() {
                    ids.push(id);
                }
            }
        }
    }
    ids.sort_unstable();
    Ok(ids)
}
