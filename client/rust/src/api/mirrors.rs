//! flutter_rust_bridge mirrors of the shared `claude-commander-protocol` wire
//! types.
//!
//! A mirror tells frb the shape of a type defined in another crate so it can
//! generate the matching Dart class. The mirror MUST match the real type
//! field-for-field — frb emits a **compile error** otherwise — so these can't
//! silently drift from the protocol crate. The real types are `pub use`d here
//! both so `mirror(...)` resolves to them and so the API functions can return
//! them directly.
//!
//! Mirrors are added incrementally, per phase, as the UI needs each type.
//! Phase 1 needs the session-list shape (`SessionInfo` + the ids/enums it
//! embeds).

use chrono::{DateTime, Utc};
use flutter_rust_bridge::frb;
use uuid::Uuid;

pub use claude_commander_protocol::api::{
    BranchInfo, CreateOptions, OperationKind, ProgramInfo, PullBlockReason, ServerStatus,
    SessionDetail, SessionInfo,
};
pub use claude_commander_protocol::pr::{PrState, ReviewDecision};
pub use claude_commander_protocol::session::{AgentState, ProjectId, SessionId, SessionStatus};
pub use claude_commander_protocol::ws::AttachKind;

use claude_commander_protocol::api::{
    AgentStatesSnapshot, DiffStat, OperationOutcome, OperationStatus, PreviewData, ProjectInfo,
    PullStatus, WorkspaceSnapshot,
};
use claude_commander_protocol::connection::ConnectionState;

// Both id newtypes are a single `Uuid`, so one mirror covers both.
#[frb(mirror(SessionId, ProjectId))]
pub struct _Id(pub Uuid);

#[frb(mirror(SessionStatus))]
pub enum _SessionStatus {
    Creating,
    Running,
    Stopped,
    Merging,
    CascadePaused,
    Pushing,
}

#[frb(mirror(PrState))]
pub enum _PrState {
    Open,
    Closed,
    Merged,
}

#[frb(mirror(ReviewDecision))]
pub enum _ReviewDecision {
    ReviewRequired,
    Approved,
    ChangesRequested,
}

#[frb(mirror(SessionInfo))]
pub struct _SessionInfo {
    pub id: String,
    pub session_id: SessionId,
    pub title: String,
    pub branch: String,
    pub status: SessionStatus,
    pub program: String,
    pub project_id: ProjectId,
    pub project_name: String,
    pub pr_number: Option<u32>,
    pub pr_url: Option<String>,
    pub pr_state: PrState,
    pub pr_draft: bool,
    pub pr_labels: Vec<String>,
    pub review_decision: Option<ReviewDecision>,
    pub pr_reviewers: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub unread: bool,
    pub stack_parent_session_id: Option<SessionId>,
    pub pr_base_branch: Option<String>,
    pub pr_merged: bool,
    pub current_section: Option<String>,
    pub section_override: Option<String>,
    pub entered_section_at: Option<DateTime<Utc>>,
    pub last_attached_at: Option<DateTime<Utc>>,
    pub worktree_path: String,
    pub tmux_session_name: String,
    pub keep_alive: bool,
}

// Phase 2 needs the detail shape: the session's live agent sub-state plus the
// diff summary and a pane snapshot. The mirror matches the Rust struct
// field-for-field (frb checks the struct, not the flattened JSON).
#[frb(mirror(AgentState))]
pub enum _AgentState {
    Working,
    Idle,
    WaitingForInput,
    Unknown,
}

#[frb(mirror(SessionDetail))]
pub struct _SessionDetail {
    pub info: SessionInfo,
    pub agent_state: AgentState,
    pub diff_stat: Option<String>,
    pub pane_content: Option<String>,
}

// ---------------------------------------------------------------------------
// Phase 2: the full server surface reaches Dart. Types with only plain fields
// (or unit enums) are mirrored directly, so the API fns return the protocol
// type unchanged. Types carrying data enums, tuples, maps, `PathBuf`, or `usize`
// are converted to plain-struct / unit-enum DTOs first (frb can't render those
// without the build_runner/freezed step — same rule as `api::review`).
// ---------------------------------------------------------------------------

// -- Direct mirrors (plain fields / unit enums) --

