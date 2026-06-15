use std::cmp::Reverse;
use std::collections::BTreeMap;

use pb_types::{AssetId, FixedPrice, FixedSize, Sequence, Side};

use crate::error::BookError;

/// Level-2 orderbook: price -> aggregate size at that level.
///
/// Bids use `Reverse<FixedPrice>` so iteration yields best (highest) bid first.
/// Asks use `FixedPrice` directly so iteration yields best (lowest) ask first.
///
/// Maintains running totals (`total_bid_raw`, `total_ask_raw`) to avoid O(n)
/// iteration on every `total_bid_size`/`total_ask_size` call.
#[derive(Debug, Clone)]
pub struct L2Book {
    pub asset_id: AssetId,
    pub bids: BTreeMap<Reverse<FixedPrice>, FixedSize>,
    pub asks: BTreeMap<FixedPrice, FixedSize>,
    pub sequence: Sequence,
    pub last_update_us: u64,
    total_bid_raw: u64,
    total_ask_raw: u64,
    /// Whether any snapshot/delta has established a sequence yet. Distinguishes
    /// "no sequence seen" from a legitimate sequence value of 0, so gap detection
    /// is not silently disabled right after a snapshot/checkpoint (A.148).
    seq_initialized: bool,
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
            total_bid_raw: 0,
            total_ask_raw: 0,
            seq_initialized: false,
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
        self.total_bid_raw = 0;
        self.total_ask_raw = 0;

        for &(price, size) in bids {
            if !size.is_zero() {
                let old_raw = self
                    .bids
                    .insert(Reverse(price), size)
                    .map_or(0, |s| s.raw());
                self.total_bid_raw = self.total_bid_raw - old_raw + size.raw();
            }
        }
        for &(price, size) in asks {
            if !size.is_zero() {
                let old_raw = self.asks.insert(price, size).map_or(0, |s| s.raw());
                self.total_ask_raw = self.total_ask_raw - old_raw + size.raw();
            }
        }

