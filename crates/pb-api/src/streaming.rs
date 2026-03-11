use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Query, State, WebSocketUpgrade};
use axum::response::Response;
use rustc_hash::FxHashMap;
use serde::Deserialize;
use tokio::sync::broadcast;
use tracing::{debug, warn};

use crate::dto::BookUpdateMessage;
use crate::error::ApiError;
use crate::server::AppState;

const BROADCAST_CAPACITY: usize = 256;

#[derive(Debug, Deserialize)]
pub struct StreamQuery {
    pub asset_id: String,
}

/// Per-asset broadcast channels for WebSocket streaming.
///
/// Each active asset gets its own `broadcast::Sender`, so WS subscribers
/// receive only updates for their subscribed asset without client-side
/// filtering. Uses `std::sync::RwLock` because critical sections are tiny
/// (hashmap lookup) with no async work inside the lock.
#[derive(Clone)]
pub struct PerAssetBroadcast {
    inner: Arc<std::sync::RwLock<FxHashMap<String, broadcast::Sender<BookUpdateMessage>>>>,
}

impl Default for PerAssetBroadcast {
    fn default() -> Self {
        Self::new()
    }
}

impl PerAssetBroadcast {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(std::sync::RwLock::new(FxHashMap::default())),
        }
    }

    /// Returns true if any asset channel has at least one subscriber.
    pub fn has_subscribers(&self) -> bool {
        let map = self.inner.read().unwrap_or_else(|e| e.into_inner());
        map.values().any(|sender| sender.receiver_count() > 0)
    }

    /// Send an update to the broadcast channel for the given asset.
    /// If no channel exists for the asset, the message is silently dropped.
    pub fn send(&self, msg: BookUpdateMessage) {
        let map = self.inner.read().unwrap_or_else(|e| e.into_inner());
        if let Some(sender) = map.get(&msg.asset_id) {
            let _ = sender.send(msg);
        }
    }

    /// Subscribe to updates for a specific asset.
    /// Returns `None` if the asset has no active broadcast channel.
    pub fn subscribe(&self, asset_id: &str) -> Option<broadcast::Receiver<BookUpdateMessage>> {
        let map = self.inner.read().unwrap_or_else(|e| e.into_inner());
        map.get(asset_id).map(|sender| sender.subscribe())
    }

    /// Activate broadcast channels for the given asset set.
    /// Creates channels for new assets and removes channels for assets
    /// no longer in the set. Removed channels are dropped, which closes
    /// all their receivers (WS sessions will see `RecvError::Closed`).
    pub fn set_active_assets(&self, assets: &[String]) {
        let mut map = self.inner.write().unwrap_or_else(|e| e.into_inner());
        // Remove channels for assets no longer active.
        map.retain(|asset_id, _| assets.iter().any(|a| a == asset_id));
        // Create channels for newly active assets.
        for asset_id in assets {
            map.entry(asset_id.clone()).or_insert_with(|| {
                let (sender, _) = broadcast::channel(BROADCAST_CAPACITY);
                sender
            });
        }
    }

    /// Returns the set of asset IDs that currently have broadcast channels.
    #[cfg(test)]
    pub fn active_assets(&self) -> Vec<String> {
        let map = self.inner.read().unwrap_or_else(|e| e.into_inner());
        map.keys().cloned().collect()
    }
}

// Keep backward compatibility: type alias for existing code that references BookBroadcast.
pub type BookBroadcast = PerAssetBroadcast;

pub async fn ws_orderbook(
    State(state): State<AppState>,
    Query(query): Query<StreamQuery>,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let asset_id = query.asset_id.clone();

    let is_active = state.live.is_asset_active(&asset_id).await;
    if !is_active {
        return Err(ApiError::NotFound(format!("asset not active: {asset_id}")));
    }

    let broadcast = state
        .broadcast
        .clone()
        .ok_or_else(|| ApiError::ServiceUnavailable("streaming not available".to_string()))?;

    // Subscribe to the asset-specific broadcast channel.
    let rx = broadcast.subscribe(&asset_id).ok_or_else(|| {
        ApiError::ServiceUnavailable(format!("no broadcast channel for asset: {asset_id}"))
    })?;

    let live = state.live.clone();
    let stale_after_secs = state.config.stale_after_secs;
    let default_depth = state.config.default_depth;

    Ok(ws.on_upgrade(move |socket| {
        handle_ws_session(socket, rx, live, asset_id, default_depth, stale_after_secs)
    }))
}

