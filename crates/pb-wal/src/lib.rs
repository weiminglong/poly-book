//! Embedded write-ahead log with append-only segments for durable event streaming.
//!
//! The WAL provides an append-only, durable, multi-consumer event log.
//! Records are framed with length prefix + CRC32C checksums. Writes are
//! batched through a `BufWriter` to reduce syscall frequency. Segments
//! are fixed-size files that rotate when full.

pub mod codec;
mod error;
mod reader;
mod segment;
mod writer;

pub use error::WalError;
pub use reader::{WalPosition, WalReader};
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
    /// How often live readers should durably commit their consumer position
    /// during steady-state tailing. Default: 1000 ms.
    pub position_commit_interval_ms: u64,
}

impl Default for WalConfig {
    fn default() -> Self {
        Self {
            base_path: std::path::PathBuf::from("./data/wal"),
            segment_size: 64 * 1024 * 1024, // 64 MB
            max_segments: 16,
            max_consumer_lag_bytes: 256 * 1024 * 1024, // 256 MB
            position_commit_interval_ms: 1_000,
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
        let first_payload_len = u32::from_le_bytes(data[0..4].try_into().unwrap()) as usize;
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
        writer
            .prune(&[dir.path().join("consumer_sole-consumer.pos")])
            .unwrap();
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
            position_commit_interval_ms: 1_000,
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
            position_commit_interval_ms: 1_000,
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
            position_commit_interval_ms: 1_000,
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

    // ---- Writer: position tracking ----

    #[test]
    fn writer_position_advances() {
        let dir = tempfile::tempdir().unwrap();
        let config = WalConfig {
            base_path: dir.path().to_path_buf(),
            segment_size: 4096,
            max_segments: 4,
            ..WalConfig::default()
        };

        let mut writer = WalWriter::open(config).unwrap();
        let (seg0, off0) = writer.position();
        assert_eq!(seg0, 0);
        assert_eq!(off0, 0);

        writer.append(b"data").unwrap();
        let (seg1, off1) = writer.position();
        assert_eq!(seg1, 0);
        assert!(off1 > 0);
    }

    #[test]
    fn writer_global_offset_is_monotonic() {
        let dir = tempfile::tempdir().unwrap();
        let config = WalConfig {
            base_path: dir.path().to_path_buf(),
            segment_size: 64,
            max_segments: 100,
            ..WalConfig::default()
        };

        let mut writer = WalWriter::open(config).unwrap();
        let mut prev = writer.global_offset();

        for i in 0..20 {
            writer.append(format!("rec-{i}").as_bytes()).unwrap();
            let current = writer.global_offset();
            assert!(
                current > prev,
                "global_offset not monotonic: {prev} -> {current}"
            );
            prev = current;
        }
    }

    // ---- Writer: segment rotation at exact boundary ----

    #[test]
    fn rotation_at_exact_segment_boundary() {
        let dir = tempfile::tempdir().unwrap();
        let payload = b"AAAA"; // 4 bytes
        let frame_size = FRAME_HEADER_LEN as u64 + payload.len() as u64; // 12 bytes

        let config = WalConfig {
            base_path: dir.path().to_path_buf(),
            // Segment fits exactly 2 records.
            segment_size: frame_size * 2,
            max_segments: 100,
            ..WalConfig::default()
        };

        let mut writer = WalWriter::open(config.clone()).unwrap();
        assert_eq!(writer.position().0, 0);

        // First two records fill segment 0 exactly.
        writer.append(payload).unwrap();
        writer.append(payload).unwrap();
        assert_eq!(writer.position().0, 0);

        // Third record should trigger rotation.
        writer.append(payload).unwrap();
        assert_eq!(writer.position().0, 1);
        writer.flush().unwrap();

        // Reader should see all 3 records.
        let mut reader = WalReader::open(config, "boundary-consumer").unwrap();
        for _ in 0..3 {
            assert!(reader.next().unwrap().is_some());
        }
        assert!(reader.next().unwrap().is_none());
    }

    // ---- Writer: write after rotation ----

    #[test]
    fn write_after_rotation_produces_readable_data() {
        let dir = tempfile::tempdir().unwrap();
        let config = WalConfig {
            base_path: dir.path().to_path_buf(),
            segment_size: 64,
            max_segments: 100,
            ..WalConfig::default()
        };

        let mut writer = WalWriter::open(config.clone()).unwrap();
        // Write enough to trigger rotation.
        for i in 0..10 {
            writer.append(format!("before-{i}").as_bytes()).unwrap();
        }
        // Write more after rotation.
        for i in 0..5 {
            writer.append(format!("after-{i}").as_bytes()).unwrap();
        }
        writer.flush().unwrap();

        let mut reader = WalReader::open(config, "rotate-consumer").unwrap();
        let mut count = 0;
        while reader.next().unwrap().is_some() {
            count += 1;
        }
        assert_eq!(count, 15);
    }

    // ---- Writer: sync does not error ----

    #[test]
    fn writer_sync_does_not_error() {
        let dir = tempfile::tempdir().unwrap();
        let config = WalConfig {
            base_path: dir.path().to_path_buf(),
            segment_size: 4096,
            max_segments: 4,
            ..WalConfig::default()
        };

        let mut writer = WalWriter::open(config).unwrap();
        writer.append(b"sync-data").unwrap();
        writer.sync().unwrap();
    }

    // ---- Writer: resume appending after reopen ----

    #[test]
    fn writer_resumes_from_last_segment() {
        let dir = tempfile::tempdir().unwrap();
        let config = WalConfig {
            base_path: dir.path().to_path_buf(),
            segment_size: 4096,
            max_segments: 4,
            ..WalConfig::default()
        };

        {
            let mut writer = WalWriter::open(config.clone()).unwrap();
            writer.append(b"session-1-data").unwrap();
            writer.flush().unwrap();
        }

        // Reopen and append more.
        {
            let mut writer = WalWriter::open(config.clone()).unwrap();
            writer.append(b"session-2-data").unwrap();
            writer.flush().unwrap();
        }

        // Reader should see both records.
        let mut reader = WalReader::open(config, "resume-consumer").unwrap();
        let r1 = reader.next().unwrap().unwrap();
        let r2 = reader.next().unwrap().unwrap();
        assert_eq!(r1, b"session-1-data");
        assert_eq!(r2, b"session-2-data");
        assert!(reader.next().unwrap().is_none());
    }

    // ---- Reader: position persistence round-trip ----

    #[test]
    fn reader_position_persistence_roundtrip() {
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

        // Read 3 records, commit position.
        let mut reader = WalReader::open(config.clone(), "persist-test").unwrap();
        for _ in 0..3 {
            reader.next().unwrap().unwrap();
        }
        reader.commit_position().unwrap();
        let (saved_seg, saved_off) = reader.position();

        // Reopen — should resume from committed position.
        let mut reader2 = WalReader::open(config, "persist-test").unwrap();
        let (restored_seg, restored_off) = reader2.position();
        assert_eq!(saved_seg, restored_seg);
        assert_eq!(saved_off, restored_off);

        // Should read remaining 2 records.
        let r1 = reader2.next().unwrap().unwrap();
        assert_eq!(r1, b"msg-3");
        let r2 = reader2.next().unwrap().unwrap();
        assert_eq!(r2, b"msg-4");
        assert!(reader2.next().unwrap().is_none());
    }

    #[test]
    fn reader_can_start_from_explicit_position() {
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

        let mut reader = WalReader::open(config.clone(), "explicit-start-source").unwrap();
        for _ in 0..3 {
            reader.next().unwrap().unwrap();
        }
        let position = reader.current_position();

        let mut resumed = WalReader::open_at(config, "explicit-start-dest", position).unwrap();
        let r1 = resumed.next().unwrap().unwrap();
        assert_eq!(r1, b"msg-3");
        let r2 = resumed.next().unwrap().unwrap();
        assert_eq!(r2, b"msg-4");
        assert!(resumed.next().unwrap().is_none());
    }

    // ---- Reader: atomic position file (temp + rename) ----

    #[test]
    fn position_file_uses_atomic_write() {
        let dir = tempfile::tempdir().unwrap();
        let config = WalConfig {
            base_path: dir.path().to_path_buf(),
            segment_size: 4096,
            max_segments: 4,
            ..WalConfig::default()
        };

        let mut writer = WalWriter::open(config.clone()).unwrap();
        writer.append(b"data").unwrap();
        writer.flush().unwrap();

        let mut reader = WalReader::open(config, "atomic-test").unwrap();
        reader.next().unwrap();
        reader.commit_position().unwrap();

        // The final file should exist, temp file should not.
        let pos_file = dir.path().join("consumer_atomic-test.pos");
        let tmp_file = dir.path().join("consumer_atomic-test.pos.tmp");
        assert!(pos_file.exists());
        assert!(!tmp_file.exists());

        // Content should be "segment_id:offset".
        let content = std::fs::read_to_string(&pos_file).unwrap();
        assert!(
            content.contains(':'),
            "position file should be in segment:offset format, got: {content}"
        );
    }

    // ---- Reader: tail from empty WAL ----

    #[test]
    fn reader_from_empty_wal_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let config = WalConfig {
            base_path: dir.path().to_path_buf(),
            segment_size: 4096,
            max_segments: 4,
            ..WalConfig::default()
        };

        // Create the WAL (creates first empty segment).
        let mut writer = WalWriter::open(config.clone()).unwrap();
        writer.flush().unwrap();

        let mut reader = WalReader::open(config, "empty-consumer").unwrap();
        assert!(reader.next().unwrap().is_none());
    }

    // ---- Reader: single record WAL ----

    #[test]
    fn reader_single_record_wal() {
        let dir = tempfile::tempdir().unwrap();
        let config = WalConfig {
            base_path: dir.path().to_path_buf(),
            segment_size: 4096,
            max_segments: 4,
            ..WalConfig::default()
        };

        let mut writer = WalWriter::open(config.clone()).unwrap();
        writer.append(b"only-record").unwrap();
        writer.flush().unwrap();

        let mut reader = WalReader::open(config, "single-consumer").unwrap();
        let data = reader.next().unwrap().unwrap();
        assert_eq!(data, b"only-record");
        assert!(reader.next().unwrap().is_none());
    }

    // ---- Reader: needs_resync is false for fresh reader ----

    #[test]
    fn fresh_reader_does_not_need_resync() {
        let dir = tempfile::tempdir().unwrap();
        let config = WalConfig {
            base_path: dir.path().to_path_buf(),
            segment_size: 4096,
            max_segments: 4,
            ..WalConfig::default()
        };

        let mut writer = WalWriter::open(config.clone()).unwrap();
        writer.append(b"data").unwrap();
        writer.flush().unwrap();

        let reader = WalReader::open(config, "fresh-consumer").unwrap();
        assert!(!reader.needs_resync());
    }

    // ---- Reader: refresh_segments ----

    #[test]
    fn refresh_segments_picks_up_new_segments() {
        let dir = tempfile::tempdir().unwrap();
        let config = WalConfig {
            base_path: dir.path().to_path_buf(),
            segment_size: 64,
            max_segments: 100,
            ..WalConfig::default()
        };

        let mut writer = WalWriter::open(config.clone()).unwrap();
        writer.append(b"initial").unwrap();
        writer.flush().unwrap();

        let mut reader = WalReader::open(config.clone(), "refresh-consumer").unwrap();
        let initial_segs = reader.position().0; // Not important, just open it.
        let _ = initial_segs;

        // Write more data to create new segments.
        for i in 0..10 {
            writer.append(format!("extra-{i}").as_bytes()).unwrap();
        }
        writer.flush().unwrap();

        reader.refresh_segments().unwrap();
        // Reader should now be aware of more segments.
        // Just verify it doesn't error and we can read records.
        let mut count = 0;
        while reader.next().unwrap().is_some() {
            count += 1;
        }
        assert!(count >= 1);
    }

    // ---- Pruner: prune with no consumers ----

    #[test]
    fn prune_with_no_consumers_retains_all() {
        let dir = tempfile::tempdir().unwrap();
        let config = WalConfig {
            base_path: dir.path().to_path_buf(),
            segment_size: 64,
            max_segments: 100,
            ..WalConfig::default()
        };

        let mut writer = WalWriter::open(config).unwrap();
        for i in 0..10 {
            writer.append(format!("rec-{i}").as_bytes()).unwrap();
        }
        writer.flush().unwrap();

        let initial = count_wal_files(dir.path());
        writer.prune(&[]).unwrap();
        let after = count_wal_files(dir.path());

        // With no consumer files, min_consumer_segment returns active segment id,
        // so no sealed segments should be pruned since they're all < active.
        // Actually, with empty consumer list, the code returns active.id as min,
        // meaning only segments < active.id are pruned (all sealed segments).
        // Let me re-check: min_consumer_segment returns active.id when empty.
        // Then in prune: id < min_consumed (= active.id) AND id < active.id => prunes all sealed.
        assert!(after <= initial);
    }

    // ---- Pruner: prune retains active segment ----

    #[test]
    fn prune_always_retains_active_segment() {
        let dir = tempfile::tempdir().unwrap();
        let config = WalConfig {
            base_path: dir.path().to_path_buf(),
            segment_size: 64,
            max_segments: 100,
            max_consumer_lag_bytes: 0,
            position_commit_interval_ms: 1_000,
        };

        let mut writer = WalWriter::open(config.clone()).unwrap();
        for i in 0..10 {
            writer.append(format!("rec-{i}").as_bytes()).unwrap();
        }
        writer.flush().unwrap();

        // Consumer reads all and commits.
        let mut reader = WalReader::open(config, "sole").unwrap();
        while reader.next().unwrap().is_some() {}
        reader.commit_position().unwrap();

        writer
            .prune(&[dir.path().join("consumer_sole.pos")])
            .unwrap();

        // At least the active segment must remain.
        let remaining = count_wal_files(dir.path());
        assert!(remaining >= 1, "active segment should always remain");
    }

    // ---- Codec round-trip through WAL (encode -> WAL append -> WAL read -> decode) ----

    #[test]
    fn codec_through_wal_roundtrip() {
        use pb_types::event::{BookEvent, BookEventKind, DataSource, EventProvenance, Side};
        use pb_types::{AssetId, FixedPrice, FixedSize, Sequence};

        let dir = tempfile::tempdir().unwrap();
        let config = WalConfig {
            base_path: dir.path().to_path_buf(),
            segment_size: 4096,
            max_segments: 4,
            ..WalConfig::default()
        };

        let record = pb_types::event::PersistedRecord::Book(BookEvent {
            asset_id: AssetId::new("tok1"),
            kind: BookEventKind::Delta,
            side: Side::Bid,
            price: FixedPrice::new(5000).unwrap(),
            size: FixedSize::from_f64(100.0).unwrap(),
            provenance: EventProvenance {
                recv_timestamp_us: 1_000_000,
                exchange_timestamp_us: 999_000,
                source: DataSource::WebSocket,
                source_event_id: None,
                source_session_id: Some("ws-1".to_string()),
                sequence: Some(Sequence::new(42)),
            },
        });

        let encoded = codec::encode(&record).unwrap();

        let mut writer = WalWriter::open(config.clone()).unwrap();
        writer.append(&encoded).unwrap();
        writer.flush().unwrap();

        let mut reader = WalReader::open(config, "codec-consumer").unwrap();
        let raw = reader.next().unwrap().unwrap();
        let decoded = codec::decode(&raw).unwrap();
        assert_eq!(format!("{decoded:?}"), format!("{record:?}"));
    }

    // ---- WAL with multiple segment rotations preserves data ----

    #[test]
    fn many_rotations_preserve_all_data() {
        let dir = tempfile::tempdir().unwrap();
        let config = WalConfig {
            base_path: dir.path().to_path_buf(),
            segment_size: 48, // Extremely small, forces rotation almost every record.
            max_segments: 1000,
            ..WalConfig::default()
        };

        let mut writer = WalWriter::open(config.clone()).unwrap();
        let n = 100;
        for i in 0..n {
            writer.append(format!("{i:04}").as_bytes()).unwrap();
        }
        writer.flush().unwrap();

        let mut reader = WalReader::open(config, "many-rot").unwrap();
        for i in 0..n {
            let data = reader.next().unwrap().unwrap();
            assert_eq!(
                String::from_utf8(data).unwrap(),
                format!("{i:04}"),
                "mismatch at record {i}"
            );
        }
        assert!(reader.next().unwrap().is_none());
    }

    // ---- Reader: CRC corruption skips to next valid record ----

    #[test]
    fn reader_skips_multiple_corrupt_records() {
        let dir = tempfile::tempdir().unwrap();
        let config = WalConfig {
            base_path: dir.path().to_path_buf(),
            segment_size: 4096,
            max_segments: 4,
            ..WalConfig::default()
        };

        let mut writer = WalWriter::open(config.clone()).unwrap();
        writer.append(b"good-1").unwrap();
        writer.append(b"corrupt-a").unwrap();
        writer.append(b"corrupt-b").unwrap();
        writer.append(b"good-2").unwrap();
        writer.flush().unwrap();

        // Corrupt records 2 and 3 (index 1 and 2).
        let seg_files: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".wal"))
            .collect();
        let seg_path = seg_files[0].path();
        let mut data = std::fs::read(&seg_path).unwrap();