        self.sequence = sequence;
        self.seq_initialized = true;
        self.last_update_us = timestamp_us;
    }

    /// Apply a single price-level delta.
    /// If size is zero, the level is removed.
    #[inline]
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
                    if let Some(old) = self.bids.remove(&Reverse(price)) {
                        self.total_bid_raw -= old.raw();
                    }
                } else {
                    let old_raw = self
                        .bids
                        .insert(Reverse(price), size)
                        .map_or(0, |s| s.raw());
                    self.total_bid_raw = self.total_bid_raw - old_raw + size.raw();
                }
            }
            Side::Ask => {
                if size.is_zero() {
                    if let Some(old) = self.asks.remove(&price) {
                        self.total_ask_raw -= old.raw();
                    }
                } else {
                    let old_raw = self.asks.insert(price, size).map_or(0, |s| s.raw());
                    self.total_ask_raw = self.total_ask_raw - old_raw + size.raw();
                }
            }
        }

        self.sequence = sequence;
        self.seq_initialized = true;
        self.last_update_us = timestamp_us;
    }

    /// Best (highest) bid price and size.
    #[inline]
    pub fn best_bid(&self) -> Option<(FixedPrice, FixedSize)> {
        self.bids.iter().next().map(|(Reverse(p), &s)| (*p, s))
    }

    /// Best (lowest) ask price and size.
    #[inline]
    pub fn best_ask(&self) -> Option<(FixedPrice, FixedSize)> {
        self.asks.iter().next().map(|(p, &s)| (*p, s))
    }

    /// Mid price = (best_bid + best_ask) / 2, as f64.
    #[inline]
    pub fn mid_price(&self) -> Option<f64> {
        match (self.best_bid(), self.best_ask()) {
            (Some((bid, _)), Some((ask, _))) => Some((bid.as_f64() + ask.as_f64()) / 2.0),
            _ => None,
        }
    }

    /// Spread = best_ask - best_bid, as f64.
    #[inline]
    pub fn spread(&self) -> Option<f64> {
        match (self.best_bid(), self.best_ask()) {
            (Some((bid, _)), Some((ask, _))) => Some(ask.as_f64() - bid.as_f64()),
            _ => None,
        }
    }

    /// Number of bid levels.
    #[inline]
    pub fn bid_depth(&self) -> usize {
        self.bids.len()
    }

    /// Number of ask levels.
    #[inline]
    pub fn ask_depth(&self) -> usize {
        self.asks.len()
    }

    /// Check if there's a sequence gap.
    ///
    /// Gap detection is active once any snapshot/delta has established a
    /// sequence, including when that sequence is 0 — previously the `> 0`
    /// sentinel disabled detection exactly post-snapshot/post-checkpoint where
    /// `sequence == 0` is legitimate (A.148).
    pub fn check_sequence(&self, incoming: Sequence) -> Result<(), BookError> {
        if self.seq_initialized && incoming.raw() != self.sequence.raw() + 1 {
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

    /// Verify structural invariants: no crossed book.
    /// Returns `Ok(())` if the book is internally consistent.
    pub fn check_integrity(&self) -> Result<(), BookError> {
        if let (Some((bid, _)), Some((ask, _))) = (self.best_bid(), self.best_ask()) {
            if bid >= ask {
                return Err(BookError::CrossedBook {
                    asset_id: self.asset_id.to_string(),
                    best_bid: bid.to_string(),
                    best_ask: ask.to_string(),
                });
            }
        }
        Ok(())
    }

    /// Total size across all bid levels. O(1) via maintained running sum.
    #[inline]
    pub fn total_bid_size(&self) -> FixedSize {
        FixedSize::new(self.total_bid_raw)
    }

    /// Total size across all ask levels. O(1) via maintained running sum.
    #[inline]
    pub fn total_ask_size(&self) -> FixedSize {
        FixedSize::new(self.total_ask_raw)
    }

    /// Size-weighted mid price: accounts for liquidity imbalance at top of book.
    /// Returns `None` if either side is empty.
    #[inline]
    pub fn weighted_mid_price(&self) -> Option<f64> {
        match (self.best_bid(), self.best_ask()) {
            (Some((bid_p, bid_s)), Some((ask_p, ask_s))) => {
                let bid_f = bid_p.as_f64();
                let ask_f = ask_p.as_f64();
                let bid_sz = bid_s.as_f64();
                let ask_sz = ask_s.as_f64();
                let total = bid_sz + ask_sz;
                if total == 0.0 {
                    return Some((bid_f + ask_f) / 2.0);
                }
                Some((bid_f * ask_sz + ask_f * bid_sz) / total)
            }
            _ => None,
        }
    }

    /// Top-N bids sorted best-to-worst.
    pub fn top_bids(&self, n: usize) -> Vec<(FixedPrice, FixedSize)> {
        self.bids
            .iter()
            .take(n)
            .map(|(Reverse(p), &s)| (*p, s))
            .collect()
    }

    /// Top-N asks sorted best-to-worst (lowest first).
    pub fn top_asks(&self, n: usize) -> Vec<(FixedPrice, FixedSize)> {
        self.asks.iter().take(n).map(|(p, &s)| (*p, s)).collect()
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
    fn test_total_sizes() {
        let book = make_book();
        let total_bid = book.total_bid_size();
        // 100 + 200 + 300 = 600.0 → 600_000_000 raw
        assert_eq!(total_bid.raw(), 600_000_000);
        let total_ask = book.total_ask_size();
        // 150 + 250 = 400.0 → 400_000_000 raw
        assert_eq!(total_ask.raw(), 400_000_000);
    }

    #[test]
    fn test_weighted_mid_price() {
        let book = make_book();
        let wmid = book.weighted_mid_price().unwrap();
        // best_bid=0.50 size=100, best_ask=0.55 size=150
        // wmid = (0.50 * 150 + 0.55 * 100) / (100 + 150)
        //      = (75.0 + 55.0) / 250.0 = 130.0 / 250.0 = 0.52
        assert!((wmid - 0.52).abs() < 1e-6);
    }

    #[test]
    fn test_top_n() {
        let book = make_book();
        let top2_bids = book.top_bids(2);
        assert_eq!(top2_bids.len(), 2);
        assert_eq!(top2_bids[0].0.raw(), 5000);
        assert_eq!(top2_bids[1].0.raw(), 4900);

        let top1_asks = book.top_asks(1);
        assert_eq!(top1_asks.len(), 1);
        assert_eq!(top1_asks[0].0.raw(), 5500);
    }

    #[test]
    fn test_check_integrity_valid() {
        let book = make_book();
        assert!(book.check_integrity().is_ok());
    }

    #[test]
    fn test_check_integrity_crossed() {
        let mut book = L2Book::new(AssetId::new("test"));
        book.apply_snapshot(
            &[(
                FixedPrice::from_f64(0.60).unwrap(),
                FixedSize::from_f64(100.0).unwrap(),
            )],
            &[(
                FixedPrice::from_f64(0.50).unwrap(),
                FixedSize::from_f64(100.0).unwrap(),
            )],
            Sequence::new(1),
            1_000_000,
        );
        assert!(book.check_integrity().is_err());
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

    // --- Empty book exhaustive tests ---

    #[test]
    fn empty_book_total_sizes_zero() {
        let book = L2Book::new(AssetId::new("empty"));
        assert_eq!(book.total_bid_size().raw(), 0);
        assert_eq!(book.total_ask_size().raw(), 0);
    }

    #[test]
    fn empty_book_weighted_mid_none() {
        let book = L2Book::new(AssetId::new("empty"));
        assert!(book.weighted_mid_price().is_none());
    }

    #[test]
    fn empty_book_depths_zero() {
        let book = L2Book::new(AssetId::new("empty"));
        assert_eq!(book.bid_depth(), 0);
        assert_eq!(book.ask_depth(), 0);
    }

    #[test]
    fn empty_book_top_n_returns_empty() {
        let book = L2Book::new(AssetId::new("empty"));
        assert!(book.top_bids(10).is_empty());
        assert!(book.top_asks(10).is_empty());
    }

    #[test]
    fn empty_book_sorted_returns_empty() {
        let book = L2Book::new(AssetId::new("empty"));
        assert!(book.bids_sorted().is_empty());
        assert!(book.asks_sorted().is_empty());
    }

    #[test]
    fn empty_book_check_integrity_ok() {
        let book = L2Book::new(AssetId::new("empty"));
        assert!(book.check_integrity().is_ok());
    }

    #[test]
    fn empty_book_default_sequence() {
        let book = L2Book::new(AssetId::new("empty"));
        assert_eq!(book.sequence.raw(), 0);
    }

    // --- Single level tests ---

    #[test]
    fn single_bid_level() {
        let mut book = L2Book::new(AssetId::new("single"));
        book.apply_snapshot(
            &[(FixedPrice::new(5000).unwrap(), FixedSize::new(100_000_000))],
            &[],
            Sequence::new(1),
            1000,
        );
        assert_eq!(book.bid_depth(), 1);
        assert_eq!(book.ask_depth(), 0);
        let (p, s) = book.best_bid().unwrap();
        assert_eq!(p.raw(), 5000);
        assert_eq!(s.raw(), 100_000_000);
        assert!(book.best_ask().is_none());
        assert!(book.mid_price().is_none());
        assert!(book.spread().is_none());
        assert!(book.weighted_mid_price().is_none());
        assert_eq!(book.total_bid_size().raw(), 100_000_000);
        assert_eq!(book.total_ask_size().raw(), 0);
    }

    #[test]
    fn single_ask_level() {
        let mut book = L2Book::new(AssetId::new("single"));
        book.apply_snapshot(
            &[],
            &[(FixedPrice::new(6000).unwrap(), FixedSize::new(200_000_000))],
            Sequence::new(1),
            1000,
        );
        assert_eq!(book.bid_depth(), 0);
        assert_eq!(book.ask_depth(), 1);
        assert!(book.best_bid().is_none());
        let (p, s) = book.best_ask().unwrap();
        assert_eq!(p.raw(), 6000);
        assert_eq!(s.raw(), 200_000_000);
        assert!(book.mid_price().is_none());
        assert_eq!(book.total_ask_size().raw(), 200_000_000);
    }

    #[test]
    fn single_bid_and_ask() {
        let mut book = L2Book::new(AssetId::new("single"));
        book.apply_snapshot(
            &[(FixedPrice::new(4000).unwrap(), FixedSize::new(50_000_000))],
            &[(FixedPrice::new(6000).unwrap(), FixedSize::new(50_000_000))],
            Sequence::new(1),
            1000,
        );
        let mid = book.mid_price().unwrap();
        assert!((mid - 0.5).abs() < 1e-6);
        let spread = book.spread().unwrap();
        assert!((spread - 0.2).abs() < 1e-6);
        assert!(book.check_integrity().is_ok());
    }

    // --- Snapshot with duplicate prices (last wins) ---

    #[test]
    fn snapshot_duplicate_bid_prices_last_wins() {
        let mut book = L2Book::new(AssetId::new("dup"));
        book.apply_snapshot(
            &[
                (FixedPrice::new(5000).unwrap(), FixedSize::new(100_000_000)),
                (FixedPrice::new(5000).unwrap(), FixedSize::new(200_000_000)),
            ],
            &[],
            Sequence::new(1),
            1000,
        );
        // Only one level should exist
        assert_eq!(book.bid_depth(), 1);
        let (_, size) = book.best_bid().unwrap();
        assert_eq!(size.raw(), 200_000_000);
        // Total size must reflect final value, not sum
        assert_eq!(book.total_bid_size().raw(), 200_000_000);
    }

    #[test]
    fn snapshot_duplicate_ask_prices_last_wins() {
        let mut book = L2Book::new(AssetId::new("dup"));
        book.apply_snapshot(
            &[],
            &[
                (FixedPrice::new(6000).unwrap(), FixedSize::new(100_000_000)),
                (FixedPrice::new(6000).unwrap(), FixedSize::new(300_000_000)),
            ],
            Sequence::new(1),
            1000,
        );
        assert_eq!(book.ask_depth(), 1);
        assert_eq!(book.total_ask_size().raw(), 300_000_000);
    }

    #[test]
    fn snapshot_many_duplicates_total_size_correct() {
        let mut book = L2Book::new(AssetId::new("dup"));
        // 5 entries at same price, sizes 10, 20, 30, 40, 50 — last wins → 50
        let bids: Vec<_> = (1..=5)
            .map(|i| {
                (
                    FixedPrice::new(5000).unwrap(),
                    FixedSize::new(i * 10_000_000),
                )
            })
            .collect();
        book.apply_snapshot(&bids, &[], Sequence::new(1), 1000);
        assert_eq!(book.bid_depth(), 1);
        assert_eq!(book.total_bid_size().raw(), 50_000_000);
    }

    // --- Delta: removing all levels then adding back ---

    #[test]
    fn delta_remove_all_then_add_back() {
        let mut book = make_book();
        // Remove all 3 bid levels
        book.apply_delta(
            Side::Bid,
            FixedPrice::new(5000).unwrap(),
            FixedSize::ZERO,
            Sequence::new(2),
            2000,
        );
        book.apply_delta(
            Side::Bid,
            FixedPrice::new(4900).unwrap(),
            FixedSize::ZERO,
            Sequence::new(3),
            3000,
        );
        book.apply_delta(
            Side::Bid,
            FixedPrice::new(4800).unwrap(),
            FixedSize::ZERO,
            Sequence::new(4),
            4000,
        );
        assert_eq!(book.bid_depth(), 0);
        assert_eq!(book.total_bid_size().raw(), 0);
        assert!(book.best_bid().is_none());

        // Remove all 2 ask levels
        book.apply_delta(
            Side::Ask,
            FixedPrice::new(5500).unwrap(),
            FixedSize::ZERO,
            Sequence::new(5),
            5000,
        );
        book.apply_delta(
            Side::Ask,
            FixedPrice::new(5600).unwrap(),
            FixedSize::ZERO,
            Sequence::new(6),
            6000,
        );
        assert_eq!(book.ask_depth(), 0);
        assert_eq!(book.total_ask_size().raw(), 0);
        assert!(book.best_ask().is_none());

        // Book is now empty
        assert!(book.mid_price().is_none());
        assert!(book.check_integrity().is_ok());

        // Add levels back
        book.apply_delta(
            Side::Bid,
            FixedPrice::new(3000).unwrap(),
            FixedSize::new(1_000_000),
            Sequence::new(7),
            7000,
        );
        book.apply_delta(
            Side::Ask,
            FixedPrice::new(7000).unwrap(),
            FixedSize::new(2_000_000),
            Sequence::new(8),
            8000,
        );
        assert_eq!(book.bid_depth(), 1);
        assert_eq!(book.ask_depth(), 1);
        assert_eq!(book.total_bid_size().raw(), 1_000_000);
        assert_eq!(book.total_ask_size().raw(), 2_000_000);
        assert!(book.check_integrity().is_ok());
    }

    // --- Delta on empty book ---

    #[test]
    fn delta_on_empty_book() {
        let mut book = L2Book::new(AssetId::new("empty-delta"));
        book.apply_delta(
            Side::Bid,
            FixedPrice::new(5000).unwrap(),
            FixedSize::new(1_000_000),
            Sequence::new(1),
            1000,
        );
        assert_eq!(book.bid_depth(), 1);
        assert_eq!(book.total_bid_size().raw(), 1_000_000);
    }

    // --- Delta removing nonexistent level (no-op) ---

    #[test]
    fn delta_remove_nonexistent_is_noop() {
        let mut book = make_book();
        let bid_depth = book.bid_depth();
        let total = book.total_bid_size().raw();
        // Try to remove a price that doesn't exist
        book.apply_delta(
            Side::Bid,
            FixedPrice::new(1000).unwrap(),
            FixedSize::ZERO,
            Sequence::new(2),
            2000,
        );
        assert_eq!(book.bid_depth(), bid_depth);
        assert_eq!(book.total_bid_size().raw(), total);
    }

    // --- Delta overwriting same price updates total correctly ---

    #[test]
    fn delta_overwrite_updates_total_size() {
        let mut book = make_book();
        let original_total = book.total_bid_size().raw(); // 600_000_000
                                                          // Overwrite 0.50 bid from 100 → 500
        book.apply_delta(
            Side::Bid,
            FixedPrice::new(5000).unwrap(),
            FixedSize::new(500_000_000),
            Sequence::new(2),
            2000,
        );
        // Delta: +400_000_000 (500 - 100)
        assert_eq!(
            book.total_bid_size().raw(),
            original_total - 100_000_000 + 500_000_000
        );
        assert_eq!(book.bid_depth(), 3); // same number of levels
    }

    // --- top_bids/top_asks requesting more than available ---

    #[test]
    fn top_bids_more_than_available() {
        let book = make_book();
        let top = book.top_bids(100);
        assert_eq!(top.len(), 3); // only 3 bid levels
    }

    #[test]
    fn top_asks_more_than_available() {
        let book = make_book();
        let top = book.top_asks(100);
        assert_eq!(top.len(), 2); // only 2 ask levels
    }

    #[test]
    fn top_bids_zero() {
        let book = make_book();
        assert!(book.top_bids(0).is_empty());
    }

    #[test]
    fn top_asks_zero() {
        let book = make_book();
        assert!(book.top_asks(0).is_empty());
    }

    // --- check_integrity edge cases ---

    #[test]
    fn check_integrity_equal_bid_ask() {
        let mut book = L2Book::new(AssetId::new("equal"));
        book.apply_snapshot(
            &[(FixedPrice::new(5000).unwrap(), FixedSize::new(1_000_000))],
            &[(FixedPrice::new(5000).unwrap(), FixedSize::new(1_000_000))],
            Sequence::new(1),
            1000,
        );
        // bid == ask is considered crossed
        assert!(book.check_integrity().is_err());
    }

    #[test]
    fn check_integrity_bid_just_below_ask() {
        let mut book = L2Book::new(AssetId::new("tight"));
        book.apply_snapshot(
            &[(FixedPrice::new(4999).unwrap(), FixedSize::new(1_000_000))],
            &[(FixedPrice::new(5000).unwrap(), FixedSize::new(1_000_000))],
            Sequence::new(1),
            1000,
        );
        // 1 tick spread — valid
        assert!(book.check_integrity().is_ok());
    }

    #[test]
    fn check_integrity_error_fields() {
        let mut book = L2Book::new(AssetId::new("my-asset"));
        book.apply_snapshot(
            &[(FixedPrice::new(6000).unwrap(), FixedSize::new(1_000_000))],
            &[(FixedPrice::new(4000).unwrap(), FixedSize::new(1_000_000))],
            Sequence::new(1),
            1000,
        );
        let err = book.check_integrity().unwrap_err();
        match err {
            BookError::CrossedBook {
                asset_id,
                best_bid,
                best_ask,
            } => {
                assert_eq!(asset_id, "my-asset");
                assert_eq!(best_bid, "0.6000");
                assert_eq!(best_ask, "0.4000");
            }
            other => panic!("expected CrossedBook, got: {other}"),
        }
    }

    // --- check_sequence edge cases ---

    #[test]
    fn check_sequence_at_zero_accepts_any() {
        let book = L2Book::new(AssetId::new("seq"));
        // sequence is 0, so any incoming should pass
        assert!(book.check_sequence(Sequence::new(1)).is_ok());
        assert!(book.check_sequence(Sequence::new(100)).is_ok());
    }

    #[test]
    fn check_sequence_gap_error_fields() {
        let mut book = L2Book::new(AssetId::new("my-asset"));
        book.sequence = Sequence::new(10);
        book.seq_initialized = true;
        let err = book.check_sequence(Sequence::new(15)).unwrap_err();
        match err {
            BookError::SequenceGap {
                asset_id,
                expected,
                got,
                gap_size,
            } => {
                assert_eq!(asset_id, "my-asset");
                assert_eq!(expected, 11);
                assert_eq!(got, 15);
                assert_eq!(gap_size, 4);
            }
            other => panic!("expected SequenceGap, got: {other}"),
        }
    }

    #[test]
    fn check_sequence_duplicate_is_gap() {
        let mut book = L2Book::new(AssetId::new("seq"));
        book.sequence = Sequence::new(5);
        book.seq_initialized = true;
        // Receiving the same sequence number is a "gap" (got 5, expected 6)
        assert!(book.check_sequence(Sequence::new(5)).is_err());
    }

    #[test]
    fn check_sequence_backwards_is_gap() {
        let mut book = L2Book::new(AssetId::new("seq"));
        book.sequence = Sequence::new(10);
        book.seq_initialized = true;
        assert!(book.check_sequence(Sequence::new(3)).is_err());
    }

    #[test]
    fn check_sequence_detects_gap_right_after_snapshot_at_zero() {
        // A snapshot establishes sequence 0; a first delta of 5 (not 1) is a gap
        // that the old `> 0` sentinel silently ignored (A.148).
        let mut book = L2Book::new(AssetId::new("seq"));
        book.apply_snapshot(&[], &[], Sequence::new(0), 1_000);
        assert!(book.check_sequence(Sequence::new(1)).is_ok());
        assert!(book.check_sequence(Sequence::new(5)).is_err());
    }

    // --- Weighted mid price edge cases ---

    #[test]
    fn weighted_mid_price_symmetric_sizes() {
        let mut book = L2Book::new(AssetId::new("wmid"));
        book.apply_snapshot(
            &[(FixedPrice::new(4000).unwrap(), FixedSize::new(100_000_000))],
            &[(FixedPrice::new(6000).unwrap(), FixedSize::new(100_000_000))],
            Sequence::new(1),
            1000,
        );
        // Equal sizes → wmid = simple mid
        let wmid = book.weighted_mid_price().unwrap();
        let mid = book.mid_price().unwrap();
        assert!((wmid - mid).abs() < 1e-10);
    }

    #[test]
    fn weighted_mid_price_skewed_towards_bid() {
        let mut book = L2Book::new(AssetId::new("wmid"));
        book.apply_snapshot(
            &[(FixedPrice::new(4000).unwrap(), FixedSize::new(1_000_000))],
            &[(
                FixedPrice::new(6000).unwrap(),
                FixedSize::new(1_000_000_000),
            )],
            Sequence::new(1),
            1000,
        );
        // ask size >> bid size → wmid closer to bid
        let wmid = book.weighted_mid_price().unwrap();
        let mid = book.mid_price().unwrap();
        assert!(wmid < mid);
    }

    #[test]
    fn weighted_mid_price_skewed_towards_ask() {
        let mut book = L2Book::new(AssetId::new("wmid"));
        book.apply_snapshot(
            &[(
                FixedPrice::new(4000).unwrap(),
                FixedSize::new(1_000_000_000),
            )],
            &[(FixedPrice::new(6000).unwrap(), FixedSize::new(1_000_000))],
            Sequence::new(1),
            1000,
        );
        // bid size >> ask size → wmid closer to ask
        let wmid = book.weighted_mid_price().unwrap();
        let mid = book.mid_price().unwrap();
        assert!(wmid > mid);
    }

    // --- Snapshot with zero-size entries ---

    #[test]
    fn snapshot_zero_size_entries_skipped() {
        let mut book = L2Book::new(AssetId::new("zero"));
        book.apply_snapshot(
            &[
                (FixedPrice::new(5000).unwrap(), FixedSize::ZERO),
                (FixedPrice::new(4000).unwrap(), FixedSize::new(100_000_000)),
            ],
            &[
                (FixedPrice::new(6000).unwrap(), FixedSize::ZERO),
                (FixedPrice::new(7000).unwrap(), FixedSize::new(200_000_000)),
            ],
            Sequence::new(1),
            1000,
        );
        assert_eq!(book.bid_depth(), 1); // zero-size bid skipped
        assert_eq!(book.ask_depth(), 1); // zero-size ask skipped
        assert_eq!(book.total_bid_size().raw(), 100_000_000);
        assert_eq!(book.total_ask_size().raw(), 200_000_000);
    }

    // --- Total size tracking through complex sequences ---

    #[test]
    fn total_size_through_snapshot_delta_snapshot() {
        let mut book = L2Book::new(AssetId::new("complex"));
        // Snapshot 1
        book.apply_snapshot(
            &[(FixedPrice::new(5000).unwrap(), FixedSize::new(100_000_000))],
            &[(FixedPrice::new(6000).unwrap(), FixedSize::new(200_000_000))],
            Sequence::new(1),
            1000,
        );
        assert_eq!(book.total_bid_size().raw(), 100_000_000);

        // Delta: add a second bid level
        book.apply_delta(
            Side::Bid,
            FixedPrice::new(4500).unwrap(),
            FixedSize::new(50_000_000),
            Sequence::new(2),
            2000,
        );
        assert_eq!(book.total_bid_size().raw(), 150_000_000);

        // Delta: update first bid level
        book.apply_delta(
            Side::Bid,
            FixedPrice::new(5000).unwrap(),
            FixedSize::new(300_000_000),
            Sequence::new(3),
            3000,
        );
        assert_eq!(book.total_bid_size().raw(), 350_000_000);

        // Delta: remove second bid level
        book.apply_delta(
            Side::Bid,
            FixedPrice::new(4500).unwrap(),
            FixedSize::ZERO,
            Sequence::new(4),
            4000,
        );
        assert_eq!(book.total_bid_size().raw(), 300_000_000);

        // Snapshot 2: should reset everything
        book.apply_snapshot(
            &[(FixedPrice::new(3000).unwrap(), FixedSize::new(10_000_000))],
            &[(FixedPrice::new(8000).unwrap(), FixedSize::new(20_000_000))],
            Sequence::new(5),
            5000,
        );
        assert_eq!(book.total_bid_size().raw(), 10_000_000);
        assert_eq!(book.total_ask_size().raw(), 20_000_000);
        assert_eq!(book.bid_depth(), 1);
    }

    // --- Timestamp and sequence tracking ---

    #[test]
    fn last_update_us_tracks_latest() {
        let mut book = L2Book::new(AssetId::new("ts"));
        assert_eq!(book.last_update_us, 0);
        book.apply_snapshot(&[], &[], Sequence::new(1), 100);
        assert_eq!(book.last_update_us, 100);
        book.apply_delta(
            Side::Bid,
            FixedPrice::new(5000).unwrap(),
            FixedSize::new(1),
            Sequence::new(2),
            200,
        );
        assert_eq!(book.last_update_us, 200);
    }

    // --- BookError Display ---

    #[test]
    fn book_error_display_messages() {
        let err = BookError::SequenceGap {
            asset_id: "tok".to_string(),
            expected: 5,
            got: 10,
            gap_size: 5,
        };
        let msg = format!("{err}");
        assert!(msg.contains("tok"));
        assert!(msg.contains("5"));
        assert!(msg.contains("10"));

        let err = BookError::CrossedBook {
            asset_id: "tok".to_string(),
            best_bid: "0.6000".to_string(),
            best_ask: "0.4000".to_string(),
        };
        let msg = format!("{err}");
        assert!(msg.contains("crossed"));
        assert!(msg.contains("0.6000"));

        let err = BookError::InvalidPrice {
            asset_id: "tok".to_string(),
            price: "bad".to_string(),
            side: "Bid".to_string(),
        };
        let msg = format!("{err}");
        assert!(msg.contains("invalid price"));

        let err = BookError::UnknownSide {
            asset_id: "tok".to_string(),
            raw: "SELL".to_string(),
        };
        let msg = format!("{err}");
        assert!(msg.contains("unknown side"));
    }

    // --- Clone ---

    #[test]
    fn book_clone_is_independent() {
        let book = make_book();
        let mut cloned = book.clone();
        cloned.apply_delta(
            Side::Bid,
            FixedPrice::new(5000).unwrap(),
            FixedSize::ZERO,
            Sequence::new(2),
            2000,
        );
        // Original unchanged
        assert_eq!(book.bid_depth(), 3);
        assert_eq!(cloned.bid_depth(), 2);
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

        /// `check_integrity` reports a crossed book exactly when
        /// `best_bid >= best_ask`. Bids and asks are drawn from the SAME
        /// overlapping price range so crossings actually occur — the
        /// disjoint-range tests above can never produce one, leaving the
        /// detector untested (audit finding A.159).
        #[test]
        fn check_integrity_detects_crossings(
            bid_prices in prop_vec(1u32..=10_000u32, 1..20),
            ask_prices in prop_vec(1u32..=10_000u32, 1..20),
        ) {
            let bids: Vec<_> = bid_prices.iter().map(|&p| {
                (FixedPrice::new(p).unwrap(), FixedSize::new(1_000_000))
            }).collect();
            let asks: Vec<_> = ask_prices.iter().map(|&p| {
                (FixedPrice::new(p).unwrap(), FixedSize::new(1_000_000))
            }).collect();

            let mut book = L2Book::new(AssetId::new("prop"));
            book.apply_snapshot(&bids, &asks, Sequence::new(1), 1_000_000);

            let crossed = match (book.best_bid(), book.best_ask()) {
                (Some((b, _)), Some((a, _))) => b >= a,
                _ => false,
            };
            prop_assert_eq!(
                book.check_integrity().is_err(),
                crossed,
                "check_integrity disagreed with best_bid>=best_ask"
            );
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
            book.seq_initialized = true;
            let result = book.check_sequence(Sequence::new(incoming));
            if incoming == current + 1 {
                prop_assert!(result.is_ok());
            } else {
                prop_assert!(result.is_err());
            }
        }

        /// check_integrity detects crossed books (best_bid >= best_ask).
        #[test]
        fn integrity_detects_crossed_book(
            bid_price in 5001u32..=10_000u32,
            ask_price in 1u32..=5000u32,
        ) {
            let mut book = L2Book::new(AssetId::new("prop"));
            book.apply_snapshot(
                &[(FixedPrice::new(bid_price).unwrap(), FixedSize::new(1_000_000))],
                &[(FixedPrice::new(ask_price).unwrap(), FixedSize::new(1_000_000))],
                Sequence::new(1),
                1_000_000,
            );
            prop_assert!(book.check_integrity().is_err());
        }

        /// check_integrity also catches equal bid/ask.
        #[test]
        fn integrity_detects_equal_bid_ask(
            price in 1u32..=10_000u32,
        ) {
            let mut book = L2Book::new(AssetId::new("prop"));
            book.apply_snapshot(
                &[(FixedPrice::new(price).unwrap(), FixedSize::new(1_000_000))],
                &[(FixedPrice::new(price).unwrap(), FixedSize::new(1_000_000))],
                Sequence::new(1),
                1_000_000,
            );
            prop_assert!(book.check_integrity().is_err());
        }

        /// After any number of deltas, total_bid_size equals brute-force sum.
        #[test]
        fn total_size_after_deltas(
            initial_bids in prop_vec(arb_level(), 0..20),
            initial_asks in prop_vec(arb_level(), 0..20),
            deltas in prop_vec((arb_side(), arb_price(), 0u64..=500_000_000u64), 1..50),
        ) {
            let mut book = L2Book::new(AssetId::new("prop"));
            book.apply_snapshot(&initial_bids, &initial_asks, Sequence::new(0), 0);

            for (i, (side, price, raw_size)) in deltas.iter().enumerate() {
                book.apply_delta(*side, *price, FixedSize::new(*raw_size), Sequence::new(i as u64 + 1), (i as u64 + 1) * 1000);
            }

            let manual_bid_sum: u64 = book.bids_sorted().iter().map(|(_, s)| s.raw()).sum();
            prop_assert_eq!(book.total_bid_size().raw(), manual_bid_sum, "bid size mismatch");

            let manual_ask_sum: u64 = book.asks_sorted().iter().map(|(_, s)| s.raw()).sum();
            prop_assert_eq!(book.total_ask_size().raw(), manual_ask_sum, "ask size mismatch");
        }

        /// Snapshot with duplicate prices: total size equals sum of unique final levels.
        #[test]
        fn snapshot_duplicates_total_size_correct(
            entries in prop_vec((1u32..=100u32, 1u64..=1_000_000u64), 1..50),
        ) {
            // Build bid levels with possible duplicate prices
            let bids: Vec<_> = entries.iter().map(|&(p, s)| {
                (FixedPrice::new(p * 100).unwrap(), FixedSize::new(s))
            }).collect();

            let mut book = L2Book::new(AssetId::new("prop"));
            book.apply_snapshot(&bids, &[], Sequence::new(1), 1_000_000);

            let manual_sum: u64 = book.bids_sorted().iter().map(|(_, s)| s.raw()).sum();
            prop_assert_eq!(book.total_bid_size().raw(), manual_sum);
        }

        /// top_bids(n) returns at most n levels, and they are in descending price order.
        #[test]
        fn top_bids_bounded_and_ordered(
            bids in prop_vec(arb_level(), 0..50),
            n in 0usize..=60,
        ) {
            let mut book = L2Book::new(AssetId::new("prop"));
            book.apply_snapshot(&bids, &[], Sequence::new(1), 1_000_000);
            let top = book.top_bids(n);
            prop_assert!(top.len() <= n);
            prop_assert!(top.len() <= book.bid_depth());
            for w in top.windows(2) {
                prop_assert!(w[0].0 >= w[1].0);
            }
        }

        /// top_asks(n) returns at most n levels, and they are in ascending price order.
        #[test]
        fn top_asks_bounded_and_ordered(
            asks in prop_vec(arb_level(), 0..50),
            n in 0usize..=60,
        ) {
            let mut book = L2Book::new(AssetId::new("prop"));
            book.apply_snapshot(&[], &asks, Sequence::new(1), 1_000_000);
            let top = book.top_asks(n);
            prop_assert!(top.len() <= n);
            prop_assert!(top.len() <= book.ask_depth());
            for w in top.windows(2) {
                prop_assert!(w[0].0 <= w[1].0);
            }
        }

        /// Weighted mid price is bounded by best bid and best ask.
        #[test]
        fn weighted_mid_bounded(
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

            if let (Some((best_bid, _)), Some((best_ask, _)), Some(wmid)) =
                (book.best_bid(), book.best_ask(), book.weighted_mid_price())
            {
                prop_assert!(wmid >= best_bid.as_f64(), "wmid {} < bid {}", wmid, best_bid.as_f64());
                prop_assert!(wmid <= best_ask.as_f64(), "wmid {} > ask {}", wmid, best_ask.as_f64());
            }
        }

        /// total_bid_size equals sum of all bid levels.
        #[test]
        fn total_size_consistent(
            bids in prop_vec(arb_level(), 0..50),
            asks in prop_vec(arb_level(), 0..50),
        ) {
            let mut book = L2Book::new(AssetId::new("prop"));
            book.apply_snapshot(&bids, &asks, Sequence::new(1), 1_000_000);

            let manual_bid_sum: u64 = book.bids_sorted().iter().map(|(_, s)| s.raw()).sum();
            prop_assert_eq!(book.total_bid_size().raw(), manual_bid_sum);

            let manual_ask_sum: u64 = book.asks_sorted().iter().map(|(_, s)| s.raw()).sum();
            prop_assert_eq!(book.total_ask_size().raw(), manual_ask_sum);
        }
    }
}