#[frb(mirror(ServerStatus))]
pub struct _ServerStatus {
    pub gh_available: bool,
    pub tmux_ok: bool,
    pub version: String,
}

/// A launch program option. Mirrored (not a DTO) so it can also be *constructed*
/// on the Dart side and passed straight back to `set_programs`.
#[frb(mirror(ProgramInfo))]
pub struct _ProgramInfo {
    pub label: String,
    pub command: String,
}

#[frb(mirror(CreateOptions))]
pub struct _CreateOptions {
    pub default_program: String,
    pub programs: Vec<ProgramInfo>,
    pub sections: Vec<String>,
}

#[frb(mirror(BranchInfo))]
pub struct _BranchInfo {
    pub name: String,
    pub is_remote: bool,
}

#[frb(mirror(OperationKind))]
pub enum _OperationKind {
    Cascade,
    PushStack,
}

/// Which pane of a session to attach to (agent vs the paired shell).
#[frb(mirror(AttachKind))]
pub enum _AttachKind {
    Agent,
    Shell,
}

#[frb(mirror(PullBlockReason))]
pub enum _PullBlockReason {
    Dirty,
    Diverged,
    WorktreeConflict,
}

// -- DTOs (converted from types frb can't render directly) --

/// A project (git repo). `repo_path` is flattened `PathBuf` → `String` (matching
/// `SessionInfo.worktree_path`), which frb transfers cleanly.
pub struct ProjectInfoDto {
    pub id: ProjectId,
    pub name: String,
    pub repo_path: String,
    pub main_branch: String,
    pub session_ids: Vec<SessionId>,
}

impl From<ProjectInfo> for ProjectInfoDto {
    fn from(p: ProjectInfo) -> Self {
        Self {
            id: p.id,
            name: p.name,
            repo_path: p.repo_path.to_string_lossy().into_owned(),
            main_branch: p.main_branch,
            session_ids: p.session_ids,
        }
    }
}

/// Which kind of [`OperationOutcomeDto`] this is (flattens the data-carrying
/// [`OperationOutcome`]). `Paused` only occurs for a cascade.
pub enum OperationOutcomeKind {
    Succeeded,
    Paused,
    Failed,
}

/// Terminal result of a stack operation. `detail` is the human summary
/// (`Succeeded`/`Paused`) or the error message (`Failed`).
pub struct OperationOutcomeDto {
    pub kind: OperationOutcomeKind,
    pub detail: String,
}

impl From<OperationOutcome> for OperationOutcomeDto {
    fn from(o: OperationOutcome) -> Self {
        match o {
            OperationOutcome::Succeeded { detail } => Self {
                kind: OperationOutcomeKind::Succeeded,
                detail,
            },
            OperationOutcome::Paused { detail } => Self {
                kind: OperationOutcomeKind::Paused,
                detail,
            },
            OperationOutcome::Failed { error } => Self {
                kind: OperationOutcomeKind::Failed,
                detail: error,
            },
        }
    }
}

/// One recent cascade / push-stack operation. `id` is monotonic per process.
pub struct OperationStatusDto {
    pub id: u64,
    pub kind: OperationKind,
    pub outcome: OperationOutcomeDto,
    pub finished_at: Option<DateTime<Utc>>,
}

impl From<OperationStatus> for OperationStatusDto {
    fn from(o: OperationStatus) -> Self {
        Self {
            id: o.id,
            kind: o.kind,
            outcome: o.outcome.into(),
            finished_at: o.finished_at,
        }
    }
}

/// Which kind of [`PullStatusDto`] this is (flattens the data-carrying
/// [`PullStatus`]); `blocked_reason` is populated only for `Blocked`.
pub enum PullStatusKind {
    Advanced,
    UpToDate,
    Blocked,
    SoftFail,
}

/// Outcome of a project's most recent background pull.
pub struct PullStatusDto {
    pub kind: PullStatusKind,
    pub blocked_reason: Option<PullBlockReason>,
}

