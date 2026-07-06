//! ADR-0002 comparison: BTreeMap book vs a dense-array book.
//!
//! Polymarket prices live on a bounded grid (`FixedPrice` raw 0..=10_000), so
//! an L2 side can be a fixed array of sizes indexed by raw tick with a
//! best-price cursor — the design an order-book interview reaches for first.
//! This bench implements that design at full strength (epoch-stamped cells so
//! a snapshot rebuild is O(levels), not an 80 KB clear) and runs it against
//! `L2Book` on identical workloads. Measured results and the resulting
//! decision live in docs/adr/0002-btreemap-orderbook.md.

use criterion::{criterion_group, criterion_main, Criterion};
use pb_book::L2Book;
use pb_types::{AssetId, FixedPrice, FixedSize, Sequence, Side};
use std::hint::black_box;

const GRID: usize = 10_001;

/// One side of a dense-array book. `stamp[t] == epoch` marks a live cell, so
/// clearing the side is `epoch += 1` instead of zeroing 80 KB.
struct ArraySide {
    sizes: Box<[u64; GRID]>,
    stamp: Box<[u64; GRID]>,
    epoch: u64,
    /// Raw tick of the best price, `None` when the side is empty.
    best: Option<u32>,
    is_bid: bool,
}

impl ArraySide {
    fn new(is_bid: bool) -> Self {
        Self {
            sizes: vec![0u64; GRID].into_boxed_slice().try_into().unwrap(),
            stamp: vec![0u64; GRID].into_boxed_slice().try_into().unwrap(),
            epoch: 1,
            best: None,
            is_bid,
        }
    }

    #[inline]
    fn live(&self, t: usize) -> u64 {
        if self.stamp[t] == self.epoch {
            self.sizes[t]
        } else {
            0
        }
    }

    fn clear(&mut self) {
        self.epoch += 1;
        self.best = None;
    }

    fn set(&mut self, tick: u32, size: u64) {
        let t = tick as usize;
        self.stamp[t] = self.epoch;
        self.sizes[t] = size;
        if size > 0 {
            self.best = Some(match self.best {
                Some(b) if self.is_bid => b.max(tick),
                Some(b) => b.min(tick),
                None => tick,
            });
        } else if self.best == Some(tick) {
            // Best level removed: scan toward worse prices for the next live
            // cell. This is the array book's structural cost — O(gap width)
            // rather than O(log n).
            self.best = if self.is_bid {
                (0..tick).rev().find(|&t| self.live(t as usize) > 0)
            } else {
                (tick + 1..GRID as u32).find(|&t| self.live(t as usize) > 0)
            };
        }
    }

    #[inline]
    fn best(&self) -> Option<(u32, u64)> {
        self.best.map(|t| (t, self.live(t as usize)))
    }

    fn top_k(&self, k: usize, out: &mut Vec<(u32, u64)>) {
        out.clear();
        let Some(best) = self.best else { return };
        if self.is_bid {
            let mut t = best as i64;
            while t >= 0 && out.len() < k {
                let size = self.live(t as usize);
                if size > 0 {
                    out.push((t as u32, size));
                }
                t -= 1;
            }
        } else {
            let mut t = best as usize;
            while t < GRID && out.len() < k {
                let size = self.live(t);
                if size > 0 {
                    out.push((t as u32, size));
                }
                t += 1;
            }
        }
    }
}

struct ArrayBook {
    bids: ArraySide,
    asks: ArraySide,
}

impl ArrayBook {
    fn new() -> Self {
        Self {
            bids: ArraySide::new(true),
            asks: ArraySide::new(false),
        }
    }

    fn apply_snapshot(&mut self, bids: &[(u32, u64)], asks: &[(u32, u64)]) {
        self.bids.clear();
        self.asks.clear();
        for &(t, s) in bids {
            self.bids.set(t, s);
        }
        for &(t, s) in asks {
            self.asks.set(t, s);
        }
    }
}

type GridSide = Vec<(u32, u64)>;
type FixedSide = Vec<(FixedPrice, FixedSize)>;

// Identical level layout to book_ops.rs: 50 levels per side, 10 ticks apart.
fn grid_levels(levels: usize) -> (GridSide, GridSide) {
    let bids = (0..levels)
        .map(|i| (5000 - i as u32 * 10, 100_000_000 + i as u64))
        .collect();
    let asks = (0..levels)
        .map(|i| ((5100 + i as u32 * 10).min(10_000), 100_000_000 + i as u64))
        .collect();
    (bids, asks)
}

