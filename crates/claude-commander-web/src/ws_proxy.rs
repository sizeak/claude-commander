//! Reverse proxy for `/ws/attach` → `claude-commander-server`.
//!
//! Bridges the browser's WebSocket to an upstream WebSocket, relaying frames in
//! both directions. The protocol is defined once in [`claude_commander_protocol`]:
//! binary frames are raw PTY bytes; text frames are JSON control messages
//! (`auth` / `attach` / `resize` / `detach`).
//!
//! In [`AuthMode::Bff`] the browser never learns the commander token, so this
//! bridge injects the mandatory `auth` control frame upstream itself before
//! relaying; the browser only ever sends `attach`/`resize`/keystrokes. In
//! [`AuthMode::PassThrough`] the browser sends its own `auth` frame and we relay
//! everything transparently.

use axum::extract::State;
use axum::extract::ws::{Message as AxMessage, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use claude_commander_protocol::ws::ClientControl;
use futures::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message as TgMessage;
use tracing::{debug, warn};

use crate::config::{AppState, AuthMode};

pub async fn attach(State(state): State<AppState>, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |socket| bridge(state, socket))
}

async fn bridge(state: AppState, browser: WebSocket) {
    let url = state.upstream_ws_url();
    let upstream = match tokio_tungstenite::connect_async(&url).await {
        Ok((stream, _resp)) => stream,
        Err(e) => {
            warn!("upstream WS connect failed ({url}): {e}");
            let mut browser = browser;
            let _ = browser
                .send(AxMessage::Text(
                    format!(r#"{{"type":"error","message":"cannot reach server: {e}"}}"#).into(),
                ))
                .await;
            return;
        }
    };

    let (mut up_tx, mut up_rx) = upstream.split();
    let (mut br_tx, mut br_rx) = browser.split();

    // BFF: authenticate the upstream socket in-band before relaying. The browser
    // has no token, so it will only send `attach`/`resize`/keystrokes.
    if let AuthMode::Bff { token, .. } = state.auth.as_ref() {
        let auth = ClientControl::Auth {
            token: token.clone(),
        };
        match auth.to_text() {
            Ok(json) => {
                if let Err(e) = up_tx.send(TgMessage::Text(json.into())).await {
                    warn!("failed to send upstream auth frame: {e}");
                    return;
                }
            }
            Err(e) => {
                warn!("failed to serialize auth frame: {e}");
                return;
            }
        }
    }

    // browser → upstream
    let b2u = async {
        while let Some(Ok(msg)) = br_rx.next().await {
            match axum_to_tg(msg) {
                Some(m) => {
                    if up_tx.send(m).await.is_err() {
                        break;
                    }
                }
                None => break, // browser closed
            }
        }
        let _ = up_tx.send(TgMessage::Close(None)).await;
    };

    // upstream → browser
    let u2b = async {
        while let Some(Ok(msg)) = up_rx.next().await {
            match tg_to_axum(msg) {
                Some(m) => {
                    if br_tx.send(m).await.is_err() {
                        break;
                    }
                }
                None => break, // upstream closed
            }
        }
        let _ = br_tx.send(AxMessage::Close(None)).await;
    };

    // Whichever side ends first tears down the other.
    tokio::select! {
        _ = b2u => debug!("browser side of attach ended"),
        _ = u2b => debug!("upstream side of attach ended"),
    }
}

/// Convert a browser (axum) frame to an upstream (tungstenite) frame. Returns
/// `None` for Close, signalling the relay to stop. Payloads are copied through a
/// `Vec<u8>` so the two crates' `Bytes`/`Utf8Bytes` types don't need to match.
fn axum_to_tg(msg: AxMessage) -> Option<TgMessage> {
    Some(match msg {
        AxMessage::Text(t) => TgMessage::Text(t.as_str().to_owned().into()),
        AxMessage::Binary(b) => TgMessage::Binary(b.to_vec().into()),
        AxMessage::Ping(b) => TgMessage::Ping(b.to_vec().into()),
        AxMessage::Pong(b) => TgMessage::Pong(b.to_vec().into()),
        AxMessage::Close(_) => return None,
    })
}

/// Convert an upstream (tungstenite) frame to a browser (axum) frame. Returns
/// `None` for Close/raw-Frame, signalling the relay to stop.
fn tg_to_axum(msg: TgMessage) -> Option<AxMessage> {
    Some(match msg {
        TgMessage::Text(t) => AxMessage::Text(t.as_str().to_owned().into()),
        TgMessage::Binary(b) => AxMessage::Binary(b.to_vec().into()),
        TgMessage::Ping(b) => AxMessage::Ping(b.to_vec().into()),
        TgMessage::Pong(b) => AxMessage::Pong(b.to_vec().into()),
        TgMessage::Close(_) | TgMessage::Frame(_) => return None,
    })
}
