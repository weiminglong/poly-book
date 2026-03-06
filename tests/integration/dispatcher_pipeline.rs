//! Test the dispatcher pipeline: feed messages -> typed persisted records.

use pb_feed::dispatcher::Dispatcher;
use pb_feed::ws::{FeedMessage, WsLifecycleEvent, WsLifecycleKind, WsRawMessage};
use pb_types::event::{BookEventKind, IngestEventKind, PersistedRecord, Side, TradeFidelity};

fn raw_message(text: serde_json::Value) -> FeedMessage {
    FeedMessage::Raw(WsRawMessage {
        text: text.to_string(),
        recv_timestamp_us: 1_000_000,
    })
}

#[tokio::test]
async fn dispatcher_splits_book_trade_and_ingest_records() {
    let (raw_tx, raw_rx) = tokio::sync::mpsc::channel::<FeedMessage>(100);
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel::<PersistedRecord>(100);
    let mut dispatcher = Dispatcher::new(raw_rx, event_tx);
    let handle = tokio::spawn(async move { dispatcher.run().await.unwrap() });

    let snapshot = serde_json::json!({
        "event_type": "book",
        "asset_id": "token-1",
        "timestamp": "1700000000000000",
        "hash": "book-1",
        "bids": [{"price": "0.55", "size": "100"}],
        "asks": [{"price": "0.60", "size": "200"}]
    });
    raw_tx.send(raw_message(snapshot)).await.unwrap();

    match event_rx.recv().await.unwrap() {
        PersistedRecord::Book(event) => {
            assert_eq!(event.kind, BookEventKind::Snapshot);
            assert_eq!(event.side, Side::Bid);
            assert_eq!(event.price.raw(), 5500);
            assert_eq!(event.provenance.sequence.unwrap().raw(), 0);
        }
        other => panic!("expected book event, got {other:?}"),
    }
    match event_rx.recv().await.unwrap() {
        PersistedRecord::Book(event) => {
            assert_eq!(event.kind, BookEventKind::Snapshot);
            assert_eq!(event.side, Side::Ask);
            assert_eq!(event.price.raw(), 6000);
            assert_eq!(event.provenance.sequence.unwrap().raw(), 1);
        }
        other => panic!("expected book event, got {other:?}"),
    }

    let delta = serde_json::json!({
        "event_type": "price_change",
        "timestamp": "1700000000001000",
        "price_changes": [{
            "asset_id": "token-1",
            "price": "0.54",
            "size": "75",
            "side": "BUY",
            "hash": "delta-1"
        }]
    });
    raw_tx.send(raw_message(delta)).await.unwrap();

    match event_rx.recv().await.unwrap() {
        PersistedRecord::Book(event) => {
            assert_eq!(event.kind, BookEventKind::Delta);
            assert_eq!(event.side, Side::Bid);
            assert_eq!(event.size.raw(), 75_000_000);
        }
        other => panic!("expected delta event, got {other:?}"),
    }

    let trade = serde_json::json!({
        "event_type": "last_trade_price",
        "asset_id": "token-1",
        "price": "0.58"
    });
    raw_tx.send(raw_message(trade)).await.unwrap();

    match event_rx.recv().await.unwrap() {
        PersistedRecord::Trade(event) => {
            assert_eq!(event.price.raw(), 5800);
            assert_eq!(event.fidelity, TradeFidelity::Partial);
            assert!(event.size.is_none());
        }
        other => panic!("expected trade event, got {other:?}"),
    }

    let stale_snapshot = serde_json::json!({
        "event_type": "book",
        "asset_id": "token-1",
        "timestamp": "1699999999999000",
        "bids": [{"price": "0.53", "size": "10"}],
        "asks": []
    });
    raw_tx.send(raw_message(stale_snapshot)).await.unwrap();

    match event_rx.recv().await.unwrap() {
        PersistedRecord::Ingest(event) => {
            assert_eq!(event.kind, IngestEventKind::StaleSnapshotSkip);
            assert_eq!(event.asset_id.unwrap().as_str(), "token-1");
        }
        other => panic!("expected ingest event, got {other:?}"),
    }

    drop(raw_tx);
    handle.await.unwrap();
}

#[tokio::test]
async fn dispatcher_persists_lifecycle_events() {
    let (raw_tx, raw_rx) = tokio::sync::mpsc::channel::<FeedMessage>(100);
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel::<PersistedRecord>(100);
    let mut dispatcher = Dispatcher::new(raw_rx, event_tx);
    let handle = tokio::spawn(async move { dispatcher.run().await.unwrap() });

    raw_tx
        .send(FeedMessage::Lifecycle(WsLifecycleEvent {
            kind: WsLifecycleKind::ReconnectSuccess,
            recv_timestamp_us: 99,
            session_id: "session-123".to_string(),
            details: Some("connected".to_string()),
        }))
        .await
        .unwrap();

    match event_rx.recv().await.unwrap() {
        PersistedRecord::Ingest(event) => {
            assert_eq!(event.kind, IngestEventKind::ReconnectSuccess);
            assert_eq!(
                event.provenance.source_session_id.as_deref(),
                Some("session-123")
            );
        }
        other => panic!("expected reconnect event, got {other:?}"),
    }
    match event_rx.recv().await.unwrap() {
        PersistedRecord::Ingest(event) => {
            assert_eq!(event.kind, IngestEventKind::SourceReset);
        }
        other => panic!("expected source reset event, got {other:?}"),
    }

    drop(raw_tx);
    handle.await.unwrap();
}
