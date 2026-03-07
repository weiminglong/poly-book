use std::cmp::Reverse;
use std::collections::BTreeMap;

use pb_types::{AssetId, FixedPrice, FixedSize, Sequence, Side};

use crate::error::BookError;

/// Level-2 orderbook: price -> aggregate size at that level.
///
/// Bids use `Reverse<FixedPrice>` so iteration yields best (highest) bid first.
/// Asks use `FixedPrice` directly so iteration yields best (lowest) ask first.
#[derive(Debug, Clone)]
pub struct L2Book {
    pub asset_id: AssetId,
    pub bids: BTreeMap<Reverse<FixedPrice>, FixedSize>,
    pub asks: BTreeMap<FixedPrice, FixedSize>,
    pub sequence: Sequence,
    pub last_update_us: u64,
}

/// A snapshot of one side of the book: Vec<(price, size)>.
pub type BookSide = Vec<(FixedPrice, FixedSize)>;

impl L2Book {
    pub fn new(asset_id: AssetId) -> Self {
        Self {
            asset_id,
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
            sequence: Sequence::default(),
            last_update_us: 0,
        }
    }

    /// Replace the entire book with a snapshot.
    pub fn apply_snapshot(
        &mut self,
        bids: &[(FixedPrice, FixedSize)],
        asks: &[(FixedPrice, FixedSize)],
        sequence: Sequence,
        timestamp_us: u64,
    ) {
        self.bids.clear();
        self.asks.clear();

        for &(price, size) in bids {
            if !size.is_zero() {
                self.bids.insert(Reverse(price), size);
            }
        }
        for &(price, size) in asks {
            if !size.is_zero() {
                self.asks.insert(price, size);
            }
        }

        self.sequence = sequence;
        self.last_update_us = timestamp_us;
    }

    /// Apply a single price-level delta.
    /// If size is zero, the level is removed.
    pub fn apply_delta(
        &mut self,
        side: Side,
        price: FixedPrice,
        size: FixedSize,
        sequence: Sequence,
        timestamp_us: u64,
    ) {
        match side {
            Side::Bid => {
                if size.is_zero() {
                    self.bids.remove(&Reverse(price));
                } else {
                    self.bids.insert(Reverse(price), size);
                }
            }
            Side::Ask => {
                if size.is_zero() {
                    self.asks.remove(&price);
                } else {
                    self.asks.insert(price, size);
                }
            }
        }

        self.sequence = sequence;
        self.last_update_us = timestamp_us;
    }

    /// Best (highest) bid price and size.
    pub fn best_bid(&self) -> Option<(FixedPrice, FixedSize)> {
        self.bids.iter().next().map(|(Reverse(p), &s)| (*p, s))
    }

    /// Best (lowest) ask price and size.
    pub fn best_ask(&self) -> Option<(FixedPrice, FixedSize)> {
        self.asks.iter().next().map(|(p, &s)| (*p, s))
    }

    /// Mid price = (best_bid + best_ask) / 2, as f64.
    pub fn mid_price(&self) -> Option<f64> {
        match (self.best_bid(), self.best_ask()) {
            (Some((bid, _)), Some((ask, _))) => Some((bid.as_f64() + ask.as_f64()) / 2.0),
            _ => None,
        }
    }

    /// Spread = best_ask - best_bid, as f64.
    pub fn spread(&self) -> Option<f64> {
        match (self.best_bid(), self.best_ask()) {
            (Some((bid, _)), Some((ask, _))) => Some(ask.as_f64() - bid.as_f64()),
            _ => None,
        }
    }

    /// Number of bid levels.
    pub fn bid_depth(&self) -> usize {
        self.bids.len()
    }

    /// Number of ask levels.
    pub fn ask_depth(&self) -> usize {
        self.asks.len()
    }

    /// Check if there's a sequence gap.
    pub fn check_sequence(&self, incoming: Sequence) -> Result<(), BookError> {
        if self.sequence.raw() > 0 && incoming.raw() != self.sequence.raw() + 1 {
            let expected = self.sequence.raw() + 1;
            let got = incoming.raw();
            return Err(BookError::SequenceGap {
                asset_id: self.asset_id.to_string(),
                expected,
                got,
                gap_size: got.abs_diff(expected),
            });
        }
        Ok(())
    }

    /// Get all bids as (price, size) sorted best-to-worst.
    pub fn bids_sorted(&self) -> Vec<(FixedPrice, FixedSize)> {
        self.bids.iter().map(|(Reverse(p), &s)| (*p, s)).collect()
    }

