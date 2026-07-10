//! Live terminal bridge + the poller-driven live feeds, exposed to Dart.
//!
//! ## Terminal attach
//!
//! Dart opens an attach with [`attach_terminal`], passing a caller-supplied
//! per-attach id (a fresh UUID) alongside the server `handle`. The control
//! registry (`terminal_send_input`/`terminal_resize`/`terminal_detach`) is keyed
//! by that `attach_id`, not the server handle, so a desktop client can hold
//! several live attaches (a persistent terminal pane plus others) against one
//! server. The `RemoteClient` is still resolved via the server `handle`, and each
//! entry records its owning `handle` so [`disconnect_server`] can tear down every
//! in-flight attach for a server before dropping it.
//!
//! The attach itself is [`claude_commander_client::RemoteClient::attach`] — the
//! raw WebSocket handshake + pump live in that shared crate now; this module only
//! bridges its [`AttachStreams`] to the Dart-facing [`TerminalEvent`] stream and
//! routes input/resize/detach control calls back to it. Output flows to Dart
//! through the `StreamSink`; the control calls are plain functions keyed by the
//! attach id.
//!
//! A generation token distinguishes successive attaches that reuse one attach id
//! (a reconnect), so a finished attach only removes its *own* control-channel
//! entry, never one a newer attach just installed.
//!
//! [`disconnect_server`]: crate::api::registry::disconnect_server
//!
//! ## Live feeds
//!
//! [`change_feed`] and [`connection_feed`] subscribe to the server's background
//! [`Poller`](claude_commander_client::Poller) watches (via the registry) and
//! forward each update to Dart — the change-feed generation counter drives a
//! re-fetch, the connection state drives the server-health header.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};

use claude_commander_client::{AttachEnd, AttachStreams};
use claude_commander_protocol::ws::AttachKind;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

use crate::api::mirrors::ConnectionStateDto;
use crate::api::registry::{
    connection_watch, generation_watch, map_client_err, parse_session_id, runtime, with_client,
};
use crate::frb_generated::StreamSink;

/// Read chunk when draining the attach's reader before forwarding to Dart.
const READ_CHUNK: usize = 8 * 1024;

/// Which kind of [`TerminalEvent`] this is. A unit-only enum so frb renders a
/// plain Dart enum (no freezed).
pub enum TerminalEventKind {
    /// Handshake done; `text` is a session label (may be empty).
    Ready,
    /// Raw PTY output bytes in `bytes` — feed straight to the terminal emulator,
    /// which handles partial UTF-8 / escape sequences.
    Output,
    /// The attach ended cleanly; `text` is the reason (client detach, session
    /// ended).
    Detached,
    /// A handshake or steady-state error; `text` is safe to show the user.
    Error,
}

/// An event streamed from an attached terminal to Dart.
///
/// Modelled as a tagged struct (not a data-carrying enum) so frb generates a
/// plain Dart class — keeping the bridge free of the freezed/build_runner step.
pub struct TerminalEvent {
    pub kind: TerminalEventKind,
    /// Populated only for [`TerminalEventKind::Output`]; empty otherwise.
    pub bytes: Vec<u8>,
    /// Session label (`Ready`), detach reason (`Detached`), or error message
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

/// A message from Dart to the attach's write half / control channel.
enum Outbound {
    Input(Vec<u8>),
    Resize { cols: u16, rows: u16 },
    Detach,
}

/// Monotonic token distinguishing successive attaches that reuse the same
/// attach id (a reconnect): a finished task only removes its *own* entry.
static NEXT_GEN: AtomicU64 = AtomicU64::new(0);

/// One live attach: the generation token (for reconnect races), the owning
/// server `handle` (so [`detach_all_for_handle`] can tear it down on
/// disconnect), and the outbound channel to its pump.
struct AttachEntry {
    generation: u64,
    handle: String,
    tx: mpsc::UnboundedSender<Outbound>,
}

type Registry = HashMap<String, AttachEntry>;

/// Live attaches keyed by the caller-supplied `attach_id`.
fn registry() -> &'static Mutex<Registry> {
    static REG: OnceLock<Mutex<Registry>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Lock the registry, recovering from a poisoned mutex instead of panicking
/// across the FFI boundary. The critical sections are tiny and panic-free.
fn lock_registry() -> MutexGuard<'static, Registry> {
    registry().lock().unwrap_or_else(|e| e.into_inner())
}

/// Remove `attach_id`'s entry only if it still carries `generation` — i.e. a
/// newer attach reusing the same id hasn't already replaced it. Returns whether
/// an entry was actually removed.
fn remove_if_current(attach_id: &str, generation: u64) -> bool {
    let mut reg = lock_registry();
    if reg.get(attach_id).map(|e| e.generation) == Some(generation) {
        reg.remove(attach_id);
        true
    } else {
        false
    }
}

