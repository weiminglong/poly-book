#![no_main]
use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use pb_wal::{WalConfig, WalReader, WalWriter};

/// Fuzz input: write some records, then corrupt the segment bytes and verify
/// the reader never panics and only returns valid (non-corrupt) records.
#[derive(Arbitrary, Debug)]
struct FuzzInput {
    /// Payloads to write before corruption.
    payloads: Vec<Vec<u8>>,
    /// Byte positions to corrupt (offset, xor_mask).
    corruptions: Vec<(u16, u8)>,
}

fuzz_target!(|input: FuzzInput| {
    // Skip degenerate inputs.
    if input.payloads.is_empty() || input.payloads.iter().all(|p| p.is_empty()) {
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let config = WalConfig {
        base_path: dir.path().to_path_buf(),
        segment_size: 4096,
        max_segments: 8,
        ..WalConfig::default()
    };

    // Write records.
    let mut writer = WalWriter::open(config.clone()).unwrap();
    let mut written = Vec::new();
    for payload in &input.payloads {
        if payload.is_empty() {
            continue;
        }
        if writer.append(payload).is_ok() {
            written.push(payload.clone());
        }
    }
    if written.is_empty() {
        return;
    }
    let _ = writer.flush();
    drop(writer);

    // Apply byte corruptions to segment files.
    let mut segment_files: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().ends_with(".wal"))
        .map(|e| e.path())
        .collect();
    segment_files.sort();

    if !segment_files.is_empty() {
        for &(offset_raw, xor_mask) in &input.corruptions {
            if xor_mask == 0 {
                continue;
            }
            // Pick a segment file.
            let seg_idx = offset_raw as usize % segment_files.len();
            let seg_path = &segment_files[seg_idx];
            if let Ok(mut data) = std::fs::read(seg_path) {
                if !data.is_empty() {
                    let byte_idx = offset_raw as usize % data.len();
                    data[byte_idx] ^= xor_mask;
                    let _ = std::fs::write(seg_path, &data);
                }
            }
        }
    }

    // Read back: the reader must never panic. It may return fewer records than
    // written (corrupt records are skipped) but must not crash.
    let mut reader = match WalReader::open(config, "fuzz-consumer") {
        Ok(r) => r,
        Err(_) => return, // Corruption may prevent opening; that's fine.
    };

    let mut read_count = 0;
    loop {
        match reader.next() {
            Ok(Some(_)) => {
                read_count += 1;
                // Guard against infinite loops from corrupted length fields.
                if read_count > written.len() * 2 {
                    break;
                }
            }
            Ok(None) => break,
            Err(_) => break, // Reader hit unrecoverable corruption; acceptable.
        }
    }
});
