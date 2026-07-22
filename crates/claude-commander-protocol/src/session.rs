//! Session identity and status wire types.
//!
//! Ids, the persisted session status, and the ephemeral agent sub-state. These
//! are shared by the server's response DTOs and by `claude-commander-core`'s
//! domain model (`WorktreeSession`/`Project`), which re-exports them so its
//! existing paths are unchanged.

use std::fmt;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Unique identifier for a project (git repository).
///
/// The inner `Uuid` is `pub` so flutter_rust_bridge can mirror this newtype for
/// the Flutter client; prefer the `from_uuid`/`as_uuid` accessors in Rust.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ProjectId(pub Uuid);

impl ProjectId {
    /// Create a new random project ID.
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Create from an existing UUID.
    pub fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }

    /// Get the inner UUID.
    pub fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl Default for ProjectId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for ProjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Use first 8 chars for display
        write!(f, "{}", &self.0.to_string()[..8])
    }
}

/// Unique identifier for a worktree session.
///
/// The inner `Uuid` is `pub` so flutter_rust_bridge can mirror this newtype for
/// the Flutter client; prefer the `from_uuid`/`as_uuid` accessors in Rust.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SessionId(pub Uuid);

impl SessionId {
    /// Create a new random session ID.
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Create from an existing UUID.
    pub fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }

    /// Get the inner UUID.
    pub fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Use first 8 chars for display
        write!(f, "{}", &self.0.to_string()[..8])
    }
}

/// Status of a worktree session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    /// Session is being created (worktree/tmux setup in progress)
    Creating,
    /// Session is running and active
    Running,
    /// Session has completed or been killed
    #[serde(alias = "paused")]
    Stopped,
    /// Cascade-merge is running `git merge` in this session's worktree.
    /// Transient: cleared as soon as the merge step completes, conflicts or
    /// not. `cleanup_stale_merging_sessions` resets any stragglers at startup
    /// in case the process died mid-merge.
    Merging,
    /// Cascade-merge hit a conflict in this session. Persists until the user
    /// runs `CascadeResume` (after resolving + committing) or `CascadeAbandon`,
    /// so the stalled session is still visible after restarting the TUI.
    CascadePaused,
    /// `git push` is running against this session's branch as part of a
    /// push-stack operation. Transient: cleared as soon as the push completes
    /// (success or failure). Same stale-cleanup treatment as `Merging` — a
    /// crash mid-push shouldn't leave the UI stuck.
    Pushing,
}

impl SessionStatus {
    /// Check if the session is active (creating, running, or mid-cascade).
    ///
    /// `Merging` and `CascadePaused` both mean the tmux session is still
    /// alive — the underlying program is running even if the worktree is
    /// temporarily locked by the cascade.
    pub fn is_active(&self) -> bool {
        matches!(
            self,
            Self::Creating | Self::Running | Self::Merging | Self::CascadePaused | Self::Pushing
        )
    }

    /// Check if the session can be attached to (Stopped sessions are allowed
    /// because get_attach_command will recreate the tmux session automatically).
    /// Cascade-involved sessions are attachable — users often attach to a
    /// `CascadePaused` session specifically to resolve conflicts there.
    pub fn can_attach(&self) -> bool {
        matches!(
            self,
            Self::Running | Self::Stopped | Self::Merging | Self::CascadePaused | Self::Pushing
        )
    }
}

impl fmt::Display for SessionStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Creating => write!(f, "creating"),
            Self::Running => write!(f, "running"),
            Self::Stopped => write!(f, "stopped"),
            Self::Merging => write!(f, "merging"),
            Self::CascadePaused => write!(f, "cascade_paused"),
            Self::Pushing => write!(f, "pushing"),
        }
    }
}

/// Where a session was created from Slack: the channel, the thread it belongs
/// to, and a permalink back to the originating message.
///
/// Recorded on a session created via the Slack bridge so the notify path can
/// route a worker's message back to the right thread. Persisted on the session
/// record and carried on the wire DTO; `#[serde(default,
/// skip_serializing_if = "Option::is_none")]` on the enclosing field keeps old
/// `state.json` files and old binaries round-tripping safely.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlackOrigin {
    /// Slack channel id the session was requested in (e.g. `C0AR48X88L9`).
    pub channel: String,
    /// Thread timestamp identifying the conversation (`app_mention`/`message.im`
    /// thread root, or the message ts when not yet threaded).
    pub thread_ts: String,
    /// Permalink to the originating Slack message, embedded in the agent's
    /// initial prompt so it can reference the source.
    pub permalink: String,
}

