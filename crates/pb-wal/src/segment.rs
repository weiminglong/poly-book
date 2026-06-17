use std::fs::{File, OpenOptions};
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use tracing::warn;

use crate::error::WalError;
use crate::FRAME_HEADER_LEN;

/// Size of the frame header: 4 bytes length + 4 bytes CRC32C.
pub const HEADER_SIZE: usize = FRAME_HEADER_LEN;

/// Maximum payload size per record (256 MB).
pub const MAX_RECORD_SIZE: usize = 256 * 1024 * 1024;

/// BufWriter capacity — 64 KiB is enough to batch many small record frames
/// while keeping memory usage low.
const BUF_WRITER_CAPACITY: usize = 64 * 1024;

/// Compute the frame CRC covering both the length prefix and the payload.
///
/// Including the length field in the checksum means a corrupted length is
/// detected as a CRC mismatch instead of being silently trusted — a flipped
/// length byte can no longer cause the reader to misparse the rest of the
/// segment (see audit finding A.126).
#[inline]
pub fn frame_crc(len: u32, payload: &[u8]) -> u32 {
    let crc = crc32c::crc32c(&len.to_le_bytes());
    crc32c::crc32c_append(crc, payload)
}

/// fsync a directory so that newly created/renamed entries (segment files,
/// position files) are durable after a power loss. On most platforms opening a
/// directory and calling `sync_all` flushes its metadata.
pub fn fsync_dir(dir: &Path) -> Result<(), WalError> {
    File::open(dir)
        .and_then(|d| d.sync_all())
        .map_err(|e| WalError::io(dir, e))
}

/// Scan a segment's bytes frame-by-frame and return the byte offset at the end
/// of the last contiguous valid frame.
///
/// A torn (partial) frame, a zero-length record, or a CRC failure marks the
/// recovery boundary: everything at or after that offset is unrecoverable tail
/// garbage (from a crash mid-append or an OS zero-fill of unsynced tail pages)
/// and must be truncated before the writer resumes appending. On a cleanly
/// written segment this returns `data.len()`.
pub fn scan_valid_end(data: &[u8], segment_id: u64) -> usize {
    let mut offset = 0usize;
    // A clean end, zero-length tail, or truncated/CRC-failed frame all stop the
    // scan: the last good data ends at `offset`.
    while let Ok(Some((_, next))) = read_record_at(data, offset, segment_id) {
        offset = next;
    }
    offset
}

