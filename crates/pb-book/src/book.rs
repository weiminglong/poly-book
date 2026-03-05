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
            return Err(BookError::SequenceGap {
                expected: self.sequence.raw() + 1,
                got: incoming.raw(),
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
