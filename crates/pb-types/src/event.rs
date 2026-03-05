use serde::{Deserialize, Serialize};

use crate::fixed::{FixedPrice, FixedSize};
use crate::newtype::{AssetId, Sequence};

/// Bid or Ask side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Side {
    Bid,
    Ask,
}

impl std::fmt::Display for Side {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Side::Bid => write!(f, "Bid"),
            Side::Ask => write!(f, "Ask"),
        }
    }
}

/// Type of orderbook event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EventType {
    Snapshot,
    Delta,
    Trade,
}

/// A normalized orderbook event ready for storage and replay.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderbookEvent {
    /// Microseconds since epoch when we received this event.
    pub recv_timestamp_us: u64,
    /// Microseconds since epoch from the exchange (if available).
    pub exchange_timestamp_us: u64,
    /// The asset (token) this event belongs to.
    pub asset_id: AssetId,
    /// Snapshot, Delta, or Trade.
    pub event_type: EventType,
    /// Bid or Ask (None for trades without side info).
    pub side: Option<Side>,
    /// Price level (as FixedPrice raw u32).
    pub price: FixedPrice,
    /// Size at this price level (as FixedSize raw u64).
    pub size: FixedSize,
    /// Sequence number for gap detection.
    pub sequence: Sequence,
}

/// A price-size level used in snapshots and deltas.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriceLevel {
    pub price: FixedPrice,
    pub size: FixedSize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_serde() {
        let event = OrderbookEvent {
            recv_timestamp_us: 1_000_000,
            exchange_timestamp_us: 999_000,
            asset_id: AssetId::new("test-token"),
            event_type: EventType::Delta,
            side: Some(Side::Bid),
            price: FixedPrice::from_f64(0.55).unwrap(),
            size: FixedSize::from_f64(100.0).unwrap(),
            sequence: Sequence::new(42),
        };
        let json = serde_json::to_string(&event).unwrap();
        let event2: OrderbookEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event.sequence, event2.sequence);
        assert_eq!(event.price, event2.price);
    }

    #[test]
    fn test_side_display() {
        assert_eq!(format!("{}", Side::Bid), "Bid");
        assert_eq!(format!("{}", Side::Ask), "Ask");
    }
}