/// Tear down every in-flight attach owned by `handle`: pull each matching entry
/// out of the registry and signal its pump to detach (leaving the server-side
/// tmux session running). Called by [`disconnect_server`] so dropping a server
/// never abandons a live WebSocket attach.
///
/// [`disconnect_server`]: crate::api::registry::disconnect_server
pub(crate) fn detach_all_for_handle(handle: &str) {
    let mut reg = lock_registry();
    let ids: Vec<String> = reg
        .iter()
        .filter(|(_, entry)| entry.handle == handle)
        .map(|(id, _)| id.clone())
        .collect();
    for id in ids {
        if let Some(entry) = reg.remove(&id) {
            // Unbounded send: buffers the Detach even if the pump is mid-read, so
            // it exits cleanly rather than being dropped mid-attach.
            let _ = entry.tx.send(Outbound::Detach);
        }
    }
}

/// Open a live terminal attach against the server behind `handle`. `attach_id` is
/// a caller-supplied, per-attach id (a fresh UUID) that keys this attach's
/// control channel; `session_id` is a full-id string; `kind` picks the agent or
/// shell pane. Later `terminal_send_input`/`terminal_resize`/`terminal_detach`
/// calls with the same `attach_id` route here. Events stream over `sink` until it
/// ends.
pub fn attach_terminal(
    handle: String,
    attach_id: String,
    session_id: String,
    kind: AttachKind,
    sink: StreamSink<TerminalEvent>,
) {
    let client = match with_client(&handle) {
        Ok(client) => client,
        Err(e) => {
            let _ = sink.add(TerminalEvent::error(e.to_string()));
            return;
        }
    };
    let sid = match parse_session_id(&session_id) {
        Ok(sid) => sid,
        Err(e) => {
            let _ = sink.add(TerminalEvent::error(e.to_string()));
            return;
        }
    };

    let (tx, rx) = mpsc::unbounded_channel();
    let generation = NEXT_GEN.fetch_add(1, Ordering::Relaxed);
    lock_registry().insert(
        attach_id.clone(),
        AttachEntry {
            generation,
            handle,
            tx,
        },
    );

    runtime().spawn(async move {
        // The server starts the PTY at a default size; the Dart page re-announces
        // its real size on the Ready event, so this initial size is transient.
        match client.attach(sid, 80, 24, kind).await {
            Ok(conn) => {
                let _ = sink.add(TerminalEvent::ready(session_id));
                pump(conn.split(), &sink, rx).await;
            }
            Err(e) => {
                let _ = sink.add(TerminalEvent::error(map_client_err(e).to_string()));
            }
        }
        // Only drop the entry if it's still ours: a reconnect reusing this
        // attach id may have replaced it with a newer attach, whose channel we
        // must keep.
        remove_if_current(&attach_id, generation);
    });
}

/// The steady-state bridge: forward attach output → Dart, Dart input → the
/// attach writer, resize/detach → the attach control seams, then emit the
/// terminal [`AttachEnd`] as a `Detached`/`Error` event.
async fn pump(
    streams: AttachStreams,
    sink: &StreamSink<TerminalEvent>,
    mut rx: mpsc::UnboundedReceiver<Outbound>,
) {
    let AttachStreams {
        mut reader,
        mut writer,
        resizer,
        mut terminator,
    } = streams;
    let mut buf = [0u8; READ_CHUNK];

    loop {
        tokio::select! {
            // Attach → Dart (raw PTY output).
            read = reader.read(&mut buf) => match read {
                // EOF: the attach ended server-side; terminator.wait has the reason.
                Ok(0) => break,
                Ok(n) => {
                    if sink.add(TerminalEvent::output(buf[..n].to_vec())).is_err() {
                        // Dart dropped the stream — detach and stop.
                        terminator.detach().await;
                        break;
                    }
                }
                Err(_) => break,
            },
            // Dart → attach (input + out-of-band control).
            out = rx.recv() => match out {
                Some(Outbound::Input(bytes)) => {
                    if writer.write_all(&bytes).await.is_err() {
                        break;
                    }
                }
                Some(Outbound::Resize { cols, rows }) => resizer.resize(cols, rows),
                Some(Outbound::Detach) => {
                    terminator.detach().await;
                    break;
                }
                // All senders dropped (handle removed): treat as a detach.
                None => {
                    terminator.detach().await;
                    break;
                }
            },
        }
    }

    // Resolve why the attach ended and surface it. `wait` returns promptly once
    // the pump has published its end (which the breaks above have triggered).
    let event = match terminator.wait().await {
        AttachEnd::Detached => TerminalEvent::detached("client detach".to_string()),
        AttachEnd::SessionEnded => TerminalEvent::detached("session ended".to_string()),
        AttachEnd::Error(message) => TerminalEvent::error(message),
    };
    let _ = sink.add(event);
}

/// Send keystrokes / raw input bytes to the attach with `attach_id`. No-op if the
/// id isn't attached (e.g. already detached).
pub fn terminal_send_input(attach_id: String, bytes: Vec<u8>) {
    if let Some(entry) = lock_registry().get(&attach_id) {
        let _ = entry.tx.send(Outbound::Input(bytes));
    }
}