    /// Get all asks as (price, size) sorted best-to-worst (lowest first).
    pub fn asks_sorted(&self) -> Vec<(FixedPrice, FixedSize)> {
        self.asks.iter().map(|(p, &s)| (*p, s)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_book() -> L2Book {
        let mut book = L2Book::new(AssetId::new("test"));
        book.apply_snapshot(
            &[
                (
                    FixedPrice::from_f64(0.50).unwrap(),
                    FixedSize::from_f64(100.0).unwrap(),
                ),
                (
                    FixedPrice::from_f64(0.49).unwrap(),
                    FixedSize::from_f64(200.0).unwrap(),
                ),
                (
                    FixedPrice::from_f64(0.48).unwrap(),
                    FixedSize::from_f64(300.0).unwrap(),
                ),
            ],
            &[
                (
                    FixedPrice::from_f64(0.55).unwrap(),
                    FixedSize::from_f64(150.0).unwrap(),
                ),
                (
                    FixedPrice::from_f64(0.56).unwrap(),
                    FixedSize::from_f64(250.0).unwrap(),
                ),
            ],
            Sequence::new(1),
            1_000_000,
        );
        book
    }

    #[test]
    fn test_snapshot_apply() {
        let book = make_book();
        assert_eq!(book.bid_depth(), 3);
        assert_eq!(book.ask_depth(), 2);
    }

    #[test]
    fn test_best_bid_ask() {
        let book = make_book();
        let (bid_price, _) = book.best_bid().unwrap();
        assert_eq!(bid_price.raw(), 5000); // 0.50

        let (ask_price, _) = book.best_ask().unwrap();
        assert_eq!(ask_price.raw(), 5500); // 0.55
    }

    #[test]
    fn test_mid_price() {
        let book = make_book();
        let mid = book.mid_price().unwrap();
        assert!((mid - 0.525).abs() < 1e-6);
    }

    #[test]
    fn test_spread() {
        let book = make_book();
        let spread = book.spread().unwrap();
        assert!((spread - 0.05).abs() < 1e-6);
    }

    #[test]
    fn test_delta_update() {
        let mut book = make_book();
        // Update existing bid level
        book.apply_delta(
            Side::Bid,
            FixedPrice::from_f64(0.50).unwrap(),
            FixedSize::from_f64(500.0).unwrap(),
            Sequence::new(2),
            2_000_000,
        );
        let (_, size) = book.best_bid().unwrap();
        assert_eq!(size.raw(), 500_000_000);
    }

    #[test]
    fn test_delta_remove() {
        let mut book = make_book();
        // Remove a bid level (size = 0)
        book.apply_delta(
            Side::Bid,
            FixedPrice::from_f64(0.50).unwrap(),
            FixedSize::ZERO,
            Sequence::new(2),
            2_000_000,
        );
        assert_eq!(book.bid_depth(), 2);
        let (bid_price, _) = book.best_bid().unwrap();
        assert_eq!(bid_price.raw(), 4900); // next best bid is 0.49
    }

    #[test]
    fn test_delta_add_new_level() {
        let mut book = make_book();
        book.apply_delta(
            Side::Ask,
            FixedPrice::from_f64(0.52).unwrap(),
            FixedSize::from_f64(75.0).unwrap(),
            Sequence::new(2),
            2_000_000,
        );
        assert_eq!(book.ask_depth(), 3);
        // New best ask should be 0.52
        let (ask_price, _) = book.best_ask().unwrap();
        assert_eq!(ask_price.raw(), 5200);
    }

    #[test]
    fn test_sequence_gap_detection() {
        let book = make_book();
        assert!(book.check_sequence(Sequence::new(2)).is_ok());
        assert!(book.check_sequence(Sequence::new(5)).is_err());
    }

    #[test]
    fn test_bids_sorted_order() {
        let book = make_book();
        let bids = book.bids_sorted();
        assert_eq!(bids[0].0.raw(), 5000); // highest first
        assert_eq!(bids[1].0.raw(), 4900);
        assert_eq!(bids[2].0.raw(), 4800);
    }

    #[test]
    fn test_asks_sorted_order() {
        let book = make_book();
        let asks = book.asks_sorted();
        assert_eq!(asks[0].0.raw(), 5500); // lowest first
        assert_eq!(asks[1].0.raw(), 5600);
    }

    #[test]
    fn test_empty_book() {
        let book = L2Book::new(AssetId::new("empty"));
        assert!(book.best_bid().is_none());
        assert!(book.best_ask().is_none());
        assert!(book.mid_price().is_none());
        assert!(book.spread().is_none());
    }

    #[test]
    fn test_snapshot_clears_previous() {
        let mut book = make_book();
        book.apply_snapshot(
            &[(
                FixedPrice::from_f64(0.30).unwrap(),
                FixedSize::from_f64(10.0).unwrap(),
            )],
            &[(
                FixedPrice::from_f64(0.70).unwrap(),
                FixedSize::from_f64(10.0).unwrap(),
            )],
            Sequence::new(10),
            5_000_000,
        );
        assert_eq!(book.bid_depth(), 1);
        assert_eq!(book.ask_depth(), 1);
        assert_eq!(book.sequence.raw(), 10);
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::collection::vec as prop_vec;
    use proptest::prelude::*;

    fn arb_price() -> impl Strategy<Value = FixedPrice> {
        (1u32..=10_000u32).prop_map(|raw| FixedPrice::new(raw).unwrap())
    }

    fn arb_nonzero_size() -> impl Strategy<Value = FixedSize> {
        (1u64..=1_000_000_000u64).prop_map(FixedSize::new)
    }

    fn arb_level() -> impl Strategy<Value = (FixedPrice, FixedSize)> {
        (arb_price(), arb_nonzero_size())
    }

    fn arb_side() -> impl Strategy<Value = Side> {
        prop_oneof![Just(Side::Bid), Just(Side::Ask)]
    }

    proptest! {
        /// After applying a snapshot, all bids are strictly descending and
        /// all asks are strictly ascending (price ordering invariant).
        #[test]
        fn snapshot_preserves_price_ordering(
            bids in prop_vec(arb_level(), 0..50),
            asks in prop_vec(arb_level(), 0..50),
        ) {
            let mut book = L2Book::new(AssetId::new("prop"));
            book.apply_snapshot(&bids, &asks, Sequence::new(1), 1_000_000);

            let sorted_bids = book.bids_sorted();
            for w in sorted_bids.windows(2) {
                prop_assert!(w[0].0 >= w[1].0, "bids not descending: {:?} < {:?}", w[0].0, w[1].0);
            }

            let sorted_asks = book.asks_sorted();
            for w in sorted_asks.windows(2) {
                prop_assert!(w[0].0 <= w[1].0, "asks not ascending: {:?} > {:?}", w[0].0, w[1].0);
            }
        }

        /// The spread is never negative when both sides have levels.
        /// This is the critical invariant: bid < ask (no crossed book).
        #[test]
        fn spread_never_negative_after_snapshot(
            bid_prices in prop_vec(1u32..=4999u32, 1..20),
            ask_prices in prop_vec(5001u32..=10_000u32, 1..20),
        ) {
            let bids: Vec<_> = bid_prices.iter().map(|&p| {
                (FixedPrice::new(p).unwrap(), FixedSize::new(1_000_000))
            }).collect();
            let asks: Vec<_> = ask_prices.iter().map(|&p| {
                (FixedPrice::new(p).unwrap(), FixedSize::new(1_000_000))
            }).collect();

            let mut book = L2Book::new(AssetId::new("prop"));
            book.apply_snapshot(&bids, &asks, Sequence::new(1), 1_000_000);

            if let Some(spread) = book.spread() {
                prop_assert!(spread >= 0.0, "negative spread: {}", spread);
            }
        }

        /// Mid price, when it exists, is bounded by best bid and best ask.
        #[test]
        fn mid_price_between_best_bid_and_ask(
            bid_prices in prop_vec(1u32..=4999u32, 1..20),
            ask_prices in prop_vec(5001u32..=10_000u32, 1..20),
        ) {
            let bids: Vec<_> = bid_prices.iter().map(|&p| {
                (FixedPrice::new(p).unwrap(), FixedSize::new(1_000_000))
            }).collect();
            let asks: Vec<_> = ask_prices.iter().map(|&p| {
                (FixedPrice::new(p).unwrap(), FixedSize::new(1_000_000))
            }).collect();

            let mut book = L2Book::new(AssetId::new("prop"));
            book.apply_snapshot(&bids, &asks, Sequence::new(1), 1_000_000);

            if let (Some((best_bid, _)), Some((best_ask, _)), Some(mid)) =
                (book.best_bid(), book.best_ask(), book.mid_price())
            {
                prop_assert!(mid >= best_bid.as_f64(), "mid {} < best_bid {}", mid, best_bid.as_f64());
                prop_assert!(mid <= best_ask.as_f64(), "mid {} > best_ask {}", mid, best_ask.as_f64());
            }
        }

        /// Removing a level (size=0 delta) never increases depth.
        #[test]
        fn zero_size_delta_removes_level(
            bids in prop_vec(arb_level(), 1..30),
            asks in prop_vec(arb_level(), 1..30),
            remove_idx in 0usize..30,
        ) {
            let mut book = L2Book::new(AssetId::new("prop"));
            book.apply_snapshot(&bids, &asks, Sequence::new(0), 0);
            let bid_depth_before = book.bid_depth();
            let ask_depth_before = book.ask_depth();

            let sorted_bids = book.bids_sorted();
            if !sorted_bids.is_empty() {
                let idx = remove_idx % sorted_bids.len();
                let (price, _) = sorted_bids[idx];
                book.apply_delta(Side::Bid, price, FixedSize::ZERO, Sequence::new(1), 1);
                prop_assert!(book.bid_depth() < bid_depth_before);
            }

            let sorted_asks = book.asks_sorted();
            if !sorted_asks.is_empty() {
                let idx = remove_idx % sorted_asks.len();
                let (price, _) = sorted_asks[idx];
                book.apply_delta(Side::Ask, price, FixedSize::ZERO, Sequence::new(2), 2);
                prop_assert!(book.ask_depth() < ask_depth_before);
            }
        }

        /// Applying a snapshot then a sequence of deltas yields a monotonically
        /// increasing sequence number.
        #[test]
        fn sequence_monotonically_increases(
            num_deltas in 1u64..100,
        ) {
            let mut book = L2Book::new(AssetId::new("prop"));
            book.apply_snapshot(&[], &[], Sequence::new(0), 0);

            for i in 1..=num_deltas {
                let price = FixedPrice::new(((i % 100) * 100).min(10_000) as u32).unwrap();
                book.apply_delta(
                    if i % 2 == 0 { Side::Bid } else { Side::Ask },
                    price,
                    FixedSize::new(1_000_000),
                    Sequence::new(i),
                    i * 1000,
                );
                prop_assert_eq!(book.sequence.raw(), i);
            }
        }

        /// Applying the same snapshot twice is idempotent.
        #[test]
        fn snapshot_idempotent(
            bids in prop_vec(arb_level(), 0..30),
            asks in prop_vec(arb_level(), 0..30),
        ) {
            let mut book1 = L2Book::new(AssetId::new("prop"));
            book1.apply_snapshot(&bids, &asks, Sequence::new(1), 1_000_000);

            let mut book2 = L2Book::new(AssetId::new("prop"));
            book2.apply_snapshot(&bids, &asks, Sequence::new(1), 1_000_000);
            book2.apply_snapshot(&bids, &asks, Sequence::new(1), 1_000_000);

            prop_assert_eq!(book1.bids_sorted(), book2.bids_sorted());
            prop_assert_eq!(book1.asks_sorted(), book2.asks_sorted());
        }

        /// Applying a delta to a non-existent level with nonzero size adds exactly one level.
        #[test]
        fn delta_adds_new_level(
            side in arb_side(),
            price in arb_price(),
            size in arb_nonzero_size(),
        ) {
            let mut book = L2Book::new(AssetId::new("prop"));
            let depth_before = match side {
                Side::Bid => book.bid_depth(),
                Side::Ask => book.ask_depth(),
            };
            book.apply_delta(side, price, size, Sequence::new(1), 1_000_000);
            let depth_after = match side {
                Side::Bid => book.bid_depth(),
                Side::Ask => book.ask_depth(),
            };
            prop_assert_eq!(depth_after, depth_before + 1);
        }

        /// Sequence gap detection is sound: only consecutive sequences pass.
        #[test]
        fn sequence_gap_detection(current in 1u64..1_000_000, incoming in 1u64..1_000_000) {
            let mut book = L2Book::new(AssetId::new("prop"));
            book.sequence = Sequence::new(current);
            let result = book.check_sequence(Sequence::new(incoming));
            if incoming == current + 1 {
                prop_assert!(result.is_ok());
            } else {
                prop_assert!(result.is_err());
            }
        }
    }
}
