//! Live terminal bridge (Phase 3 spike): a WebSocket attach to the server's
//! `/ws/attach`, exposed to Dart as an event stream plus a few control calls.
//!
//! Design: Dart supplies a `handle` string when it opens an attach. The Rust
//! side spawns the socket on a shared tokio runtime and registers the attach's
//! outbound channel under that handle. Output (and lifecycle events) flow to
//! Dart through the `StreamSink`; input/resize/detach are plain functions keyed
//! by the same handle. This keeps the frb surface trivial — one stream function
//! and three fire-and-forget calls — with no opaque handle types to thread
//! around.
//!
//! Framing matches `claude-commander-protocol`: raw PTY bytes are WS *binary*
//! frames both ways; the handshake (`auth` → `attach`) and `resize`/`detach` are
//! JSON *text* frames.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use anyhow::Context;
use claude_commander_protocol::ws::{ClientControl, ServerControl};
use crate::frb_generated::StreamSink;
use futures_util::{SinkExt, StreamExt};
use tokio::runtime::Runtime;
use tokio::sync::mpsc;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

/// Which kind of [`TerminalEvent`] this is. A unit-only enum so frb renders a
/// plain Dart enum (no freezed).
pub enum TerminalEventKind {
    /// Handshake done; the server echoed the resolved tmux session name (`text`).
    Ready,
    /// Raw PTY output bytes in `bytes` — feed straight to the terminal emulator,
    /// which handles partial UTF-8 / escape sequences.
    Output,
    /// The attach ended cleanly; `text` is the reason (client detach, session
    /// ended, transport).
    Detached,
    /// A handshake or steady-state error; `text` is safe to show the user.
    Error,
}

/// An event streamed from an attached terminal to Dart.
///
/// Modelled as a tagged struct (not a data-carrying enum) so frb generates a
/// plain Dart class — keeping the spike free of the freezed/build_runner
/// codegen step. If the project later adopts freezed, this can become a proper
/// sealed class.
pub struct TerminalEvent {
    pub kind: TerminalEventKind,
    /// Populated only for [`TerminalEventKind::Output`]; empty otherwise.
    pub bytes: Vec<u8>,
    /// Session name (`Ready`), detach reason (`Detached`), or error message
    /// (`Error`); empty for `Output`.
    pub text: String,
}

impl TerminalEvent {
    fn ready(session: String) -> Self {
        Self { kind: TerminalEventKind::Ready, bytes: Vec::new(), text: session }
    }
    fn output(bytes: Vec<u8>) -> Self {
        Self { kind: TerminalEventKind::Output, bytes, text: String::new() }
    }
    fn detached(reason: String) -> Self {
        Self { kind: TerminalEventKind::Detached, bytes: Vec::new(), text: reason }
    }
    fn error(message: String) -> Self {
        Self { kind: TerminalEventKind::Error, bytes: Vec::new(), text: message }
    }
}

/// A message from Dart to the socket's write half.
enum Outbound {
    Input(Vec<u8>),
    Resize { cols: u16, rows: u16 },
    Detach,
}

/// Shared multi-thread runtime that owns all attach sockets, so the work
/// happens off the Dart isolate's frb worker thread.
fn runtime() -> &'static Runtime {
    static RT: OnceLock<Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("failed to build tokio runtime")
    })
}

/// Live attaches keyed by the Dart-supplied handle, holding each one's outbound
/// channel. Entries are removed when the attach task ends.
fn registry() -> &'static Mutex<HashMap<String, mpsc::UnboundedSender<Outbound>>> {
    static REG: OnceLock<Mutex<HashMap<String, mpsc::UnboundedSender<Outbound>>>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Turn the HTTP base URL into the `/ws/attach` WebSocket URL.
fn ws_url(base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    let ws = if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        base.to_string()
    };
    format!("{ws}/ws/attach")
}

/// Open a live terminal attach. `handle` is a caller-chosen id used to route
/// later `terminal_send_input`/`terminal_resize`/`terminal_detach` calls back to
/// this socket. Events stream over `sink` until the attach ends.
pub fn attach_terminal(
    handle: String,
    base_url: String,
    token: String,
    session_id: String,
    sink: StreamSink<TerminalEvent>,
) {
    let (tx, rx) = mpsc::unbounded_channel();
    registry()
        .lock()
        .expect("registry mutex poisoned")
        .insert(handle.clone(), tx);
    let url = ws_url(&base_url);
    runtime().spawn(async move {
        run_attach(&url, token, session_id, &sink, rx).await;
        registry()
            .lock()
            .expect("registry mutex poisoned")
            .remove(&handle);
    });
}

/// Send keystrokes / raw input bytes to an attached terminal. No-op if the
/// handle isn't attached (e.g. already detached).
pub fn terminal_send_input(handle: String, bytes: Vec<u8>) {
    if let Some(tx) = registry()
        .lock()
        .expect("registry mutex poisoned")
        .get(&handle)
    {
        let _ = tx.send(Outbound::Input(bytes));
    }
}

