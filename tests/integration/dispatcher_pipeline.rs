//! Test the dispatcher pipeline: raw WS messages -> OrderbookEvents.

use pb_feed::dispatcher::Dispatcher;
use pb_feed::ws::WsRawMessage;
use pb_types::event::{EventType, Side};

#[tokio::test]
async fn test_dispatch_book_message() {
    let (raw_tx, raw_rx) = tokio::sync::mpsc::channel::<WsRawMessage>(100);
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(100);

    let mut dispatcher = Dispatcher::new(raw_rx, event_tx);
    tokio::spawn(async move {
        let _ = dispatcher.run().await;
    });

    // Send a book (snapshot) message
    let book_msg = serde_json::json!({
        "event_type": "book",
        "asset_id": "token-123",
        "bids": [
            {"price": "0.55", "size": "100"},
            {"price": "0.50", "size": "200"}
        ],
        "asks": [
            {"price": "0.60", "size": "150"}
        ]
    });

    raw_tx
        .send(WsRawMessage {
            text: book_msg.to_string(),
            recv_timestamp_us: 1_000_000,
        })
        .await
        .unwrap();

    // Should produce 3 events (2 bids + 1 ask)
    let e1 = event_rx.recv().await.unwrap();
    assert_eq!(e1.event_type, EventType::Snapshot);
    assert_eq!(e1.side, Some(Side::Bid));
    assert_eq!(e1.price.raw(), 5500);
    assert_eq!(e1.recv_timestamp_us, 1_000_000);

    let e2 = event_rx.recv().await.unwrap();
    assert_eq!(e2.event_type, EventType::Snapshot);
    assert_eq!(e2.side, Some(Side::Bid));
    assert_eq!(e2.price.raw(), 5000);

    let e3 = event_rx.recv().await.unwrap();
    assert_eq!(e3.event_type, EventType::Snapshot);
    assert_eq!(e3.side, Some(Side::Ask));
    assert_eq!(e3.price.raw(), 6000);

    // Verify sequential sequence numbers
    assert_eq!(e1.sequence.raw(), 0);
    assert_eq!(e2.sequence.raw(), 1);
    assert_eq!(e3.sequence.raw(), 2);
}

#[tokio::test]
async fn test_dispatch_price_change() {
    let (raw_tx, raw_rx) = tokio::sync::mpsc::channel::<WsRawMessage>(100);
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(100);

    let mut dispatcher = Dispatcher::new(raw_rx, event_tx);
    tokio::spawn(async move {
        let _ = dispatcher.run().await;
    });

    let delta_msg = serde_json::json!({
        "event_type": "price_change",
        "asset_id": "token-456",
        "side": "BUY",
        "price": "0.65",
        "size": "300"
    });

    raw_tx
        .send(WsRawMessage {
            text: delta_msg.to_string(),
            recv_timestamp_us: 2_000_000,
        })
        .await
        .unwrap();

    let event = event_rx.recv().await.unwrap();
    assert_eq!(event.event_type, EventType::Delta);
    assert_eq!(event.side, Some(Side::Bid));
    assert_eq!(event.price.raw(), 6500);
    assert_eq!(event.size.raw(), 300_000_000);
    assert_eq!(event.asset_id.as_str(), "token-456");
}

#[tokio::test]
async fn test_dispatch_last_trade_price() {
    let (raw_tx, raw_rx) = tokio::sync::mpsc::channel::<WsRawMessage>(100);
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(100);

    let mut dispatcher = Dispatcher::new(raw_rx, event_tx);
    tokio::spawn(async move {
        let _ = dispatcher.run().await;
    });

    let trade_msg = serde_json::json!({
        "event_type": "last_trade_price",
        "asset_id": "token-789",
        "price": "0.42"
    });

    raw_tx
        .send(WsRawMessage {
            text: trade_msg.to_string(),
            recv_timestamp_us: 3_000_000,
        })
        .await
        .unwrap();

    let event = event_rx.recv().await.unwrap();
    assert_eq!(event.event_type, EventType::Trade);
    assert_eq!(event.side, None);
    assert_eq!(event.price.raw(), 4200);
}

#[tokio::test]
async fn test_dispatch_invalid_message_skipped() {
    let (raw_tx, raw_rx) = tokio::sync::mpsc::channel::<WsRawMessage>(100);
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(100);

    let mut dispatcher = Dispatcher::new(raw_rx, event_tx);
    tokio::spawn(async move {
        let _ = dispatcher.run().await;
    });

    // Send garbage, then a valid message
    raw_tx
        .send(WsRawMessage {
            text: "not valid json {{{".to_string(),
            recv_timestamp_us: 1,
        })
        .await
        .unwrap();

    let valid_msg = serde_json::json!({
        "event_type": "last_trade_price",
        "asset_id": "token-ok",
        "price": "0.50"
    });
    raw_tx
        .send(WsRawMessage {
            text: valid_msg.to_string(),
            recv_timestamp_us: 2,
        })
        .await
        .unwrap();

    // Should skip invalid and deliver valid
    let event = event_rx.recv().await.unwrap();
    assert_eq!(event.asset_id.as_str(), "token-ok");
}