/// A single WAL segment file.
///
/// Segments are append-only files with a fixed maximum size. Records are
/// framed with a 4-byte little-endian length prefix followed by a 4-byte
/// CRC32C checksum, then the payload bytes. Writes are batched through a
/// `BufWriter` to reduce syscall frequency.
pub struct Segment {
    pub id: u64,
    pub path: PathBuf,
    pub write_offset: u64,
    writer: BufWriter<File>,
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
            writer: BufWriter::with_capacity(BUF_WRITER_CAPACITY, file),
        })
    }

    /// Open an existing segment file for appending, recovering a torn or
    /// zero-filled tail first.
    ///
    /// Before resuming, the segment is scanned frame-by-frame from the start. If
    /// the last bytes do not end on a valid frame boundary — a crash mid-append
    /// left a partial frame, or an OS crash zero-filled unsynced tail pages —
    /// the file is truncated back to the end of the last valid frame so new
    /// appends are correctly framed. Without this, a stale length field in the
    /// torn tail would point into freshly appended data and desync the reader,
    /// silently losing every post-restart record (audit finding A.30).
    pub fn open_append(id: u64, dir: &Path) -> Result<Self, WalError> {
        let path = segment_path(dir, id);
        let mut file = OpenOptions::new()
            .write(true)
            .read(true)
            .open(&path)
            .map_err(|e| WalError::io(&path, e))?;

        let file_len = file.metadata().map_err(|e| WalError::io(&path, e))?.len();

        // Read existing contents once (open is cold, not on the hot path) and
        // find the end of the last valid frame.
        let existing = std::fs::read(&path).map_err(|e| WalError::io(&path, e))?;
        let valid_end = scan_valid_end(&existing, id) as u64;

        if valid_end < file_len {
            warn!(
                segment_id = id,
                file_len,
                valid_end,
                discarded_bytes = file_len - valid_end,
                "recovering WAL segment: truncating torn/zeroed tail to last valid frame"
            );
            file.set_len(valid_end)
                .map_err(|e| WalError::io(&path, e))?;
            file.sync_data().map_err(|e| WalError::io(&path, e))?;
        }

        // Seek to the recovered end so the BufWriter appends correctly.
        use std::io::Seek;
        file.seek(std::io::SeekFrom::Start(valid_end))
            .map_err(|e| WalError::io(&path, e))?;
        Ok(Self {
            id,
            path,
            write_offset: valid_end,
            writer: BufWriter::with_capacity(BUF_WRITER_CAPACITY, file),
        })
    }

    /// Append a framed record (header + payload) to this segment.
    ///
    /// The 8-byte frame header (length + CRC32C) and payload are assembled
    /// into a single `write_all` call via the internal `BufWriter`, reducing
    /// syscall frequency compared to three separate writes.
    pub fn append(&mut self, payload: &[u8]) -> Result<u64, WalError> {
        if payload.len() > MAX_RECORD_SIZE {
            return Err(WalError::RecordTooLarge {
                size: payload.len(),
                max: MAX_RECORD_SIZE,
            });
        }

        let offset = self.write_offset;
        let len = payload.len() as u32;
        let crc = frame_crc(len, payload);

        // Assemble frame header into a stack buffer, then write header + payload
        // through BufWriter (typically coalesced into a single syscall).
        let mut header = [0u8; HEADER_SIZE];
        header[..4].copy_from_slice(&len.to_le_bytes());
        header[4..8].copy_from_slice(&crc.to_le_bytes());

        use std::io::Write;
        self.writer
            .write_all(&header)
            .map_err(|e| WalError::io(&self.path, e))?;
        self.writer
            .write_all(payload)
            .map_err(|e| WalError::io(&self.path, e))?;

        self.write_offset += HEADER_SIZE as u64 + payload.len() as u64;
        Ok(offset)
    }

    /// Flush buffered writes to the OS page cache.
    pub fn flush(&mut self) -> Result<(), WalError> {
        use std::io::Write;
        self.writer.flush().map_err(|e| WalError::io(&self.path, e))
    }

    /// Flush and fsync to guarantee durability on disk.
    pub fn sync(&mut self) -> Result<(), WalError> {
        self.flush()?;
        self.writer
            .get_ref()
            .sync_data()
            .map_err(|e| WalError::io(&self.path, e))
    }

    /// Returns the remaining capacity in this segment.
    pub fn remaining(&self, segment_size: u64) -> u64 {
        segment_size.saturating_sub(self.write_offset)
    }
}

impl std::fmt::Debug for Segment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Segment")
            .field("id", &self.id)
            .field("path", &self.path)
            .field("write_offset", &self.write_offset)
            .finish()
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
    let computed_crc = frame_crc(len as u32, payload);

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

#[cfg(test)]
mod tests {
    use super::*;

    // ---- read_record_at tests ----

