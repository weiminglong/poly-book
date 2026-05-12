use serde::Deserialize;

/// Raw WebSocket message from Polymarket CLOB.
///
/// Wire contract matches the V2 `/ws/market` channel (live as of 2026-04-28).
/// V2 adds `tick_size_change` to the baseline event set; the previously
/// existing `book`, `price_change`, and `last_trade_price` payloads are
/// unchanged. Premium V2 events (`best_bid_ask`, `new_market`,
/// `market_resolved`) require `custom_feature_enabled: true` on subscribe
/// and are not modeled here yet.
///
/// Uses `serde(borrow)` for zero-copy deserialization where possible.
#[derive(Debug, Deserialize)]
#[serde(tag = "event_type", bound(deserialize = "'de: 'a"))]
pub enum WsMessage<'a> {
    #[serde(rename = "book")]
    Book(BookMessage<'a>),
    #[serde(rename = "price_change")]
    PriceChange(PriceChangeMessage<'a>),
    #[serde(rename = "last_trade_price")]
    LastTradePrice(LastTradePriceMessage<'a>),
    #[serde(rename = "tick_size_change")]
    TickSizeChange(TickSizeChangeMessage<'a>),
}

#[derive(Debug, Deserialize)]
pub struct BookMessage<'a> {
    #[serde(borrow)]
    pub asset_id: &'a str,
    pub market: Option<&'a str>,
    pub timestamp: Option<&'a str>,
    pub bids: Vec<OrderEntry<'a>>,
    pub asks: Vec<OrderEntry<'a>>,
    pub hash: Option<&'a str>,
}

#[derive(Debug, Deserialize)]
pub struct PriceChangeMessage<'a> {
    #[serde(borrow)]
    pub market: Option<&'a str>,
    pub price_changes: Vec<PriceChangeEntry<'a>>,
    pub timestamp: Option<&'a str>,
}

#[derive(Debug, Deserialize)]
pub struct PriceChangeEntry<'a> {
    #[serde(borrow)]
    pub asset_id: &'a str,
    pub price: &'a str,
    pub size: &'a str,
    pub side: &'a str,
    pub hash: Option<&'a str>,
    pub best_bid: Option<&'a str>,
    pub best_ask: Option<&'a str>,
}

#[derive(Debug, Deserialize)]
pub struct LastTradePriceMessage<'a> {
    #[serde(borrow)]
    pub asset_id: &'a str,
    pub market: Option<&'a str>,
    pub price: &'a str,
    pub size: Option<&'a str>,
    pub side: Option<&'a str>,
    /// V2: still emitted; reflects the fee actually charged at match time.
    pub fee_rate_bps: Option<&'a str>,
    pub timestamp: Option<&'a str>,
    pub transaction_hash: Option<&'a str>,
}

/// V2 event signaling that a market's minimum tick size has changed
/// (typically when last price crosses 0.04 / 0.96 thresholds).
#[derive(Debug, Deserialize)]
pub struct TickSizeChangeMessage<'a> {
    #[serde(borrow)]
    pub asset_id: &'a str,
    pub market: Option<&'a str>,
    pub old_tick_size: Option<&'a str>,
    pub new_tick_size: Option<&'a str>,
    pub timestamp: Option<&'a str>,
}

/// A single [price, size] entry from the order book.
#[derive(Debug, Deserialize)]
pub struct OrderEntry<'a> {
    pub price: &'a str,
    pub size: &'a str,
}

