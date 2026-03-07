use criterion::{criterion_group, criterion_main, Criterion};
use pb_book::L2Book;
use pb_types::{AssetId, FixedPrice, FixedSize, Sequence, Side};
use std::hint::black_box;

type LevelVec = Vec<(FixedPrice, FixedSize)>;

fn make_snapshot_data(levels: usize) -> (LevelVec, LevelVec) {
    let bids: Vec<_> = (0..levels)
        .map(|i| {
            (
                FixedPrice::new((5000 - i as u32 * 10).min(10000)).unwrap(),
                FixedSize::from_f64(100.0 + i as f64).unwrap(),
            )
        })
        .collect();
    let asks: Vec<_> = (0..levels)
        .map(|i| {
            (
                FixedPrice::new((5100 + i as u32 * 10).min(10000)).unwrap(),
                FixedSize::from_f64(100.0 + i as f64).unwrap(),
            )
        })
        .collect();
    (bids, asks)
}

fn bench_apply_snapshot(c: &mut Criterion) {
    let (bids, asks) = make_snapshot_data(50);
    c.bench_function("L2Book::apply_snapshot (50 levels)", |b| {
        let mut book = L2Book::new(AssetId::new("bench"));
        b.iter(|| {
            book.apply_snapshot(
                black_box(&bids),
                black_box(&asks),
                Sequence::new(1),
                1_000_000,
            );
        })
    });
}

fn bench_apply_delta(c: &mut Criterion) {
    let (bids, asks) = make_snapshot_data(50);
    let mut book = L2Book::new(AssetId::new("bench"));
    book.apply_snapshot(&bids, &asks, Sequence::new(0), 0);

    c.bench_function("L2Book::apply_delta", |b| {
        let mut seq = 1u64;
        b.iter(|| {
            book.apply_delta(
                black_box(Side::Bid),
                black_box(FixedPrice::new(4950).unwrap()),
                black_box(FixedSize::from_f64(50.0).unwrap()),
                Sequence::new(seq),
                seq * 1000,
            );
            seq += 1;
        })
    });
}

fn bench_best_bid_ask(c: &mut Criterion) {
    let (bids, asks) = make_snapshot_data(50);
    let mut book = L2Book::new(AssetId::new("bench"));
    book.apply_snapshot(&bids, &asks, Sequence::new(1), 1_000_000);

    c.bench_function("L2Book::best_bid + best_ask", |b| {
        b.iter(|| {
            black_box(book.best_bid());
            black_box(book.best_ask());
        })
    });
}

fn bench_mid_price(c: &mut Criterion) {
    let (bids, asks) = make_snapshot_data(50);
    let mut book = L2Book::new(AssetId::new("bench"));
    book.apply_snapshot(&bids, &asks, Sequence::new(1), 1_000_000);

    c.bench_function("L2Book::mid_price", |b| {
        b.iter(|| black_box(book.mid_price()))
    });
}

fn bench_million_deltas(c: &mut Criterion) {
    let (bids, asks) = make_snapshot_data(20);

    c.bench_function("1M delta applies", |b| {
        b.iter(|| {
            let mut book = L2Book::new(AssetId::new("bench"));
            book.apply_snapshot(&bids, &asks, Sequence::new(0), 0);
            for i in 0u64..1_000_000 {
                let price_raw = (i % 50 * 10 + 4800) as u32;
                let side = if i % 2 == 0 { Side::Bid } else { Side::Ask };
                book.apply_delta(
                    side,
                    FixedPrice::new(price_raw.min(10000)).unwrap(),
                    FixedSize::from_f64((i % 1000) as f64).unwrap(),
                    Sequence::new(i + 1),
                    i * 1000,
                );
            }
        })
    });
}

criterion_group!(
    benches,
    bench_apply_snapshot,
    bench_apply_delta,
    bench_best_bid_ask,
    bench_mid_price,
    bench_million_deltas,
);
criterion_main!(benches);