fn fixed_levels(levels: usize) -> (FixedSide, FixedSide) {
    let (b, a) = grid_levels(levels);
    let conv = |v: GridSide| -> FixedSide {
        v.into_iter()
            .map(|(t, s)| (FixedPrice::new(t).unwrap(), FixedSize::new(s)))
            .collect()
    };
    (conv(b), conv(a))
}

fn bench_compare(c: &mut Criterion) {
    let (gb, ga) = grid_levels(50);
    let (fb, fa) = fixed_levels(50);

    // --- snapshot rebuild ---
    let mut group = c.benchmark_group("book_compare/apply_snapshot_50");
    group.bench_function("btreemap", |b| {
        let mut book = L2Book::new(AssetId::new("bench"));
        b.iter(|| book.apply_snapshot(black_box(&fb), black_box(&fa), Sequence::new(1), 1_000_000));
    });
    group.bench_function("array", |b| {
        let mut book = ArrayBook::new();
        b.iter(|| book.apply_snapshot(black_box(&gb), black_box(&ga)));
    });
    group.finish();

    // --- single-level delta (resting level size change) ---
    let mut group = c.benchmark_group("book_compare/apply_delta");
    group.bench_function("btreemap", |b| {
        let mut book = L2Book::new(AssetId::new("bench"));
        book.apply_snapshot(&fb, &fa, Sequence::new(0), 0);
        let mut seq = 1u64;
        b.iter(|| {
            book.apply_delta(
                black_box(Side::Bid),
                black_box(FixedPrice::new(4950).unwrap()),
                black_box(FixedSize::new(50_000_000)),
                Sequence::new(seq),
                seq * 1000,
            );
            seq += 1;
        });
    });
    group.bench_function("array", |b| {
        let mut book = ArrayBook::new();
        book.apply_snapshot(&gb, &ga);
        b.iter(|| book.bids.set(black_box(4950), black_box(50_000_000)));
    });
    group.finish();

    // --- top-of-book read ---
    let mut group = c.benchmark_group("book_compare/best_bid_ask");
    group.bench_function("btreemap", |b| {
        let mut book = L2Book::new(AssetId::new("bench"));
        book.apply_snapshot(&fb, &fa, Sequence::new(0), 0);
        b.iter(|| (black_box(book.best_bid()), black_box(book.best_ask())));
    });
    group.bench_function("array", |b| {
        let mut book = ArrayBook::new();
        book.apply_snapshot(&gb, &ga);
        b.iter(|| (black_box(book.bids.best()), black_box(book.asks.best())));
    });
    group.finish();

    // --- top-10 depth ---
    let mut group = c.benchmark_group("book_compare/top_10");
    group.bench_function("btreemap", |b| {
        let mut book = L2Book::new(AssetId::new("bench"));
        book.apply_snapshot(&fb, &fa, Sequence::new(0), 0);
        b.iter(|| black_box(book.top_bids(10)));
    });
    group.bench_function("array", |b| {
        let mut book = ArrayBook::new();
        book.apply_snapshot(&gb, &ga);
        let mut out = Vec::with_capacity(10);
        b.iter(|| {
            book.bids.top_k(10, &mut out);
            black_box(&out);
        });
    });
    group.finish();

    // --- best-level churn: remove the best bid, then restore it. The array
    // book pays a scan to the next level on every removal; the BTreeMap pays
    // O(log n) twice. This is the adversarial pattern for the cursor design.
    let mut group = c.benchmark_group("book_compare/best_level_churn");
    group.bench_function("btreemap", |b| {
        let mut book = L2Book::new(AssetId::new("bench"));
        book.apply_snapshot(&fb, &fa, Sequence::new(0), 0);
        let mut seq = 1u64;
        b.iter(|| {
            book.apply_delta(
                Side::Bid,
                FixedPrice::new(5000).unwrap(),
                FixedSize::new(0),
                Sequence::new(seq),
                seq,
            );
            seq += 1;
            book.apply_delta(
                Side::Bid,
                FixedPrice::new(5000).unwrap(),
                FixedSize::new(100_000_000),
                Sequence::new(seq),
                seq,
            );
            seq += 1;
        });
    });
    group.bench_function("array", |b| {
        let mut book = ArrayBook::new();
        book.apply_snapshot(&gb, &ga);
        b.iter(|| {
            book.bids.set(black_box(5000), 0);
            book.bids.set(black_box(5000), 100_000_000);
        });
    });
    group.finish();
}

criterion_group!(benches, bench_compare);
criterion_main!(benches);
