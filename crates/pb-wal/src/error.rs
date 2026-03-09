use std::path::PathBuf;

/// Errors from the write-ahead log.
#[derive(Debug, thiserror::Error)]
pub enum WalError {
    #[error("I/O error on {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("CRC mismatch at segment {segment_id} offset {offset}: expected {expected:#010x}, got {actual:#010x}")]
    CrcMismatch {
        segment_id: u64,
        offset: u64,
        expected: u32,
        actual: u32,
    },

    #[error("record too large: {size} bytes exceeds max {max} bytes")]
    RecordTooLarge { size: usize, max: usize },

    #[error("truncated record at segment {segment_id} offset {offset}")]
    TruncatedRecord { segment_id: u64, offset: u64 },

    #[error("codec error: {0}")]
    Codec(String),

    #[error("segment gap: consumer {consumer} at segment {committed_segment} but earliest available is {earliest_available}")]
    SegmentGap {
        consumer: String,
        committed_segment: u64,
        earliest_available: u64,
    },
}

impl WalError {
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}
