use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use pb_types::wire::WsMessage;
use std::hint::black_box;

const BOOK_MSG: &str = r#"{"event_type":"book","asset_id":"21742633143463906290569050155826241533067272736897614950488156847949938836455","timestamp":"1700000000000","bids":[{"price":"0.50","size":"100"},{"price":"0.49","size":"200"},{"price":"0.48","size":"300"},{"price":"0.47","size":"400"},{"price":"0.46","size":"500"},{"price":"0.45","size":"600"},{"price":"0.44","size":"700"},{"price":"0.43","size":"800"},{"price":"0.42","size":"900"},{"price":"0.41","size":"1000"}],"asks":[{"price":"0.55","size":"150"},{"price":"0.56","size":"250"},{"price":"0.57","size":"350"},{"price":"0.58","size":"450"},{"price":"0.59","size":"550"},{"price":"0.60","size":"650"},{"price":"0.61","size":"750"},{"price":"0.62","size":"850"},{"price":"0.63","size":"950"},{"price":"0.64","size":"1050"}]}"#;

const PRICE_CHANGE_MSG: &str = r#"{"event_type":"price_change","market":"0x1234","price_changes":[{"asset_id":"21742633143463906290569050155826241533067272736897614950488156847949938836455","price":"0.55","size":"50","side":"BUY","hash":"abc123","best_bid":"0.55","best_ask":"0.60"}],"timestamp":"1700000000000"}"#;

const LAST_TRADE_MSG: &str = r#"{"event_type":"last_trade_price","asset_id":"21742633143463906290569050155826241533067272736897614950488156847949938836455","price":"0.55","size":"25","side":"BUY","timestamp":"1700000000000"}"#;

fn bench_book_deser(c: &mut Criterion) {
    let mut group = c.benchmark_group("wire_deser");
    group.throughput(Throughput::Bytes(BOOK_MSG.len() as u64));
    group.bench_function("book_snapshot_10_levels", |b| {
        b.iter(|| {
            let _: WsMessage<'_> = serde_json::from_str(black_box(BOOK_MSG)).unwrap();
        })
    });
    group.finish();
}

fn bench_price_change_deser(c: &mut Criterion) {
    let mut group = c.benchmark_group("wire_deser");
    group.throughput(Throughput::Bytes(PRICE_CHANGE_MSG.len() as u64));
    group.bench_function("price_change_delta", |b| {
        b.iter(|| {
            let _: WsMessage<'_> = serde_json::from_str(black_box(PRICE_CHANGE_MSG)).unwrap();
        })
    });
    group.finish();
}

fn bench_last_trade_deser(c: &mut Criterion) {
    let mut group = c.benchmark_group("wire_deser");
    group.throughput(Throughput::Bytes(LAST_TRADE_MSG.len() as u64));
    group.bench_function("last_trade_price", |b| {
        b.iter(|| {
            let _: WsMessage<'_> = serde_json::from_str(black_box(LAST_TRADE_MSG)).unwrap();
        })
    });
    group.finish();
}

fn bench_batch_deser(c: &mut Criterion) {
    let messages = vec![BOOK_MSG, PRICE_CHANGE_MSG, LAST_TRADE_MSG];
    let total_bytes: u64 = messages.iter().map(|m| m.len() as u64).sum();

    let mut group = c.benchmark_group("wire_deser");
    group.throughput(Throughput::Bytes(total_bytes * 100));
    group.bench_function("batch_100_mixed_messages", |b| {
        b.iter(|| {
            for _ in 0..100 {
                for msg in &messages {
                    let _: WsMessage<'_> = serde_json::from_str(black_box(msg)).unwrap();
                }
            }
        })
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_book_deser,
    bench_price_change_deser,
    bench_last_trade_deser,
    bench_batch_deser,
);
criterion_main!(benches);
