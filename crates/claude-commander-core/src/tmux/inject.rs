//! Typing text into an attached pane from outside the attach loop.
//!
//! The attach loop's stdin pump owns the only writer to the pane (a local PTY or
//! a remote WebSocket). Dictation transcripts arrive asynchronously on a task
//! that has no access to that writer, so this module provides the handle through
//! which such a task queues bytes into the pump's outbound buffer — exactly
//! where typed keystrokes go — plus the description of the pane currently on
//! screen that the dictation submit policy needs.

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