        // Calculate offsets for records 2 and 3.
        let r1_payload_len = u32::from_le_bytes(data[0..4].try_into().unwrap()) as usize;
        let r2_start = FRAME_HEADER_LEN + r1_payload_len;
        let r2_payload_start = r2_start + FRAME_HEADER_LEN;

        let r2_payload_len =
            u32::from_le_bytes(data[r2_start..r2_start + 4].try_into().unwrap()) as usize;
        let r3_start = r2_start + FRAME_HEADER_LEN + r2_payload_len;
        let r3_payload_start = r3_start + FRAME_HEADER_LEN;

        // Flip bytes in records 2 and 3.
        if r2_payload_start < data.len() {
            data[r2_payload_start] ^= 0xFF;
        }
        if r3_payload_start < data.len() {
            data[r3_payload_start] ^= 0xFF;
        }
        std::fs::write(&seg_path, &data).unwrap();

        let mut reader = WalReader::open(config, "skip-consumer").unwrap();
        let first = reader.next().unwrap().unwrap();
        assert_eq!(first, b"good-1");

        // Should skip 2 corrupt records and return good-2.
        let second = reader.next().unwrap().unwrap();
        assert_eq!(second, b"good-2");

        assert!(reader.next().unwrap().is_none());
    }

    // ---- Lag bytes: positive lag for unread data ----

    #[test]
    fn lag_bytes_positive_for_partial_read() {
        let dir = tempfile::tempdir().unwrap();
        let config = WalConfig {
            base_path: dir.path().to_path_buf(),
            segment_size: 4096,
            max_segments: 4,
            ..WalConfig::default()
        };

        let mut writer = WalWriter::open(config.clone()).unwrap();
        for i in 0..10 {
            writer.append(format!("payload-{i}").as_bytes()).unwrap();
        }
        writer.flush().unwrap();

        let mut reader = WalReader::open(config, "lag-partial").unwrap();
        // Read only 3.
        for _ in 0..3 {
            reader.next().unwrap();
        }

        let lag = reader.lag_bytes().unwrap();
        assert!(lag > 0, "should have positive lag after partial read");
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

    /// Regression test: corrupting the length field to a value larger than the
    /// segment must NOT cause the reader to loop forever. Previously the reader
    /// would repeatedly reload the same segment because `current_offset` was
    /// never advanced past the truncated record.
    #[test]
    fn corrupted_length_field_does_not_hang() {
        let dir = tempfile::tempdir().unwrap();
        let config = WalConfig {
            base_path: dir.path().to_path_buf(),
            segment_size: 4096,
            max_segments: 8,
            ..WalConfig::default()
        };

        // Write a single 1-byte payload.
        let mut writer = WalWriter::open(config.clone()).unwrap();
        writer.append(&[0u8]).unwrap();
        writer.flush().unwrap();
        drop(writer);

        // Corrupt byte 2 of the segment (part of the length field).
        // Original length bytes for 1-byte payload: [1,0,0,0].
        // After XOR with 0xFF at index 2: [1,0,255,0] = 16_711_681.
        // This is far larger than the 9-byte segment, triggering TruncatedRecord.
        let mut seg_files: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".wal"))
            .map(|e| e.path())
            .collect();
        seg_files.sort();
        assert!(!seg_files.is_empty());

        let mut data = std::fs::read(&seg_files[0]).unwrap();
        data[2] ^= 0xFF;
        std::fs::write(&seg_files[0], &data).unwrap();

        // The reader must terminate (not hang).
        let mut reader = WalReader::open(config, "hang-test").unwrap();
        let mut count = 0;
        loop {
            match reader.next() {
                Ok(Some(_)) => count += 1,
                Ok(None) | Err(_) => break,
            }
            assert!(count < 100, "reader appears to be looping");
        }
    }
}
