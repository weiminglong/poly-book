use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use pb_book::L2Book;
use pb_types::{AssetId, FixedPrice, FixedSize, Sequence, Side};
use std::hint::black_box;

fn make_book(levels: usize) -> L2Book {
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
    let mut book = L2Book::new(AssetId::new("bench"));
    book.apply_snapshot(&bids, &asks, Sequence::new(1), 1_000_000);
    book
}

fn bench_depth_iteration(c: &mut Criterion) {
    let mut group = c.benchmark_group("book_depth_iteration");
    for depth in [10, 50, 100, 200] {
        group.bench_with_input(
            BenchmarkId::new("bids_sorted", depth),
            &depth,
            |b, &depth| {
                let book = make_book(depth);
                b.iter(|| black_box(book.bids_sorted()))
            },
        );
        group.bench_with_input(
            BenchmarkId::new("asks_sorted", depth),
            &depth,
            |b, &depth| {
                let book = make_book(depth);
                b.iter(|| black_box(book.asks_sorted()))
            },
        );
    }
    group.finish();
}

fn bench_spread_at_depth(c: &mut Criterion) {
    let mut group = c.benchmark_group("spread_at_depth");
    for depth in [10, 50, 100, 200] {
        group.bench_with_input(BenchmarkId::new("spread", depth), &depth, |b, &depth| {
            let book = make_book(depth);
            b.iter(|| black_box(book.spread()))
        });
        group.bench_with_input(BenchmarkId::new("mid_price", depth), &depth, |b, &depth| {
            let book = make_book(depth);
            b.iter(|| black_box(book.mid_price()))
        });
    }
    group.finish();
}

fn bench_snapshot_at_depth(c: &mut Criterion) {
    let mut group = c.benchmark_group("snapshot_at_depth");
    for depth in [10, 50, 100, 200] {
        let bids: Vec<_> = (0..depth)
            .map(|i| {
                (
                    FixedPrice::new((5000 - i as u32 * 10).min(10000)).unwrap(),
                    FixedSize::from_f64(100.0 + i as f64).unwrap(),
                )
            })
            .collect();
        let asks: Vec<_> = (0..depth)
            .map(|i| {
                (
                    FixedPrice::new((5100 + i as u32 * 10).min(10000)).unwrap(),
                    FixedSize::from_f64(100.0 + i as f64).unwrap(),
                )
            })
            .collect();
        group.bench_with_input(BenchmarkId::new("apply_snapshot", depth), &depth, |b, _| {
            let mut book = L2Book::new(AssetId::new("bench"));
            b.iter(|| {
                book.apply_snapshot(
                    black_box(&bids),
                    black_box(&asks),
                    Sequence::new(1),
                    1_000_000,
                )
            })
        });
    }
    group.finish();
}

fn bench_mixed_delta_workload(c: &mut Criterion) {
    let mut group = c.benchmark_group("mixed_workload");
    group.bench_function("10k_mixed_updates_inserts_deletes", |b| {
        b.iter(|| {
            let mut book = L2Book::new(AssetId::new("bench"));
            let bids: Vec<_> = (0..20)
                .map(|i| {
                    (
                        FixedPrice::new(5000 - i * 10).unwrap(),
                        FixedSize::new(1_000_000),
                    )
                })
                .collect();
            let asks: Vec<_> = (0..20)
                .map(|i| {
                    (
                        FixedPrice::new(5100 + i * 10).unwrap(),
                        FixedSize::new(1_000_000),
                    )
                })
                .collect();
            book.apply_snapshot(&bids, &asks, Sequence::new(0), 0);

            for i in 0u64..10_000 {
                let side = if i % 2 == 0 { Side::Bid } else { Side::Ask };
                let price_raw = match i % 3 {
                    0 => (4800 + (i % 20) * 10) as u32,
                    1 => (5100 + (i % 20) * 10) as u32,
                    _ => (4900 + (i % 10) * 10) as u32,
                };
                let size = if i % 7 == 0 {
                    FixedSize::ZERO
                } else {
                    FixedSize::new(((i % 1000) + 1) * 1_000)
                };
                book.apply_delta(
                    side,
                    FixedPrice::new(price_raw.min(10_000)).unwrap(),
                    size,
                    Sequence::new(i + 1),
                    i * 1000,
                );
            }
        })
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_depth_iteration,
    bench_spread_at_depth,
    bench_snapshot_at_depth,
    bench_mixed_delta_workload,
);
criterion_main!(benches);
