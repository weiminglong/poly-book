//! Embedded write-ahead log with mmap segments for durable event streaming.
//!
//! The WAL provides an append-only, durable, multi-consumer event log.
//! Records are framed with length prefix + CRC32C checksums. Segments
//! are fixed-size mmap'd files that rotate when full.

pub mod codec;
mod error;
mod reader;
mod segment;
mod writer;

pub use error::WalError;
pub use reader::WalReader;
pub use segment::HEADER_SIZE;
pub use writer::WalWriter;

/// Configuration for the write-ahead log.
#[derive(Debug, Clone)]
pub struct WalConfig {
    /// Base directory for WAL segment files.
    pub base_path: std::path::PathBuf,
    /// Maximum size of each segment file in bytes. Default: 64 MB.
    pub segment_size: u64,
    /// Maximum number of retained segments. Oldest sealed segments are pruned
    /// when this limit is exceeded and all consumers have advanced past them.
    /// Default: 16.
    pub max_segments: usize,
    /// Maximum allowed consumer lag in bytes before pruning is paused.
    /// Default: 256 MB.
    pub max_consumer_lag_bytes: u64,
}

impl Default for WalConfig {
    fn default() -> Self {
        Self {
            base_path: std::path::PathBuf::from("./data/wal"),
            segment_size: 64 * 1024 * 1024, // 64 MB
            max_segments: 16,
            max_consumer_lag_bytes: 256 * 1024 * 1024, // 256 MB
        }
    }
}

