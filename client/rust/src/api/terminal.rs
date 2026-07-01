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
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};

use crate::frb_generated::StreamSink;
use anyhow::Context;
use claude_commander_protocol::ws::{ClientControl, ServerControl};
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
        Self {
            kind: TerminalEventKind::Ready,
            bytes: Vec::new(),
            text: session,
        }
    }
    fn output(bytes: Vec<u8>) -> Self {
        Self {
            kind: TerminalEventKind::Output,
            bytes,
            text: String::new(),
        }
    }
    fn detached(reason: String) -> Self {
        Self {
            kind: TerminalEventKind::Detached,
            bytes: Vec::new(),
            text: reason,
        }
    }
    fn error(message: String) -> Self {
        Self {
            kind: TerminalEventKind::Error,
            bytes: Vec::new(),
            text: message,
        }
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

/// Monotonic token distinguishing successive attaches that reuse the same
/// handle (e.g. a reconnect): a finished task only removes its *own* entry, not
/// one a newer attach just installed under the same handle.
static NEXT_GEN: AtomicU64 = AtomicU64::new(0);

type Registry = HashMap<String, (u64, mpsc::UnboundedSender<Outbound>)>;

/// Live attaches keyed by the Dart-supplied handle; each entry is the attach's
/// generation token plus its outbound channel.
fn registry() -> &'static Mutex<Registry> {
    static REG: OnceLock<Mutex<Registry>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Lock the registry, recovering from a poisoned mutex instead of panicking
/// across the FFI boundary. The critical sections are tiny and panic-free, so a
/// poisoned lock would only ever follow an unrelated panic.
fn lock_registry() -> MutexGuard<'static, Registry> {
    registry().lock().unwrap_or_else(|e| e.into_inner())
}

/// Remove `handle`'s entry only if it still carries `generation` — i.e. a newer
/// attach reusing the same handle hasn't already replaced it. Returns whether an
/// entry was actually removed.
fn remove_if_current(handle: &str, generation: u64) -> bool {
    let mut reg = lock_registry();
    if reg.get(handle).map(|(g, _)| *g) == Some(generation) {
        reg.remove(handle);
        true
    } else {
        false
    }
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
    let generation = NEXT_GEN.fetch_add(1, Ordering::Relaxed);
    lock_registry().insert(handle.clone(), (generation, tx));
    let url = ws_url(&base_url);
    runtime().spawn(async move {
        run_attach(&url, token, session_id, &sink, rx).await;
        // Only drop the entry if it's still ours: a reconnect reusing this
        // handle may have replaced it with a newer attach, whose channel we
        // must not delete.
        remove_if_current(&handle, generation);
    });
}

/// Send keystrokes / raw input bytes to an attached terminal. No-op if the
/// handle isn't attached (e.g. already detached).
pub fn terminal_send_input(handle: String, bytes: Vec<u8>) {
    if let Some((_, tx)) = lock_registry().get(&handle) {
        let _ = tx.send(Outbound::Input(bytes));
    }
}

/// Tell the remote PTY the viewport changed.
pub fn terminal_resize(handle: String, cols: u16, rows: u16) {
    if let Some((_, tx)) = lock_registry().get(&handle) {
        let _ = tx.send(Outbound::Resize { cols, rows });
    }
}

/// Detach (leaves the tmux session running server-side).
pub fn terminal_detach(handle: String) {
    if let Some((_, tx)) = lock_registry().get(&handle) {
        let _ = tx.send(Outbound::Detach);
    }
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
    let (ws, _resp) = connect_async(url)
        .await
        .context("websocket connect failed")?;
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

    /// Reconnect race: a re-attach reuses the same handle and replaces the
    /// registry entry; the *old* task finishing must not delete the *new*
    /// task's channel. (Unique handle so it doesn't race the shared registry.)
    #[test]
    fn finished_attach_only_removes_its_own_generation() {
        let h = "reconnect-race-test".to_string();
        let (tx0, _rx0) = mpsc::unbounded_channel();
        let (tx1, _rx1) = mpsc::unbounded_channel();

        // Attach gen 0, then a reconnect (same handle) installs gen 1.
        lock_registry().insert(h.clone(), (0, tx0));
        lock_registry().insert(h.clone(), (1, tx1));

        // The gen-0 task ends and cleans up: must NOT remove gen 1's entry.
        assert!(!remove_if_current(&h, 0));
        assert!(lock_registry().contains_key(&h));

        // The gen-1 task ends later and removes its own entry.
        assert!(remove_if_current(&h, 1));
        assert!(!lock_registry().contains_key(&h));
    }
}
