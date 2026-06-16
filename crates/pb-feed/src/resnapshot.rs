//! REST resnapshot worker (audit finding A.74 self-healing).
//!
//! When the dispatcher detects that our reconstructed book diverged from the
//! venue-stated best bid/ask, it requests a resnapshot for that asset. This
//! worker fetches a fresh REST book snapshot and re-injects it into the feed as a
//! synthetic WS `book` message, so the dispatcher's normal snapshot path rebuilds
//! the shadow book, resets the sequence, and emits a fresh snapshot — recovering
//! from the divergence without operator action.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use pb_types::wire::RestBookResponse;

use crate::rest::RestClient;
use crate::ws::{FeedMessage, WsRawMessage};

/// Minimum interval between REST resnapshots for the *same* asset, so a burst of
/// diverging deltas cannot hammer the REST endpoint.
const RESNAPSHOT_DEBOUNCE: Duration = Duration::from_secs(5);

fn now_micros() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(0)
}

/// Convert a REST book response into a synthetic WS `book` raw message, so it can
/// be re-injected into the dispatcher and handled by the existing snapshot path
/// (staleness/hash-dedup/shadow-book-rebuild/atomic emission all reused). The
/// JSON shape must match the live WS `book` wire format — verified by a
/// round-trip test against `WsMessage`.
pub(crate) fn rest_book_to_raw_message(resp: &RestBookResponse, recv_us: u64) -> WsRawMessage {
    let to_levels = |entries: &[pb_types::wire::RestOrderEntry]| -> Vec<serde_json::Value> {
        entries
            .iter()
            .map(|e| serde_json::json!({ "price": e.price, "size": e.size }))
            .collect()
    };
    let mut obj = serde_json::json!({
        "event_type": "book",
        "asset_id": resp.asset_id,
        "bids": to_levels(&resp.bids),
        "asks": to_levels(&resp.asks),
    });
    if let Some(ts) = &resp.timestamp {
        obj["timestamp"] = serde_json::Value::String(ts.clone());
    }
    if let Some(hash) = &resp.hash {
        obj["hash"] = serde_json::Value::String(hash.clone());
    }
    WsRawMessage {
        text: obj.to_string(),
        recv_timestamp_us: recv_us,
    }
}

/// Consume resnapshot requests (asset ids), fetch a fresh REST book per asset
/// (debounced), and re-inject it into the feed via `raw_tx`. Runs until the
/// shutdown token fires or either channel closes.
pub async fn run_resnapshot_worker(
    rest: RestClient,
    raw_tx: mpsc::Sender<FeedMessage>,
    mut rx: mpsc::Receiver<Arc<str>>,
    shutdown: CancellationToken,
) {
    let mut last_fetch: HashMap<Arc<str>, Instant> = HashMap::new();
    loop {
        tokio::select! {
            biased;
            _ = shutdown.cancelled() => break,
            req = rx.recv() => {
                let Some(asset) = req else { break };
                let now = Instant::now();
                if let Some(&prev) = last_fetch.get(&asset) {
                    if now.duration_since(prev) < RESNAPSHOT_DEBOUNCE {
                        continue;
                    }
                }
                // Evict entries older than the debounce window: they no longer
                // affect debouncing, so retaining them would grow the map without
                // bound as markets rotate over a long-running ingest (HFT-review
                // #8). n is small, so the O(n) sweep per request is negligible.
                last_fetch.retain(|_, t| now.duration_since(*t) < RESNAPSHOT_DEBOUNCE);
                last_fetch.insert(asset.clone(), now);
                match rest.fetch_book(&asset).await {
                    Ok(resp) => {
                        let msg = rest_book_to_raw_message(&resp, now_micros());
                        if raw_tx.send(FeedMessage::Raw(msg)).await.is_err() {
                            warn!("resnapshot raw channel closed; stopping worker");
                            break;
                        }
                        debug!(asset = %asset, "re-injected REST resnapshot");
                    }
                    Err(e) => {
                        warn!(asset = %asset, error = %e, "resnapshot REST fetch failed");
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pb_types::wire::{RestOrderEntry, WsMessage};

    fn sample_response() -> RestBookResponse {
        RestBookResponse {
            market: None,
            asset_id: "tok1".to_string(),
            bids: vec![RestOrderEntry {
                price: "0.50".to_string(),
                size: "10".to_string(),
            }],
            asks: vec![RestOrderEntry {
                price: "0.60".to_string(),
                size: "20".to_string(),
            }],
            hash: Some("h1".to_string()),
            timestamp: Some("1700000000000000".to_string()),
            tick_size: None,
            min_order_size: None,
            neg_risk: None,
            last_trade_price: None,
        }
    }

    #[test]
    fn rest_book_round_trips_to_ws_book_message() {
        // The synthetic message must parse back as a WsMessage::Book with every
        // field intact — this is what guarantees the dispatcher handles a
        // re-injected resnapshot exactly like a live snapshot (A.74).
        let raw = rest_book_to_raw_message(&sample_response(), 123);
        assert_eq!(raw.recv_timestamp_us, 123);

        let parsed: WsMessage = serde_json::from_str(&raw.text).expect("must parse as WsMessage");
        match parsed {
            WsMessage::Book(book) => {
                assert_eq!(book.asset_id, "tok1");
                assert_eq!(book.timestamp, Some("1700000000000000"));
                assert_eq!(book.hash, Some("h1"));
                assert_eq!(book.bids.len(), 1);
                assert_eq!(book.bids[0].price, "0.50");
                assert_eq!(book.bids[0].size, "10");
                assert_eq!(book.asks[0].price, "0.60");
                assert_eq!(book.asks[0].size, "20");
            }
            other => panic!("expected Book, got {other:?}"),
        }
    }

    #[test]
    fn rest_book_with_missing_optionals_still_round_trips() {
        let mut resp = sample_response();
        resp.hash = None;
        resp.timestamp = None;
        let raw = rest_book_to_raw_message(&resp, 1);
        let parsed: WsMessage = serde_json::from_str(&raw.text).unwrap();
        match parsed {
            WsMessage::Book(book) => {
                assert_eq!(book.hash, None);
                assert_eq!(book.timestamp, None);
                assert_eq!(book.asks.len(), 1);
            }
            other => panic!("expected Book, got {other:?}"),
        }
    }
}
