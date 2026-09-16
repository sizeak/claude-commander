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
    tx: Option<mpsc::UnboundedSender<Vec<u8>>>,
    pane: Option<PaneInfo>,
}

impl PaneInjector {
    /// Install (or swap in) the current attach's injection channel, dropping any
    /// previous one. Called by each attach as its pumps start.
    pub fn install(&self, tx: mpsc::UnboundedSender<Vec<u8>>) {
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
        matches!(&self.0.lock().unwrap().tx, Some(tx) if tx.send(bytes.to_vec()).is_ok())
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

        drop(rx);
        assert!(
            !injector.send(b"hi"),
            "a dead receiver means the attach is over"
        );
    }
}