/// Record frame layout:
///
/// ```text
/// ┌───────────┬───────────┬──────────────────┐
/// │ len: u32  │ crc: u32  │ payload: [u8]    │
/// └───────────┴───────────┴──────────────────┘
/// ```
///
/// `len` is the payload length (not including the 8-byte header).
/// `crc` is CRC32C of the payload bytes.
pub const FRAME_HEADER_LEN: usize = 8; // 4 bytes len + 4 bytes crc

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_read_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let config = WalConfig {
            base_path: dir.path().to_path_buf(),
            segment_size: 4096,
            max_segments: 4,
            ..WalConfig::default()
        };

        let mut writer = WalWriter::open(config.clone()).unwrap();
        let payloads: Vec<Vec<u8>> = (0..10)
            .map(|i| format!("record-{i}").into_bytes())
            .collect();

        for payload in &payloads {
            writer.append(payload).unwrap();
        }
        writer.flush().unwrap();

        let mut reader = WalReader::open(config, "test-consumer").unwrap();
        for expected in &payloads {
            let record = reader.next().unwrap();
            assert!(record.is_some(), "expected a record");
            assert_eq!(&record.unwrap(), expected);
        }
        // No more records.
        assert!(reader.next().unwrap().is_none());
    }

    #[test]
    fn segment_rotation() {
        let dir = tempfile::tempdir().unwrap();
        let config = WalConfig {
            base_path: dir.path().to_path_buf(),
            // Tiny segments to force rotation.
            segment_size: 128,
            max_segments: 8,
            ..WalConfig::default()
        };

        let mut writer = WalWriter::open(config.clone()).unwrap();
        let mut total_written = 0;
        let payload = b"hello-world-payload!";
        for _ in 0..20 {
            writer.append(payload).unwrap();
            total_written += 1;
        }
        writer.flush().unwrap();

        // Should have created multiple segment files.
        let segment_count = std::fs::read_dir(dir.path())
            .unwrap()
            .filter(|entry| {
                entry
                    .as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".wal")
            })
            .count();
        assert!(
            segment_count > 1,
            "expected multiple segments, got {segment_count}"
        );

        let mut reader = WalReader::open(config, "test-consumer").unwrap();
        let mut total_read = 0;
        while let Some(data) = reader.next().unwrap() {
            assert_eq!(data, payload);
            total_read += 1;
        }
        assert_eq!(total_read, total_written);
    }

    #[test]
    fn crc_corruption_detected() {
        let dir = tempfile::tempdir().unwrap();
        let config = WalConfig {
            base_path: dir.path().to_path_buf(),
            segment_size: 4096,
            max_segments: 4,
            ..WalConfig::default()
        };

        let mut writer = WalWriter::open(config.clone()).unwrap();
        writer.append(b"good-record").unwrap();
        writer.append(b"will-be-corrupted").unwrap();
        writer.append(b"after-corruption").unwrap();
        writer.flush().unwrap();

        // Corrupt the second record's payload.
        let segment_files: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".wal"))
            .collect();
        assert!(!segment_files.is_empty());

        let seg_path = segment_files[0].path();
        let mut data = std::fs::read(&seg_path).unwrap();
        // The first record starts at offset 0: 4-byte len + 4-byte crc + payload.
        let first_payload_len =
            u32::from_le_bytes(data[0..4].try_into().unwrap()) as usize;
        let second_record_start = FRAME_HEADER_LEN + first_payload_len;
        let second_payload_start = second_record_start + FRAME_HEADER_LEN;
        // Flip a byte in the second record's payload.
        if second_payload_start < data.len() {
            data[second_payload_start] ^= 0xFF;
        }
        std::fs::write(&seg_path, &data).unwrap();

        let mut reader = WalReader::open(config, "test-consumer").unwrap();
        // First record should be fine.
        let first = reader.next().unwrap();
        assert_eq!(first.as_deref(), Some(b"good-record".as_slice()));

        // Second record should be a CRC error — reader skips it.
        let second = reader.next().unwrap();
        // Reader skips corrupt records and returns the next valid one.
        assert_eq!(second.as_deref(), Some(b"after-corruption".as_slice()));

        // No more records.
        assert!(reader.next().unwrap().is_none());
    }

    #[test]
    fn multi_consumer_independent_positions() {
        let dir = tempfile::tempdir().unwrap();
        let config = WalConfig {
            base_path: dir.path().to_path_buf(),
            segment_size: 4096,
            max_segments: 4,
            ..WalConfig::default()
        };

        let mut writer = WalWriter::open(config.clone()).unwrap();
        for i in 0..5 {
            writer.append(format!("msg-{i}").as_bytes()).unwrap();
        }
        writer.flush().unwrap();

        // Consumer A reads all 5.
        let mut reader_a = WalReader::open(config.clone(), "consumer-a").unwrap();
        for i in 0..5 {
            let data = reader_a.next().unwrap().unwrap();
            assert_eq!(data, format!("msg-{i}").as_bytes());
        }
        assert!(reader_a.next().unwrap().is_none());

        // Consumer B reads only 3, then stops.
        let mut reader_b = WalReader::open(config.clone(), "consumer-b").unwrap();
        for i in 0..3 {
            let data = reader_b.next().unwrap().unwrap();
            assert_eq!(data, format!("msg-{i}").as_bytes());
        }
        reader_b.commit_position().unwrap();

        // Consumer B resumes and reads remaining 2.
        let mut reader_b2 = WalReader::open(config, "consumer-b").unwrap();
        for i in 3..5 {
            let data = reader_b2.next().unwrap().unwrap();
            assert_eq!(data, format!("msg-{i}").as_bytes());
        }
        assert!(reader_b2.next().unwrap().is_none());
    }

    #[test]
    fn pruning_removes_fully_consumed_segments() {
        let dir = tempfile::tempdir().unwrap();
        let config = WalConfig {
            base_path: dir.path().to_path_buf(),
            // Tiny segments — each record gets its own segment.
            segment_size: 64,
            max_segments: 100,
            ..WalConfig::default()
        };

        let mut writer = WalWriter::open(config.clone()).unwrap();
        for i in 0..10 {
            writer.append(format!("rec-{i}").as_bytes()).unwrap();
        }
        writer.flush().unwrap();

        let initial_segments = count_wal_files(dir.path());
        assert!(initial_segments > 1);

        // Consumer reads all records and commits.
        let mut reader = WalReader::open(config.clone(), "sole-consumer").unwrap();
        while reader.next().unwrap().is_some() {}
        reader.commit_position().unwrap();

        // Prune.
        writer.prune(&[dir.path().join("consumer_sole-consumer.pos")]).unwrap();
        let after_prune = count_wal_files(dir.path());
        // Active segment is always retained; sealed ones consumed by all should be pruned.
        assert!(
            after_prune < initial_segments,
            "expected pruning to reduce segments: before={initial_segments}, after={after_prune}"
        );
    }

    fn count_wal_files(dir: &std::path::Path) -> usize {
        std::fs::read_dir(dir)
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".wal")
            })
            .count()
    }

    #[test]
    fn gap_detection_after_pruning() {
        let dir = tempfile::tempdir().unwrap();
        let config = WalConfig {
            base_path: dir.path().to_path_buf(),
            segment_size: 64,
            max_segments: 100,
            max_consumer_lag_bytes: 0,
        };

        let mut writer = WalWriter::open(config.clone()).unwrap();
        for i in 0..10 {
            writer.append(format!("rec-{i}").as_bytes()).unwrap();
        }
        writer.flush().unwrap();

        // Consumer A reads all records and commits.
        let mut reader_a = WalReader::open(config.clone(), "consumer-a").unwrap();
        while reader_a.next().unwrap().is_some() {}
        reader_a.commit_position().unwrap();

        // Consumer B reads only the first record and commits (at segment 0).
        let mut reader_b = WalReader::open(config.clone(), "consumer-b").unwrap();
        let _ = reader_b.next().unwrap();
        reader_b.commit_position().unwrap();
        let (b_seg, _) = reader_b.position();

        // Prune old segments (only consumer-a is passed, so segments B
        // still references will be pruned).
        writer
            .prune(&[dir.path().join("consumer_consumer-a.pos")])
            .unwrap();

        // Verify that segments before consumer A's position were actually pruned.
        let remaining = segment::list_segment_ids(dir.path()).unwrap();
        let earliest_remaining = *remaining.first().unwrap();
        assert!(
            earliest_remaining > b_seg,
            "expected pruning to remove segment {b_seg}, earliest remaining is {earliest_remaining}"
        );

        // Re-open consumer B from its saved position — it should detect a gap.
        let reader_b2 = WalReader::open(config, "consumer-b").unwrap();
        assert!(
            reader_b2.needs_resync(),
            "consumer with stale position should detect segment gap after pruning"
        );
    }

    #[test]
    fn lag_bytes_calculation() {
        let dir = tempfile::tempdir().unwrap();
        let config = WalConfig {
            base_path: dir.path().to_path_buf(),
            segment_size: 4096,
            max_segments: 4,
            max_consumer_lag_bytes: 256 * 1024 * 1024,
        };

        let mut writer = WalWriter::open(config.clone()).unwrap();
        for i in 0..5 {
            writer.append(format!("payload-{i}").as_bytes()).unwrap();
        }
        writer.flush().unwrap();

        let mut reader = WalReader::open(config, "lag-consumer").unwrap();

        // Before reading, lag should be positive.
        let initial_lag = reader.lag_bytes().unwrap();
        assert!(initial_lag > 0, "expected positive lag before reading");

        // Read all records.
        while reader.next().unwrap().is_some() {}

        // After reading all, lag should be 0.
        let final_lag = reader.lag_bytes().unwrap();
        assert_eq!(final_lag, 0, "expected zero lag after reading all records");
    }

    #[test]
    fn prune_respects_backpressure() {
        let dir = tempfile::tempdir().unwrap();
        let config = WalConfig {
            base_path: dir.path().to_path_buf(),
            segment_size: 64,
            max_segments: 100,
            // Set a very large retention window so no segments are pruned.
            max_consumer_lag_bytes: 1024 * 1024,
        };

        let mut writer = WalWriter::open(config.clone()).unwrap();
        for i in 0..10 {
            writer.append(format!("rec-{i}").as_bytes()).unwrap();
        }
        writer.flush().unwrap();

        let initial_segments = count_wal_files(dir.path());
        assert!(initial_segments > 1);

        // Consumer reads all records and commits.
        let mut reader = WalReader::open(config.clone(), "bp-consumer").unwrap();
        while reader.next().unwrap().is_some() {}
        reader.commit_position().unwrap();

        // Prune with backpressure — large retention window should keep all segments.
        writer
            .prune_with_backpressure(&[dir.path().join("consumer_bp-consumer.pos")])
            .unwrap();

        let after_prune = count_wal_files(dir.path());
        assert_eq!(
            after_prune, initial_segments,
            "backpressure should retain all segments within retention window"
        );
    }

    mod proptests {
        use proptest::prelude::*;

        use super::*;

        proptest! {
            #[test]
            fn arbitrary_payloads_survive_roundtrip(
                payloads in prop::collection::vec(
                    prop::collection::vec(any::<u8>(), 1..512),
                    1..20
                )
            ) {
                let dir = tempfile::tempdir().unwrap();
                let config = WalConfig {
                    base_path: dir.path().to_path_buf(),
                    segment_size: 256,
                    max_segments: 64,
                    ..WalConfig::default()
                };

                let mut writer = WalWriter::open(config.clone()).unwrap();
                for payload in &payloads {
                    writer.append(payload).unwrap();
                }
                writer.flush().unwrap();

                let mut reader = WalReader::open(config, "prop-consumer").unwrap();
                for expected in &payloads {
                    let actual = reader.next().unwrap().unwrap();
                    prop_assert_eq!(&actual, expected);
                }
                prop_assert!(reader.next().unwrap().is_none());
            }

            #[test]
            fn segment_rotation_preserves_ordering(
                count in 5..50usize,
            ) {
                let dir = tempfile::tempdir().unwrap();
                let config = WalConfig {
                    base_path: dir.path().to_path_buf(),
                    // Tiny segments to force rotation on every few records.
                    segment_size: 64,
                    max_segments: 100,
                    ..WalConfig::default()
                };

                let mut writer = WalWriter::open(config.clone()).unwrap();
                for i in 0..count {
                    let payload = i.to_le_bytes();
                    writer.append(&payload).unwrap();
                }
                writer.flush().unwrap();

                let mut reader = WalReader::open(config, "ordering-consumer").unwrap();
                for i in 0..count {
                    let data = reader.next().unwrap().unwrap();
                    let expected = i.to_le_bytes();
                    prop_assert_eq!(data, expected.to_vec());
                }
                prop_assert!(reader.next().unwrap().is_none());
            }
        }
    }
}
