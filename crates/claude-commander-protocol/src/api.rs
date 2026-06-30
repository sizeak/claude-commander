//! HTTP API request/response DTOs.
//!
//! These mirror the JSON bodies the server's `/api` surface accepts and returns.
//! They embed the lower-level wire types from this crate's other modules
//! (`session`, `pr`, `diff`, `comment`), so a client deserializes a whole
//! session/review payload with no hand-maintained mirror.
//!
//! Construction helpers that need the server's domain model (e.g. building a
//! [`SessionInfo`] from a `WorktreeSession`, or validating program flags) live
//! in `claude-commander-core`, since they depend on types that can't cross the
//! network. These are plain data.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::comment::{Comment, CommentSide};
use crate::diff::ParsedDiff;
use crate::pr::{PrState, ReviewDecision};
use crate::session::{AgentState, ProjectId, SessionId, SessionStatus};

/// A session as returned by the list/find/detail endpoints.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
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

/// A session plus its live detail: agent sub-state, diff summary, and a pane
/// snapshot. `info` is flattened so the JSON is a single object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionDetail {
    #[serde(flatten)]
    pub info: SessionInfo,
    pub agent_state: AgentState,
    pub diff_stat: Option<String>,
    pub pane_content: Option<String>,
}

/// Request to stage a new comment on a session's review diff.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewComment {
    pub file: String,
    pub side: CommentSide,
    pub line_range: (usize, usize),
    pub snippet: String,
    pub comment: String,
}

/// Which side of a diff a binary blob fetch refers to: the base ("before") or
/// the working tree ("after").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffSide {
    Old,
    New,
}

/// Result of opening the review view: the parsed diff plus the session's
/// (re-anchored) comments and the base they were computed against.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewSnapshot {
    pub base: String,
    pub diff: ParsedDiff,
    pub comments: Vec<Comment>,
    /// Display paths of files still marked reviewed (stale marks pruned).
    pub reviewed: Vec<String>,
    /// xxh3 hash of the raw unified diff this snapshot was built from, so an
    /// open review view can cheaply tell whether a re-compose actually changed
    /// anything before rebuilding.
    pub content_hash: u64,
}

/// Options for creating a session (request body for `POST /sessions`). Optional
/// fields default to absent so a minimal `{project_path, title}` body is valid.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSessionOpts {
    pub project_path: PathBuf,
    pub title: String,
    #[serde(default)]
    pub program: Option<String>,
    #[serde(default)]
    pub initial_prompt: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub base_branch: Option<String>,
    #[serde(default)]
    pub section: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_session_opts_minimal_body_deserializes() {
        // The optional fields are `#[serde(default)]`, so a minimal body is valid.
        let opts: CreateSessionOpts =
            serde_json::from_str(r#"{"project_path":"/repo","title":"x"}"#).unwrap();
        assert_eq!(opts.title, "x");
        assert!(opts.program.is_none());
        assert!(opts.effort.is_none());
    }

    #[test]
    fn new_comment_requires_all_fields() {
        // Unlike CreateSessionOpts, NewComment fields are all required.
        assert!(serde_json::from_str::<NewComment>(r#"{"file":"a.rs"}"#).is_err());
        let c: NewComment = serde_json::from_str(
            r#"{"file":"a.rs","side":"new","line_range":[1,2],"snippet":"x","comment":"y"}"#,
        )
        .unwrap();
        assert_eq!(c.side, CommentSide::New);
        assert_eq!(c.line_range, (1, 2));
    }

    #[test]
    fn diff_side_wire_form() {
        assert_eq!(serde_json::to_string(&DiffSide::Old).unwrap(), r#""old""#);
        assert_eq!(
            serde_json::from_str::<DiffSide>(r#""new""#).unwrap(),
            DiffSide::New
        );
    }
}
