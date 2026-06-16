//! Throughput benchmark for the dispatcher normalize path (audit: HFT standard).
//!
//! The dispatcher is the ingest CPU stage between wire deserialization and the
//! WAL: it parses each raw WS frame, normalizes it into `PersistedRecord`s,
//! maintains a per-asset shadow `L2Book`, and cross-checks our reconstructed
//! top-of-book against the venue-stated best bid/ask (A.74). That shadow-book
//! cross-check is new cost that was previously only estimated in docs/latency.md;
//! this measures the real per-`price_change`-entry throughput end-to-end through
//! the public async pipeline (channels included).

use criterion::{criterion_group, criterion_main, BatchSize, Criterion, Throughput};
use pb_feed::{Dispatcher, FeedMessage, WsRawMessage};
use tokio::sync::mpsc;

/// A realistic `price_change` batch with `entries` deltas, each carrying the
/// venue best bid/ask so the shadow-book cross-check path is exercised.
fn price_change_frame(entries: usize) -> String {
    let mut changes = String::new();
    for i in 0..entries {
        if i > 0 {
            changes.push(',');
        }
        // Prices walk across the book; sides alternate. best_bid/best_ask are
        // included so detect_book_mismatch runs against the shadow book.
        let price = 0.30 + (i as f64) * 0.001;
        let side = if i % 2 == 0 { "BUY" } else { "SELL" };
        changes.push_str(&format!(
            "{{\"asset_id\":\"tok1\",\"price\":\"{price:.3}\",\"size\":\"{}\",\"side\":\"{side}\",\"best_bid\":\"0.49\",\"best_ask\":\"0.51\"}}",
            (i % 50) + 1
        ));
    }
    format!(
        "{{\"event_type\":\"price_change\",\"timestamp\":\"1700000000000000\",\"price_changes\":[{changes}]}}"
    )
}

fn messages(count: usize, entries_per: usize) -> Vec<FeedMessage> {
    (0..count)
        .map(|i| {
            FeedMessage::Raw(WsRawMessage {
                text: price_change_frame(entries_per),
                recv_timestamp_us: 1_700_000_000_000_000 + i as u64,
            })
        })
        .collect()
}

fn bench_dispatch_price_change(c: &mut Criterion) {
    const FRAMES: usize = 200;
    const ENTRIES: usize = 5;
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();

    let mut group = c.benchmark_group("dispatcher");
    // Throughput is in price-change *entries* (one book delta + shadow-book apply
    // + cross-check each), the unit that actually scales with venue activity.
    group.throughput(Throughput::Elements((FRAMES * ENTRIES) as u64));
    group.bench_function("price_change normalize+shadow-book (200x5 entries)", |b| {
        b.iter_batched(
            || messages(FRAMES, ENTRIES),
            |msgs| {
                rt.block_on(async move {
                    // Capacity >= produced records so the dispatcher never blocks
                    // on a full output channel (we measure normalize, not
                    // backpressure).
                    let (raw_tx, raw_rx) = mpsc::channel(FRAMES + 1);
                    let (event_tx, mut event_rx) = mpsc::channel(FRAMES * ENTRIES + 1);
                    let mut dispatcher = Dispatcher::new(raw_rx, event_tx);
                    for m in msgs {
                        raw_tx.send(m).await.unwrap();
                    }
                    drop(raw_tx); // closing the input makes run() drain then return
                    dispatcher.run().await.unwrap();
                    // Drain outputs so they are not dropped mid-measurement.
                    let mut n = 0usize;
                    while event_rx.try_recv().is_ok() {
                        n += 1;
                    }
                    n
                })
            },
            BatchSize::SmallInput,
        )
    });
    group.finish();
}

criterion_group!(benches, bench_dispatch_price_change);
criterion_main!(benches);
