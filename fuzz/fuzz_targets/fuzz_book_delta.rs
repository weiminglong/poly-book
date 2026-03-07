#![no_main]
use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use pb_book::L2Book;
use pb_types::{AssetId, FixedPrice, FixedSize, Sequence, Side};

#[derive(Arbitrary, Debug)]
struct FuzzDelta {
    is_bid: bool,
    price_raw: u16,
    size_raw: u32,
}

#[derive(Arbitrary, Debug)]
struct FuzzInput {
    initial_bids: Vec<(u16, u32)>,
    initial_asks: Vec<(u16, u32)>,
    deltas: Vec<FuzzDelta>,
}

fuzz_target!(|input: FuzzInput| {
    let mut book = L2Book::new(AssetId::new("fuzz"));

    let bids: Vec<_> = input.initial_bids.iter()
        .filter_map(|&(p, s)| {
            let price = FixedPrice::new((p as u32).min(10_000)).ok()?;
            Some((price, FixedSize::new(s as u64)))
        })
        .filter(|(_, s)| !s.is_zero())
        .collect();

    let asks: Vec<_> = input.initial_asks.iter()
        .filter_map(|&(p, s)| {
            let price = FixedPrice::new((p as u32).min(10_000)).ok()?;
            Some((price, FixedSize::new(s as u64)))
        })
        .filter(|(_, s)| !s.is_zero())
        .collect();

    book.apply_snapshot(&bids, &asks, Sequence::new(0), 0);

    // Invariant: bids sorted descending
    let sorted = book.bids_sorted();
    for w in sorted.windows(2) {
        assert!(w[0].0 >= w[1].0, "bid ordering violated");
    }

    // Invariant: asks sorted ascending
    let sorted = book.asks_sorted();
    for w in sorted.windows(2) {
        assert!(w[0].0 <= w[1].0, "ask ordering violated");
    }

    for (i, delta) in input.deltas.iter().enumerate() {
        let price_raw = (delta.price_raw as u32).min(10_000);
        if price_raw == 0 { continue; }
        let price = FixedPrice::new(price_raw).unwrap();
        let size = FixedSize::new(delta.size_raw as u64);
        let side = if delta.is_bid { Side::Bid } else { Side::Ask };

        book.apply_delta(side, price, size, Sequence::new(i as u64 + 1), (i as u64 + 1) * 1000);

        // Post-delta invariant: ordering preserved
        let sorted_bids = book.bids_sorted();
        for w in sorted_bids.windows(2) {
            assert!(w[0].0 >= w[1].0, "bid ordering violated after delta {i}");
        }
        let sorted_asks = book.asks_sorted();
        for w in sorted_asks.windows(2) {
            assert!(w[0].0 <= w[1].0, "ask ordering violated after delta {i}");
        }
    }
});
