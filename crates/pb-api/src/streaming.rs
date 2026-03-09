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