async fn handle_ws_session(
    mut socket: WebSocket,
    mut rx: broadcast::Receiver<BookUpdateMessage>,
    live: crate::live_state::LiveReadModel,
    asset_id: String,
    depth: usize,
    stale_after_secs: u64,
) {
    // Send initial full snapshot.
    match live.snapshot(&asset_id, depth, stale_after_secs).await {
        Ok(snapshot) => {
            let init_msg = BookUpdateMessage {
                asset_id: snapshot.asset_id.clone(),
                slug: snapshot.slug.clone(),
                sequence: snapshot.sequence,
                last_update_us: snapshot.last_update_us,
                bids: snapshot.bids,
                asks: snapshot.asks,
                mid_price: snapshot.mid_price,
                spread: snapshot.spread,
            };
            if send_json(&mut socket, &init_msg).await.is_err() {
                return;
            }
        }
        Err(_) => {
            debug!(asset_id, "snapshot not ready for ws initial send");
        }
    }

    loop {
        tokio::select! {
            msg = rx.recv() => {
                match msg {
                    Ok(update) => {
                        // No need to filter by asset_id — this receiver is
                        // already subscribed to the asset-specific channel.
                        if send_json(&mut socket, &update).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        warn!(asset_id, skipped, "ws subscriber lagged, sending resync snapshot");
                        if let Ok(snapshot) = live.snapshot(&asset_id, depth, stale_after_secs).await {
                            let resync = BookUpdateMessage {
                                asset_id: snapshot.asset_id.clone(),
                                slug: snapshot.slug.clone(),
                                sequence: snapshot.sequence,
                                last_update_us: snapshot.last_update_us,
                                bids: snapshot.bids,
                                asks: snapshot.asks,
                                mid_price: snapshot.mid_price,
                                spread: snapshot.spread,
                            };
                            if send_json(&mut socket, &resync).await.is_err() {
                                break;
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            ws_msg = socket.recv() => {
                match ws_msg {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(Message::Ping(data))) => {
                        if socket.send(Message::Pong(data)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(_)) => {} // ignore text/binary from client
                    Some(Err(_)) => break,
                }
            }
        }
    }

    debug!(asset_id, "ws session closed");
}

async fn send_json(socket: &mut WebSocket, msg: &BookUpdateMessage) -> Result<(), ()> {
    match serde_json::to_string(msg) {
        Ok(text) => socket
            .send(Message::Text(text.into()))
            .await
            .map_err(|_| ()),
        Err(_) => Err(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_update(asset_id: &str) -> BookUpdateMessage {
        BookUpdateMessage {
            asset_id: asset_id.to_string(),
            slug: None,
            sequence: 1,
            last_update_us: 100,
            bids: vec![],
            asks: vec![],
            mid_price: None,
            spread: None,
        }
    }

    #[test]
    fn per_asset_broadcast_new_is_empty() {
        let b = PerAssetBroadcast::new();
        assert!(!b.has_subscribers());
        assert!(b.active_assets().is_empty());
    }

    #[test]
    fn per_asset_broadcast_default_same_as_new() {
        let b = PerAssetBroadcast::default();
        assert!(!b.has_subscribers());
    }

    #[test]
    fn set_active_assets_creates_channels() {
        let b = PerAssetBroadcast::new();
        b.set_active_assets(&["a".to_string(), "b".to_string()]);
        let active = b.active_assets();
        assert!(active.contains(&"a".to_string()));
        assert!(active.contains(&"b".to_string()));
    }

    #[test]
    fn set_active_assets_removes_stale_channels() {
        let b = PerAssetBroadcast::new();
        b.set_active_assets(&["a".to_string(), "b".to_string()]);
        b.set_active_assets(&["b".to_string()]);
        let active = b.active_assets();
        assert!(!active.contains(&"a".to_string()));
        assert!(active.contains(&"b".to_string()));
    }

    #[test]
    fn subscribe_returns_none_for_unknown_asset() {
        let b = PerAssetBroadcast::new();
        assert!(b.subscribe("unknown").is_none());
    }

    #[test]
    fn subscribe_returns_receiver_for_known_asset() {
        let b = PerAssetBroadcast::new();
        b.set_active_assets(&["tok1".to_string()]);
        assert!(b.subscribe("tok1").is_some());
    }

    #[test]
    fn has_subscribers_after_subscribe() {
        let b = PerAssetBroadcast::new();
        b.set_active_assets(&["tok1".to_string()]);
        assert!(!b.has_subscribers());
        let _rx = b.subscribe("tok1").unwrap();
        assert!(b.has_subscribers());
    }

    #[test]
    fn send_delivers_to_subscriber() {
        let b = PerAssetBroadcast::new();
        b.set_active_assets(&["tok1".to_string()]);
        let mut rx = b.subscribe("tok1").unwrap();
        b.send(test_update("tok1"));
        let msg = rx.try_recv().unwrap();
        assert_eq!(msg.asset_id, "tok1");
    }

    #[test]
    fn send_silently_drops_for_unknown_asset() {
        let b = PerAssetBroadcast::new();
        // No panic, no error
        b.send(test_update("unknown"));
    }

    #[test]
    fn send_to_wrong_asset_does_not_deliver() {
        let b = PerAssetBroadcast::new();
        b.set_active_assets(&["tok1".to_string(), "tok2".to_string()]);
        let mut rx1 = b.subscribe("tok1").unwrap();
        b.send(test_update("tok2"));
        assert!(rx1.try_recv().is_err());
    }

    #[test]
    fn dropping_subscriber_reduces_count() {
        let b = PerAssetBroadcast::new();
        b.set_active_assets(&["tok1".to_string()]);
        let rx = b.subscribe("tok1").unwrap();
        assert!(b.has_subscribers());
        drop(rx);
        assert!(!b.has_subscribers());
    }

    #[test]
    fn removing_asset_drops_channel_receivers() {
        let b = PerAssetBroadcast::new();
        b.set_active_assets(&["tok1".to_string()]);
        let mut rx = b.subscribe("tok1").unwrap();
        b.set_active_assets(&[]); // removes tok1
                                  // Channel should now be closed
        assert!(rx.try_recv().is_err());
    }
}
