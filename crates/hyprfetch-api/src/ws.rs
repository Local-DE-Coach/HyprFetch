//! WebSocket endpoint — live event fan-out (`/ws`).
//!
//! Each connected client gets its own subscription to the engine's event
//! bus. The server pushes three event kinds as JSON text frames:
//!
//! - `task:progress` — `{ event, task_id, downloaded_bytes, total_bytes, speed_bps, ts }`
//! - `task:state`    — `{ event, task_id, state, error, ts }`
//! - `global:speed`  — `{ event, speed_bps, active_tasks, ts }`
//!
//! Client→server messages are ignored (except protocol-level pings, which
//! axum/tungstenite answer automatically). When the bus lags a slow client,
//! missed events are skipped — progress is self-correcting because every
//! event carries absolute totals.

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::IntoResponse;
use futures::{SinkExt, StreamExt};
use tokio::sync::broadcast::error::RecvError;

use crate::AppState;

/// `GET /ws` — upgrade to WebSocket and stream engine events.
pub async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: AppState) {
    let (mut sink, mut stream) = socket.split();
    let mut rx = state.engine.subscribe();

    loop {
        tokio::select! {
            // Server → client: forward engine events as JSON text frames.
            ev = rx.recv() => {
                match ev {
                    Ok(ev) => {
                        match serde_json::to_string(&ev) {
                            Ok(json) => {
                                if sink.send(Message::Text(json.into())).await.is_err() {
                                    break; // client went away
                                }
                            }
                            Err(e) => {
                                tracing::warn!(error = %e, "failed to serialize engine event");
                            }
                        }
                    }
                    Err(RecvError::Lagged(skipped)) => {
                        tracing::debug!(skipped, "ws client lagged; skipping missed events");
                        continue;
                    }
                    Err(RecvError::Closed) => break, // engine dropped
                }
            }
            // Client → server: drain so pings/close are handled; ignore data.
            msg = stream.next() => {
                match msg {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => {} // pings are auto-replied by tungstenite
                    Some(Err(_)) => break,
                }
            }
        }
    }

    let _ = sink.close().await;
}
