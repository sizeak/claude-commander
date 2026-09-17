//! Typing text into an attached pane from outside the attach loop.
//!
//! The attach loop's stdin pump owns the only writer to the pane (a local PTY or
//! a remote WebSocket). Dictation transcripts arrive asynchronously on a task
//! that has no access to that writer, so this module provides the handle through
//! which such a task queues bytes into the pump's outbound buffer — exactly
//! where typed keystrokes go — plus the description of the pane currently on
//! screen that the dictation submit policy needs.

use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;

use crate::agent::AgentKind;
use crate::backend::AttachKind;

/// What is on the other side of the attached tmux client right now: which pane
/// of the session (agent or shell) and, for an agent pane, which harness runs
/// there. The dictation planner uses it to decide whether an Enter follows the
/// typed text and how long to wait before sending it
/// ([`AgentKind::submit_key_delay`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaneInfo {
    pub kind: AttachKind,
    pub agent: AgentKind,
}

/// One item on the injection channel: bytes for the pane, or a notice for the
/// operator. Both go to the pump because both need what only it has — the
/// pane writer, and the live name of the tmux session the client is showing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaneInput {
    /// Type these bytes into the pane, exactly as keystrokes would be.
    Bytes(Vec<u8>),
    /// Show `text` in the attached client's tmux status line. With `hold` the
    /// message stays until the next keypress (`display-message -d 0`, tmux(1):
    /// "a delay of zero waits for a key press"); otherwise tmux's own
    /// `display-time` applies. Hold is for states the user is *in* (recording,
    /// transcribing); a plain notice is for a moment that has passed.
    Notice { text: String, hold: bool },
}

/// The way in to the attached pane for a task that is not the attach loop.
///
/// A respawn-stable handle, in the same shape and for the same reason as
/// [`ListenerHandle`](crate::conversation::ListenerHandle): the frontend holds
/// one for the whole process lifetime and hands it to every attach it starts,
/// and each attach [`install`](Self::install)s its own channel into it — so the
/// dictation task never has to be told that an attach began, ended, or was
/// replaced by another one.
///
/// **The contract that makes that safe is [`send`](Self::send)'s return.** The
/// receiving end lives *inside* the stdin pump task, which
/// [`AttachSession::finish`](crate::tmux::AttachSession::finish) aborts and then
/// awaits before it returns; so once an attach is over, the receiver is dropped
/// and every later `send` is `false`. A failed `send` **is** the "there is no
/// pane to type into" signal — there is deliberately no separate `is_attached`,
/// which could only ever be a stale answer by the time the caller acted on it.
///
/// [`set_pane`](Self::set_pane) carries the other half: *what* is on screen, for
/// the submit policy that decides whether an Enter follows the typed text.
#[derive(Clone, Default)]
pub struct PaneInjector(Arc<Mutex<InjectorInner>>);

#[derive(Default)]
struct InjectorInner {
    tx: Option<mpsc::UnboundedSender<PaneInput>>,
    pane: Option<PaneInfo>,
}

impl PaneInjector {
    /// Install (or swap in) the current attach's injection channel, dropping any
    /// previous one. Called by each attach as its pumps start.
    pub fn install(&self, tx: mpsc::UnboundedSender<PaneInput>) {
        self.0.lock().unwrap().tx = Some(tx);
    }

    /// Record which pane the attached client is showing now. The frontend
    /// updates this whenever the client moves (a shell toggle, a switcher pick).
    pub fn set_pane(&self, pane: PaneInfo) {
        self.0.lock().unwrap().pane = Some(pane);
    }

    /// The pane the attached client is showing, if one has been recorded.
    pub fn pane(&self) -> Option<PaneInfo> {
        self.0.lock().unwrap().pane
    }

    /// Queue `bytes` for the pane, exactly where the attach loop puts typed
    /// keystrokes. Returns `false` when no attach has installed a channel or the
    /// one it installed is gone — see the type docs: that is the caller's signal
    /// that there is nothing attached to type into.
    pub fn send(&self, bytes: &[u8]) -> bool {
        self.push(PaneInput::Bytes(bytes.to_vec()))
    }

    /// Show a status-line notice in the attached client — the only place the
    /// operator can see feedback while a pane covers the TUI. `hold` keeps it up
    /// until the next keypress (see [`PaneInput::Notice`]). Same `false` contract
    /// as [`send`](Self::send); a notice with nobody attached is simply dropped.
    pub fn notice(&self, text: impl Into<String>, hold: bool) -> bool {
        self.push(PaneInput::Notice {
            text: text.into(),
            hold,
        })
    }

    fn push(&self, input: PaneInput) -> bool {
        matches!(&self.0.lock().unwrap().tx, Some(tx) if tx.send(input).is_ok())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn send_without_install_is_false() {
        // A fresh injector — the frontend's, before any attach — has nowhere to
        // put bytes, and says so rather than swallowing them.
        let injector = PaneInjector::default();
        assert!(!injector.send(b"hello"));
    }

    #[test]
    fn set_pane_then_pane_roundtrips() {
        let injector = PaneInjector::default();
        assert_eq!(injector.pane(), None);
        let pane = PaneInfo {
            kind: AttachKind::Shell,
            agent: AgentKind::Claude,
        };
        injector.set_pane(pane);
        assert_eq!(injector.pane(), Some(pane));
    }

    #[tokio::test]
    async fn send_fails_after_receiver_dropped() {
        // The receiver lives in the stdin pump, so dropping it is what the end of
        // an attach looks like from here.
        let injector = PaneInjector::default();
        let (tx, rx) = mpsc::unbounded_channel();
        injector.install(tx);
        assert!(injector.send(b"hi"), "an installed channel takes bytes");
        assert!(injector.notice("hi", false), "and notices");

        drop(rx);
        assert!(
            !injector.send(b"hi"),
            "a dead receiver means the attach is over"
        );
    }

    #[tokio::test]
    async fn bytes_and_notices_share_one_ordered_channel() {
        // The consumer types text and then announces it; the pump must see them
        // in that order, so both ride the same channel rather than two.
        let injector = PaneInjector::default();
        let (tx, mut rx) = mpsc::unbounded_channel();
        injector.install(tx);
        assert!(injector.send(b"typed"));
        assert!(injector.notice("Dictating…", true));
        assert_eq!(rx.recv().await, Some(PaneInput::Bytes(b"typed".to_vec())));
        assert_eq!(
            rx.recv().await,
            Some(PaneInput::Notice {
                text: "Dictating…".into(),
                hold: true
            })
        );
    }
}