impl From<PullStatus> for PullStatusDto {
    fn from(s: PullStatus) -> Self {
        match s {
            PullStatus::Advanced => Self {
                kind: PullStatusKind::Advanced,
                blocked_reason: None,
            },
            PullStatus::UpToDate => Self {
                kind: PullStatusKind::UpToDate,
                blocked_reason: None,
            },
            PullStatus::Blocked { reason } => Self {
                kind: PullStatusKind::Blocked,
                blocked_reason: Some(reason),
            },
            PullStatus::SoftFail => Self {
                kind: PullStatusKind::SoftFail,
                blocked_reason: None,
            },
        }
    }
}

/// One project's pull status — the flattened form of the snapshot's
/// `BTreeMap<ProjectId, PullStatus>` (frb doesn't render maps).
pub struct ProjectPullDto {
    pub project_id: ProjectId,
    pub status: PullStatusDto,
}

/// A single snapshot of everything the session tree renders. The `BTreeMap`
/// pull statuses are flattened to a `Vec`; every data enum is flattened above.
pub struct WorkspaceSnapshotDto {
    pub projects: Vec<ProjectInfoDto>,
    pub sessions: Vec<SessionInfo>,
    pub cascade_paused: Option<SessionId>,
    pub pending_comment_sessions: Vec<SessionId>,
    pub project_pull: Vec<ProjectPullDto>,
    pub operations: Vec<OperationStatusDto>,
    pub server: ServerStatus,
}

impl From<WorkspaceSnapshot> for WorkspaceSnapshotDto {
    fn from(s: WorkspaceSnapshot) -> Self {
        Self {
            projects: s.projects.into_iter().map(Into::into).collect(),
            sessions: s.sessions,
            cascade_paused: s.cascade_paused,
            pending_comment_sessions: s.pending_comment_sessions,
            project_pull: s
                .project_pull
                .into_iter()
                .map(|(project_id, status)| ProjectPullDto {
                    project_id,
                    status: status.into(),
                })
                .collect(),
            operations: s.operations.into_iter().map(Into::into).collect(),
            server: s.server,
        }
    }
}

/// One session's agent state (flattened from the snapshot's
/// `BTreeMap<SessionId, AgentState>`). The commander *sentinel* id is never
/// present here — [`AgentStatesSnapshotDto::from`] drops it.
pub struct AgentStateEntryDto {
    pub session_id: SessionId,
    pub state: AgentState,
}

/// Bulk agent-state snapshot, map flattened to a `Vec` with the synthetic
/// commander sentinel entry stripped (its running-ness is `commander_running`).
pub struct AgentStatesSnapshotDto {
    pub states: Vec<AgentStateEntryDto>,
    pub commander_running: bool,
}

/// The commander's fixed sentinel id. Mirrors core's `commander_sentinel_id()`
/// (`crates/claude-commander-core/src/commander.rs`): a reserved `SessionId`
/// the server injects into the agent-state map to carry the commander chip's
/// live state. It maps to no real session/worktree, so per-session rows must
/// skip it. Replicated here because the cdylib can't depend on core.
fn commander_sentinel_id() -> SessionId {
    SessionId::from_uuid(Uuid::from_u128(
        0xc0_3a_de_cc_00_00_00_00_00_00_00_00_00_00_00_00,
    ))
}

impl From<AgentStatesSnapshot> for AgentStatesSnapshotDto {
    fn from(s: AgentStatesSnapshot) -> Self {
        let sentinel = commander_sentinel_id();
        Self {
            states: s
                .states
                .into_iter()
                .filter(|(id, _)| *id != sentinel)
                .map(|(session_id, state)| AgentStateEntryDto { session_id, state })
                .collect(),
            commander_running: s.commander_running,
        }
    }
}

/// Numeric diff counts. `usize` → `u32` (matching `api::review`'s line numbers).
pub struct DiffStatDto {
    pub files_changed: u32,
    pub lines_added: u32,
    pub lines_removed: u32,
}

impl From<DiffStat> for DiffStatDto {
    fn from(s: DiffStat) -> Self {
        Self {
            files_changed: s.files_changed as u32,
            lines_added: s.lines_added as u32,
            lines_removed: s.lines_removed as u32,
        }
    }
}

/// Preview payload for a session or project: pane/shell snapshots plus the diff
/// text, its one-line stat, and structured counts.
pub struct PreviewDataDto {
    pub pane: Option<String>,
    pub diff_text: String,
    pub diff_stat: Option<String>,
    pub stats: Option<DiffStatDto>,
    pub shell: Option<String>,
}

