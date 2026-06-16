//! Benchmarks for the WAL durability hot path (audit: HFT latency standard).
//!
//! The recv→durable path ends at the WAL: every ingested record is encoded and
//! appended here, so its cost bounds ingest throughput and the durable-write tail
//! latency. These benches isolate three stages:
//!
//! - `encode`: codec serialization CPU only (no I/O).
//! - `append+flush`: framed append + flush to the OS page cache (steady state — a
//!   tailing reader sees the record; fsync is amortized separately on the cadence).
//! - `append+fdatasync-each`: append + fdatasync per record (worst case — full
//!   durability on every record, which is why the WAL batches fsync instead).

use criterion::{criterion_group, criterion_main, BatchSize, Criterion, Throughput};
use pb_types::event::{BookEvent, BookEventKind, DataSource, EventProvenance, PersistedRecord};
use pb_types::{AssetId, FixedPrice, FixedSize, Sequence};
use pb_wal::{codec, WalConfig, WalWriter};

fn sample_record(seq: u64) -> PersistedRecord {
    PersistedRecord::Book(BookEvent {
        asset_id: AssetId::new("0xbtc-updown-5m-token-yes"),
        kind: BookEventKind::Delta,
        side: pb_types::event::Side::Bid,
        price: FixedPrice::new(5123).unwrap(),
        size: FixedSize::from_f64(42.5).unwrap(),
        provenance: EventProvenance {
            recv_timestamp_us: 1_700_000_000_000_000 + seq,
            exchange_timestamp_us: 1_700_000_000_000_000 + seq,
            source: DataSource::WebSocket,
            source_event_id: Some("a1b2c3d4".to_string()),
            source_session_id: Some("session-1".to_string()),
            sequence: Some(Sequence::new(seq)),
            ingest_ordinal: Some(seq),
        },
    })
}

fn bench_config(dir: &std::path::Path) -> WalConfig {
    WalConfig {
        base_path: dir.to_path_buf(),
        // Large segment so segment rotation does not perturb the append micro-bench.
        segment_size: 1 << 30,
        max_segments: 64,
        ..WalConfig::default()
    }
}

fn bench_encode(c: &mut Criterion) {
    let record = sample_record(1);
    c.bench_function("codec::encode (book delta)", |b| {
        b.iter(|| codec::encode(std::hint::black_box(&record)).unwrap())
    });
}

// Pre-encode the batch so the I/O benches measure append/flush/sync, not codec
// serialization (covered separately by `bench_encode`).
fn encoded_batch(n: u64) -> Vec<Vec<u8>> {
    (0..n)
        .map(|seq| codec::encode(&sample_record(seq)).unwrap())
        .collect()
}

fn bench_append_flush(c: &mut Criterion) {
    const BATCH: u64 = 1_000;
    let frames = encoded_batch(BATCH);
    let mut group = c.benchmark_group("wal_append");
    group.throughput(Throughput::Elements(BATCH));
    group.bench_function("append+flush (1k records)", |b| {
        b.iter_batched(
            // Untimed setup: tempdir + open the writer (flock + file create) so
            // only the appends and the flush are measured.
            || {
                let dir = tempfile::tempdir().unwrap();
                let writer = WalWriter::open(bench_config(dir.path())).unwrap();
                (dir, writer)
            },
            |(dir, mut writer)| {
                for frame in &frames {
                    writer.append(frame).unwrap();
                }
                writer.flush().unwrap();
                // Return both so the writer/dir drop happens after timing.
                (dir, writer)
            },
            BatchSize::SmallInput,
        )
    });
    group.finish();
}

fn bench_append_sync(c: &mut Criterion) {
    // fdatasync is milliseconds and filesystem/hardware-bound, so a small batch
    // with few samples keeps this tractable. The point is to expose the per-record
    // sync cost that motivates the WAL's batched-fsync cadence (sync_interval_ms),
    // not to micro-optimize it.
    const BATCH: u64 = 100;
    let frames = encoded_batch(BATCH);
    let mut group = c.benchmark_group("wal_append");
    group.sample_size(10);
    group.throughput(Throughput::Elements(BATCH));
    group.bench_function("append+fdatasync-each (100 records)", |b| {
        b.iter_batched(
            || {
                let dir = tempfile::tempdir().unwrap();
                let writer = WalWriter::open(bench_config(dir.path())).unwrap();
                (dir, writer)
            },
            |(dir, mut writer)| {
                for frame in &frames {
                    writer.append(frame).unwrap();
                    writer.sync().unwrap();
                }
                (dir, writer)
            },
            BatchSize::SmallInput,
        )
    });
    group.finish();
}

criterion_group!(benches, bench_encode, bench_append_flush, bench_append_sync);
criterion_main!(benches);
