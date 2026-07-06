//! `GET /ws/attach` — interactive terminal over a WebSocket.
//!
//! Bridges a browser/native WebSocket to a tmux attach via core's
//! transport-agnostic [`HeadlessAttach`] bridge. The handshake authenticates
//! in-band (browsers can't set headers on the WS upgrade), resolves the target
//! session, and spawns the bridge; steady state pumps raw bytes both ways and
//! honours `resize`/`detach` control frames.
//!
//! Detach semantics: a client `detach`, a closed socket, or a heartbeat-ping
//! timeout kills the `tmux attach-session` child **only** — the tmux session and
//! the program inside it keep running. The bridge's [`ChildGuard`] guarantees
//! the child is reaped even on an ungraceful drop, so a closed browser tab never
//! leaks an attach process.

use std::time::Duration;

use axum::{
    extract::{
        State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    response::Response,
};
use claude_commander_core::tmux::HeadlessAttach;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{debug, info, warn};

use super::protocol::{
    AttachKind, ClientControl, DetachReason, ServerControl, WS_ERR_AUTH, WS_ERR_NO_SESSION,
};
use crate::state::AppState;

/// How long to wait for the mandatory `auth` then `attach` handshake frames
/// before giving up. Keeps a connecting-but-silent socket from holding the
/// upgrade open indefinitely.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);

/// Heartbeat interval. A ping is sent this often, and on each tick we check
/// that a pong arrived since the previous one. Detects half-open sockets that
/// never send a close.
const PING_INTERVAL: Duration = Duration::from_secs(20);

/// How many consecutive ping ticks may pass with no intervening pong (or any
/// inbound frame) before the peer is declared dead and the socket is torn down.
/// At 2 (with `PING_INTERVAL` = 20s) a peer tolerates a single dropped pong /
/// scheduling hiccup and is torn down on the tick after ~2 missed intervals.
const MISSED_PONG_LIMIT: u32 = 2;

/// Default PTY size used until the client sends its first `resize`. tmux clamps
/// a shared session to its smallest attached client, so this is only a starting
/// guess.
const DEFAULT_COLS: u16 = 80;
const DEFAULT_ROWS: u16 = 24;

/// Axum handler for `GET /ws/attach`. Performs the protocol upgrade; all real
/// work happens in [`handle_socket`] once the socket is established.
pub async fn attach(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

/// Send a control message as a JSON text frame. Returns `Err` if the socket is
/// gone.
async fn send_control(socket: &mut WebSocket, msg: &ServerControl) -> Result<(), axum::Error> {
    socket.send(Message::Text(msg.to_text().into())).await
}

/// Drive a single attached socket through handshake → steady state → teardown.
async fn handle_socket(mut socket: WebSocket, state: AppState) {
    // -- Handshake: auth frame --
    if !authenticate(&mut socket, &state).await {
        // `authenticate` has already sent an error frame where appropriate.
        return;
    }

    // -- Handshake: attach frame → resolve session → spawn bridge --
    let (session_name, bridge) = match attach_session(&mut socket, &state).await {
        Some(pair) => pair,
        None => return,
    };

    info!("WS attached to tmux session {}", session_name);
    if send_control(
        &mut socket,
        &ServerControl::Ready {
            session: session_name,
        },
    )
    .await
    .is_err()
    {
        return;
    }

    // -- Steady state --
    let reason = pump(socket, bridge).await;
    debug!("WS attach loop ended: {:?}", reason);
}

/// Read the mandatory first `auth` frame and validate the token. The token is
/// **never logged**. Returns true on success; on failure sends an `error` frame
/// and returns false.
async fn authenticate(socket: &mut WebSocket, state: &AppState) -> bool {
    match next_control(socket).await {
        Some(ClientControl::Auth { token }) => {
            if state.auth.authorizes_token(&token) {
                true
            } else {
                warn!("WS auth rejected: invalid token");
                let _ = send_control(
                    socket,
                    &ServerControl::Error {
                        message: WS_ERR_AUTH.into(),
                    },
                )
                .await;
                false
            }
        }
        Some(_) => {
            let _ = send_control(
                socket,
                &ServerControl::Error {
                    message: "expected auth frame first".into(),
                },
            )
            .await;
            false
        }
        None => false,
    }
}

/// Read the `attach` frame, resolve the session to its tmux name, and spawn the
/// bridge. Returns `(tmux_session_name, bridge)` on success; on failure sends an
/// `error` frame and returns `None`.
async fn attach_session(
    socket: &mut WebSocket,
    state: &AppState,
) -> Option<(String, HeadlessAttach)> {
    let (session_query, kind) = match next_control(socket).await {
        Some(ClientControl::Attach { session_id, kind }) => (session_id, kind),
        Some(_) => {
            let _ = send_control(
                socket,
                &ServerControl::Error {
                    message: "expected attach frame".into(),
                },
            )
            .await;
            return None;
        }
        None => return None,
    };

    // Resolve the requested pane to a tmux session name through the same service
    // method `LocalBackend::attach` uses, so both transports get identical
    // revive-on-attach (a dead agent tmux session is recreated) and MRU-stamp
    // (`last_attached_at`) behaviour. The agent pane is the session's primary
    // tmux session; the shell pane (`Ctrl+\` partner) is created on demand.
    let core_kind = match kind {
        AttachKind::Agent => claude_commander_core::backend::AttachKind::Agent,
        AttachKind::Shell => claude_commander_core::backend::AttachKind::Shell,
    };
    let resolved = state
        .service
        .resolve_attach_session(&session_query, core_kind)
        .await;
    let tmux_name = match resolved {
        Ok(Some(name)) => name,
        Ok(None) => {
            let _ = send_control(
                socket,
                &ServerControl::Error {
                    message: WS_ERR_NO_SESSION.into(),
                },
            )
            .await;
            return None;
        }
        Err(e) => {
            let _ = send_control(
                socket,
                &ServerControl::Error {
                    message: format!("failed to resolve session: {e}"),
                },
            )
            .await;
            return None;
        }
    };

    // Honour the socket-dir isolation knob so a hermetic test attaches to the
    // same throwaway tmux server its session was created on, not the real one.
    let tmux_tmpdir = state.service.read_config().tmux_tmpdir;
    match HeadlessAttach::spawn(
        &tmux_name,
        DEFAULT_COLS,
        DEFAULT_ROWS,
        tmux_tmpdir.as_deref(),
    ) {
        Ok(bridge) => Some((tmux_name, bridge)),
        Err(e) => {
            let _ = send_control(
                socket,
                &ServerControl::Error {
                    message: format!("failed to attach: {e}"),
                },
            )
            .await;
            None
        }
    }
}

/// Steady-state pump: WS binary → PTY, PTY → WS binary, `resize`/`detach`
/// control frames, and a pong-tracked heartbeat. Each interval sends a ping and
/// counts it as outstanding; any inbound frame (a pong, or real traffic) clears
/// the count. After `MISSED_PONG_LIMIT` un-answered intervals the peer is
/// declared dead and the loop tears down — so a half-open socket whose sends
/// still nominally succeed is still detected, not just one where `send` errors.
/// Returns once any teardown condition fires; the bridge's `ChildGuard` reaps
/// the attach child on the way out.
async fn pump(mut socket: WebSocket, bridge: HeadlessAttach) -> DetachReason {
    let (mut pty_reader, mut pty_writer, resize, mut child) = bridge.split();
    let mut pty_buf = [0u8; 4096];
    let mut ping = tokio::time::interval(PING_INTERVAL);
    // Skip the immediate first tick so we don't ping before any traffic.
    ping.tick().await;
    // Liveness: a pong (or any inbound frame) resets this; each ping tick
    // increments it. Past `MISSED_PONG_LIMIT` consecutive un-ponged ticks the
    // peer is declared dead even if the socket send still appears to succeed.
    let mut missed_pongs: u32 = 0;

    let reason = loop {
        tokio::select! {
            // PTY output → WS binary frame. `send().await` applies backpressure:
            // a slow client pauses this branch, which pauses PTY reads, so memory
            // never grows unbounded (no intermediate channel).
            read = pty_reader.read(&mut pty_buf) => match read {
                Ok(0) => break DetachReason::SessionEnded,
                Ok(n) => {
                    if socket
                        .send(Message::Binary(pty_buf[..n].to_vec().into()))
                        .await
                        .is_err()
                    {
                        break DetachReason::Transport;
                    }
                }
                Err(e) => {
                    // EIO (raw os error 5) is the expected signal that the PTY
                    // closed; anything else is worth a warning.
                    if e.raw_os_error() != Some(5) {
                        warn!("WS PTY read error: {e}");
                    }
                    break DetachReason::SessionEnded;
                }
            },

            // WS frame → PTY or control handling. Any inbound frame is evidence
            // the peer is alive, so reset the missed-pong counter.
            frame = socket.recv() => {
                missed_pongs = 0;
                match frame {
                    Some(Ok(Message::Binary(bytes))) => {
                        if pty_writer.write_all(&bytes).await.is_err() {
                            break DetachReason::SessionEnded;
                        }
                        let _ = pty_writer.flush().await;
                    }
                    Some(Ok(Message::Text(text))) => match ClientControl::from_text(&text) {
                        Ok(ClientControl::Resize { cols, rows }) => resize.resize(cols, rows),
                        Ok(ClientControl::Detach) => break DetachReason::ClientRequest,
                        // `auth`/`attach` are handshake-only; ignore once attached.
                        Ok(_) => debug!("ignoring unexpected control frame in steady state"),
                        Err(e) => debug!("ignoring malformed control frame: {e}"),
                    },
                    Some(Ok(Message::Close(_))) | None => break DetachReason::Transport,
                    // `Pong` (and `Ping`, auto-answered by axum) just refresh
                    // liveness, already handled by the counter reset above.
                    Some(Ok(_)) => {}
                    Some(Err(_)) => break DetachReason::Transport,
                }
            }

            // Heartbeat: ping the peer to detect a half-open socket. A failed
            // send means the transport is gone; too many un-ponged ticks means a
            // half-open socket where sends still nominally succeed.
            _ = ping.tick() => {
                if missed_pongs >= MISSED_PONG_LIMIT {
                    warn!("WS peer missed {missed_pongs} heartbeat pongs; treating as dead");
                    break DetachReason::Transport;
                }
                if socket.send(Message::Ping(Vec::new().into())).await.is_err() {
                    break DetachReason::Transport;
                }
                missed_pongs += 1;
            }
        }
    };

    // Kill the attach child explicitly (deterministic teardown) and notify the
    // client. Killing the `tmux attach-session` client detaches; the session and
    // its program keep running.
    child.kill().await;
    let _ = send_control(&mut socket, &ServerControl::Detached { reason }).await;
    let _ = socket.send(Message::Close(None)).await;
    reason
}

/// Read frames until the next control (text) frame arrives, parsing it. Binary
/// frames during the handshake are unexpected and ignored. Returns `None` on
/// close, transport error, parse failure, or handshake timeout.
async fn next_control(socket: &mut WebSocket) -> Option<ClientControl> {
    let recv = async {
        loop {
            match socket.recv().await {
                Some(Ok(Message::Text(text))) => match ClientControl::from_text(&text) {
                    Ok(msg) => return Some(msg),
                    Err(e) => {
                        debug!("malformed handshake control frame: {e}");
                        return None;
                    }
                },
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return None,
                // Binary before the handshake completes is unexpected; skip it.
                // Ping/pong are handled by axum; skip them too.
                Some(Ok(_)) => continue,
            }
        }
    };

    match tokio::time::timeout(HANDSHAKE_TIMEOUT, recv).await {
        Ok(result) => result,
        Err(_) => {
            debug!("WS handshake timed out");
            None
        }
    }
}
