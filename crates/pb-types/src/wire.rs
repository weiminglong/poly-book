use serde::Deserialize;

/// Raw WebSocket message from Polymarket CLOB.
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
    pub fee_rate_bps: Option<&'a str>,
    pub timestamp: Option<&'a str>,
    pub transaction_hash: Option<&'a str>,
}

/// A single [price, size] entry from the order book.
#[derive(Debug, Deserialize)]
pub struct OrderEntry<'a> {
    pub price: &'a str,
    pub size: &'a str,
}

/// REST API book response.
#[derive(Debug, Deserialize)]
pub struct RestBookResponse {
    pub market: Option<String>,
    pub asset_id: String,
    pub bids: Vec<RestOrderEntry>,
    pub asks: Vec<RestOrderEntry>,
    pub hash: Option<String>,
    pub timestamp: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RestOrderEntry {
    pub price: String,
    pub size: String,
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