/// REST API book response.
///
/// V2 adds `min_order_size`, `tick_size`, `neg_risk`, and `last_trade_price`
/// to the snapshot payload. They are optional so older fixtures still parse.
#[derive(Debug, Deserialize)]
pub struct RestBookResponse {
    pub market: Option<String>,
    pub asset_id: String,
    pub bids: Vec<RestOrderEntry>,
    pub asks: Vec<RestOrderEntry>,
    pub hash: Option<String>,
    pub timestamp: Option<String>,
    #[serde(default)]
    pub tick_size: Option<String>,
    #[serde(default)]
    pub min_order_size: Option<String>,
    #[serde(default)]
    pub neg_risk: Option<bool>,
    #[serde(default)]
    pub last_trade_price: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RestOrderEntry {
    pub price: String,
    pub size: String,
}

/// V2 `GET /clob-markets/{condition_id}` response.
///
/// Returns CLOB-level parameters for a market: minimum tick, minimum order
/// size, fee schedule, and the list of outcome tokens. Field names are the
/// short keys used in the V2 API (`mts`, `mos`, `fd`, `t`, etc.).
#[derive(Debug, Deserialize)]
pub struct ClobMarketInfo {
    /// Game start time (ISO 8601) for sports markets; null otherwise.
    pub gst: Option<String>,
    /// Outcome tokens.
    #[serde(default)]
    pub t: Vec<ClobMarketToken>,
    /// Minimum order size.
    pub mos: Option<f64>,
    /// Minimum tick size (price increment).
    pub mts: Option<f64>,
    /// Maker base fee in basis points.
    pub mbf: Option<i64>,
    /// Taker base fee in basis points.
    pub tbf: Option<i64>,
    /// RFQ enabled flag.
    pub rfqe: Option<bool>,
    /// Taker order delay enabled flag.
    pub itode: Option<bool>,
    /// Blockaid check enabled flag.
    pub ibce: Option<bool>,
    /// Fee curve parameters.
    pub fd: Option<ClobMarketFeeDetails>,
    /// Minimum order age in seconds.
    pub oas: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct ClobMarketToken {
    /// Token ID (asset ID).
    pub t: String,
    /// Outcome label (e.g. "Yes" / "No").
    pub o: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ClobMarketFeeDetails {
    /// Fee rate.
    pub r: Option<f64>,
    /// Fee curve exponent.
    pub e: Option<f64>,
    /// Taker-only flag.
    pub to: Option<bool>,
}

/// Gamma API event response for market discovery.
#[derive(Debug, Deserialize)]
pub struct GammaEvent {
    pub id: Option<String>,
    pub title: Option<String>,
    pub slug: Option<String>,
    pub description: Option<String>,
    pub markets: Option<Vec<GammaMarket>>,
}

#[derive(Debug, Deserialize)]
pub struct GammaMarket {
    pub id: Option<String>,
    #[serde(rename = "conditionId")]
    pub condition_id: Option<String>,
    pub question: Option<String>,
    pub slug: Option<String>,
    #[serde(rename = "tokenId")]
    pub token_id: Option<String>,
    /// Clob token IDs - typically [yes_token, no_token]
    #[serde(rename = "clobTokenIds")]
    pub clob_token_ids: Option<String>,
    pub active: Option<bool>,
    pub closed: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_zero_copy_book_deser() {
        let raw = r#"{
            "event_type": "book",
            "asset_id": "token123",
            "bids": [{"price": "0.55", "size": "100"}],
            "asks": [{"price": "0.60", "size": "200"}]
        }"#;
        let msg: WsMessage = serde_json::from_str(raw).unwrap();
        match msg {
            WsMessage::Book(book) => {
                assert_eq!(book.asset_id, "token123");
                assert_eq!(book.bids.len(), 1);
                assert_eq!(book.asks.len(), 1);
                assert_eq!(book.bids[0].price, "0.55");
            }
            _ => panic!("expected Book"),
        }
    }

    #[test]
    fn test_price_change_deser() {
        let raw = r#"{
            "event_type": "price_change",
            "market": "0x1234",
            "price_changes": [
                {
                    "asset_id": "token123",
                    "price": "0.55",
                    "size": "50",
                    "side": "BUY",
                    "hash": "abc123",
                    "best_bid": "0.55",
                    "best_ask": "0.60"
                }
            ],
            "timestamp": "1757908892351"
        }"#;
        let msg: WsMessage = serde_json::from_str(raw).unwrap();
        match msg {
            WsMessage::PriceChange(pc) => {
                assert_eq!(pc.price_changes.len(), 1);
                assert_eq!(pc.price_changes[0].asset_id, "token123");
                assert_eq!(pc.price_changes[0].side, "BUY");
            }
            _ => panic!("expected PriceChange"),
        }
    }

    #[test]
    fn test_last_trade_deser() {
        let raw = r#"{
            "event_type": "last_trade_price",
            "asset_id": "token123",
            "price": "0.55"
        }"#;
        let msg: WsMessage = serde_json::from_str(raw).unwrap();
        match msg {
            WsMessage::LastTradePrice(lt) => {
                assert_eq!(lt.price, "0.55");
            }
            _ => panic!("expected LastTradePrice"),
        }
    }

