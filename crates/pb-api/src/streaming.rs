use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Query, State, WebSocketUpgrade};
use axum::response::Response;
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

#[derive(Clone)]
pub struct BookBroadcast {
    sender: Arc<broadcast::Sender<BookUpdateMessage>>,
}

impl Default for BookBroadcast {
    fn default() -> Self {
        Self::new()
    }
}

impl BookBroadcast {
    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(BROADCAST_CAPACITY);
        Self {
            sender: Arc::new(sender),
        }
    }

    pub fn send(&self, msg: BookUpdateMessage) {
        // Ignore send errors — they just mean no active subscribers.
        let _ = self.sender.send(msg);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<BookUpdateMessage> {
        self.sender.subscribe()
    }
}

pub async fn ws_orderbook(
    State(state): State<AppState>,
    Query(query): Query<StreamQuery>,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let asset_id = query.asset_id.clone();

    let is_active = state
        .live
        .is_asset_active(&asset_id)
        .await;
    if !is_active {
        return Err(ApiError::NotFound(format!("asset not active: {asset_id}")));
    }

    let broadcast = state
        .broadcast
        .clone()
        .ok_or_else(|| ApiError::ServiceUnavailable("streaming not available".to_string()))?;

    let live = state.live.clone();
    let stale_after_secs = state.config.stale_after_secs;
    let default_depth = state.config.default_depth;

    Ok(ws.on_upgrade(move |socket| {
        handle_ws_session(socket, broadcast, live, asset_id, default_depth, stale_after_secs)
    }))
}

async fn handle_ws_session(
    mut socket: WebSocket,
    broadcast: BookBroadcast,
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

    let mut rx = broadcast.subscribe();

    loop {
        tokio::select! {
            msg = rx.recv() => {
                match msg {
                    Ok(update) => {
                        if update.asset_id != asset_id {
                            continue;
                        }
                        if send_json(&mut socket, &update).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        warn!(asset_id, skipped, "ws subscriber lagged, sending resync snapshot");
                        if let Ok(snapshot) = live.snapshot(&asset_id, depth, stale_after_secs).await {
                            let resync = BookUpdateMessage {
                                asset_id: snapshot.asset_id.clone(),
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
        Ok(text) => socket.send(Message::Text(text.into())).await.map_err(|_| ()),
        Err(_) => Err(()),
    }
}