    #[test]
    fn read_record_at_empty_data_returns_none() {
        let result = read_record_at(&[], 0, 0).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn read_record_at_offset_past_end_returns_none() {
        let data = vec![0u8; 16];
        let result = read_record_at(&data, 100, 0).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn read_record_at_truncated_header_returns_none() {
        // Less than HEADER_SIZE bytes remaining.
        let data = vec![0u8; 4];
        let result = read_record_at(&data, 0, 0).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn read_record_at_zero_length_returns_none() {
        // A zero-length record marks end of written data.
        let mut data = vec![0u8; HEADER_SIZE];
        data[..4].copy_from_slice(&0u32.to_le_bytes());
        data[4..8].copy_from_slice(&0u32.to_le_bytes());
        let result = read_record_at(&data, 0, 0).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn read_record_at_valid_frame() {
        let payload = b"hello";
        let len = payload.len() as u32;
        let crc = frame_crc(len, payload);

        let mut data = Vec::new();
        data.extend_from_slice(&len.to_le_bytes());
        data.extend_from_slice(&crc.to_le_bytes());
        data.extend_from_slice(payload);

        let result = read_record_at(&data, 0, 0).unwrap().unwrap();
        assert_eq!(result.0, payload);
        assert_eq!(result.1, HEADER_SIZE + payload.len());
    }

    #[test]
    fn read_record_at_crc_mismatch() {
        let payload = b"hello";
        let len = payload.len() as u32;
        let bad_crc = 0xDEADBEEFu32;

        let mut data = Vec::new();
        data.extend_from_slice(&len.to_le_bytes());
        data.extend_from_slice(&bad_crc.to_le_bytes());
        data.extend_from_slice(payload);

        let err = read_record_at(&data, 0, 42).unwrap_err();
        match err {
            WalError::CrcMismatch {
                segment_id,
                offset,
                expected,
                actual,
            } => {
                assert_eq!(segment_id, 42);
                assert_eq!(offset, 0);
                assert_eq!(expected, bad_crc);
                assert_eq!(actual, frame_crc(payload.len() as u32, payload));
            }
            other => panic!("expected CrcMismatch, got {other:?}"),
        }
    }

    #[test]
    fn read_record_at_truncated_payload() {
        let len = 100u32; // Claims 100 bytes of payload.
        let crc = 0u32;

        let mut data = Vec::new();
        data.extend_from_slice(&len.to_le_bytes());
        data.extend_from_slice(&crc.to_le_bytes());
        data.extend_from_slice(&[0u8; 10]); // Only 10 bytes, not 100.

        let err = read_record_at(&data, 0, 7).unwrap_err();
        match err {
            WalError::TruncatedRecord { segment_id, offset } => {
                assert_eq!(segment_id, 7);
                assert_eq!(offset, 0);
            }
            other => panic!("expected TruncatedRecord, got {other:?}"),
        }
    }

    #[test]
    fn read_record_at_multiple_records() {
        // Write two valid records back to back.
        let p1 = b"first";
        let p2 = b"second";
        let mut data = Vec::new();

        for payload in [p1.as_slice(), p2.as_slice()] {
            let len = payload.len() as u32;
            let crc = frame_crc(len, payload);
            data.extend_from_slice(&len.to_le_bytes());
            data.extend_from_slice(&crc.to_le_bytes());
            data.extend_from_slice(payload);
        }

        let (rec1, next) = read_record_at(&data, 0, 0).unwrap().unwrap();
        assert_eq!(rec1, p1);
        let (rec2, _) = read_record_at(&data, next, 0).unwrap().unwrap();
        assert_eq!(rec2, p2);
    }

    #[test]
    fn read_record_at_corrupt_single_byte_in_payload() {
        let payload = b"integrity-check";
        let len = payload.len() as u32;
        let crc = frame_crc(len, payload);

        let mut data = Vec::new();
        data.extend_from_slice(&len.to_le_bytes());
        data.extend_from_slice(&crc.to_le_bytes());
        data.extend_from_slice(payload);

        // Flip one bit in the middle of the payload.
        let mid = HEADER_SIZE + payload.len() / 2;
        data[mid] ^= 0x01;

        let err = read_record_at(&data, 0, 0).unwrap_err();
        assert!(matches!(err, WalError::CrcMismatch { .. }));
    }

    // ---- Segment create/append/flush tests ----

    #[test]
    fn segment_create_and_append() {
        let dir = tempfile::tempdir().unwrap();
        let mut seg = Segment::create(0, dir.path()).unwrap();

        assert_eq!(seg.id, 0);
        assert_eq!(seg.write_offset, 0);

        let offset = seg.append(b"test-payload").unwrap();
        assert_eq!(offset, 0);
        assert_eq!(
            seg.write_offset,
            HEADER_SIZE as u64 + b"test-payload".len() as u64
        );
    }

    #[test]
    fn segment_flush_makes_data_readable() {
        let dir = tempfile::tempdir().unwrap();
        let mut seg = Segment::create(0, dir.path()).unwrap();
        seg.append(b"flush-test").unwrap();
        seg.flush().unwrap();

        // Read back the file and verify the frame.
        let data = std::fs::read(&seg.path).unwrap();
        let (payload, _) = read_record_at(&data, 0, 0).unwrap().unwrap();
        assert_eq!(payload, b"flush-test");
    }

    #[test]
    fn segment_sync_does_not_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut seg = Segment::create(0, dir.path()).unwrap();
        seg.append(b"sync-test").unwrap();
        seg.sync().unwrap();
    }

    #[test]
    fn segment_remaining_decreases_on_append() {
        let dir = tempfile::tempdir().unwrap();
        let mut seg = Segment::create(0, dir.path()).unwrap();
        let seg_size = 4096u64;

        assert_eq!(seg.remaining(seg_size), seg_size);

        seg.append(b"data").unwrap();
        let expected_used = HEADER_SIZE as u64 + 4;
        assert_eq!(seg.remaining(seg_size), seg_size - expected_used);
    }

    #[test]
    fn segment_rejects_oversized_record() {
        let dir = tempfile::tempdir().unwrap();
        let mut seg = Segment::create(0, dir.path()).unwrap();

        let huge = vec![0u8; MAX_RECORD_SIZE + 1];
        let err = seg.append(&huge).unwrap_err();
        assert!(matches!(err, WalError::RecordTooLarge { .. }));
    }

    #[test]
    fn segment_open_append_resumes_offset() {
        let dir = tempfile::tempdir().unwrap();
        {
            let mut seg = Segment::create(5, dir.path()).unwrap();
            seg.append(b"record-1").unwrap();
            seg.append(b"record-2").unwrap();
            seg.flush().unwrap();
        }

        let seg = Segment::open_append(5, dir.path()).unwrap();
        let expected_offset = 2 * (HEADER_SIZE as u64 + 8); // two 8-byte payloads
        assert_eq!(seg.write_offset, expected_offset);
    }

    // ---- segment_path and list_segment_ids tests ----

    #[test]
    fn segment_path_format() {
        let dir = Path::new("/tmp/wal");
        let path = segment_path(dir, 42);
        assert_eq!(
            path,
            PathBuf::from("/tmp/wal/segment_00000000000000000042.wal")
        );
    }

    #[test]
    fn list_segment_ids_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let ids = list_segment_ids(dir.path()).unwrap();
        assert!(ids.is_empty());
    }

    #[test]
    fn list_segment_ids_sorted() {
        let dir = tempfile::tempdir().unwrap();
        // Create segments out of order.
        for id in [3, 1, 5, 2] {
            Segment::create(id, dir.path()).unwrap();
        }
        let ids = list_segment_ids(dir.path()).unwrap();
        assert_eq!(ids, vec![1, 2, 3, 5]);
    }

    // ---- Frame CRC covers the length field ----

    #[test]
    fn corrupt_length_field_detected_as_crc_mismatch() {
        // A valid frame whose length is then corrupted to a smaller in-bounds
        // value must be rejected (CRC covers the length), not silently
        // misparsed.
        let payload = b"abcdefghij"; // 10 bytes
        let len = payload.len() as u32;
        let crc = frame_crc(len, payload);

        let mut data = Vec::new();
        data.extend_from_slice(&len.to_le_bytes());
        data.extend_from_slice(&crc.to_le_bytes());
        data.extend_from_slice(payload);

        // Corrupt the length to 4 (still in-bounds). Old code trusted it.
        data[0] = 4;
        let err = read_record_at(&data, 0, 0).unwrap_err();
        assert!(
            matches!(err, WalError::CrcMismatch { .. }),
            "corrupted in-bounds length must be a CRC mismatch, got {err:?}"
        );
    }

    // ---- Torn / zeroed tail recovery on reopen ----

    #[test]
    fn scan_valid_end_clean_segment() {
        let dir = tempfile::tempdir().unwrap();
        let mut seg = Segment::create(0, dir.path()).unwrap();
        seg.append(b"one").unwrap();
        seg.append(b"two").unwrap();
        seg.flush().unwrap();
        let data = std::fs::read(&seg.path).unwrap();
        assert_eq!(scan_valid_end(&data, 0), data.len());
    }

    #[test]
    fn open_append_truncates_torn_tail() {
        let dir = tempfile::tempdir().unwrap();
        let good_end = {
            let mut seg = Segment::create(0, dir.path()).unwrap();
            seg.append(b"record-1").unwrap();
            seg.append(b"record-2").unwrap();
            seg.flush().unwrap();
            seg.write_offset
        };
        // Simulate a crash mid-append: append a partial frame (header claiming
        // a 50-byte payload, but only 5 payload bytes written).
        {
            use std::io::Write;
            let path = segment_path(dir.path(), 0);
            let mut f = OpenOptions::new().append(true).open(&path).unwrap();
            let mut hdr = [0u8; HEADER_SIZE];
            hdr[..4].copy_from_slice(&50u32.to_le_bytes());
            hdr[4..8].copy_from_slice(&frame_crc(50, b"short").to_le_bytes());
            f.write_all(&hdr).unwrap();
            f.write_all(b"short").unwrap();
            f.sync_all().unwrap();
        }
        // Reopen for append: the torn tail must be truncated to the last valid
        // frame, and new appends must land cleanly after it.
        {
            let mut seg = Segment::open_append(0, dir.path()).unwrap();
            assert_eq!(
                seg.write_offset, good_end,
                "torn tail should be truncated to last valid frame"
            );
            seg.append(b"record-3").unwrap();
            seg.flush().unwrap();
        }
        // All three good records must read back, with no desync.
        let data = std::fs::read(segment_path(dir.path(), 0)).unwrap();
        let (r1, n1) = read_record_at(&data, 0, 0).unwrap().unwrap();
        let (r2, n2) = read_record_at(&data, n1, 0).unwrap().unwrap();
        let (r3, _) = read_record_at(&data, n2, 0).unwrap().unwrap();
        assert_eq!(r1, b"record-1");
        assert_eq!(r2, b"record-2");
        assert_eq!(r3, b"record-3");
    }

    #[test]
    fn open_append_truncates_zero_filled_tail() {
        let dir = tempfile::tempdir().unwrap();
        let good_end = {
            let mut seg = Segment::create(0, dir.path()).unwrap();
            seg.append(b"durable").unwrap();
            seg.flush().unwrap();
            seg.write_offset
        };
        // Simulate ext4/XFS zero-fill of unsynced tail pages after an OS crash.
        {
            use std::io::Write;
            let path = segment_path(dir.path(), 0);
            let mut f = OpenOptions::new().append(true).open(&path).unwrap();
            f.write_all(&[0u8; 4096]).unwrap();
            f.sync_all().unwrap();
        }
        let mut seg = Segment::open_append(0, dir.path()).unwrap();
        assert_eq!(
            seg.write_offset, good_end,
            "zero-filled tail should be truncated back to the last valid frame"
        );
        // A record appended after recovery must be readable (not buried behind
        // the zero region).
        seg.append(b"after-recovery").unwrap();
        seg.flush().unwrap();
        let data = std::fs::read(segment_path(dir.path(), 0)).unwrap();
        let (r1, n1) = read_record_at(&data, 0, 0).unwrap().unwrap();
        let (r2, _) = read_record_at(&data, n1, 0).unwrap().unwrap();
        assert_eq!(r1, b"durable");
        assert_eq!(r2, b"after-recovery");
    }

    #[test]
    fn list_segment_ids_ignores_non_wal_files() {
        let dir = tempfile::tempdir().unwrap();
        Segment::create(0, dir.path()).unwrap();
        // Create a non-WAL file.
        std::fs::write(dir.path().join("consumer_test.pos"), "0:0").unwrap();
        std::fs::write(dir.path().join("random.txt"), "data").unwrap();

        let ids = list_segment_ids(dir.path()).unwrap();
        assert_eq!(ids, vec![0]);
    }
}
