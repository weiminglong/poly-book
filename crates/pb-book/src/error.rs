use thiserror::Error;

#[derive(Debug, Error)]
pub enum BookError {
    #[error("orderbook {asset_id}: invalid price {price} on {side} side")]
    InvalidPrice {
        asset_id: String,
        price: String,
        side: String,
    },

    #[error("orderbook {asset_id}: unknown side '{raw}'")]
    UnknownSide { asset_id: String, raw: String },

    #[error("orderbook {asset_id}: sequence gap {expected} -> {got} (dropped {gap_size} updates)")]
    SequenceGap {
        asset_id: String,
        expected: u64,
        got: u64,
        gap_size: u64,
    },

    #[error(
        "orderbook {asset_id}: crossed book detected, best_bid={best_bid} >= best_ask={best_ask}"
    )]
    CrossedBook {
        asset_id: String,
        best_bid: String,
        best_ask: String,
    },
}