/// Tell the remote PTY the viewport changed.
pub fn terminal_resize(handle: String, cols: u16, rows: u16) {
    if let Some(tx) = registry()
        .lock()
        .expect("registry mutex poisoned")
        .get(&handle)
    {
        let _ = tx.send(Outbound::Resize { cols, rows });
    }
}

/// Detach (leaves the tmux session running server-side).
pub fn terminal_detach(handle: String) {
    if let Some(tx) = registry()
        .lock()
        .expect("registry mutex poisoned")
        .get(&handle)
    {
        let _ = tx.send(Outbound::Detach);
    }
}

/// Spike benchmark: flood the same event-stream path with `chunks` chunks of
/// `chunk_bytes` of synthetic printable PTY output, as fast as the bridge
/// accepts them. Lets Dart measure end-to-end frb → decode → terminal-render
/// throughput with no server or socket. Not part of the real terminal feature.
pub fn bench_terminal_stream(chunks: u32, chunk_bytes: u32, sink: StreamSink<TerminalEvent>) {
    runtime().spawn(async move {
        // One reusable chunk of printable ASCII with periodic newlines, so the
        // emulator does realistic wrapping/scrolling rather than one long line.
        let chunk: Vec<u8> = (0..chunk_bytes)
            .map(|i| {
                let m = (i % 64) as u8;
                if m == 63 { b'\n' } else { 0x20 + m }
            })
            .collect();
        for _ in 0..chunks {
            if sink.add(TerminalEvent::output(chunk.clone())).is_err() {
                break;
            }
        }
        let _ = sink.add(TerminalEvent::detached("bench_complete".to_string()));
    });
}

/// Run the attach to completion, emitting any terminal error to the sink so the
/// UI can surface it.
async fn run_attach(
    url: &str,
    token: String,
    session_id: String,
    sink: &StreamSink<TerminalEvent>,
    rx: mpsc::UnboundedReceiver<Outbound>,
) {
    if let Err(e) = attach_inner(url, token, session_id, sink, rx).await {
        let _ = sink.add(TerminalEvent::error(e.to_string()));
    }
}

async fn attach_inner(
    url: &str,
    token: String,
    session_id: String,
    sink: &StreamSink<TerminalEvent>,
    mut rx: mpsc::UnboundedReceiver<Outbound>,
) -> anyhow::Result<()> {
    let (ws, _resp) = connect_async(url).await.context("websocket connect failed")?;
    let (mut write, mut read) = ws.split();

    // Handshake: auth, then attach.
    write
        .send(Message::Text(ClientControl::Auth { token }.to_text()?))
        .await
        .context("failed to send auth frame")?;
    write
        .send(Message::Text(
            ClientControl::Attach { session_id }.to_text()?,
        ))
        .await
        .context("failed to send attach frame")?;

    loop {
        tokio::select! {
            inbound = read.next() => {
                let Some(msg) = inbound else { break };
                let msg = msg.context("websocket read error")?;
                match msg {
                    Message::Binary(bytes) => {
                        if sink.add(TerminalEvent::output(bytes.to_vec())).is_err() {
                            break; // Dart dropped the stream
                        }
                    }
                    Message::Text(text) => match ServerControl::from_text(text.as_str()) {
                        Ok(ServerControl::Ready { session }) => {
                            let _ = sink.add(TerminalEvent::ready(session));
                        }
                        Ok(ServerControl::Detached { reason }) => {
                            let _ = sink.add(TerminalEvent::detached(format!("{reason:?}")));
                            break;
                        }
                        Ok(ServerControl::Error { message }) => {
                            let _ = sink.add(TerminalEvent::error(message));
                            break;
                        }
                        Err(_) => { /* ignore unrecognised control frames */ }
                    },
                    Message::Ping(payload) => {
                        // The server pings every ~20s and tears down peers that
                        // miss pongs, so answer promptly.
                        let _ = write.send(Message::Pong(payload)).await;
                    }
                    Message::Close(_) => break,
                    _ => {}
                }
            }
            outbound = rx.recv() => {
                let Some(out) = outbound else { break }; // all senders dropped
                match out {
                    Outbound::Input(bytes) => {
                        write.send(Message::Binary(bytes)).await.context("failed to send input")?;
                    }
                    Outbound::Resize { cols, rows } => {
                        write
                            .send(Message::Text(ClientControl::Resize { cols, rows }.to_text()?))
                            .await
                            .context("failed to send resize")?;
                    }
                    Outbound::Detach => {
                        let _ = write
                            .send(Message::Text(ClientControl::Detach.to_text()?))
                            .await;
                        break;
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ws_url_maps_scheme_and_appends_path() {
        assert_eq!(ws_url("http://host:8080"), "ws://host:8080/ws/attach");
        assert_eq!(ws_url("https://host:8080/"), "wss://host:8080/ws/attach");
        // Unknown scheme is left as-is, path still appended.
        assert_eq!(ws_url("host:8080"), "host:8080/ws/attach");
    }
}