    #[test]
    fn test_tick_size_change_deser() {
        let raw = r#"{
            "event_type": "tick_size_change",
            "asset_id": "token123",
            "market": "0xabc",
            "old_tick_size": "0.01",
            "new_tick_size": "0.001",
            "timestamp": "1713398400000"
        }"#;
        let msg: WsMessage = serde_json::from_str(raw).unwrap();
        match msg {
            WsMessage::TickSizeChange(t) => {
                assert_eq!(t.asset_id, "token123");
                assert_eq!(t.old_tick_size, Some("0.01"));
                assert_eq!(t.new_tick_size, Some("0.001"));
            }
            _ => panic!("expected TickSizeChange"),
        }
    }

    #[test]
    fn test_v2_book_with_extra_fields_deser() {
        // V2 emits new top-level fields in the WS book payload; serde must
        // silently ignore them so we remain forward-compatible.
        let raw = r#"{
            "event_type": "book",
            "asset_id": "token123",
            "market": "0xabc",
            "bids": [{"price": "0.55", "size": "100"}],
            "asks": [{"price": "0.60", "size": "200"}],
            "timestamp": "1713398400000",
            "hash": "0xdeadbeef",
            "tick_size": "0.01",
            "min_order_size": "5"
        }"#;
        let msg: WsMessage = serde_json::from_str(raw).unwrap();
        match msg {
            WsMessage::Book(book) => assert_eq!(book.asset_id, "token123"),
            _ => panic!("expected Book"),
        }
    }

    #[test]
    fn test_v2_last_trade_with_fee_rate_bps() {
        // Confirm fee_rate_bps still parses on V2 (FAQ-confirmed unchanged).
        let raw = r#"{
            "event_type": "last_trade_price",
            "asset_id": "token123",
            "market": "0xabc",
            "price": "0.55",
            "size": "10",
            "side": "BUY",
            "fee_rate_bps": "20",
            "timestamp": "1713398400000",
            "transaction_hash": "0xfeed"
        }"#;
        let msg: WsMessage = serde_json::from_str(raw).unwrap();
        match msg {
            WsMessage::LastTradePrice(lt) => {
                assert_eq!(lt.fee_rate_bps, Some("20"));
                assert_eq!(lt.size, Some("10"));
                assert_eq!(lt.side, Some("BUY"));
            }
            _ => panic!("expected LastTradePrice"),
        }
    }

    #[test]
    fn test_v2_rest_book_response_with_new_fields() {
        let raw = r#"{
            "market": "0xabc",
            "asset_id": "token123",
            "timestamp": "1713398400000",
            "hash": "0xdeadbeef",
            "bids": [{"price": "0.55", "size": "100"}],
            "asks": [{"price": "0.60", "size": "200"}],
            "min_order_size": "5",
            "tick_size": "0.01",
            "neg_risk": false,
            "last_trade_price": "0.575"
        }"#;
        let book: RestBookResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(book.asset_id, "token123");
        assert_eq!(book.tick_size.as_deref(), Some("0.01"));
        assert_eq!(book.min_order_size.as_deref(), Some("5"));
        assert_eq!(book.neg_risk, Some(false));
        assert_eq!(book.last_trade_price.as_deref(), Some("0.575"));
    }

    #[test]
    fn test_v2_rest_book_response_without_new_fields() {
        // Older / minimal payload must still parse.
        let raw = r#"{
            "asset_id": "token123",
            "bids": [],
            "asks": []
        }"#;
        let book: RestBookResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(book.asset_id, "token123");
        assert!(book.tick_size.is_none());
        assert!(book.neg_risk.is_none());
    }

    #[test]
    fn test_clob_market_info_deser() {
        let raw = r#"{
            "gst": "2024-01-15T14:30:00Z",
            "r": {},
            "t": [
                {"t": "71321045679252212594626385532706912750332728571942532289631379312455583992563", "o": "Yes"},
                {"t": "12345", "o": "No"}
            ],
            "mos": 5.0,
            "mts": 0.01,
            "mbf": 0,
            "tbf": 0,
            "rfqe": true,
            "itode": false,
            "ibce": true,
            "fd": {"r": 0.02, "e": 2.0, "to": true},
            "oas": 300
        }"#;
        let info: ClobMarketInfo = serde_json::from_str(raw).unwrap();
        assert_eq!(info.mts, Some(0.01));
        assert_eq!(info.mos, Some(5.0));
        assert_eq!(info.t.len(), 2);
        assert_eq!(info.t[0].o.as_deref(), Some("Yes"));
        let fd = info.fd.unwrap();
        assert_eq!(fd.to, Some(true));
        assert_eq!(fd.r, Some(0.02));
    }

    #[test]
    fn test_gamma_event_deser() {
        let raw = r#"{
            "id": "evt1",
            "title": "BTC 5min up/down",
            "markets": [{
                "id": "mkt1",
                "conditionId": "cond1",
                "question": "Will BTC go up?",
                "tokenId": "tok1",
                "clobTokenIds": "[\"tok1\",\"tok2\"]",
                "active": true,
                "closed": false
            }]
        }"#;
        let event: GammaEvent = serde_json::from_str(raw).unwrap();
        assert_eq!(event.title.as_deref(), Some("BTC 5min up/down"));
        assert_eq!(event.markets.as_ref().unwrap().len(), 1);
    }
}