impl From<PreviewData> for PreviewDataDto {
    fn from(p: PreviewData) -> Self {
        Self {
            pane: p.pane,
            diff_text: p.diff_text,
            diff_stat: p.diff_stat,
            stats: p.stats.map(Into::into),
            shell: p.shell,
        }
    }
}

/// Which kind of [`ConnectionStateDto`] this is (flattens the data-carrying
/// [`ConnectionState`]); `reason` is populated only for `Degraded`.
pub enum ConnectionStateKind {
    Connecting,
    Connected,
    Degraded,
}

/// A backend's connection health, streamed over `connection_feed`.
pub struct ConnectionStateDto {
    pub kind: ConnectionStateKind,
    /// Short user-facing note (`Degraded` only); empty otherwise.
    pub reason: String,
}

impl From<ConnectionState> for ConnectionStateDto {
    fn from(s: ConnectionState) -> Self {
        match s {
            ConnectionState::Connecting => Self {
                kind: ConnectionStateKind::Connecting,
                reason: String::new(),
            },
            ConnectionState::Connected => Self {
                kind: ConnectionStateKind::Connected,
                reason: String::new(),
            },
            ConnectionState::Degraded { reason } => Self {
                kind: ConnectionStateKind::Degraded,
                reason,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    #[test]
    fn agent_states_dto_drops_commander_sentinel() {
        let real = SessionId::new();
        let mut states = BTreeMap::new();
        states.insert(real, AgentState::Working);
        states.insert(commander_sentinel_id(), AgentState::Idle);
        let dto: AgentStatesSnapshotDto = AgentStatesSnapshot {
            states,
            commander_running: true,
        }
        .into();
        assert_eq!(dto.states.len(), 1, "sentinel must be filtered out");
        assert_eq!(dto.states[0].session_id, real);
        assert!(dto.commander_running);
    }

    #[test]
    fn workspace_dto_flattens_pull_map_and_operations() {
        let pid = ProjectId::new();
        let mut project_pull = BTreeMap::new();
        project_pull.insert(
            pid,
            PullStatus::Blocked {
                reason: PullBlockReason::Dirty,
            },
        );
        let snap = WorkspaceSnapshot {
            projects: vec![ProjectInfo {
                id: pid,
                name: "repo".into(),
                repo_path: PathBuf::from("/repo"),
                main_branch: "main".into(),
                session_ids: vec![],
            }],
            sessions: vec![],
            cascade_paused: None,
            pending_comment_sessions: vec![],
            project_pull,
            operations: vec![OperationStatus {
                id: 3,
                kind: OperationKind::Cascade,
                outcome: OperationOutcome::Paused {
                    detail: "2 merged".into(),
                },
                finished_at: None,
            }],
            server: ServerStatus {
                gh_available: true,
                tmux_ok: true,
                version: "0.0.0".into(),
            },
        };
        let dto: WorkspaceSnapshotDto = snap.into();
        assert_eq!(dto.projects[0].repo_path, "/repo");
        assert_eq!(dto.project_pull.len(), 1);
        assert_eq!(dto.project_pull[0].project_id, pid);
        assert!(matches!(
            dto.project_pull[0].status.kind,
            PullStatusKind::Blocked
        ));
        assert!(matches!(
            dto.project_pull[0].status.blocked_reason,
            Some(PullBlockReason::Dirty)
        ));
        assert_eq!(dto.operations[0].id, 3);
        assert!(matches!(
            dto.operations[0].outcome.kind,
            OperationOutcomeKind::Paused
        ));
        assert_eq!(dto.operations[0].outcome.detail, "2 merged");
    }

    #[test]
    fn connection_state_dto_flattens_degraded_reason() {
        let dto: ConnectionStateDto = ConnectionState::Degraded {
            reason: "boom".into(),
        }
        .into();
        assert!(matches!(dto.kind, ConnectionStateKind::Degraded));
        assert_eq!(dto.reason, "boom");

        let ok: ConnectionStateDto = ConnectionState::Connected.into();
        assert!(matches!(ok.kind, ConnectionStateKind::Connected));
        assert!(ok.reason.is_empty());
    }
}
