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

fn check_ordering(book: &L2Book, context: &str) {
    let sorted_bids = book.bids_sorted();
    for w in sorted_bids.windows(2) {
        assert!(
            w[0].0 >= w[1].0,
            "bid ordering violated {context}: {:?} < {:?}",
            w[0].0,
            w[1].0
        );
    }
    let sorted_asks = book.asks_sorted();
    for w in sorted_asks.windows(2) {
        assert!(
            w[0].0 <= w[1].0,
            "ask ordering violated {context}: {:?} > {:?}",
            w[0].0,
            w[1].0
        );
    }
}

fuzz_target!(|input: FuzzInput| {
    let mut book = L2Book::new(AssetId::new("fuzz"));

    let bids: Vec<_> = input
        .initial_bids
        .iter()
        .filter_map(|&(p, s)| {
            let price = FixedPrice::new((p as u32).min(10_000)).ok()?;
            (s > 0).then(|| (price, FixedSize::new(s as u64)))
        })
        .collect();

    let asks: Vec<_> = input
        .initial_asks
        .iter()
        .filter_map(|&(p, s)| {
            let price = FixedPrice::new((p as u32).min(10_000)).ok()?;
            (s > 0).then(|| (price, FixedSize::new(s as u64)))
        })
        .collect();

    book.apply_snapshot(&bids, &asks, Sequence::new(0), 0);
    check_ordering(&book, "after snapshot");

    assert_eq!(
        book.total_bid_size().raw(),
        book.bids_sorted().iter().map(|(_, s)| s.raw()).sum::<u64>(),
        "total_bid_size inconsistent"
    );

    for (i, delta) in input.deltas.iter().enumerate() {
        let price_raw = (delta.price_raw as u32).min(10_000);
        if price_raw == 0 {
            continue;
        }
        let price = FixedPrice::new(price_raw).unwrap();
        let size = FixedSize::new(delta.size_raw as u64);
        let side = if delta.is_bid { Side::Bid } else { Side::Ask };

        let depth_before = match side {
            Side::Bid => book.bid_depth(),
            Side::Ask => book.ask_depth(),
        };
        let had_level = match side {
            Side::Bid => book
                .bids_sorted()
                .iter()
                .any(|(p, _)| *p == price),
            Side::Ask => book
                .asks_sorted()
                .iter()
                .any(|(p, _)| *p == price),
        };

        book.apply_delta(
            side,
            price,
            size,
            Sequence::new(i as u64 + 1),
            (i as u64 + 1) * 1000,
        );

        let depth_after = match side {
            Side::Bid => book.bid_depth(),
            Side::Ask => book.ask_depth(),
        };

        if size.is_zero() && had_level {
            assert!(depth_after < depth_before, "zero-size delta didn't remove");
        } else if !size.is_zero() && !had_level {
            assert_eq!(depth_after, depth_before + 1, "new level not added");
        }

        check_ordering(&book, &format!("after delta {i}"));
    }
});