/// Tell the remote PTY the viewport changed.
pub fn terminal_resize(attach_id: String, cols: u16, rows: u16) {
    if let Some(entry) = lock_registry().get(&attach_id) {
        let _ = entry.tx.send(Outbound::Resize { cols, rows });
    }
}

/// Detach (leaves the tmux session running server-side).
pub fn terminal_detach(attach_id: String) {
    if let Some(entry) = lock_registry().get(&attach_id) {
        let _ = entry.tx.send(Outbound::Detach);
    }
}

// ---------------------------------------------------------------------------
// Live feeds (poller-driven).
// ---------------------------------------------------------------------------

/// Stream the server's change-feed generation counter: the poller bumps it
/// whenever observable state moves (or on reconnect), and Dart re-fetches the
/// workspace snapshot on each new value. The stream ends when the server is
/// disconnected (the poller's watch closes).
pub fn change_feed(handle: String, sink: StreamSink<u64>) {
    let mut watch = match generation_watch(&handle) {
        Ok(watch) => watch,
        Err(_) => return, // not connected → an empty feed
    };
    runtime().spawn(async move {
        // Emit the current generation immediately so a late subscriber gets one
        // value, then forward every subsequent change.
        if sink.add(*watch.borrow()).is_err() {
            return;
        }
        while watch.changed().await.is_ok() {
            let value = *watch.borrow();
            if sink.add(value).is_err() {
                break;
            }
        }
    });
}

/// Stream the server's [`ConnectionState`](claude_commander_protocol::connection::ConnectionState)
/// as a [`ConnectionStateDto`] for the health header. Ends when the server is
/// disconnected.
pub fn connection_feed(handle: String, sink: StreamSink<ConnectionStateDto>) {
    let mut watch = match connection_watch(&handle) {
        Ok(watch) => watch,
        Err(_) => return,
    };
    runtime().spawn(async move {
        if sink.add(watch.borrow().clone().into()).is_err() {
            return;
        }
        while watch.changed().await.is_ok() {
            let state = watch.borrow().clone();
            if sink.add(state.into()).is_err() {
                break;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(generation: u64, handle: &str) -> (AttachEntry, mpsc::UnboundedReceiver<Outbound>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (
            AttachEntry {
                generation,
                handle: handle.to_string(),
                tx,
            },
            rx,
        )
    }

    /// Reconnect race: a re-attach reuses the same attach id and replaces the
    /// registry entry; the *old* task finishing must not delete the *new* task's
    /// channel.
    #[test]
    fn finished_attach_only_removes_its_own_generation() {
        let id = "reconnect-race-test".to_string();
        let (e0, _rx0) = entry(0, "srv");
        let (e1, _rx1) = entry(1, "srv");

        lock_registry().insert(id.clone(), e0);
        lock_registry().insert(id.clone(), e1);

        // The gen-0 task ends and cleans up: must NOT remove gen 1's entry.
        assert!(!remove_if_current(&id, 0));
        assert!(lock_registry().contains_key(&id));

        // The gen-1 task ends later and removes its own entry.
        assert!(remove_if_current(&id, 1));
        assert!(!lock_registry().contains_key(&id));
    }

    /// Control calls for an unknown attach id are silent no-ops (already detached).
    #[test]
    fn control_calls_for_unknown_id_are_noops() {
        terminal_send_input("nope".to_string(), vec![1, 2, 3]);
        terminal_resize("nope".to_string(), 80, 24);
        terminal_detach("nope".to_string());
    }

    /// `disconnect_server`'s teardown must signal Detach to every attach owned by
    /// that handle (and drop their entries), while leaving other servers' attaches
    /// untouched.
    #[test]
    fn detach_all_for_handle_signals_and_drops_matching_attaches() {
        let (e_a, mut rx_a) = entry(10, "server-A");
        let (e_a2, mut rx_a2) = entry(11, "server-A");
        let (e_b, mut rx_b) = entry(12, "server-B");
        lock_registry().insert("attach-a1".to_string(), e_a);
        lock_registry().insert("attach-a2".to_string(), e_a2);
        lock_registry().insert("attach-b1".to_string(), e_b);

        detach_all_for_handle("server-A");

        // Both server-A attaches are gone and each was told to detach.
        assert!(!lock_registry().contains_key("attach-a1"));
        assert!(!lock_registry().contains_key("attach-a2"));
        assert!(matches!(rx_a.try_recv(), Ok(Outbound::Detach)));
        assert!(matches!(rx_a2.try_recv(), Ok(Outbound::Detach)));

        // server-B's attach is untouched and got no signal.
        assert!(lock_registry().contains_key("attach-b1"));
        assert!(rx_b.try_recv().is_err());
        lock_registry().remove("attach-b1");
    }
}
