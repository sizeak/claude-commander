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

pub use claude_commander_protocol::api::SessionInfo;
pub use claude_commander_protocol::pr::{PrState, ReviewDecision};
pub use claude_commander_protocol::session::{ProjectId, SessionId, SessionStatus};

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
}