/// Sub-state of a Running Claude Code session, detected via pane content parsing.
/// This is ephemeral (not persisted) and only meaningful when SessionStatus == Running.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    /// Claude is actively generating output
    Working,
    /// Claude has finished and is at the input prompt
    Idle,
    /// Claude is waiting for user permission or input
    WaitingForInput,
    /// State could not be determined (non-Claude program, detection failure, etc.)
    Unknown,
}

impl fmt::Display for AgentState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Working => write!(f, "working"),
            Self::Idle => write!(f, "idle"),
            Self::WaitingForInput => write!(f, "waiting"),
            Self::Unknown => write!(f, "unknown"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_status_round_trips_and_aliases_paused() {
        // Canonical snake_case wire form.
        assert_eq!(
            serde_json::to_string(&SessionStatus::Stopped).unwrap(),
            r#""stopped""#
        );
        // The legacy `"paused"` value still deserializes to `Stopped` (back-compat).
        assert_eq!(
            serde_json::from_str::<SessionStatus>(r#""paused""#).unwrap(),
            SessionStatus::Stopped
        );
    }

    #[test]
    fn agent_state_round_trips() {
        for s in [
            AgentState::Working,
            AgentState::Idle,
            AgentState::WaitingForInput,
            AgentState::Unknown,
        ] {
            let json = serde_json::to_string(&s).unwrap();
            assert_eq!(serde_json::from_str::<AgentState>(&json).unwrap(), s);
        }
        assert_eq!(
            serde_json::to_string(&AgentState::WaitingForInput).unwrap(),
            r#""waiting_for_input""#
        );
    }

    #[test]
    fn project_id_from_uuid_round_trips() {
        // Kills the mutant that replaces ProjectId::from_uuid with
        // Default::default(): Default uses Uuid::new_v4(), so the inner UUID
        // would not match the one we pass in.
        let uuid = Uuid::from_u128(0x1234_5678_9abc_def0_1234_5678_9abc_def0);
        let id = ProjectId::from_uuid(uuid);
        assert_eq!(id.0, uuid);

        // And the nil UUID — Default would never produce this.
        let nil = ProjectId::from_uuid(Uuid::nil());
        assert_eq!(nil.0, Uuid::nil());
    }

    #[test]
    fn session_id_from_uuid_and_as_uuid_round_trip() {
        // Kills two mutants together:
        //   * `from_uuid -> Self with Default::default()` — Default would
        //     generate a fresh v4 UUID, so the round-trip would fail.
        //   * `as_uuid -> &Uuid with Box::leak(Box::new(Default::default()))`
        //     — the leaked value would be Uuid::nil(), not the wrapped UUID.
        let uuid = Uuid::from_u128(0xdead_beef_0000_0000_0000_0000_dead_beef);
        let id = SessionId::from_uuid(uuid);
        assert_eq!(*id.as_uuid(), uuid);

        // Reference identity: as_uuid must borrow the inner field, not return
        // a reference to a freshly leaked value.
        assert!(std::ptr::eq(id.as_uuid(), &id.0));

        // Nil UUID also round-trips — pins down both mutants in a second case.
        let nil = SessionId::from_uuid(Uuid::nil());
        assert_eq!(*nil.as_uuid(), Uuid::nil());
    }

    #[test]
    fn slack_origin_round_trips() {
        let origin = SlackOrigin {
            channel: "C0AR48X88L9".to_string(),
            thread_ts: "1700000000.000100".to_string(),
            permalink: "https://slack.example/archives/C0AR48X88L9/p1700000000000100".to_string(),
        };
        let json = serde_json::to_string(&origin).unwrap();
        let back: SlackOrigin = serde_json::from_str(&json).unwrap();
        assert_eq!(back, origin);
    }

    #[test]
    fn status_active_and_attach_flags() {
        assert!(SessionStatus::Running.is_active());
        assert!(!SessionStatus::Stopped.is_active());
        assert!(SessionStatus::Stopped.can_attach());
    }
}
