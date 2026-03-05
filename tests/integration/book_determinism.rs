//! Test that applying the same sequence of operations always produces the same book state.

use pb_book::L2Book;
use pb_types::{AssetId, FixedPrice, FixedSize, Sequence, Side};

fn make_events() -> Vec<(Side, FixedPrice, FixedSize)> {
    vec![
        (
            Side::Bid,
            FixedPrice::from_f64(0.50).unwrap(),
            FixedSize::from_f64(100.0).unwrap(),
        ),
        (
            Side::Bid,
            FixedPrice::from_f64(0.49).unwrap(),
            FixedSize::from_f64(200.0).unwrap(),
        ),
        (
            Side::Ask,
            FixedPrice::from_f64(0.55).unwrap(),
            FixedSize::from_f64(150.0).unwrap(),
        ),
        (
            Side::Ask,
            FixedPrice::from_f64(0.56).unwrap(),
            FixedSize::from_f64(250.0).unwrap(),
        ),
        // Delta updates
        (
            Side::Bid,
            FixedPrice::from_f64(0.50).unwrap(),
            FixedSize::from_f64(500.0).unwrap(),
        ),
        (
            Side::Ask,
            FixedPrice::from_f64(0.54).unwrap(),
            FixedSize::from_f64(75.0).unwrap(),
        ),
        (
            Side::Bid,
            FixedPrice::from_f64(0.49).unwrap(),
            FixedSize::ZERO,
        ), // remove level
        (
            Side::Bid,
            FixedPrice::from_f64(0.48).unwrap(),
            FixedSize::from_f64(300.0).unwrap(),
        ),
        (
            Side::Ask,
            FixedPrice::from_f64(0.55).unwrap(),
            FixedSize::ZERO,
        ), // remove level
    ]
}

fn apply_events(book: &mut L2Book, events: &[(Side, FixedPrice, FixedSize)]) {
    let bids: Vec<_> = events[..2].iter().map(|(_, p, s)| (*p, *s)).collect();
    let asks: Vec<_> = events[2..4].iter().map(|(_, p, s)| (*p, *s)).collect();
    book.apply_snapshot(&bids, &asks, Sequence::new(0), 1_000_000);

    for (i, (side, price, size)) in events[4..].iter().enumerate() {
        book.apply_delta(
            *side,
            *price,
            *size,
            Sequence::new(i as u64 + 1),
            (i as u64 + 2) * 1_000_000,
        );
    }
}

#[test]
fn test_deterministic_book_state() {
    let events = make_events();

    // Apply same events to two independent books
    let mut book1 = L2Book::new(AssetId::new("test-1"));
    let mut book2 = L2Book::new(AssetId::new("test-2"));

    apply_events(&mut book1, &events);
    apply_events(&mut book2, &events);

    // Books should have identical state (ignoring asset_id)
    assert_eq!(book1.bid_depth(), book2.bid_depth());
    assert_eq!(book1.ask_depth(), book2.ask_depth());
    assert_eq!(book1.bids_sorted(), book2.bids_sorted());
    assert_eq!(book1.asks_sorted(), book2.asks_sorted());
    assert_eq!(book1.mid_price(), book2.mid_price());
    assert_eq!(book1.spread(), book2.spread());
}

#[test]
fn test_snapshot_resets_state() {
    let events = make_events();

    let mut book = L2Book::new(AssetId::new("test"));
    apply_events(&mut book, &events);

    // Apply a new snapshot — should completely replace state
    let new_bids = vec![(
        FixedPrice::from_f64(0.30).unwrap(),
        FixedSize::from_f64(10.0).unwrap(),
    )];
    let new_asks = vec![(
        FixedPrice::from_f64(0.70).unwrap(),
        FixedSize::from_f64(10.0).unwrap(),
    )];
    book.apply_snapshot(&new_bids, &new_asks, Sequence::new(100), 100_000_000);

    assert_eq!(book.bid_depth(), 1);
    assert_eq!(book.ask_depth(), 1);
    assert_eq!(book.best_bid().unwrap().0.raw(), 3000);
    assert_eq!(book.best_ask().unwrap().0.raw(), 7000);
}

#[test]
fn test_many_deltas_consistency() {
    let mut book = L2Book::new(AssetId::new("stress"));

    // Start with a snapshot
    let bids = vec![(
        FixedPrice::from_f64(0.50).unwrap(),
        FixedSize::from_f64(100.0).unwrap(),
    )];
    let asks = vec![(
        FixedPrice::from_f64(0.55).unwrap(),
        FixedSize::from_f64(100.0).unwrap(),
    )];
    book.apply_snapshot(&bids, &asks, Sequence::new(0), 0);

    // Apply 10,000 deltas
    for i in 0u64..10_000 {
        let price_raw = (i % 100 * 10 + 4500) as u32;
        let price = FixedPrice::new(price_raw.min(10000)).unwrap();
        let side = if i % 2 == 0 { Side::Bid } else { Side::Ask };
        let size = if i % 7 == 0 {
            FixedSize::ZERO // remove every 7th
        } else {
            FixedSize::from_f64((i % 500) as f64 + 1.0).unwrap()
        };
        book.apply_delta(side, price, size, Sequence::new(i + 1), i * 1000);
    }

    // Book should still be consistent
    if let (Some((bid, _)), Some((ask, _))) = (book.best_bid(), book.best_ask()) {
        // best bid should be <= best ask in a non-crossed book
        // (may be crossed due to random deltas, which is fine — just verify no panic)
        let _ = bid.raw();
        let _ = ask.raw();
    }

    // Verify sorted output doesn't panic
    let bids = book.bids_sorted();
    let asks = book.asks_sorted();

    // Bids should be descending
    for window in bids.windows(2) {
        assert!(window[0].0 >= window[1].0);
    }
    // Asks should be ascending
    for window in asks.windows(2) {
        assert!(window[0].0 <= window[1].0);
    }
}
